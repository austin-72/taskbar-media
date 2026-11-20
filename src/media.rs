//! Background worker that talks to the Windows media session API
//! (GlobalSystemMediaTransportControls). Anything that reports "now playing"
//! to Windows shows up here: Spotify, browsers, Media Player, foobar2000, etc.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{Error, Interface, Result, RuntimeType, HRESULT};
use windows_future::{AsyncStatus, IAsyncOperation};
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
    let mut chooser = Chooser::default();
    let mut preferred_missing: Option<Instant> = None;
    let mut read_errors: u32 = 0;

    loop {
        // Nothing playing changes slowly, so poll less often then.
        let pause_ms = if fast_polls > 0 {
            120
        } else if last.playing {
            500
        } else {
            1000
        };
        match rx.recv_timeout(Duration::from_millis(pause_ms)) {
            Ok(first) => {
                // Run everything that is queued before the next (slower) read,
                // so quick repeated clicks do not each wait for a poll. Of
                // several queued seeks only the last one matters.
                let mut queue = vec![first];
                while let Ok(more) = rx.try_recv() {
                    if matches!(more, Cmd::Seek(_)) {
                        queue.retain(|c| !matches!(c, Cmd::Seek(_)));
                    }
                    queue.push(more);
                }
                // Controlling a player keeps the widget on it for a while.
                // (A bare refresh, sent when the choice of player changes,
                // is not that.)
                if queue.iter().any(|c| !matches!(c, Cmd::Refresh)) {
                    chooser.user_acted(Instant::now());
                }
                for cmd in queue {
                    if let Some(s) = &session {
                        let _ = execute(s, cmd);
                    }
                }
                // Poll quickly for a moment so the UI reflects the change.
                fast_polls = 8;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        fast_polls = fast_polls.saturating_sub(1);

        if manager.is_none() {
            manager = Manager::RequestAsync().and_then(wait).ok();
        }
        let Some(mgr) = &manager else { continue };

        let sessions = session_ids(mgr);

        // A player picked by hand that has been gone for a while (closed) is
        // forgotten, so it does not grab the widget back when it next starts.
        // (Not while "Spotify only" is merely hiding it.)
        match preferred() {
            Some(wanted) if !sessions.contains(&wanted) && !SPOTIFY_ONLY.load(Ordering::Relaxed) => {
                let since = *preferred_missing.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(15) {
                    set_preferred(None);
                    preferred_missing = None;
                }
            }
            _ => preferred_missing = None,
        }

        session = pick_session(mgr, &mut chooser);
        let mut snap = match &session {
            Some(s) => match read_session(s) {
                Ok((snap, thumb, have_meta)) => {
                    read_errors = 0;
                    let mut snap = snap;
                    let mut thumb = thumb;
                    // A single failed metadata read must not blank the text
                    // (and restart the artwork logic) for a track we know.
                    if !have_meta && last.active && last.app_id == snap.app_id {
                        snap.title = last.title.clone();
                        snap.artist = last.artist.clone();
                        snap.album = last.album.clone();
                        thumb = None;
                    }
                    let skip_art = !have_meta;
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
                    // Some players keep serving the previous cover for a while,
                    // or replace a low-quality one later, so keep checking at
                    // a low rate for as long as the track is shown.
                    let should_read = polls_since_change <= 6
                        || (snap.art.is_none() && polls_since_change % 6 == 0)
                        || polls_since_change % 12 == 0;
                    if should_read && !skip_art {
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
                    // One failed read (a slow player, a call that timed out)
                    // is not worth blanking the widget for: keep what is
                    // shown and try again.
                    read_errors += 1;
                    if read_errors < 3 && last.active {
                        continue;
                    }
                    read_errors = 0;
                    // The session may have gone away, or the manager died
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
    // Windows reorders its list as players start and stop; a fixed order
    // keeps the switcher arrows and the menu predictable.
    ids.sort_by_key(|id| id.to_lowercase());
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

/// Chooses which player the widget follows. Pure bookkeeping over
/// (app id, is it playing) pairs, so that it can be tested without Windows.
///
/// A player picked by hand wins. Otherwise: stay with the player being shown
/// while it plays. When it stops, move to a player that started around then
/// (shortly before the pause, or any time after it), but do not jump to
/// something that has long been making noise in the background (a muted tab,
/// a game) just because the user pressed pause. After a minute of being
/// paused the plain rule applies again: whatever is playing, preferring the
/// session Windows considers current, else that session, else any.
#[derive(Default)]
struct Chooser {
    current: String,
    /// When the followed player was last seen playing, or last controlled.
    last_active: Option<Instant>,
    /// When the followed player was first seen not playing.
    paused_at: Option<Instant>,
    /// When each player that is playing now began to.
    began: Vec<(String, Instant)>,
    had_preference: bool,
}

/// How long the widget stays on a paused player while others play.
const HOLD_AFTER_PAUSE: Duration = Duration::from_secs(60);
/// "I started the video, then paused the music": a player that began this
/// shortly before the pause counts as what the user moved on to.
const STARTED_BEFORE_PAUSE: Duration = Duration::from_secs(10);

impl Chooser {
    fn user_acted(&mut self, now: Instant) {
        if !self.current.is_empty() {
            self.last_active = Some(now);
        }
    }

    /// Which of `players` (app id, playing) to follow: its index. `preferred`
    /// is the player picked by hand and `system_current` the one Windows
    /// considers current, if any.
    fn decide(
        &mut self,
        players: &[(String, bool)],
        preferred: Option<&str>,
        system_current: Option<&str>,
        now: Instant,
    ) -> Option<usize> {
        // Note who has started playing since the last look.
        self.began.retain(|(id, _)| players.iter().any(|p| p.1 && p.0 == *id));
        for (id, _) in players.iter().filter(|p| p.1) {
            if !self.began.iter().any(|b| b.0 == *id) {
                self.began.push((id.clone(), now));
            }
        }
        // Handing the choice back ("Automatic") means choosing afresh, not
        // staying put on whatever had been picked.
        if self.had_preference && preferred.is_none() {
            self.current.clear();
            self.last_active = None;
            self.paused_at = None;
        }
        self.had_preference = preferred.is_some();

        let chosen = self.choose(players, preferred, system_current, now);
        match chosen {
            Some(i) => {
                let id = &players[i].0;
                let playing = players[i].1;
                if *id != self.current {
                    self.current = id.clone();
                    self.last_active = Some(now);
                    self.paused_at = (!playing).then_some(now);
                } else if playing {
                    self.last_active = Some(now);
                    self.paused_at = None;
                } else if self.paused_at.is_none() {
                    self.paused_at = Some(now);
                }
            }
            None => {
                self.current.clear();
                self.last_active = None;
                self.paused_at = None;
            }
        }
        chosen
    }

    fn choose(
        &self,
        players: &[(String, bool)],
        preferred: Option<&str>,
        system_current: Option<&str>,
        now: Instant,
    ) -> Option<usize> {
        // Of several sessions with one id (two windows of a browser), the
        // one that is playing.
        let index_of = |id: &str| {
            players
                .iter()
                .position(|p| p.0 == id && p.1)
                .or_else(|| players.iter().position(|p| p.0 == id))
        };

        // A player chosen from the menu wins for as long as it exists.
        if let Some(i) = preferred.and_then(index_of) {
            return Some(i);
        }

        if let Some(i) = index_of(&self.current).filter(|_| !self.current.is_empty()) {
            if players[i].1 {
                return Some(i);
            }
            // Paused. Whatever started around then, or since, takes over
            // (the latest, if several)...
            let paused = self.paused_at.unwrap_or(now);
            let successor = self
                .began
                .iter()
                .filter(|(id, at)| *id != self.current && *at + STARTED_BEFORE_PAUSE >= paused)
                .max_by_key(|(_, at)| *at);
            if let Some(j) = successor.and_then(|(id, _)| index_of(id)) {
                return Some(j);
            }
            // ...but otherwise stay put for a while.
            // (Counted from the pause, or from the last time it was
            // controlled, whichever is later.)
            let since = self.last_active.map_or(paused, |t| t.max(paused));
            if now.saturating_duration_since(since) < HOLD_AFTER_PAUSE {
                return Some(i);
            }
        }

        // Plain rule.
        if let Some(i) = system_current.and_then(|id| players.iter().position(|p| p.0 == id && p.1)) {
            return Some(i);
        }
        if let Some(i) = players.iter().position(|p| p.1) {
            return Some(i);
        }
        system_current.and_then(index_of).or(if players.is_empty() { None } else { Some(0) })
    }
}

/// The session to follow now, per the `Chooser`.
fn pick_session(mgr: &Manager, chooser: &mut Chooser) -> Option<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    let mut players: Vec<(String, bool)> = Vec::new();
    if let Ok(all) = mgr.GetSessions() {
        for s in all {
            if allowed(&s) {
                players.push((app_id(&s), is_playing(&s)));
                sessions.push(s);
            }
        }
    }
    let system_current = mgr.GetCurrentSession().ok().filter(allowed).map(|s| app_id(&s));
    let preferred = preferred();
    let chosen = chooser.decide(&players, preferred.as_deref(), system_current.as_deref(), Instant::now());
    chosen.and_then(|i| sessions.get(i).cloned())
}

/// Waits for a WinRT operation, but never for long: a player that stops
/// answering must not freeze the worker (and with it every button).
fn wait<T: RuntimeType + 'static>(op: IAsyncOperation<T>) -> Result<T> {
    const LIMIT: Duration = Duration::from_millis(2000);
    let begun = Instant::now();
    let mut pause = 1u64;
    loop {
        if op.Status()? != AsyncStatus::Started {
            return op.GetResults();
        }
        if begun.elapsed() > LIMIT {
            let _ = op.Cancel();
            // ERROR_TIMEOUT as an HRESULT.
            return Err(Error::from(HRESULT(0x800705B4u32 as i32)));
        }
        std::thread::sleep(Duration::from_millis(pause));
        pause = (pause * 2).min(16);
    }
}

fn read_session(s: &Session) -> Result<(Snapshot, Option<IRandomAccessStreamReference>, bool)> {
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
    let mut have_meta = false;
    if let Ok(props) = s.TryGetMediaPropertiesAsync().and_then(wait) {
        have_meta = true;
        snap.title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        snap.artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
        snap.album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
        if snap.artist.is_empty() {
            snap.artist = props.AlbumArtist().map(|h| h.to_string()).unwrap_or_default();
        }
        thumb = props.Thumbnail().ok();
    }
    Ok((snap, thumb, have_meta))
}

fn read_thumbnail(r: &IRandomAccessStreamReference) -> Result<Vec<u8>> {
    let stream = wait(r.OpenReadAsync()?)?;
    let size = stream.Size()?;
    if size == 0 || size > 32 * 1024 * 1024 {
        return Err(Error::empty());
    }
    let reader = DataReader::CreateDataReader(&stream)?;
    // One LoadAsync may deliver only part of the stream; a cut-off JPEG would
    // be drawn half grey and, being "the same bytes", never replaced.
    let size = size as u32;
    let mut loaded = 0u32;
    while loaded < size {
        let op: IAsyncOperation<u32> = reader.LoadAsync(size - loaded)?.cast()?;
        let got = wait(op)?;
        if got == 0 {
            break;
        }
        loaded += got;
    }
    let complete = loaded == size;
    let mut buf = vec![0u8; loaded.min(size) as usize];
    let read = if complete { reader.ReadBytes(&mut buf) } else { Ok(()) };
    let _ = reader.Close();
    let _ = stream.Close();
    read?;
    if !complete || !looks_like_image(&buf) {
        return Err(Error::empty());
    }
    Ok(buf)
}

/// An image by its signature: JPEG, PNG, GIF, BMP, WebP, TIFF or ICO (more
/// than players actually publish). Anything else is a failed read.
fn looks_like_image(b: &[u8]) -> bool {
    b.len() > 16
        && (b.starts_with(&[0xFF, 0xD8])
            || b.starts_with(&[0x89, b'P', b'N', b'G'])
            || b.starts_with(b"GIF8")
            || b.starts_with(b"BM")
            || b.starts_with(b"II*\0")
            || b.starts_with(b"MM\0*")
            || b.starts_with(&[0, 0, 1, 0])
            || (b.starts_with(b"RIFF") && &b[8..12] == b"WEBP"))
}

fn execute(s: &Session, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Prev => wait(s.TrySkipPreviousAsync()?)?,
        Cmd::Next => wait(s.TrySkipNextAsync()?)?,
        Cmd::Toggle => {
            // Some players offer Play and Pause but not the combined toggle.
            let info = s.GetPlaybackInfo()?;
            let toggle = info
                .Controls()
                .and_then(|c| c.IsPlayPauseToggleEnabled())
                .unwrap_or(true);
            if toggle {
                wait(s.TryTogglePlayPauseAsync()?)?
            } else if info.PlaybackStatus()? == PlaybackStatus::Playing {
                wait(s.TryPauseAsync()?)?
            } else {
                wait(s.TryPlayAsync()?)?
            }
        }
        Cmd::Seek(ticks) => wait(s.TryChangePlaybackPositionAsync(ticks)?)?,
        Cmd::Shuffle(on) => wait(s.TryChangeShuffleActiveAsync(on)?)?,
        Cmd::Repeat(mode) => {
            let mode = match mode {
                1 => MediaPlaybackAutoRepeatMode::Track,
                2 => MediaPlaybackAutoRepeatMode::List,
                _ => MediaPlaybackAutoRepeatMode::None,
            };
            wait(s.TryChangeAutoRepeatModeAsync(mode)?)?
        }
        Cmd::Refresh => true,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn players(list: &[(&str, bool)]) -> Vec<(String, bool)> {
        list.iter().map(|(id, playing)| (id.to_string(), *playing)).collect()
    }

    /// Run the chooser over a script of (seconds, players, preferred,
    /// system current) and return the id chosen at each step.
    fn run_script(steps: &[(u64, &[(&str, bool)], Option<&str>, Option<&str>)]) -> Vec<String> {
        let start = Instant::now();
        let mut chooser = Chooser::default();
        steps
            .iter()
            .map(|(at, list, preferred, current)| {
                let list = players(list);
                let now = start + Duration::from_secs(*at);
                match chooser.decide(&list, *preferred, *current, now) {
                    Some(i) => list[i].0.clone(),
                    None => String::new(),
                }
            })
            .collect()
    }

    #[test]
    fn nothing_then_one_player() {
        let got = run_script(&[(0, &[], None, None), (1, &[("spotify", false)], None, Some("spotify"))]);
        assert_eq!(got, ["", "spotify"]);
    }

    #[test]
    fn at_start_whatever_is_playing_wins() {
        let got = run_script(&[(0, &[("chrome", false), ("spotify", true)], None, Some("chrome"))]);
        assert_eq!(got, ["spotify"]);
        // Nothing playing: the session Windows considers current.
        let got = run_script(&[(0, &[("chrome", false), ("spotify", false)], None, Some("spotify"))]);
        assert_eq!(got, ["spotify"]);
    }

    #[test]
    fn pausing_does_not_jump_to_background_noise() {
        // A tab has been playing all along; Spotify is paused at 100 s.
        let both: &[(&str, bool)] = &[("chrome", true), ("spotify", true)];
        let paused: &[(&str, bool)] = &[("chrome", true), ("spotify", false)];
        let got = run_script(&[
            (0, &[("spotify", true)], None, Some("spotify")),
            (5, both, None, Some("spotify")),
            (100, paused, None, Some("spotify")),
            (130, paused, None, Some("spotify")),
            // After a minute paused, the plain rule: what is playing.
            (161, paused, None, Some("spotify")),
        ]);
        assert_eq!(got, ["spotify", "spotify", "spotify", "spotify", "chrome"]);
    }

    #[test]
    fn starting_something_then_pausing_moves_on() {
        // Start a video, then pause the music a few seconds later.
        let got = run_script(&[
            (0, &[("spotify", true)], None, Some("spotify")),
            (50, &[("chrome", true), ("spotify", true)], None, Some("spotify")),
            (54, &[("chrome", true), ("spotify", false)], None, Some("chrome")),
        ]);
        assert_eq!(got, ["spotify", "spotify", "chrome"]);
    }

    #[test]
    fn something_starting_while_paused_takes_over() {
        let got = run_script(&[
            (0, &[("spotify", true)], None, Some("spotify")),
            (10, &[("spotify", false)], None, Some("spotify")),
            (20, &[("chrome", false), ("spotify", false)], None, Some("spotify")),
            (30, &[("chrome", true), ("spotify", false)], None, Some("chrome")),
            // And resuming the first while the second still plays does not
            // steal it back.
            (40, &[("chrome", true), ("spotify", true)], None, Some("chrome")),
        ]);
        assert_eq!(got, ["spotify", "spotify", "spotify", "chrome", "chrome"]);
    }

    #[test]
    fn a_player_that_closes_is_left() {
        let got = run_script(&[
            (0, &[("chrome", false), ("spotify", true)], None, Some("spotify")),
            (5, &[("chrome", false)], None, Some("chrome")),
            (6, &[], None, None),
        ]);
        assert_eq!(got, ["spotify", "chrome", ""]);
    }

    #[test]
    fn a_hand_picked_player_wins_and_automatic_chooses_afresh() {
        let list: &[(&str, bool)] = &[("chrome", true), ("spotify", false)];
        let got = run_script(&[
            (0, list, None, Some("chrome")),
            (5, list, Some("spotify"), Some("chrome")),
            (6, list, Some("spotify"), Some("chrome")),
            // "Automatic": straight back to what is playing, no hold.
            (7, list, None, Some("chrome")),
            // A preference for a player that is not there is ignored.
            (8, list, Some("vlc"), Some("chrome")),
        ]);
        assert_eq!(got, ["chrome", "spotify", "spotify", "chrome", "chrome"]);
    }

    #[test]
    fn controlling_a_paused_player_keeps_the_widget_on_it() {
        let paused: &[(&str, bool)] = &[("chrome", true), ("spotify", false)];
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut chooser = Chooser::default();
        let list = players(&[("chrome", true), ("spotify", true)]);
        assert_eq!(chooser.decide(&players(&[("spotify", true)]), None, None, at(0)), Some(0));
        assert_eq!(chooser.decide(&list, None, None, at(1)), Some(1));
        let list = players(paused);
        assert_eq!(chooser.decide(&list, None, None, at(100)), Some(1));
        // The user seeks in the paused player at 150 s: held until 210 s.
        chooser.user_acted(at(150));
        assert_eq!(chooser.decide(&list, None, None, at(200)), Some(1));
        assert_eq!(chooser.decide(&list, None, None, at(211)), Some(0));
    }

    #[test]
    fn of_two_sessions_with_one_id_the_playing_one() {
        let got = Chooser::default().decide(&players(&[("chrome", false), ("chrome", true)]), None, None, Instant::now());
        assert_eq!(got, Some(1));
    }

    #[test]
    fn recognises_images_by_signature() {
        let with = |head: &[u8]| {
            let mut bytes = head.to_vec();
            bytes.resize(64, 0);
            bytes
        };
        assert!(looks_like_image(&with(&[0xFF, 0xD8, 0xFF, 0xE0])));
        assert!(looks_like_image(&with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A])));
        assert!(looks_like_image(&with(b"GIF89a")));
        assert!(looks_like_image(&with(b"BM")));
        assert!(looks_like_image(&with(b"RIFF\0\0\0\0WEBPVP8 ")));
        assert!(!looks_like_image(&with(b"RIFF\0\0\0\0WAVEfmt ")));
        assert!(!looks_like_image(&with(b"<html>")));
        assert!(!looks_like_image(&[0xFF, 0xD8]));
        assert!(!looks_like_image(&[]));
    }
}
