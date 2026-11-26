//! Logitech HID++ (VID 046D): Unifying / Bolt / Lightspeed / Nano receivers, wired or Bluetooth
//! devices and G headsets. Features are discovered through the root feature, so devices are
//! supported without a per-model table.
//!
//! Message: [report][device index][feature index][function << 4 | sw id][params...]
//!   report 0x10 = short (7 bytes), 0x11 = long (20 bytes)
//!   HID++ 2.0 error: [.., 0xFF, feature, fn|sw, code]   HID++ 1.0 error: [.., 0x8F, sub id, address, code]
//! Battery features: 0x1004 unified battery, 0x1000 battery level, 0x1001 battery voltage,
//! 0x1F20 ADC measurement (headsets). HID++ 1.0 devices: registers 0x0D and 0x07.
//! References: Solaar, HeadsetControl (see the original logitech.cpp).
//!
//! Receivers (Unifying / LIGHTSPEED / Nano), as Solaar (GPL-2.0+, lib/logitech_receiver/receiver.py,
//! base.py, notifications.py) documents them; implemented here from those facts (issue #41):
//!   * long register 0xB5, sub-register 0x20 + n - 1 = pairing information of slot n:
//!     [sub][destination][report interval][wpid hi][wpid lo][..][..][kind (low nibble)]; an error
//!     reply means the slot is empty. Only paired slots are pinged, and a paired device that does
//!     not answer (asleep, switched off) is reported as `connected: false`, so the app checks the
//!     receiver again every few seconds and shows the device as soon as it wakes - instead of only
//!     at the next full refresh.
//!   * register 0x00 (notification flags), byte 1 bit 0 = wireless notifications; writing register
//!     0x02 = 0x02 makes the receiver send a 0x41 "device connection" notification for every paired
//!     device: [10][index][41][protocol][flags][wpid lo][wpid hi], flags bit 6 = link not established.
//!   * a sleeping device answers a ping late: Solaar waits up to 4 s; error 0x09 (resource error) /
//!     0x04 (connect failed) = unreachable, 0x08 = no device with that index.
//!   * long register 0xB5, sub-register 0x30 + n - 1 = extended pairing information of slot n:
//!     [sub][serial x4][..]. The serial is the device's unit id (0x0003), so a device keeps the
//!     key `logi:unit:<serial>` while it sleeps or when reading its unit id fails (issue #586;
//!     checked on a G515 LS TKL and a G309 on C547 receivers). Bolt keeps it in 0x50 + n.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{self, HidColl, HidDev};
use crate::util::{Deadline, guess_kind, hex4, interpolate};
use std::collections::HashMap;

const SW_ID: u8 = 0x0D;

/// LIGHTSPEED dongles that speak only HID++ 2.0 and relay one paired device on index 1.
fn is_passthrough_dongle(pid: u16) -> bool {
    pid == 0xC54F
}

/// Logi Bolt receivers keep their pairing information in other sub-registers of 0xB5 than
/// Unifying / LIGHTSPEED (Solaar, receiver.py `BoltReceiver`): 0x50 + n instead of 0x20 + n - 1.
fn is_bolt_receiver(pid: u16) -> bool {
    pid == 0xC548
}

enum Reply {
    Ok(Vec<u8>),
    Err(u8),
    None,
}

struct Hidpp {
    s: Option<HidDev>,
    l: Option<HidDev>,
}

impl Hidpp {
    fn open(short_c: Option<&HidColl>, long_c: Option<&HidColl>) -> Option<Self> {
        let s = short_c.and_then(HidDev::open);
        let l = long_c.and_then(HidDev::open);
        (s.is_some() || l.is_some()).then_some(Self { s, l })
    }

    fn request(&self, idx: u8, b2: u8, b3: u8, params: &[u8], force_short: bool, timeout_ms: u64) -> Reply {
        let use_long = self.l.is_some() && !force_short;
        let dev = if use_long { self.l.as_ref() } else { self.s.as_ref() };
        let Some(dev) = dev else { return Reply::None };
        let mut msg = vec![0u8; if use_long { 20 } else { 7 }];
        msg[0] = if use_long { 0x11 } else { 0x10 };
        msg[1] = idx;
        msg[2] = b2;
        msg[3] = b3;
        for (i, v) in params.iter().enumerate() {
            if 4 + i < msg.len() {
                msg[4 + i] = *v;
            }
        }
        if !dev.write(&msg) {
            return Reply::None;
        }
        let end = Deadline::in_ms(timeout_ms);
        while !end.passed() {
            for d in [self.l.as_ref(), self.s.as_ref()].into_iter().flatten() {
                let Some(r) = d.read(15) else { continue };
                if r.len() < 7 || (r[0] != 0x10 && r[0] != 0x11) || r[1] != idx {
                    continue;
                }
                if r[2] == b2 && r[3] == b3 {
                    return Reply::Ok(r[4..].to_vec());
                }
                if (r[2] == 0xFF || r[2] == 0x8F) && r[3] == b2 && r[4] == b3 {
                    return Reply::Err(r[5]);
                }
            }
        }
        Reply::None
    }

