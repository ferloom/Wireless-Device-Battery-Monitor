//! One passive BLE advertisement watcher shared by the drivers that read battery levels from
//! broadcasts (AirPods proximity messages and Google Fast Pair). It only listens; nothing is sent.
//! Port of ble_adverts.cpp / fastpair.cpp, using the `windows` crate's WinRT projection instead of
//! hand-written ABI declarations.
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementWatcherStatus, BluetoothLEScanningMode,
};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, IBuffer};

use crate::drivers::airpods;

// ---- Fast Pair ---------------------------------------------------------------------------------

/// Battery split from the latest Fast Pair broadcast (percent, -1 unknown).
#[derive(Clone, Copy, Debug)]
pub struct FastPairBattery {
    pub left: i32,
    pub right: i32,
    pub case: i32,
    pub left_chg: bool,
    pub right_chg: bool,
    pub case_chg: bool,
    pub at: Instant,
}

fn fast_pair_map() -> &'static Mutex<HashMap<u64, FastPairBattery>> {
    static M: OnceLock<Mutex<HashMap<u64, FastPairBattery>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn fp_component(b: u8) -> (i32, bool) {
    let level = (b & 0x7F) as i32;
    if level <= 100 { (level, b & 0x80 != 0) } else { (-1, false) }
}

/// Service data of 16-bit UUID 0xFE2C after the UUID: [0] version 0, then fields with header
/// 0bLLLLTTTT; types 3/4 = battery (left, right, case; bit 7 charging, 0x7F unknown).
fn parse_fast_pair(d: &[u8]) -> Option<FastPairBattery> {
    if d.len() < 2 || d[0] != 0 {
        return None;
    }
    let mut i = 1;
    while i < d.len() {
        let len = (d[i] >> 4) as usize;
        let ty = d[i] & 0x0F;
        if i + 1 + len > d.len() {
            return None;
        }
        if (ty == 3 || ty == 4) && len >= 1 {
            let (left, left_chg) = fp_component(d[i + 1]);
            let (right, right_chg) = if len >= 2 { fp_component(d[i + 2]) } else { (-1, false) };
            let (case, case_chg) = if len >= 3 { fp_component(d[i + 3]) } else { (-1, false) };
            let b = FastPairBattery { left, right, case, left_chg, right_chg, case_chg, at: Instant::now() };
            return (left >= 0 || right >= 0 || case >= 0).then_some(b);
        }
        i += 1 + len;
    }
    None
}

/// Broadcasts are kept this long, and at most this many addresses (phones and earbuds nearby
/// rotate their random addresses every few minutes, so without limits a crowded place would grow
/// the map without end).
pub const KEEP_SECS: u64 = 300;
pub const MAX_ADDRESSES: usize = 64;

/// Inserts `v` for `addr`, drops entries older than `keep_secs`, and evicts the oldest entries
/// beyond `max`.
pub fn insert_bounded<V>(m: &mut HashMap<u64, V>, addr: u64, v: V, at: impl Fn(&V) -> Instant, keep_secs: u64, max: usize) {
    m.insert(addr, v);
    m.retain(|_, v| at(v).elapsed().as_secs() < keep_secs);
    while m.len() > max {
        let Some(oldest) = m.iter().min_by_key(|(_, v)| at(v)).map(|(k, _)| *k) else { break };
        m.remove(&oldest);
    }
}

/// Latest broadcast of the device with this Bluetooth address, if heard in the last 5 minutes.
pub fn fast_pair_lookup(addr: u64) -> Option<FastPairBattery> {
    let m = fast_pair_map().lock().ok()?;
    m.get(&addr).filter(|b| b.at.elapsed().as_secs() < KEEP_SECS).copied()
}

// ---- watcher -----------------------------------------------------------------------------------

fn buffer_bytes(buf: &IBuffer) -> Option<Vec<u8>> {
    let len = buf.Length().ok()? as usize;
    let reader = DataReader::FromBuffer(buf).ok()?;
    let mut v = vec![0u8; len];
    reader.ReadBytes(&mut v).ok()?;
    Some(v)
}

static RECEIVED: AtomicUsize = AtomicUsize::new(0);

/// Number of advertisements received so far (diagnostics).
pub fn received_count() -> usize {
    RECEIVED.load(Ordering::Relaxed)
}

