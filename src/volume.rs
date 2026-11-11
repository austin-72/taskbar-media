//! Per-app volume: changes the Windows volume-mixer level of the player
//! itself rather than the whole system.

use windows::core::Interface;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

/// Volume change per wheel notch.
const STEP: f32 = 0.04;

/// Raise or lower the mixer volume of the app with this id, on the default
/// output device. Returns the new level as a percentage, or None if the app
/// has no audio session there (the caller then falls back to system volume).
pub fn adjust(app_id: &str, up: bool) -> Option<u8> {
    if app_id.is_empty() {
        return None;
    }
    unsafe { adjust_inner(&app_id.to_lowercase(), up).ok().flatten() }
}

unsafe fn adjust_inner(app_id: &str, up: bool) -> windows::core::Result<Option<u8>> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
    let sessions = manager.GetSessionEnumerator()?;

    // A player can have several sessions (browsers do); keep them in step.
    let mut result = None;
    for i in 0..sessions.GetCount()? {
        let Ok(control) = sessions.GetSession(i) else { continue };
        let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
        let pid = control2.GetProcessId().unwrap_or(0);
        if pid == 0 || !crate::launch::process_matches(pid, app_id) {
            continue;
        }
        let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
        let level = match result {
            Some(level) => level,
            None => {
                let current = volume.GetMasterVolume().unwrap_or(1.0);
                let level = (current + if up { STEP } else { -STEP }).clamp(0.0, 1.0);
                result = Some(level);
                level
            }
        };
        let _ = volume.SetMasterVolume(level, std::ptr::null());
    }
    Ok(result.map(|level| (level * 100.0).round() as u8))
}

/// A change to make before reading the volume back.
#[derive(Clone, Copy)]
enum Change {
    Nothing,
    Level(f32),
    Mute(bool),
}

/// The current volume as (level 0..=1, muted). With `system` false this is
/// the player's own mixer volume, falling back to the system volume when the
/// player has no mixer entry on the default output.
pub fn read(app_id: &str, system: bool) -> Option<(f32, bool)> {
    apply(app_id, system, Change::Nothing)
}

/// Set the volume and return the new state.
pub fn set_level(app_id: &str, system: bool, level: f32) -> Option<(f32, bool)> {
    apply(app_id, system, Change::Level(level.clamp(0.0, 1.0)))
}

/// Mute or unmute and return the new state.
pub fn set_mute(app_id: &str, system: bool, mute: bool) -> Option<(f32, bool)> {
    apply(app_id, system, Change::Mute(mute))
}

fn apply(app_id: &str, system: bool, change: Change) -> Option<(f32, bool)> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        if !system && !app_id.is_empty() {
            if let Some(state) = apply_to_player(&device, &app_id.to_lowercase(), change) {
                return Some(state);
            }
        }
        let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        match change {
            Change::Level(level) => {
                let _ = endpoint.SetMasterVolumeLevelScalar(level, std::ptr::null());
            }
            Change::Mute(mute) => {
                let _ = endpoint.SetMute(mute, std::ptr::null());
            }
            Change::Nothing => {}
        }
        let level = endpoint.GetMasterVolumeLevelScalar().ok()?;
        let muted = endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false);
        Some((level, muted))
    }
}

/// The same for the player's own audio sessions. None if it has none.
unsafe fn apply_to_player(device: &IMMDevice, app_id: &str, change: Change) -> Option<(f32, bool)> {
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).ok()?;
    let sessions = manager.GetSessionEnumerator().ok()?;
    let mut state = None;
    for i in 0..sessions.GetCount().ok()? {
        let Ok(control) = sessions.GetSession(i) else { continue };
        let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
        let pid = control2.GetProcessId().unwrap_or(0);
        if pid == 0 || !crate::launch::process_matches(pid, app_id) {
            continue;
        }
        let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
        match change {
            Change::Level(level) => {
                let _ = volume.SetMasterVolume(level, std::ptr::null());
            }
            Change::Mute(mute) => {
                let _ = volume.SetMute(mute, std::ptr::null());
            }
            Change::Nothing => {}
        }
        if state.is_none() {
            let level = volume.GetMasterVolume().unwrap_or(1.0);
            let muted = volume.GetMute().map(|m| m.as_bool()).unwrap_or(false);
            state = Some((level, muted));
        }
    }
    state
}

/// One app in the Windows volume mixer.
#[derive(Clone, PartialEq, Default)]
pub struct MixerEntry {
    /// Identifies the app: its exe name in lower case, or "" for the system
    /// (master) volume.
    pub key: String,
    /// Name to show.
    pub name: String,
    /// 0..=1.
    pub level: f32,
    pub muted: bool,
}

const SYSTEM_SOUNDS: &str = "*system sounds";

/// Run `visit(key, display name, volume control)` for every app audio
/// session on the default output device.
unsafe fn each_session(mut visit: impl FnMut(&str, &str, &ISimpleAudioVolume)) -> Option<()> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).ok()?;
    let sessions = manager.GetSessionEnumerator().ok()?;
    for i in 0..sessions.GetCount().ok()? {
        let Ok(control) = sessions.GetSession(i) else { continue };
        // Sessions of apps that have closed linger for a while as expired.
        if control.GetState().is_ok_and(|state| state == AudioSessionStateExpired) {
            continue;
        }
        let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
        let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
        let pid = control2.GetProcessId().unwrap_or(0);
        // S_OK (0) means it is the session for Windows' own sounds.
        if control2.IsSystemSoundsSession().0 == 0 || pid == 0 {
            visit(SYSTEM_SOUNDS, "System sounds", &volume);
        } else if let Some(exe) = crate::launch::process_exe(pid) {
            visit(&exe.to_lowercase(), &crate::launch::display_name(&exe), &volume);
        }
    }
    Some(())
}

/// Every app currently in the volume mixer, one entry per app (an app's
/// several sessions are treated as one), sorted by name with "System sounds"
/// last.
pub fn mixer() -> Vec<MixerEntry> {
    let mut entries: Vec<MixerEntry> = Vec::new();
    unsafe {
        each_session(|key, name, volume| {
            if entries.iter().any(|e| e.key == key) {
                return;
            }
            entries.push(MixerEntry {
                key: key.to_string(),
                name: name.to_string(),
                level: volume.GetMasterVolume().unwrap_or(1.0),
                muted: volume.GetMute().map(|m| m.as_bool()).unwrap_or(false),
            });
        });
    }
    entries.sort_by_key(|e| (e.key == SYSTEM_SOUNDS, e.name.to_lowercase()));
    entries
}

/// Set the volume of one app in the mixer (all of its sessions).
pub fn mixer_set_level(key: &str, level: f32) {
    let level = level.clamp(0.0, 1.0);
    unsafe {
        each_session(|k, _, volume| {
            if k == key {
                let _ = volume.SetMasterVolume(level, std::ptr::null());
            }
        });
    }
}

/// Mute or unmute one app in the mixer.
pub fn mixer_set_mute(key: &str, mute: bool) {
    unsafe {
        each_session(|k, _, volume| {
            if k == key {
                let _ = volume.SetMute(mute, std::ptr::null());
            }
        });
    }
}