    /// HID++ 2.0 call: feature index + function number.
    fn call(&self, idx: u8, feature: u8, func: u8, params: &[u8]) -> Option<Vec<u8>> {
        match self.request(idx, feature, (func << 4) | SW_ID, params, false, 500) {
            Reply::Ok(p) => Some(p),
            _ => None,
        }
    }

    fn feature_index(&self, idx: u8, feature_id: u16) -> u8 {
        self.call(idx, 0x00, 0, &[(feature_id >> 8) as u8, feature_id as u8]).and_then(|p| p.first().copied()).unwrap_or(0)
    }

    /// Sends a short HID++ 1.0 message to the receiver and returns every report read within `ms`
    /// (used to collect the 0x41 notifications that follow a "scan devices" request).
    fn send_and_collect(&self, b2: u8, b3: u8, params: &[u8], ms: u64) -> Vec<Vec<u8>> {
        let Some(s) = self.s.as_ref() else { return Vec::new() };
        let mut msg = vec![0x10, 0xFF, b2, b3, 0, 0, 0];
        for (i, v) in params.iter().take(3).enumerate() {
            msg[4 + i] = *v;
        }
        if !s.write(&msg) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let end = Deadline::in_ms(ms);
        while !end.passed() {
            for d in [self.s.as_ref(), self.l.as_ref()].into_iter().flatten() {
                if let Some(r) = d.read(15) {
                    out.push(r);
                }
            }
        }
        out
    }
}

/// One paired slot of a receiver: index, device kind from the pairing register, and whether the
/// receiver's 0x41 notification said the link is up (None = no notification seen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    idx: u8,
    kind: DeviceKind,
    link: Option<bool>,
    /// Wireless product id from the pairing register.
    wpid: u16,
    /// Serial from the pairing register (the device's unit id), so a sleeping device keeps the
    /// same key it had when it answered.
    serial: Option<u32>,
}

/// HID++ 1.0 device kind (pairing register / 0x41 notification, low nibble) -> DeviceKind.
fn pairing_kind(k: u8) -> DeviceKind {
    match k & 0x0F {
        0x01 => DeviceKind::Keyboard,
        0x02 | 0x08 => DeviceKind::Mouse,
        0x0B | 0x0C => DeviceKind::Gamepad,
        0x0D => DeviceKind::Headset,
        _ => DeviceKind::Other,
    }
}

/// Reply parameters of a 0xB5 read of sub-register `sub` (0x20 + n - 1) -> (wpid, kind nibble).
/// None when the slot is empty or the reply is not this sub-register.
fn parse_pairing(sub: u8, p: &[u8]) -> Option<(u16, u8)> {
    if p.len() < 8 || p[0] != sub {
        return None;
    }
    let wpid = ((p[3] as u16) << 8) | p[4] as u16;
    (wpid != 0 && wpid != 0xFFFF).then_some((wpid, p[7] & 0x0F))
}

/// Reply parameters of a 0xB5 read of Bolt sub-register `sub` (0x50 + n) -> (wpid, kind nibble,
/// serial). Layout as Solaar reads it: [sub][kind (low nibble)][wpid lo][wpid hi][serial x4].
fn parse_bolt_pairing(sub: u8, p: &[u8]) -> Option<(u16, u8, Option<u32>)> {
    if p.len() < 4 || p[0] != sub {
        return None;
    }
    let wpid = ((p[3] as u16) << 8) | p[2] as u16;
    if wpid == 0 || wpid == 0xFFFF {
        return None;
    }
    let serial = (p.len() >= 8).then(|| u32::from_be_bytes([p[4], p[5], p[6], p[7]])).filter(|&v| v != 0 && v != u32::MAX);
    Some((wpid, p[1] & 0x0F, serial))
}

/// Reply parameters of a 0xB5 read of extended pairing sub-register `sub` (0x30 + n - 1) ->
/// serial. Layout as Solaar reads it: [sub][serial x4][..].
fn parse_ext_pairing(sub: u8, p: &[u8]) -> Option<u32> {
    if p.len() < 5 || p[0] != sub {
        return None;
    }
    Some(u32::from_be_bytes([p[1], p[2], p[3], p[4]])).filter(|&v| v != 0 && v != u32::MAX)
}

