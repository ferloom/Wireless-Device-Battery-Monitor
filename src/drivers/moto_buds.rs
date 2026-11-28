//! Motorola Moto Buds over their vendor GATT service. Windows does not expose the three battery
//! levels, but the official Moto Buds app asks opcode 0x0005 and receives a notification with
//! [left, right, case]: 0xFF = unknown, bit 7 = charging, low 7 bits = percent. Both generations
//! of the service UUID are probed, and the notification of the matching generation is subscribed
//! before the query. Nothing is ever written to the OTA service.
//!
//! Protocol facts from Juanipis/motobuds, ThisWasAryan/MotoBudsDesktopUtility and StudioKeys/Quietbar
//! (all MIT). Covers the plain Moto Buds / Buds+ / Buds Bass family; "Buds Pro / Sound by Bose" is a
//! different device and is not matched here.
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Devices::Bluetooth::BluetoothCacheMode;
use windows::Devices::Bluetooth::BluetoothLEDevice;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus, GattSession, GattValueChangedEventArgs,
};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};
use windows::Win32::Devices::Properties::DEVPKEY_Device_FriendlyName;
use windows::core::{GUID, Ref};

use super::sdi::{DevSet, IS_CONNECTED};
use crate::ble;
use crate::device::{Buds, DeviceKind, DeviceStatus, PollContext};
use crate::util::debug_log;

const OP_BATTERY: u16 = 5;
const OP_BATTERY_NOTIFY: u16 = 9;
const UNKNOWN: u8 = 0xFF;
const CHARGING: u8 = 0x80;