fn on_received(args: &BluetoothLEAdvertisementReceivedEventArgs) -> windows::core::Result<()> {
    RECEIVED.fetch_add(1, Ordering::Relaxed);
    let adv = args.Advertisement()?;
    let addr = args.BluetoothAddress()?;
    let rssi = args.RawSignalStrengthInDBm()? as i32;

    let mdata = adv.GetManufacturerDataByCompanyId(0x004C)?;
    for i in 0..mdata.Size()? {
        if let Some(bytes) = mdata.GetAt(i).and_then(|m| m.Data()).ok().and_then(|b| buffer_bytes(&b)) {
            airpods::on_advert(addr, rssi, &bytes);
        }
    }
    // AD type 0x16 = Service Data, 16-bit UUID; 2C FE (LE) = Google Fast Pair.
    let sections = adv.GetSectionsByType(0x16)?;
    for i in 0..sections.Size()? {
        if let Some(bytes) = sections.GetAt(i).and_then(|s| s.Data()).ok().and_then(|b| buffer_bytes(&b))
            && bytes.len() > 2
            && bytes[0] == 0x2C
            && bytes[1] == 0xFE
            && let Some(b) = parse_fast_pair(&bytes[2..])
            && let Ok(mut m) = fast_pair_map().lock()
        {
            insert_bounded(&mut m, addr, b, |v| v.at, KEEP_SECS, MAX_ADDRESSES);
        }
    }
    Ok(())
}

/// When the watcher last started listening: broadcasts heard after it are fresh.
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);

/// When the current listening started (None = not listening yet).
pub fn listening_since() -> Option<Instant> {
    *STARTED.lock().ok()?
}

fn mark_started() {
    if let Ok(mut t) = STARTED.lock() {
        *t = Some(Instant::now());
    }
}

fn watcher() -> &'static Mutex<Option<BluetoothLEAdvertisementWatcher>> {
    static W: OnceLock<Mutex<Option<BluetoothLEAdvertisementWatcher>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(None))
}