/// A 0x41 device connection notification -> (device index, link established).
fn parse_link_notification(r: &[u8]) -> Option<(u8, bool)> {
    if r.len() < 7 || r[0] != 0x10 || r[2] != 0x41 || r[3] == 0 || !(1..=6).contains(&r[1]) {
        return None;
    }
    Some((r[1], r[4] & 0x40 == 0))
}

/// Paired slots of a receiver, from its pairing registers, with the link state the receiver
/// reports for each. None when the receiver has no readable pairing register (then every index
/// is pinged, as before).
fn receiver_slots(h: &Hidpp, bolt: bool) -> Option<Vec<Slot>> {
    let mut slots = Vec::new();
    let mut answered = false;
    for n in 1..=6u8 {
        let sub = if bolt { 0x50 + n } else { 0x20 + n - 1 };
        match h.request(0xFF, 0x83, 0xB5, &[sub], true, 900) {
            Reply::Ok(p) => {
                answered = true;
                let parsed = if bolt { parse_bolt_pairing(sub, &p) } else { parse_pairing(sub, &p).map(|(w, k)| (w, k, None)) };
                if let Some((wpid, kind, mut serial)) = parsed {
                    if !bolt {
                        let ext = 0x30 + n - 1;
                        if let Reply::Ok(p) = h.request(0xFF, 0x83, 0xB5, &[ext], true, 900) {
                            serial = parse_ext_pairing(ext, &p);
                        }
                    }
                    crate::util::debug_log(|| format!("  slot {n}: paired, wpid {}, kind {kind}, serial {serial:08X?}", hex4(wpid)));
                    slots.push(Slot { idx: n, kind: pairing_kind(kind), link: None, wpid, serial });
                }
            }
            Reply::Err(e) => {
                answered = true;
                crate::util::debug_log(|| format!("  slot {n}: empty (error {e:02X})"));
            }
            Reply::None => {}
        }
    }
    if !answered || slots.is_empty() {
        return None;
    }
    // Wireless notifications on (Solaar does the same), then ask for one 0x41 per paired device.
    if let Reply::Ok(f) = h.request(0xFF, 0x81, 0x00, &[], true, 900)
        && f.len() >= 3
        && f[1] & 0x01 == 0
    {
        let _ = h.request(0xFF, 0x80, 0x00, &[f[0], f[1] | 0x01, f[2]], true, 900);
    }
    for r in h.send_and_collect(0x80, 0x02, &[0x02], 400) {
        if let Some((idx, up)) = parse_link_notification(&r) {
            crate::util::debug_log(|| format!("  slot {idx}: link {}", if up { "up" } else { "down" }));
            if let Some(s) = slots.iter_mut().find(|s| s.idx == idx) {
                s.link = Some(up);
            }
        }
    }
    Some(slots)
}

/// Voltage -> % for a typical Li-ion cell (mice/keyboards).
fn li_ion_percent(mv: i32) -> i32 {
    const V: [i32; 13] = [4186, 4067, 3989, 3922, 3859, 3811, 3778, 3751, 3717, 3671, 3646, 3579, 3500];
    const P: [i32; 13] = [100, 90, 80, 70, 60, 50, 40, 30, 20, 10, 5, 2, 0];
    interpolate(mv, &V, &P)
}

/// Voltage -> % for G headsets (HeadsetControl and G HUB power models).
fn headset_percent(pid: u16, mv: i32) -> i32 {
    const P6: [i32; 6] = [100, 50, 30, 20, 5, 0];
    const P21: [i32; 21] = [100, 95, 90, 85, 80, 75, 70, 65, 60, 55, 50, 45, 40, 35, 30, 25, 20, 15, 10, 5, 0];
    match pid {
        0x0A66 => interpolate(mv, &[4200, 3850, 3790, 3750, 3680, 3330], &P6),
        0x0AC4 => interpolate(mv, &[4175, 3817, 3766, 3730, 3664, 3310], &P6),
        // G733 (0AFE revision): fitted to G HUB's readings on one headset, 3.81-3.83 V measured,
        // the rest estimated (issue #429). G HUB is much higher than the G633 curve here.
        0x0AFE => interpolate(
            mv,
            &[4100, 3950, 3870, 3840, 3828, 3820, 3813, 3790, 3760, 3730, 3700, 3670, 3600],
            &[100, 97, 93, 89, 86, 83, 78, 66, 48, 30, 14, 5, 0],
        ),
        0x0A5C | 0x0A89 | 0x0A5B | 0x0A87 => interpolate(mv, &[4100, 3950, 3850, 3750, 3650, 3500, 3300, 3150], &[100, 80, 60, 40, 20, 10, 5, 0]),
        0x0AB5 => interpolate(
            mv,
            &[4175, 4127, 4086, 4054, 4001, 3968, 3944, 3914, 3875, 3840, 3818, 3800, 3783, 3771, 3759, 3744, 3725, 3695, 3675, 3630, 3341],
            &P21,
        ),
        0x0AAA | 0x0ABA => interpolate(
            mv,
            &[4102, 4046, 3993, 3950, 3915, 3877, 3838, 3809, 3789, 3772, 3758, 3747, 3736, 3722, 3707, 3687, 3661, 3644, 3548, 3426, 3293],
            &P21,
        ),
        0x0AA7 | 0x0AFB => interpolate(mv, &[4150, 3830, 3780, 3740, 3670, 3320], &P6),
        _ => interpolate(mv, &[4100, 3850, 3780, 3730, 3650, 3300], &P6),
    }
}

