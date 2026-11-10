//! Per-app volume: changes the Windows volume-mixer level of the player
//! itself rather than the whole system.

use windows::core::Interface;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator,
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