/// Starts the watcher, or restarts it after the radio was switched off. Returns true when it was
/// just started, so the caller can give devices a moment (they broadcast every 1-2 s) to be heard.
pub fn ensure_watcher() -> bool {
    let Ok(mut guard) = watcher().lock() else { return false };
    if let Some(w) = guard.as_ref() {
        // A stop requested at the end of the previous read may still be in progress.
        for _ in 0..20 {
            if w.Status().ok() != Some(BluetoothLEAdvertisementWatcherStatus::Stopping) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let st = w.Status().unwrap_or(BluetoothLEAdvertisementWatcherStatus::Started);
        if st == BluetoothLEAdvertisementWatcherStatus::Stopped
            || st == BluetoothLEAdvertisementWatcherStatus::Aborted
            || st == BluetoothLEAdvertisementWatcherStatus::Created
        {
            let ok = w.Start().is_ok();
            if ok {
                mark_started();
            }
            return ok;
        }
        return false;
    }
    let start = || -> windows::core::Result<BluetoothLEAdvertisementWatcher> {
        let w = BluetoothLEAdvertisementWatcher::new()?;
        w.SetScanningMode(BluetoothLEScanningMode::Passive)?;
        w.Received(&TypedEventHandler::new(|_, args: windows::core::Ref<BluetoothLEAdvertisementReceivedEventArgs>| {
            if let Some(a) = args.as_ref() {
                let _ = on_received(a);
            }
            Ok(())
        }))?;
        w.Start()?;
        Ok(w)
    };
    match start() {
        Ok(w) => {
            mark_started();
            *guard = Some(w);
            true
        }
        Err(_) => false,
    }
}

// ---- GATT Battery Service (phones) --------------------------------------------------------------

/// Waits up to `ms` for a WinRT operation; cancels it when it takes longer.
fn wait<T: windows::core::RuntimeType + 'static>(op: windows_future::IAsyncOperation<T>, ms: u64) -> Option<T> {
    use windows_future::AsyncStatus;
    let end = Instant::now() + std::time::Duration::from_millis(ms);
    loop {
        match op.Status().ok()? {
            AsyncStatus::Completed => return op.GetResults().ok(),
            AsyncStatus::Started => {}
            _ => return None,
        }
        if Instant::now() >= end {
            let _ = op.Cancel();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// When a phone never answered: not asked again for this long (a phone out of range, or one
/// without the Battery Service, would otherwise slow every read down). A phone that answered
/// before is asked again sooner.
const GATT_RETRY_SECS: u64 = 15 * 60;
const GATT_RETRY_KNOWN_SECS: u64 = 60;

fn gatt_failed() -> &'static Mutex<HashMap<u64, Instant>> {
    static M: OnceLock<Mutex<HashMap<u64, Instant>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// A phone's Battery Level characteristic, kept between reads with a GATT session that keeps the
/// Bluetooth LE link up. Without it the phone drops the link after a read, and Windows does not
/// always bring it back for the next one, so the level stayed at its first value.
struct Link {
    _device: windows::Devices::Bluetooth::BluetoothLEDevice,
    session: Option<windows::Devices::Bluetooth::GenericAttributeProfile::GattSession>,
    level: windows::Devices::Bluetooth::GenericAttributeProfile::GattCharacteristic,
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Some(session) = &self.session {
            let _ = session.SetMaintainConnection(false);
        }
    }
}

// Record the first complete snapshot in which a link was missing. One missed poll
// after a long sleep must not discard the link immediately.
const GATT_ABSENCE_GRACE: std::time::Duration = std::time::Duration::from_secs(60);
type CachedLinks<T> = HashMap<u64, (T, Option<Instant>)>;

fn prune_absent_links<T>(links: &mut CachedLinks<T>, active: &BTreeSet<u64>, now: Instant) {
    links.retain(|addr, (_, absent_since)| {
        if active.contains(addr) {
            *absent_since = None;
            true
        } else {
            now.duration_since(*absent_since.get_or_insert(now)) < GATT_ABSENCE_GRACE
        }
    });
}

pub(crate) fn retain_gatt_links(active: impl Iterator<Item = u64>) {
    if let Ok(mut cache) = links().lock()
        && !cache.is_empty()
    {
        prune_absent_links(&mut cache, &active.collect(), Instant::now());
    }
}

fn links() -> &'static Mutex<CachedLinks<Link>> {
    static M: OnceLock<Mutex<CachedLinks<Link>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn open_link(addr: u64) -> Option<Link> {
    use windows::Devices::Bluetooth::BluetoothCacheMode;
    use windows::Devices::Bluetooth::BluetoothLEDevice;
    use windows::Devices::Bluetooth::GenericAttributeProfile::{GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids, GattSession};
    let log = |m: &str| crate::util::debug_log(|| format!("phone {addr:012X}: {m}"));
    let Some(device) = wait(BluetoothLEDevice::FromBluetoothAddressAsync(addr).ok()?, 4000) else {
        log("no Bluetooth LE device");
        return None;
    };
    // Keeps the link up between reads (the phone would drop it after a few seconds).
    let session = device
        .BluetoothDeviceId()
        .ok()
        .and_then(|id| GattSession::FromDeviceIdAsync(&id).ok())
        .and_then(|op| wait(op, 4000));
    if let Some(s) = &session {
        let _ = s.SetMaintainConnection(true);
    }
    let services = wait(device.GetGattServicesForUuidWithCacheModeAsync(GattServiceUuids::Battery().ok()?, BluetoothCacheMode::Uncached).ok()?, 6000)?;
    if services.Status().ok()? != GattCommunicationStatus::Success {
        log(&format!("Battery Service: {:?}", services.Status().ok()));
        return None;
    }
    let list = services.Services().ok()?;
    let Some(svc) = (list.Size().ok()? > 0).then(|| list.GetAt(0).ok()).flatten() else {
        log("no Battery Service");
        return None;
    };
    let chars = wait(svc.GetCharacteristicsForUuidWithCacheModeAsync(GattCharacteristicUuids::BatteryLevel().ok()?, BluetoothCacheMode::Cached).ok()?, 4000)?;
    let list = chars.Characteristics().ok()?;
    let level = (list.Size().ok()? > 0).then(|| list.GetAt(0).ok()).flatten()?;
    log(&format!("linked (session kept: {})", session.is_some()));
    Some(Link { _device: device, session, level })
}

/// Battery level from the standard Bluetooth LE Battery Service (0x180F, Battery Level 0x2A19)
/// of a paired device, e.g. an iPhone. Only reads of a standard characteristic: nothing is
/// written. None when the device does not answer within a few seconds.
pub fn read_gatt_battery(addr: u64) -> Option<u8> {
    use windows::Devices::Bluetooth::BluetoothCacheMode;
    use windows::Devices::Bluetooth::GenericAttributeProfile::GattCommunicationStatus;
    let mut m = links().lock().ok()?;
    #[allow(clippy::map_entry)] // the lock is dropped before the failure is noted
    if !m.contains_key(&addr) {
        if gatt_failed().lock().ok()?.get(&addr).is_some_and(|at| at.elapsed().as_secs() < GATT_RETRY_SECS) {
            return None;
        }
        let link = open_link(addr);
        match link {
            Some(l) => {
                m.insert(addr, (l, None));
            }
            None => {
                drop(m);
                if let Ok(mut f) = gatt_failed().lock() {
                    insert_bounded(&mut f, addr, Instant::now(), |t| *t, GATT_RETRY_SECS, 64);
                }
                return None;
            }
        }
    }
    let read = |l: &Link| -> Option<u8> {
        let r = wait(l.level.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached).ok()?, 4000)?;
        if r.Status().ok()? != GattCommunicationStatus::Success {
            crate::util::debug_log(|| format!("phone {addr:012X}: read {:?}", r.Status().ok()));
            return None;
        }
        buffer_bytes(&r.Value().ok()?)?.first().copied().filter(|&b| b <= 100)
    };
    let level = m.get_mut(&addr).and_then(|(link, absent_since)| {
        *absent_since = None;
        read(link)
    });
    crate::util::debug_log(|| format!("phone {addr:012X}: level {level:?}"));
    if level.is_none() {
        // Linked before: dropped, opened again at the next read (after a short pause).
        m.remove(&addr);
        drop(m);
        if let Ok(mut f) = gatt_failed().lock() {
            let back = Instant::now() - std::time::Duration::from_secs(GATT_RETRY_SECS - GATT_RETRY_KNOWN_SECS);
            insert_bounded(&mut f, addr, back, |t| *t, GATT_RETRY_SECS, 64);
        }
    } else if let Ok(mut f) = gatt_failed().lock() {
        f.remove(&addr);
    }
    level
}

/// Stops listening until the next `ensure_watcher` (called at the end of every read). Broadcasts
/// heard so far stay available; the next read starts the watcher again and waits for fresh ones.
pub fn stop_watcher() {
    let Ok(guard) = watcher().lock() else { return };
    if let Some(w) = guard.as_ref()
        && w.Status().ok() == Some(BluetoothLEAdvertisementWatcherStatus::Started)
    {
        let _ = w.Stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_links_expire_only_after_a_grace_period() {
        let start = Instant::now();
        let mut cache = HashMap::from([(1, ((), None)), (2, ((), None))]);
        prune_absent_links(&mut cache, &BTreeSet::new(), start);
        assert_eq!(cache.len(), 2);
        prune_absent_links(&mut cache, &BTreeSet::from([1]), start + GATT_ABSENCE_GRACE);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[&1].1, None);
        prune_absent_links(&mut cache, &BTreeSet::new(), start + GATT_ABSENCE_GRACE);
        assert!(cache.contains_key(&1));
    }

    #[test]
    fn fast_pair_battery_field() {
        // version 0, then a type-3 field of length 3: left 85, right 80 charging, case unknown.
        let b = parse_fast_pair(&[0x00, 0x33, 85, 0x80 | 80, 0x7F]).unwrap();
        assert_eq!((b.left, b.right, b.case), (85, 80, -1));
        assert!(!b.left_chg && b.right_chg);
        // An account-key field before the battery field is skipped.
        let b = parse_fast_pair(&[0x00, 0x20, 0xAA, 0xBB, 0x13, 40]).unwrap();
        assert_eq!(b.left, 40);
        // Discoverable-mode model id only: no battery.
        assert!(parse_fast_pair(&[0x12, 0x34, 0x56]).is_none());
    }

    #[test]
    fn broadcast_caches_stay_bounded() {
        let mut m: HashMap<u64, Instant> = HashMap::new();
        for addr in 0..1000u64 {
            insert_bounded(&mut m, addr, Instant::now(), |v| *v, KEEP_SECS, MAX_ADDRESSES);
        }
        assert_eq!(m.len(), MAX_ADDRESSES);
        assert!(m.contains_key(&999)); // the newest stay
        // Entries past their time are dropped.
        let old = Instant::now() - std::time::Duration::from_secs(KEEP_SECS + 1);
        let mut m: HashMap<u64, Instant> = HashMap::from([(1, old), (2, old)]);
        insert_bounded(&mut m, 3, Instant::now(), |v| *v, KEEP_SECS, MAX_ADDRESSES);
        assert_eq!(m.keys().copied().collect::<Vec<_>>(), vec![3]);
    }
}