fn kind_from_type(t: u8) -> DeviceKind {
    match t {
        0x00 | 0x02 => DeviceKind::Keyboard,
        0x03..=0x05 => DeviceKind::Mouse,
        0x08 => DeviceKind::Headset,
        0x0B | 0x0C => DeviceKind::Gamepad,
        _ => DeviceKind::Other,
    }
}

/// Fills battery fields from whichever battery feature the device has. Returns false if none.
fn read_battery20(h: &Hidpp, idx: u8, pid: u16, s: &mut DeviceStatus) -> bool {
    let f = h.feature_index(idx, 0x1004); // UNIFIED_BATTERY: get_status (fn 1)
    if f != 0
        && let Some(p) = h.call(idx, f, 1, &[]).filter(|p| p.len() >= 3)
    {
        let (soc, level, st) = (p[0] as i32, p[1], p[2]);
        if soc > 0 && soc <= 100 {
            s.percent = soc;
        } else if level != 0 {
            let (pct, txt) = if level & 8 != 0 {
                (100, "Full")
            } else if level & 4 != 0 {
                (60, "Good")
            } else if level & 2 != 0 {
                (20, "Low")
            } else {
                (5, "Critical")
            };
            s.percent = pct;
            s.level_text = txt.into();
        }
        s.charging = matches!(st, 1..=3);
        s.full = st == 3;
        return true;
    }
    let f = h.feature_index(idx, 0x1000); // BATTERY_STATUS
    if f != 0
        && let Some(p) = h.call(idx, f, 0, &[]).filter(|p| p.len() >= 3)
    {
        let st = p[2];
        s.charging = (1..=4).contains(&st);
        s.full = st == 3;
        s.percent = if p[0] <= 100 { p[0] as i32 } else { -1 };
        if s.percent == 0 && st == 3 {
            s.percent = 100;
        }
        return true;
    }
    let f = h.feature_index(idx, 0x1001); // BATTERY_VOLTAGE
    if f != 0
        && let Some(p) = h.call(idx, f, 0, &[]).filter(|p| p.len() >= 3)
    {
        s.millivolts = ((p[0] as i32) << 8) | p[1] as i32;
        s.charging = p[2] & 0x80 != 0;
        if s.millivolts != 0 {
            s.percent = li_ion_percent(s.millivolts);
        }
        return true;
    }
    let f = h.feature_index(idx, 0x1F20); // ADC_MEASUREMENT (headsets)
    if f != 0
        && let Some(p) = h.call(idx, f, 0, &[]).filter(|p| p.len() >= 3)
    {
        s.millivolts = ((p[0] as i32) << 8) | p[1] as i32;
        s.charging = p[2] == 0x03;
        if s.millivolts != 0 {
            s.percent = headset_percent(pid, s.millivolts);
        }
        return true;
    }
    false
}

fn read_name_and_type20(h: &Hidpp, idx: u8, s: &mut DeviceStatus) {
    let f = h.feature_index(idx, 0x0005); // DEVICE_NAME
    if f == 0 {
        return;
    }
    if let Some(p) = h.call(idx, f, 0, &[]).filter(|p| !p.is_empty()) {
        let len = p[0] as usize;
        let mut name = String::new();
        while name.len() < len {
            let Some(p) = h.call(idx, f, 1, &[name.len() as u8]).filter(|p| !p.is_empty()) else { break };
            let before = name.len();
            for &ch in &p {
                if name.len() >= len || ch == 0 {
                    break;
                }
                name.push(ch as char);
            }
            if name.len() == before {
                break;
            }
        }
        if !name.is_empty() {
            s.name = if name.starts_with("Logitech") || name.starts_with("G ") || name.starts_with("MX") { name } else { format!("Logitech {name}") };
        }
    }
    if let Some(p) = h.call(idx, f, 2, &[]).filter(|p| !p.is_empty()) {
        s.kind = kind_from_type(p[0]);
    }
}

