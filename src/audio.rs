//! WASAPI peak-meter gate for drivers whose battery read disturbs the headset's audio.
//!
//! A driver that parks its dongle in a bootloader / remote mode must not do it in the middle of a
//! call: the microphone (or playback) would go silent for the length of the session. The endpoints
//! belonging to the device are read twice, 120 ms apart; both flows must be at zero before the
//! driver is allowed to talk. When a meter cannot be read at all, the device counts as busy: it is
//! always safer to show the last known level than to cut a stream.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// True when a matching endpoint exists and every one of them is silent.
pub fn silent(name_part: &str) -> bool {
    // Meters need COM. Poll threads are in the MTA; CoInitializeEx keeps this correct on any
    // thread (S_FALSE for an apartment already up still balances one CoUninitialize).
    unsafe {
        let com = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED).is_ok();
        let quiet = silent_com(name_part);
        if com {
            windows::Win32::System::Com::CoUninitialize();
        }
        quiet
    }
}

unsafe fn silent_com(name_part: &str) -> bool {
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return false;
        };
        let mut found = false;
        for flow in [eRender, eCapture] {
            let Ok(coll) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
                return false;
            };
            let Ok(n) = coll.GetCount() else { continue };
            for i in 0..n {
                let Ok(dev) = coll.Item(i) else { continue };
                if !is_endpoint(&dev, name_part) {
                    continue;
                }
                found = true;
                let Ok(meter) = dev.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) else {
                    return false;
                };
                let Ok(a) = meter.GetPeakValue() else { return false };
                crate::util::sleep_ms(120);
                let Ok(b) = meter.GetPeakValue() else { return false };
                if a > 0.004 || b > 0.004 {
                    return false;
                }
            }
        }
        // No matching endpoint: the name may be localized differently, so do not risk the audio.
        found
    }
}

/// Endpoint id -> friendly name match, cached once per endpoint per name fragment. Names are
/// localized around the device part, so only a fragment of the hardware name is matched.
static MATCH: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

unsafe fn is_endpoint(dev: &windows::Win32::Media::Audio::IMMDevice, name_part: &str) -> bool {
    unsafe {
        let Ok(pw) = dev.GetId() else { return false };
        let id = pw.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(pw.0 as *const core::ffi::c_void));
        let map = MATCH.get_or_init(|| Mutex::new(HashMap::new()));
        let key = format!("{name_part}|{id}");
        let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = map.get(&key) {
            return *known;
        }
        let is = friendly_name(dev).to_lowercase().contains(&name_part.to_lowercase());
        map.insert(key, is);
        is
    }
}

unsafe fn friendly_name(dev: &windows::Win32::Media::Audio::IMMDevice) -> String {
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Variant::VT_LPWSTR;
    unsafe {
        let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { return String::new() };
        let Ok(mut pv) = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME) else { return String::new() };
        let name = if pv.Anonymous.Anonymous.vt == VT_LPWSTR {
            windows::core::PCWSTR::from_raw(pv.Anonymous.Anonymous.Anonymous.pwszVal.0).to_string().unwrap_or_default()
        } else {
            String::new()
        };
        let _ = PropVariantClear(&mut pv);
        name
    }
}

/// PKEY_Device_FriendlyName (a45c254e-df1c-4efd-8020-67d146a850e0, pid 14).
const PKEY_DEVICE_FRIENDLY_NAME: windows::Win32::Foundation::PROPERTYKEY =
    windows::Win32::Foundation::PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
