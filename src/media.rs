//! Background worker that talks to the Windows media session API
//! (GlobalSystemMediaTransportControls). Anything that reports "now playing"
//! to Windows shows up here: Spotify, browsers, Media Player, foobar2000, etc.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::Result;
use windows::Media::MediaPlaybackAutoRepeatMode;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Clone, PartialEq, Default)]
pub struct Snapshot {
    /// False when nothing on the system is exposing a media session.
    pub active: bool,
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub can_prev: bool,
    pub can_next: bool,
    pub can_toggle: bool,
    pub can_seek: bool,
    pub can_shuffle: bool,
    pub shuffle: bool,
    pub can_repeat: bool,
    /// 0 off, 1 repeat the track, 2 repeat the list.
    pub repeat: u8,
    /// App ids of every player currently known to Windows.
    pub sessions: Vec<String>,
    /// Timeline, in 100 ns ticks.
    pub start: i64,
    pub end: i64,
    pub position: i64,
    /// When `position` was reported, as FILETIME ticks (UTC). 0 if unknown.
    pub last_updated: i64,
    pub rate: f64,
    /// Encoded image bytes (JPEG/PNG) of the album art.
    pub art: Option<Arc<Vec<u8>>>,
    pub art_gen: u64,
}

pub enum Cmd {
    Prev,
    Next,
    Toggle,
    /// Absolute position in 100 ns ticks.
    Seek(i64),
    Shuffle(bool),
    /// 0 off, 1 track, 2 list.
    Repeat(u8),
    /// Nothing to do; just re-read the players now.
    Refresh,
}