/// DEVICE_INFORMATION (0x0003) unit id: identifies the physical device across receiver / cable.
fn read_unit_id20(h: &Hidpp, idx: u8) -> String {
    let f = h.feature_index(idx, 0x0003);
    if f == 0 {
        return String::new();
    }
    match h.call(idx, f, 0, &[]) {
        Some(p) if p.len() >= 5 && (p[1] | p[2] | p[3] | p[4]) != 0 => {
            format!("{}{}", hex4(((p[1] as u16) << 8) | p[2] as u16), hex4(((p[3] as u16) << 8) | p[4] as u16))
        }
        _ => String::new(),
    }
}

/// HID++ 1.0 devices: register 0x0D (battery %) or 0x07 (battery level 1..7).
fn read_battery10(h: &Hidpp, idx: u8, s: &mut DeviceStatus) -> bool {
    if let Reply::Ok(p) = h.request(idx, 0x81, 0x0D, &[], true, 500)
        && p.len() >= 3
        && p[0] != 0
        && p[0] <= 100
    {
        s.percent = p[0] as i32;
        s.charging = (0x21..=0x26).contains(&p[2]);
        return true;
    }
    if let Reply::Ok(p) = h.request(idx, 0x81, 0x07, &[], true, 500)
        && p.len() >= 2
        && p[0] != 0
    {
        const PCT: [i32; 8] = [0, 5, 10, 20, 35, 50, 75, 100];
        let lvl = p[0].min(7);
        s.percent = PCT[lvl as usize];
        s.level_text = match lvl {
            7 => "Full",
            5 | 6 => "Good",
            3 | 4 => "Low",
            _ => "Critical",
        }
        .into();
        s.charging = (0x21..=0x26).contains(&p[1]);
        return true;
    }
    false
}

/// A device behind a receiver: the receiver's `physical_key`, the index and the wpid (0 without a
/// pairing register).
type UnitSlot = (String, u8, u16);

/// Unit ids read while devices answered: a device keeps its `logi:unit:` key when reading the unit
/// id fails or it sleeps and its receiver has no serial for it (issue #586).
static UNIT_IDS: std::sync::Mutex<Option<HashMap<UnitSlot, String>>> = std::sync::Mutex::new(None);

/// The unit id for a device's key: the one just read, else the receiver's serial for the slot,
/// else the one remembered from an earlier read.
fn stable_unit(read: &str, serial: Option<u32>, remembered: Option<String>) -> Option<String> {
    if !read.is_empty() {
        return Some(read.to_string());
    }
    serial.map(|v| format!("{v:08X}")).or(remembered)
}

fn remember_unit(id: &UnitSlot, unit: &str) {
    if let Ok(mut m) = UNIT_IDS.lock() {
        m.get_or_insert_with(HashMap::new).insert(id.clone(), unit.to_string());
    }
}

fn remembered_unit(id: &UnitSlot) -> Option<String> {
    UNIT_IDS.lock().ok()?.as_ref()?.get(id).cloned()
}

/// Identifies the physical device a HID collection belongs to, so two receivers with the same
/// product id are polled separately (strips "&colNN" and the last "&NNNN" of the instance part).
fn physical_key(c: &HidColl) -> String {
    let p = c.path.to_lowercase();
    let parts: Vec<&str> = p.splitn(4, '#').collect();
    if parts.len() < 4 {
        return String::new();
    }
    let mut hw = parts[1].to_string();
    if let Some(col) = hw.find("&col") {
        let end = (col + 6).min(hw.len());
        hw.replace_range(col..end, "");
    }
    let mut inst = parts[2].to_string();
    if let Some(amp) = inst.rfind('&') {
        inst.truncate(amp);
    }
    format!("{hw}#{inst}")
}