/// (service, command write, notify) of the two UUID generations the app probes.
const SERVICES: [(u128, u128, u128); 2] = [
    (0xFC9D9FE0_4899_11EE_BE56_0242AC120002, 0xFC9D9FF0_4899_11EE_BE56_0242AC120002, 0xFC9D9FF1_4899_11EE_BE56_0242AC120002),
    (0x00009FE0_4899_11EE_BE56_0242AC120002, 0x00009FF0_4899_11EE_BE56_0242AC120002, 0x00009FF1_4899_11EE_BE56_0242AC120002),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Battery {
    left: i32,
    right: i32,
    case: i32,
    left_chg: bool,
    right_chg: bool,
    case_chg: bool,
}

fn level(byte: u8) -> (i32, bool) {
    if byte == UNKNOWN { (-1, false) } else { ((byte & !CHARGING).min(100) as i32, byte & CHARGING != 0) }
}

fn battery(payload: &[u8]) -> Option<Battery> {
    if payload.len() < 3 {
        return None;
    }
    let (left, left_chg) = level(payload[0]);
    let (right, right_chg) = level(payload[1]);
    let (case, case_chg) = level(payload[2]);
    (left >= 0 || right >= 0 || case >= 0).then_some(Battery { left, right, case, left_chg, right_chg, case_chg })
}

/// An 8-byte header (opcode BE, type, result, inner length LE, seq LE) then the payload.
fn frame(opcode: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend_from_slice(&opcode.to_be_bytes());
    v.extend_from_slice(&[0x80, 0x00]); // command, ack requested
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn decode(v: &[u8]) -> Option<(u16, &[u8])> {
    let len = u16::from_le_bytes([v.get(4).copied()?, v.get(5).copied()?]) as usize;
    if v.len() < 8 + len {
        return None;
    }
    Some((u16::from_be_bytes([v[0], v[1]]), &v[8..8 + len]))
}

/// The battery answer among the notifications received so far; other ones are ignored.
fn battery_reply(frames: &[Vec<u8>]) -> Option<Battery> {
    frames
        .iter()
        .filter_map(|v| decode(v))
        .filter(|(opcode, _)| matches!(*opcode, OP_BATTERY | OP_BATTERY_NOTIFY))
        .find_map(|(_, payload)| battery(payload))
}

/// The device key of the buds at Bluetooth address `addr`.
pub(super) fn key(addr: u64) -> String {
    format!("moto:{addr:012X}")
}

fn bytes(buf: &IBuffer) -> Option<Vec<u8>> {
    let len = buf.Length().ok()? as usize;
    let reader = DataReader::FromBuffer(buf).ok()?;
    let mut v = vec![0u8; len];
    reader.ReadBytes(&mut v).ok()?;
    Some(v)
}

#[derive(Clone)]
struct Found {
    addr: u64,
    name: String,
    /// Windows often reports paired earbuds simply as "Headphones"; then the GATT service, not the
    /// friendly name, identifies them and the UI gets a useful name.
    known_name: bool,
}

fn bt_addr(inst: &str) -> Option<u64> {
    let p = inst.find("\\DEV_")? + 5;
    u64::from_str_radix(inst.get(p..p + 12)?, 16).ok()
}

fn is_moto(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("moto buds") || n.contains("motorola buds")
}

/// Candidates whose vendor GATT service is worth probing. Windows can rename paired earbuds to a
/// generic audio name, so a name match alone misses them; the service check in `open` decides.
fn sounds_audio(name: &str) -> bool {
    let n = name.to_lowercase();
    ["headphone", "headset", "earbud", "buds", "pods", "audio", "motorola"].iter().any(|s| n.contains(s))
}

/// Paired buds that Windows reports as connected. Addresses are stable for a paired classic/LE
/// device, so they can be used both as the key and for opening the GATT session.
fn paired() -> Vec<Found> {
    let mut out: BTreeMap<u64, Found> = BTreeMap::new();
    for enumerator in ["BTHENUM", "BTHLE", "BTHLEDEVICE"] {
        let Some(set) = DevSet::present(Some(enumerator)) else { continue };
        for n in set.nodes() {
            let Some(addr) = bt_addr(&n.instance_id) else { continue };
            if !n.boolean(&IS_CONNECTED).unwrap_or(false) {
                continue;
            }
            let Some(name) = n.string(&DEVPKEY_Device_FriendlyName) else { continue };
            let known_name = is_moto(&name);
            if known_name || sounds_audio(&name) {
                out.entry(addr).or_insert(Found { addr, name, known_name });
            }
        }
    }
    out.into_values().collect()
}

struct Link {
    _device: BluetoothLEDevice,
    _session: Option<GattSession>,
    write: GattCharacteristic,
    notify: GattCharacteristic,
    token: i64,
    frames: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Some(s) = &self._session {
            let _ = s.SetMaintainConnection(false);
        }
        let _ = self.notify.RemoveValueChanged(self.token);
    }
}

fn open(addr: u64) -> Option<Link> {
    let log = |m: &str| debug_log(|| format!("moto buds {addr:012X}: {m}"));
    let device = ble::wait(BluetoothLEDevice::FromBluetoothAddressAsync(addr).ok()?, 4000)?;
    let session = device
        .BluetoothDeviceId()
        .ok()
        .and_then(|id| GattSession::FromDeviceIdAsync(&id).ok())
        .and_then(|op| ble::wait(op, 4000));
    if let Some(s) = &session {
        let _ = s.SetMaintainConnection(true);
    }
    for &(service, write_uuid, notify_uuid) in &SERVICES {
        let services = ble::wait(
            device.GetGattServicesForUuidWithCacheModeAsync(GUID::from_u128(service), BluetoothCacheMode::Uncached).ok()?,
            6000,
        )?;
        if services.Status().ok() != Some(GattCommunicationStatus::Success) {
            continue;
        }
        let list = services.Services().ok()?;
        if list.Size().ok()? == 0 {
            continue;
        }
        let svc = list.GetAt(0).ok()?;
        let writes = ble::wait(svc.GetCharacteristicsForUuidWithCacheModeAsync(GUID::from_u128(write_uuid), BluetoothCacheMode::Cached).ok()?, 4000)?;
        let notifies = ble::wait(svc.GetCharacteristicsForUuidWithCacheModeAsync(GUID::from_u128(notify_uuid), BluetoothCacheMode::Cached).ok()?, 4000)?;
        let (Ok(writes), Ok(notifies)) = (writes.Characteristics(), notifies.Characteristics()) else { continue };
        if writes.Size().ok()? == 0 || notifies.Size().ok()? == 0 {
            continue;
        }
        let write = writes.GetAt(0).ok()?;
        let notify = notifies.GetAt(0).ok()?;
        let frames = Arc::new(Mutex::new(Vec::new()));
        let sink = frames.clone();
        let handler = TypedEventHandler::new(move |_: Ref<GattCharacteristic>, args: Ref<GattValueChangedEventArgs>| {
            if let Some(args) = args.as_ref()
                && let Some(v) = args.CharacteristicValue().ok().and_then(|b| bytes(&b))
                && let Ok(mut m) = sink.lock()
            {
                m.push(v);
            }
            Ok(())
        });
        let Ok(token) = notify.ValueChanged(&handler) else { continue };
        let enabled = ble::wait(
            notify.WriteClientCharacteristicConfigurationDescriptorWithResultAsync(GattClientCharacteristicConfigurationDescriptorValue::Notify).ok()?,
            4000,
        );
        if enabled.and_then(|r| r.Status().ok()) != Some(GattCommunicationStatus::Success) {
            let _ = notify.RemoveValueChanged(token);
            continue;
        }
        log("linked");
        return Some(Link { _device: device, _session: session, write, notify, token, frames });
    }
    log("no vendor GATT service");
    None
}

fn ask(link: &Link) -> Option<Battery> {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let writer = DataWriter::new().ok()?;
    writer.WriteBytes(&frame(OP_BATTERY, seq, &[])).ok()?;
    let buf = writer.DetachBuffer().ok()?;
    if let Ok(mut m) = link.frames.lock() {
        m.clear(); // a notification can arrive while the write is in flight
    }
    let result = ble::wait(link.write.WriteValueWithResultAsync(&buf).ok()?, 4000)?;
    if result.Status().ok() != Some(GattCommunicationStatus::Success) {
        return None;
    }
    let end = Instant::now() + Duration::from_millis(2500);
    loop {
        if let Ok(m) = link.frames.lock()
            && let Some(b) = battery_reply(&m)
        {
            return Some(b);
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn cache() -> &'static Mutex<HashMap<u64, (Instant, Battery)>> {
    static M: OnceLock<Mutex<HashMap<u64, (Instant, Battery)>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn retry_after() -> &'static Mutex<HashMap<u64, Instant>> {
    static M: OnceLock<Mutex<HashMap<u64, Instant>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

const CACHE_SECS: u64 = 300;
const RETRY_SECS: u64 = 30;
static SEQ: AtomicU16 = AtomicU16::new(0);

pub fn poll(ctx: &mut PollContext) {
    let devices = paired();
    if devices.is_empty() {
        return;
    }
    for d in devices {
        let now = Instant::now();
        let mut fresh = None;
        if retry_after().lock().is_ok_and(|m| m.get(&d.addr).is_none_or(|at| at.elapsed().as_secs() >= RETRY_SECS)) {
            if let Some(link) = open(d.addr) {
                fresh = ask(&link);
            }
            if fresh.is_none() {
                if let Ok(mut m) = retry_after().lock() {
                    m.insert(d.addr, now);
                }
            } else if let Ok(mut m) = retry_after().lock() {
                m.remove(&d.addr);
            }
        }
        let b = if let Some(b) = fresh {
            if let Ok(mut m) = cache().lock() {
                m.insert(d.addr, (now, b));
            }
            b
        } else {
            let Some((at, b)) = cache().lock().ok().and_then(|m| m.get(&d.addr).copied()) else { continue };
            if at.elapsed().as_secs() > CACHE_SECS {
                continue;
            }
            b
        };
        let buds = Buds {
            left: b.left,
            right: b.right,
            left_charging: b.left_chg,
            right_charging: b.right_chg,
            case: (b.case >= 0).then_some((b.case, b.case_chg)),
        };
        let (percent, charging) = buds.weaker().unwrap_or((b.case, b.case_chg));
        let case = buds.case.map(|(p, c)| format!("case {p}%{}", if c { " ⚡" } else { "" })).unwrap_or_default();
        let s = DeviceStatus {
            key: key(d.addr),
            name: if d.known_name { d.name } else { "Moto Buds".into() },
            kind: DeviceKind::Earbuds,
            via: "Bluetooth".into(),
            percent,
            charging,
            buds: Some(buds),
            detail: format!("{}  {}", buds.text(), case),
            grace_sec: 300,
            ..Default::default()
        };
        ctx.out.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_levels_and_charging_bits() {
        let b = battery(&[50, 0x80 | 72, UNKNOWN]).unwrap();
        assert_eq!((b.left, b.right, b.case), (50, 72, -1));
        assert!(!b.left_chg && b.right_chg && !b.case_chg);
        assert_eq!(battery(&[UNKNOWN, UNKNOWN, UNKNOWN]), None);
        assert_eq!(battery(&[50, 60]), None);
        assert_eq!(level(0xFF), (-1, false));
        assert_eq!(level(0x80 | 100), (100, true));
    }

    #[test]
    fn request_and_reply_frames() {
        assert_eq!(frame(OP_BATTERY, 7, &[]), [0x00, 0x05, 0x80, 0x00, 0x00, 0x00, 0x07, 0x00]);
        let reply = frame(OP_BATTERY_NOTIFY, 7, &[50, 72, 90]);
        assert_eq!(decode(&reply), Some((OP_BATTERY_NOTIFY, &[50, 72, 90][..])));
        assert_eq!(decode(&reply[..10]), None);
    }

    #[test]
    fn the_battery_answer_is_found_after_other_notifications() {
        let frames = [frame(1, 1, &[0]), vec![0, 5], frame(OP_BATTERY, 1, &[50, 60, 70])];
        assert_eq!(battery_reply(&frames).map(|b| (b.left, b.right, b.case)), Some((50, 60, 70)));
        assert_eq!(battery_reply(&frames[..2]), None);
    }

    #[test]
    fn moto_names() {
        assert!(is_moto("Moto Buds+"));
        assert!(is_moto("motorola buds (LE)"));
        assert!(!is_moto("JBL Go 3"));
    }

    #[test]
    fn generic_windows_audio_names_are_probed() {
        assert!(sounds_audio("Headphones"));
        assert!(sounds_audio("Moto Buds+"));
        assert!(sounds_audio("Motorola Buds LE"));
        assert!(!sounds_audio("JBL Go 3"));
        assert!(!sounds_audio("Mouse"));
    }
}
