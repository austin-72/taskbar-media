//! Audio output devices: list them, find the current one, switch to another.

use std::ffi::c_void;

use windows::core::{IUnknown, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

#[derive(Clone, PartialEq, Default)]
pub struct Device {
    /// Windows' identifier for the device; stable across restarts.
    pub id: String,
    /// For example "Speakers (Realtek Audio)".
    pub name: String,
    /// Headphones or a headset, as opposed to speakers or a display.
    pub headphones: bool,
}

const PKEY_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};
const PKEY_FORM_FACTOR: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e),
    pid: 0,
};

unsafe fn describe(device: &IMMDevice) -> Option<Device> {
    let raw = device.GetId().ok()?;
    let id = raw.to_string().unwrap_or_default();
    CoTaskMemFree(Some(raw.0 as *const c_void));
    if id.is_empty() {
        return None;
    }
    let mut name = String::new();
    let mut headphones = false;
    if let Ok(store) = device.OpenPropertyStore(STGM_READ) {
        if let Ok(value) = store.GetValue(&PKEY_FRIENDLY_NAME) {
            name = value.to_string();
        }
        if let Ok(value) = store.GetValue(&PKEY_FORM_FACTOR) {
            // EndpointFormFactor: 3 is Headphones, 5 is Headset.
            headphones = matches!(u32::try_from(&value), Ok(3) | Ok(5));
        }
    }
    if name.is_empty() {
        name = "Audio device".to_string();
    }
    let lower = name.to_lowercase();
    headphones |= ["headphone", "headset", "earphone", "earbud", "airpods", "buds"]
        .iter()
        .any(|word| lower.contains(word));
    Some(Device { id, name, headphones })
}

unsafe fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
}

/// Every connected, enabled playback device.
pub fn list() -> Vec<Device> {
    unsafe {
        let mut devices = Vec::new();
        let Ok(collection) = enumerator().and_then(|e| e.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE))
        else {
            return devices;
        };
        for i in 0..collection.GetCount().unwrap_or(0) {
            if let Some(device) = collection.Item(i).ok().and_then(|d| describe(&d)) {
                devices.push(device);
            }
        }
        devices.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        devices
    }
}

/// The device sound currently plays through.
pub fn current() -> Option<Device> {
    unsafe {
        let device = enumerator().ok()?.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        describe(&device)
    }
}

/// Make the device with this id the default output, as picking it in the
/// Windows sound flyout would.
///
/// Windows has no public API for this. Like every other switcher, this uses
/// the undocumented `IPolicyConfig` interface, which has been the same on
/// every version since Windows 7 but could change without notice; any
/// failure is reported as `false` and nothing else is affected.
pub fn set_default(id: &str) -> bool {
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    // Position of SetDefaultEndpoint in the interface: three IUnknown
    // methods, then ten others.
    const SET_DEFAULT_ENDPOINT: usize = 13;
    const RELEASE: usize = 2;
    type SetDefaultEndpoint = unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;

    if id.is_empty() {
        return false;
    }
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let Ok(unknown) = CoCreateInstance::<_, IUnknown>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else {
            return false;
        };
        let mut policy: *mut c_void = std::ptr::null_mut();
        if unknown.query(&IID_POLICY_CONFIG, &mut policy).is_err() || policy.is_null() {
            return false;
        }
        let vtable = *(policy as *const *const *const c_void);
        let set: SetDefaultEndpoint = std::mem::transmute(*vtable.add(SET_DEFAULT_ENDPOINT));
        let release: Release = std::mem::transmute(*vtable.add(RELEASE));
        // Roles 0 (console) and 1 (multimedia): what the Windows flyout
        // changes. Role 2, the device for calls, is left alone.
        let ok = set(policy, PCWSTR(wide.as_ptr()), 0).is_ok();
        let _ = set(policy, PCWSTR(wide.as_ptr()), 1);
        release(policy);
        ok
    }
}