fn poll_device(ctx: &mut PollContext, pid: u16, shorts: &[HidColl], longs: &[HidColl], suffix: &str) -> bool {
    if shorts.is_empty() && longs.is_empty() {
        return false;
    }
    let Some(h) = Hidpp::open(shorts.first(), longs.first()) else { return false };
    let main = longs.first().or(shorts.first()).unwrap();
    let product = hid::product_string(main);
    let is_receiver = product.contains("Receiver");

    let path = main.path.to_lowercase();
    let bluetooth = path.contains("{00001812-") || path.contains("{00001124-");
    let passthrough = is_passthrough_dongle(pid);
    let ping_ms = |idx: u8, ms: u64| match h.request(idx, 0x00, 0x10 | SW_ID, &[0, 0, 0x5A], false, ms) {
        Reply::Ok(p) if p.len() >= 3 && p[0] >= 2 => Reply::Ok(p),
        r => r,
    };
    let ping = |idx: u8| ping_ms(idx, 400);
    crate::util::debug_log(|| {
        format!("logitech {}: product '{product}', receiver {is_receiver}, bluetooth {bluetooth}, short {}, long {}", hex4(pid), shorts.len(), longs.len())
    });
    // A receiver that lists its paired devices: ping only those, give a sleeping device time to
    // answer, and skip those the receiver reports as not linked.
    let slots = if is_receiver && !passthrough && !bluetooth { receiver_slots(&h, is_bolt_receiver(pid)) } else { None };
    let indices: Vec<u8> = if let Some(slots) = &slots {
        slots.iter().map(|s| s.idx).collect()
    } else if passthrough {
        vec![1]
    } else if bluetooth || (!is_receiver && matches!(ping(0xFF), Reply::Ok(p) if p.len() >= 3 && p[0] >= 2)) {
        vec![0xFF]
    } else {
        vec![1, 2, 3, 4, 5, 6]
    };
    crate::util::debug_log(|| format!("  indices {indices:?}, slots {slots:?}"));

    let mut any = false;
    for idx in indices {
        let mut s = DeviceStatus {
            key: format!("logi:{}{}:{}", hex4(pid), suffix, idx),
            name: if idx == 0xFF && !product.is_empty() { product.clone() } else { "Logitech wireless device".into() },
            via: if idx == 0xFF { "USB / Bluetooth".into() } else { "Logitech receiver".into() },
            grace_sec: if idx == 0xFF { 0 } else { 1800 },
            ..Default::default()
        };
        s.kind = if passthrough { DeviceKind::Mouse } else { guess_kind(&s.name) };

        let slot = slots.as_ref().and_then(|v| v.iter().find(|s| s.idx == idx).copied());
        let unit_id = (physical_key(main), idx, slot.map_or(0, |sl| sl.wpid));
        if let Some(sl) = slot
            && sl.kind != DeviceKind::Other
        {
            s.kind = sl.kind;
        }
        let mut found = false;
        let reply = match slot {
            Some(Slot { link: Some(false), .. }) => ping(idx), // receiver: not linked, short try only
            Some(_) => ping_ms(idx, 2000),
            None => ping(idx),
        };
        crate::util::debug_log(|| match &reply {
            Reply::Ok(p) => format!("  index {idx}: ping ok {p:02X?}"),
            Reply::Err(e) => format!("  index {idx}: error {e:02X}"),
            Reply::None => format!("  index {idx}: no reply"),
        });
        match reply {
            Reply::Ok(p) if p.len() >= 3 && p[0] >= 2 => {
                read_name_and_type20(&h, idx, &mut s);
                found = read_battery20(&h, idx, pid, &mut s);
                let read = if found { read_unit_id20(&h, idx) } else { String::new() };
                if !read.is_empty() {
                    remember_unit(&unit_id, &read);
                }
                let unit = if found { stable_unit(&read, slot.and_then(|sl| sl.serial), remembered_unit(&unit_id)) } else { None };
                if let Some(unit) = unit {
                    crate::util::debug_log(|| format!("  index {idx}: unit id '{read}', key unit {unit}"));
                    s.key = format!("logi:unit:{unit}");
                    if idx == 0xFF {
                        s.grace_sec = 60;
                    }
                }
                if found && s.kind == DeviceKind::Other && longs.first().is_some_and(|l| l.page() == 0xFF43) {
                    s.kind = DeviceKind::Headset;
                }
            }
            Reply::Err(0x01) => found = read_battery10(&h, idx, &mut s), // HID++ 1.0 device
            _ => {}
        }
        crate::util::debug_log(|| format!("  index {idx}: battery found {found}, '{}' {}%", s.name, s.percent));
        if found {
            if s.kind == DeviceKind::Other {
                s.kind = guess_kind(&s.name);
            }
            s.vid = 0x046D;
            s.hw = Some((0x046D, pid));
            ctx.out.push(s);
            any = true;
        } else if slot.is_some() {
            // Paired but silent (asleep / off): the receiver is checked again soon, and a device
            // shown before keeps its last level, dimmed.
            s.name = match s.kind {
                DeviceKind::Mouse => "Logitech wireless mouse".into(),
                DeviceKind::Keyboard => "Logitech wireless keyboard".into(),
                _ => s.name,
            };
            if let Some(unit) = stable_unit("", slot.and_then(|sl| sl.serial), remembered_unit(&unit_id)) {
                s.key = format!("logi:unit:{unit}");
            }
            s.connected = false;
            s.vid = 0x046D;
            s.hw = Some((0x046D, pid));
            ctx.out.push(s);
            any = true;
        }
    }
    any
}