pub fn run(
    shared: Arc<Mutex<Snapshot>>,
    rx: Receiver<Cmd>,
    hwnd: &'static AtomicIsize,
    notify_msg: u32,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut manager: Option<Manager> = None;
    let mut session: Option<Session> = None;
    let mut last = Snapshot::default();
    let mut track_key = String::new();
    let mut polls_since_change: u32 = 0;
    let mut art_failures: u32 = 0;
    let mut fast_polls: u32 = 0;

    loop {
        let wait = if fast_polls > 0 { 120 } else { 500 };
        match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(cmd) => {
                if let Some(s) = &session {
                    let _ = execute(s, cmd);
                }
                // Poll quickly for a moment so the UI reflects the change.
                fast_polls = 8;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        fast_polls = fast_polls.saturating_sub(1);

        if manager.is_none() {
            manager = Manager::RequestAsync().and_then(|op| op.join()).ok();
        }
        let Some(mgr) = &manager else { continue };

        let sessions = session_ids(mgr);
        session = pick_session(mgr);
        let mut snap = match &session {
            Some(s) => match read_session(s) {
                Ok((snap, thumb)) => {
                    let mut snap = snap;
                    let key = format!(
                        "{}\u{1}{}\u{1}{}\u{1}{}",
                        snap.app_id, snap.title, snap.artist, snap.album
                    );
                    if key != track_key {
                        track_key = key;
                        polls_since_change = 0;
                        art_failures = 0;
                    } else {
                        polls_since_change = polls_since_change.saturating_add(1);
                    }

                    // Players often publish the title first and the artwork a
                    // moment later, so re-read the thumbnail for the first few
                    // polls after a track change, and now and then while there
                    // is none.
                    snap.art = last.art.clone();
                    snap.art_gen = last.art_gen;
                    let should_read = polls_since_change <= 6
                        || (snap.art.is_none() && polls_since_change % 6 == 0);
                    if should_read {
                        match thumb.as_ref().map(read_thumbnail) {
                            Some(Ok(bytes)) => {
                                art_failures = 0;
                                let same = snap.art.as_deref().is_some_and(|a| *a == bytes);
                                if !same {
                                    snap.art = Some(Arc::new(bytes));
                                    snap.art_gen = last.art_gen.wrapping_add(1);
                                }
                            }
                            _ => {
                                // Keep the previous art briefly to avoid a
                                // flash of the placeholder between tracks.
                                art_failures += 1;
                                if art_failures >= 3 && snap.art.is_some() {
                                    snap.art = None;
                                    snap.art_gen = last.art_gen.wrapping_add(1);
                                }
                            }
                        }
                    }
                    snap
                }
                Err(_) => {
                    // The session may have just gone away, or the manager died
                    // (for example after an Explorer restart). Start over.
                    manager = None;
                    session = None;
                    Snapshot { art_gen: last.art_gen, ..Default::default() }
                }
            },
            None => Snapshot { art_gen: last.art_gen, ..Default::default() },
        };

        if !snap.active {
            track_key.clear();
            snap.art = None;
        }
        snap.sessions = sessions;

        if snap != last {
            last = snap.clone();
            if let Ok(mut guard) = shared.lock() {
                *guard = snap;
            }
            let raw = hwnd.load(Ordering::Relaxed);
            if raw != 0 {
                unsafe {
                    let _ = PostMessageW(Some(HWND(raw as *mut _)), notify_msg, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
}

fn is_playing(s: &Session) -> bool {
    s.GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|st| st == PlaybackStatus::Playing)
        .unwrap_or(false)
}

/// Set from the settings window: ignore every player except Spotify.
pub static SPOTIFY_ONLY: AtomicBool = AtomicBool::new(false);

/// The player chosen from the menu (an app id), or None to pick automatically.
static PREFERRED: Mutex<Option<String>> = Mutex::new(None);

pub fn preferred() -> Option<String> {
    PREFERRED.lock().ok().and_then(|p| p.clone())
}

pub fn set_preferred(app_id: Option<String>) {
    if let Ok(mut p) = PREFERRED.lock() {
        *p = app_id;
    }
}

fn app_id(s: &Session) -> String {
    s.SourceAppUserModelId().map(|id| id.to_string()).unwrap_or_default()
}

fn session_ids(mgr: &Manager) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    if let Ok(list) = mgr.GetSessions() {
        for s in list {
            let id = app_id(&s);
            if allowed(&s) && !id.is_empty() && !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

fn allowed(s: &Session) -> bool {
    if !SPOTIFY_ONLY.load(Ordering::Relaxed) {
        return true;
    }
    s.SourceAppUserModelId()
        .map(|id| id.to_string().to_lowercase().contains("spotify"))
        .unwrap_or(false)
}

/// Prefer whatever is actually playing; otherwise fall back to the session
/// Windows considers current (the one the hardware media keys would control).
fn pick_session(mgr: &Manager) -> Option<Session> {
    // A player chosen from the menu wins for as long as it exists.
    if let Some(wanted) = preferred() {
        if let Ok(list) = mgr.GetSessions() {
            for s in list {
                if allowed(&s) && app_id(&s) == wanted {
                    return Some(s);
                }
            }
        }
    }
    let current = mgr.GetCurrentSession().ok().filter(allowed);
    if let Some(c) = &current {
        if is_playing(c) {
            return current;
        }
    }
    let mut first = None;
    if let Ok(list) = mgr.GetSessions() {
        for s in list {
            if !allowed(&s) {
                continue;
            }
            if is_playing(&s) {
                return Some(s);
            }
            if first.is_none() {
                first = Some(s);
            }
        }
    }
    current.or(first)
}

fn read_session(s: &Session) -> Result<(Snapshot, Option<IRandomAccessStreamReference>)> {
    let mut snap = Snapshot { active: true, rate: 1.0, ..Default::default() };
    snap.app_id = s.SourceAppUserModelId()?.to_string();

    let info = s.GetPlaybackInfo()?;
    snap.playing = info.PlaybackStatus()? == PlaybackStatus::Playing;
    if let Ok(c) = info.Controls() {
        snap.can_prev = c.IsPreviousEnabled().unwrap_or(false);
        snap.can_next = c.IsNextEnabled().unwrap_or(false);
        snap.can_toggle = c.IsPlayPauseToggleEnabled().unwrap_or(false)
            || c.IsPlayEnabled().unwrap_or(false)
            || c.IsPauseEnabled().unwrap_or(false);
        snap.can_seek = c.IsPlaybackPositionEnabled().unwrap_or(false);
        snap.can_shuffle = c.IsShuffleEnabled().unwrap_or(false);
        snap.can_repeat = c.IsRepeatEnabled().unwrap_or(false);
    }
    snap.shuffle = info.IsShuffleActive().and_then(|v| v.Value()).unwrap_or(false);
    snap.repeat = match info.AutoRepeatMode().and_then(|v| v.Value()) {
        Ok(MediaPlaybackAutoRepeatMode::Track) => 1,
        Ok(MediaPlaybackAutoRepeatMode::List) => 2,
        _ => 0,
    };
    if let Ok(rate) = info.PlaybackRate().and_then(|r| r.Value()) {
        if rate.is_finite() && rate > 0.0 {
            snap.rate = rate;
        }
    }

    if let Ok(tl) = s.GetTimelineProperties() {
        snap.start = tl.StartTime().map(|t| t.Duration).unwrap_or(0);
        snap.end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
        snap.position = tl.Position().map(|t| t.Duration).unwrap_or(0);
        snap.last_updated = tl.LastUpdatedTime().map(|t| t.UniversalTime).unwrap_or(0);
    }

    // Media properties can legitimately be unavailable for a moment while a
    // player is starting up; treat that as "no metadata yet", not an error.
    let mut thumb = None;
    if let Ok(props) = s.TryGetMediaPropertiesAsync().and_then(|op| op.join()) {
        snap.title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        snap.artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
        snap.album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
        if snap.artist.is_empty() {
            snap.artist = props.AlbumArtist().map(|h| h.to_string()).unwrap_or_default();
        }
        thumb = props.Thumbnail().ok();
    }
    Ok((snap, thumb))
}

fn read_thumbnail(r: &IRandomAccessStreamReference) -> Result<Vec<u8>> {
    let stream = r.OpenReadAsync()?.join()?;
    let size = stream.Size()?;
    if size == 0 || size > 32 * 1024 * 1024 {
        return Err(windows::core::Error::empty());
    }
    let reader = DataReader::CreateDataReader(&stream)?;
    let loaded = reader.LoadAsync(size as u32)?.join()? as usize;
    if loaded == 0 {
        return Err(windows::core::Error::empty());
    }
    let mut buf = vec![0u8; loaded];
    reader.ReadBytes(&mut buf)?;
    let _ = reader.Close();
    let _ = stream.Close();
    Ok(buf)
}

fn execute(s: &Session, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Prev => s.TrySkipPreviousAsync()?.join()?,
        Cmd::Next => s.TrySkipNextAsync()?.join()?,
        Cmd::Toggle => s.TryTogglePlayPauseAsync()?.join()?,
        Cmd::Seek(ticks) => s.TryChangePlaybackPositionAsync(ticks)?.join()?,
        Cmd::Shuffle(on) => s.TryChangeShuffleActiveAsync(on)?.join()?,
        Cmd::Repeat(mode) => {
            let mode = match mode {
                1 => MediaPlaybackAutoRepeatMode::Track,
                2 => MediaPlaybackAutoRepeatMode::List,
                _ => MediaPlaybackAutoRepeatMode::None,
            };
            s.TryChangeAutoRepeatModeAsync(mode)?.join()?
        }
        Cmd::Refresh => true,
    };
    Ok(())
}