pub fn poll(ctx: &mut PollContext) {
    for (vid, pid) in ctx.products(0x046D) {
        if ctx.is_claimed(vid, pid) {
            continue; // read by a model-specific driver (logitech_g533)
        }
        let passthrough = is_passthrough_dongle(pid);
        let all_shorts = ctx.find(vid, pid, |c| {
            c.out_len() == 7 && ((c.page() == 0xFF00 && c.usage() == 0x0001) || (passthrough && c.page() == 0xFF43 && c.usage() == 0x0301))
        });
        let all_longs = ctx.find(vid, pid, |c| {
            c.out_len() == 20
                && ((c.page() == 0xFF00 && c.usage() == 0x0002) || (c.page() == 0xFF43 && (c.usage() == 0x0202 || (passthrough && c.usage() == 0x0302))))
        });
        let mut groups: Vec<String> = Vec::new();
        for c in all_shorts.iter().chain(all_longs.iter()) {
            let k = physical_key(c);
            if !groups.contains(&k) {
                groups.push(k);
            }
        }
        let mut any = false;
        for (g, key) in groups.iter().enumerate() {
            let shorts: Vec<HidColl> = all_shorts.iter().filter(|c| &physical_key(c) == key).cloned().collect();
            let longs: Vec<HidColl> = all_longs.iter().filter(|c| &physical_key(c) == key).cloned().collect();
            let suffix = if g > 0 { format!("#{}", g + 1) } else { String::new() };
            any |= poll_device(ctx, pid, &shorts, &longs, &suffix);
        }
        if any {
            ctx.claim(vid, pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn li_ion_curve_end_points_and_order() {
        assert_eq!(li_ion_percent(4300), 100);
        assert_eq!(li_ion_percent(3400), 0);
        assert_eq!(li_ion_percent(3811), 50);
        let mut last = 101;
        for mv in (3400..4300).rev().step_by(25) {
            let p = li_ion_percent(mv);
            assert!(p <= last, "curve must not rise as the voltage falls");
            last = p;
        }
    }

    #[test]
    fn headset_curves() {
        assert_eq!(headset_percent(0x0AB5, 4200), 100); // G733
        assert_eq!(headset_percent(0x0AB5, 3300), 0);
        assert_eq!(headset_percent(0x0A66, 3850), 50); // G533
        // G733 0AFE: G HUB's readings (issue #429) within 3 points.
        for (mv, ghub) in [(3828, 86), (3827, 87), (3822, 84), (3821, 83), (3818, 82), (3816, 81), (3813, 78)] {
            assert!((headset_percent(0x0AFE, mv) - ghub).abs() <= 3, "{mv} mV");
        }
        assert_eq!(headset_percent(0x0AFE, 4200), 100);
        assert_eq!(headset_percent(0x0AFE, 3500), 0);
        let mut last = 101;
        for mv in (3500..4200).rev().step_by(5) {
            let p = headset_percent(0x0AFE, mv);
            assert!(p <= last, "0AFE curve must not rise as the voltage falls");
            last = p;
        }
    }

    #[test]
    fn pairing_register() {
        // 0xB5 sub 0x20 (slot 1): dest 0x51, interval 1 ms, wpid 40B1 (a LIGHTSPEED mouse), kind 2.
        let p = [0x20, 0x51, 0x01, 0x40, 0xB1, 0x00, 0x00, 0x02, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_pairing(0x20, &p), Some((0x40B1, 2)));
        assert_eq!(pairing_kind(2), DeviceKind::Mouse);
        assert_eq!(parse_pairing(0x21, &p), None); // another sub-register's reply
        let empty = [0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(parse_pairing(0x21, &empty), None);
        assert_eq!(parse_pairing(0x20, &p[..5]), None);
    }

    #[test]
    fn bolt_pairing_register() {
        // 0xB5 sub 0x51 (Bolt slot 1): kind 1 (keyboard), wpid B378, serial 8D760B87.
        let p = [0x51, 0x41, 0x78, 0xB3, 0x8D, 0x76, 0x0B, 0x87, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_bolt_pairing(0x51, &p), Some((0xB378, 1, Some(0x8D76_0B87))));
        assert_eq!(pairing_kind(1), DeviceKind::Keyboard);
        assert_eq!(parse_bolt_pairing(0x52, &p), None); // another slot's reply
        assert_eq!(parse_bolt_pairing(0x52, &[0x52, 0, 0, 0, 0, 0, 0, 0]), None); // empty slot
        assert_eq!(parse_bolt_pairing(0x51, &p[..4]), Some((0xB378, 1, None)));
        assert_eq!(format!("{:08X}", 0x8D76_0B87u32), "8D760B87"); // same form as read_unit_id20
    }

    #[test]
    fn extended_pairing_register() {
        // 0xB5 sub 0x30 (slot 1) of two C547 receivers; the serials are the unit ids the devices
        // report (0x0003): G515 LS TKL F550A06C, G309 LIGHTSPEED 5E50C067.
        let keyboard = [0x30, 0xF5, 0x50, 0xA0, 0x6C, 0x0E, 0x40, 0xA0, 0x01, 0x07, 0, 0, 0, 0, 0, 0];
        let mouse = [0x30, 0x5E, 0x50, 0xC0, 0x67, 0x0E, 0x40, 0xA0, 0x00, 0x0F, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_ext_pairing(0x30, &keyboard), Some(0xF550_A06C));
        assert_eq!(parse_ext_pairing(0x30, &mouse), Some(0x5E50_C067));
        assert_eq!(parse_ext_pairing(0x31, &keyboard), None); // another slot's reply
        assert_eq!(parse_ext_pairing(0x31, &[0x31, 0, 0, 0, 0, 0, 0, 0]), None); // no serial
        assert_eq!(parse_ext_pairing(0x30, &keyboard[..4]), None);
    }

    #[test]
    fn a_device_keeps_its_unit_key() {
        // The unit id just read wins; then the receiver's serial; then the one remembered.
        assert_eq!(stable_unit("F550A06C", Some(0x1234_5678), Some("AAAAAAAA".into())).as_deref(), Some("F550A06C"));
        assert_eq!(stable_unit("", Some(0xF550_A06C), Some("AAAAAAAA".into())).as_deref(), Some("F550A06C"));
        assert_eq!(stable_unit("", None, Some("F550A06C".into())).as_deref(), Some("F550A06C"));
        assert_eq!(stable_unit("", None, None), None); // first read failed, nothing known: slot key
        // Remembered per receiver, index and wpid.
        let id = ("test-receiver#586".to_string(), 1, 0x40B4);
        assert_eq!(remembered_unit(&id), None);
        remember_unit(&id, "F550A06C");
        assert_eq!(remembered_unit(&id).as_deref(), Some("F550A06C"));
        assert_eq!(remembered_unit(&("test-receiver#586".to_string(), 1, 0x40B1)), None); // another device paired there
        assert_eq!(remembered_unit(&("test-receiver#586".to_string(), 2, 0x40B4)), None);
    }

    #[test]
    fn link_notifications() {
        // [10][index][41][protocol][flags][wpid lo][wpid hi]; flags bit 6 = link not established.
        assert_eq!(parse_link_notification(&[0x10, 0x01, 0x41, 0x0C, 0x02, 0xB1, 0x40]), Some((1, true)));
        assert_eq!(parse_link_notification(&[0x10, 0x01, 0x41, 0x0C, 0x42, 0xB1, 0x40]), Some((1, false)));
        assert_eq!(parse_link_notification(&[0x10, 0xFF, 0x80, 0x02, 0x00, 0x00, 0x00]), None); // the ack
        assert_eq!(parse_link_notification(&[0x10, 0x01, 0x41, 0x00, 0x02, 0xB1, 0x40]), None); // no protocol
        assert_eq!(parse_link_notification(&[0x11, 0x01, 0x41, 0x0C, 0x02, 0xB1, 0x40]), None);
    }

    #[test]
    fn receiver_collections_group_by_physical_device() {
        let a = HidColl { path: r"\\?\hid#vid_046d&pid_c547&mi_02&col01#8&2a2b3c4d&0&0000#{guid}".into(), ..Default::default() };
        let b = HidColl { path: r"\\?\hid#vid_046d&pid_c547&mi_02&col02#8&2a2b3c4d&0&0001#{guid}".into(), ..Default::default() };
        let other = HidColl { path: r"\\?\hid#vid_046d&pid_c547&mi_02&col01#8&99999999&0&0000#{guid}".into(), ..Default::default() };
        assert_eq!(physical_key(&a), physical_key(&b));
        assert_ne!(physical_key(&a), physical_key(&other));
    }
}
