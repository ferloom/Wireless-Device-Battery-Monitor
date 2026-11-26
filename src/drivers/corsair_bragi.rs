//! Corsair "Bragi" headset receivers and Slipstream mice (VID 1B1C).
//! Headsets include Virtuoso RGB Wireless (XT) and related models (VID 1B1C,
//! receivers 0A64, 0A62, 0A3E, 0A3D, 0A97, 2A08, 0A6B), the Void Wireless v2 receiver 2A08 (issue #168, listed
//! by the original) and the HS80 RGB Wireless receiver 0A6B (#161; the original sends Bragi GETs to
//! every Corsair FF42 64-byte collection, which covers it). The Void family's older protocol is in
//! corsair.rs.
//!
//! Protocol as documented by the original C++ BatteryHub (GPL-3.0, src/drivers/corsair.cpp, which
//! lists these receivers and follows the Bragi GET framing OpenRGB uses for Corsair's newer
//! peripherals); implemented here from those facts. On the receiver's vendor collection (usage page FF42, 64-byte
//! input and output reports):
//!
//!   GET      02 TT 02 PP                TT = target: 08 the receiver itself, 09+ paired sub-devices
//!   reply    RR SS 02 ST V0 V1 V2 V3    RR = report id, SS = source (TT - 08: 00 receiver, 01 child 1,
//!                                       02 child 2, ...), ST = status (00 ok), V = value (little endian)
//!
//! The request goes out on report 02; the reply comes back on input report 01 on the Virtuoso XT
//! (HeadsetControl, GPL-3.0, lib/devices/corsair_virtuoso_xt.hpp: REPORT_ID_OUT 02, REPORT_ID_IN 01,
//! tested on that headset) and on the HS80 Slipstream Multipoint receiver 0A6B, and on 02 as the
//! original expects, so both are accepted.
//!   PP 0F    battery level, 0..1000 (tenths of a percent)
//!   PP 10    charge state: 1 charging, 2 discharging, 3 full
//!   PP 12    USB product id of the target (e.g. 0A69 HS80 headset, 1B5E Harpoon RGB Wireless)
//!   PP 36    on target 08: bitmask of active paired sub-devices (bit 1 -> 09, bit 2 -> 0A, ...)
//!
//! Slipstream receivers (2B00, 1BDC, 1BA6) and mice use the same GET in unnumbered 64-byte reports:
//!
//!   GET      TT 02 PP                   TT = 08 the USB device itself, 09+ paired sub-devices
//!   reply    SS 02 ST V0 V1             SS = TT - 08
//!   PP 12    USB product id             2B22 = Scimitar Elite Wireless SE, 1B5E = Harpoon RGB Wireless
//!
//! Verified on a Scimitar Elite Wireless SE on receiver 2B00 with iCUE running (cable 2B22 untested),
//! and on an HS80 RGB Wireless + Harpoon RGB Wireless pair in Slipstream Multipoint mode on 0A6B.
//!
//! GET is read-only; the receiver is never switched into software mode. Other reports (e.g. the
//! receiver's own notifications) are skipped. A receiver whose headset does not answer is shown off.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{Deadline, hex4};

const VID: u16 = 0x1B1C;
const RECEIVERS: [u16; 7] = [0x0A64, 0x0A62, 0x0A3E, 0x0A3D, 0x0A97, 0x2A08, 0x0A6B];
const TARGET_RECEIVER: u8 = 0x08;
const TARGET_HEADSET: u8 = 0x09;
pub const PROP_BATTERY: u8 = 0x0F;
pub const PROP_CHARGE: u8 = 0x10;
const PROP_PRODUCT: u8 = 0x12;
const PROP_SUBDEVICES: u8 = 0x36;
const REPLY_MS: u64 = 400;
const HARPOON_PID: u16 = 0x1B5E;
const SCIMITAR_PID: u16 = 0x2B22;
const HS80_HEADSET_PID: u16 = 0x0A69;
const SLIPSTREAM_RECEIVERS: [u16; 3] = [0x2B00, 0x1BDC, 0x1BA6];

pub fn request(target: u8, prop: u8) -> [u8; 4] {
    [0x02, target, 0x02, prop]
}

/// Value of a reply from `source` (report id included). Some(None) = the device answered with an
/// error status (e.g. no device paired / switched off / unsupported property).
pub fn parse(r: &[u8], source: u8) -> Option<Option<u32>> {
    if r.len() < 8 || !matches!(r[0], 0x01 | 0x02) || r[1] != source || r[2] != 0x02 {
        return None;
    }
    if r[3] != 0x00 {
        return Some(None);
    }
    Some(Some(u32::from_le_bytes([r[4], r[5], r[6], r[7]])))
}

/// Level in percent from the 0..1000 battery value.
pub fn percent(v: u32) -> Option<i32> {
    (v <= 1000).then(|| ((v + 5) / 10) as i32)
}

/// Active sub-device targets (`0x09..=0x0F`) from the receiver's `PROP_SUBDEVICES` (`0x36`) bitmask.
pub fn subdevice_targets(mask: u32) -> Vec<u8> {
    (1u8..=7).filter(|&bit| (mask & (1 << bit)) != 0).map(|bit| TARGET_RECEIVER + bit).collect()
}

/// Frame for a Slipstream receiver / mouse; its reports are unnumbered, so the report id is 0.
fn mouse_request(target: u8, prop: u8) -> [u8; 4] {
    [0x00, target, 0x02, prop]
}

/// 16-bit value of a successful GET reply from `target` (report id 0 already stripped). The answer
/// names the source as target - 08: 00 the USB device itself, 01 channel 1, 02 channel 2, ...
fn mouse_reply(r: &[u8], target: u8) -> Option<u32> {
    let source = target.checked_sub(TARGET_RECEIVER)?;
    (r.len() >= 5 && r[0] == source && r[1] == 0x02 && r[2] == 0).then(|| u16::from_le_bytes([r[3], r[4]]) as u32)
}

fn mouse_get_once(d: &HidDev, target: u8, prop: u8) -> Option<u32> {
    d.flush();
    if !d.write(&mouse_request(target, prop)) {
        return None;
    }
    let end = Deadline::in_ms(REPLY_MS);
    while !end.passed() {
        if let Some(v) = d.read(end.left_ms()).and_then(|r| mouse_reply(&r, target)) {
            return Some(v);
        }
    }
    None
}

/// Replies do not name the property, so the answer to a GET sent by iCUE at the same moment looks
/// like ours: a value only counts once two reads in a row agree.
fn mouse_get(d: &HidDev, target: u8, prop: u8) -> Option<u32> {
    agreed(|| mouse_get_once(d, target, prop))
}

fn agreed<T: Copy + PartialEq>(mut read: impl FnMut() -> Option<T>) -> Option<T> {
    let mut last = read()?;
    for _ in 0..3 {
        let v = read()?;
        if v == last {
            return Some(v);
        }
        last = v;
    }
    None
}

/// Mouse name by its USB product id (reported by the receiver as property 12).
fn mouse_name(product: u32) -> &'static str {
    match product {
        x if x == HARPOON_PID as u32 => "Corsair Harpoon RGB Wireless",
        x if x == SCIMITAR_PID as u32 => "Corsair Scimitar Elite Wireless SE",
        _ => "Corsair wireless mouse",
    }
}

fn is_mouse_product(product: u32) -> bool {
    matches!(product as u16, HARPOON_PID | SCIMITAR_PID)
}

fn is_headset_product(rx_pid: u16, product: u32) -> bool {
    product == HS80_HEADSET_PID as u32 || product == rx_pid as u32 || RECEIVERS.contains(&(product as u16))
}

fn headset_base(pid: u16) -> DeviceStatus {
    DeviceStatus {
        key: format!("corsair-bragi:{}", hex4(pid)),
        name: match pid {
            0x0A64 => "Corsair Virtuoso XT".into(),
            0x0A97 => "Corsair HS80 MAX Wireless".into(),
            0x2A08 => "Corsair Void Wireless v2".into(),
            0x0A6B => "Corsair HS80 RGB Wireless".into(),
            _ => "Corsair wireless headset".into(),
        },
        kind: DeviceKind::Headset,
        via: "Corsair receiver".into(),
        vid: VID,
        hw: Some((VID, pid)),
        grace_sec: 60,
        ..Default::default()
    }
}

fn mouse_status(c: &HidColl, product: u32, level: i32, charge: Option<u32>, via: &str) -> DeviceStatus {
    DeviceStatus {
        key: format!("corsair:{}", hex4(product as u16)),
        name: mouse_name(product).into(),
        kind: DeviceKind::Mouse,
        via: via.into(),
        percent: level,
        charging: charge == Some(1),
        full: charge == Some(3),
        grace_sec: 1800,
        vid: VID,
        hw: Some((c.vid, c.pid)),
        ..Default::default()
    }
}

fn read_slipstream_mouse(d: &HidDev, c: &HidColl, target: u8, receiver: bool) -> Option<DeviceStatus> {
    let product = if receiver { mouse_get(d, target, PROP_PRODUCT)? } else { SCIMITAR_PID as u32 };
    if product == 0 || product > 0xFFFF {
        return None;
    }
    let level = percent(mouse_get(d, target, PROP_BATTERY)?)?;
    let charge = mouse_get(d, target, PROP_CHARGE);
    Some(mouse_status(c, product, level, charge, if receiver { "2.4 GHz receiver" } else { "USB cable" }))
}

fn read_slipstream(c: &HidColl, target: u8, receiver: bool) -> Vec<DeviceStatus> {
    let Some(d) = HidDev::open(c) else { return Vec::new() };
    if !receiver {
        return read_slipstream_mouse(&d, c, target, false).into_iter().collect();
    }
    let targets = match mouse_get(&d, TARGET_RECEIVER, PROP_SUBDEVICES) {
        Some(mask) if mask != 0 => subdevice_targets(mask),
        _ => vec![target],
    };
    targets.into_iter().filter_map(|t| read_slipstream_mouse(&d, c, t, true)).collect()
}

fn get(d: &HidDev, target: u8, prop: u8) -> Option<Option<u32>> {
    let source = target.checked_sub(TARGET_RECEIVER)?;
    d.flush();
    if !d.write(&request(target, prop)) {
        return None;
    }
    let end = Deadline::in_ms(REPLY_MS);
    while !end.passed() {
        if let Some(v) = d.read(end.left_ms()).and_then(|r| parse(&r, source)) {
            return Some(v);
        }
    }
    None
}

fn get_agreed(d: &HidDev, target: u8, prop: u8) -> Option<Option<u32>> {
    agreed(|| get(d, target, prop))
}

/// None = this collection does not speak Bragi; Some(statuses) = receiver answered.
fn read_receiver(c: &HidColl, rx_pid: u16) -> Option<Vec<DeviceStatus>> {
    let d = HidDev::open(c)?;
    let (receiver_answered, targets) = match get_agreed(&d, TARGET_RECEIVER, PROP_SUBDEVICES) {
        Some(Some(mask)) => (true, subdevice_targets(mask)),
        Some(None) => (true, vec![TARGET_HEADSET]),
        None => (false, vec![TARGET_HEADSET]),
    };
    let mut out = Vec::new();
    let mut any_reply = receiver_answered;
    let mut headset_seen = false;
    for target in targets {
        let Some(mut level) = get_agreed(&d, target, PROP_BATTERY) else { continue };
        any_reply = true;
        // The first reading after the device wakes up can be 0; ask again once.
        if level == Some(0) {
            level = get_agreed(&d, target, PROP_BATTERY).flatten().or(level);
        }
        let Some(pct) = level.and_then(percent) else { continue };
        let charge = get_agreed(&d, target, PROP_CHARGE).flatten();
        let product = get_agreed(&d, target, PROP_PRODUCT).flatten().filter(|&p| p > 0 && p <= 0xFFFF);
        let is_mouse = product.is_some_and(is_mouse_product) || (target > TARGET_HEADSET && !product.is_some_and(|p| is_headset_product(rx_pid, p)));
        if is_mouse {
            if let Some(prod) = product {
                out.push(mouse_status(c, prod, pct, charge, "2.4 GHz receiver"));
            }
        } else if !headset_seen {
            headset_seen = true;
            out.push(DeviceStatus { percent: pct, charging: charge == Some(1), full: charge == Some(3), ..headset_base(rx_pid) });
        }
    }
    if !any_reply {
        return None;
    }
    if !headset_seen {
        out.push(DeviceStatus { connected: false, ..headset_base(rx_pid) });
    }
    Some(out)
}

pub fn poll(ctx: &mut PollContext) {
    for pid in RECEIVERS {
        if ctx.is_claimed(VID, pid) {
            continue;
        }
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFF42 && c.out_len() == 64 && c.in_len() == 64);
        let Some(statuses) = colls.iter().find_map(|c| read_receiver(c, pid)) else { continue };
        for s in statuses {
            if ctx.out.iter().any(|o| o.key == s.key) {
                ctx.claim(VID, pid);
            } else {
                ctx.add(s, VID, pid);
            }
        }
        ctx.claim(VID, pid);
    }

    for (vid, pid) in ctx.products(VID) {
        let (target, receiver) = if SLIPSTREAM_RECEIVERS.contains(&pid) {
            (TARGET_HEADSET, true)
        } else if pid == SCIMITAR_PID {
            (TARGET_RECEIVER, false)
        } else {
            continue;
        };
        let colls = ctx.find(vid, pid, |c| c.page() == 0xFF42 && c.in_len() >= 6 && c.out_len() >= 6);
        let statuses = colls.iter().map(|c| read_slipstream(c, target, receiver)).find(|v| !v.is_empty()).unwrap_or_default();
        for s in statuses {
            if let Some(old) = ctx.out.iter_mut().find(|o| o.key == s.key) {
                if s.via == "USB cable" {
                    *old = s;
                }
                ctx.claim(vid, pid);
            } else {
                ctx.add(s, vid, pid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(source: u8, status: u8, v: u32) -> Vec<u8> {
        let mut r = vec![0u8; 64];
        r[..4].copy_from_slice(&[0x02, source, 0x02, status]);
        r[4..8].copy_from_slice(&v.to_le_bytes());
        r
    }

    #[test]
    fn request_frame() {
        assert_eq!(request(TARGET_HEADSET, PROP_BATTERY), [0x02, 0x09, 0x02, 0x0F]);
        assert_eq!(request(0x0A, PROP_PRODUCT), [0x02, 0x0A, 0x02, 0x12]);
    }

    #[test]
    fn replies() {
        assert_eq!(parse(&reply(0x01, 0x00, 875), 0x01), Some(Some(875)));
        let mut on_report_1 = reply(0x01, 0x00, 875);
        on_report_1[0] = 0x01; // the Virtuoso XT / HS80 answers on input report 01
        assert_eq!(parse(&on_report_1, 0x01), Some(Some(875)));
        let mut other = reply(0x01, 0x00, 875);
        other[0] = 0x0E; // volume event
        assert_eq!(parse(&other, 0x01), None);
        assert_eq!(parse(&reply(0x01, 0x05, 0), 0x01), Some(None)); // error status
        assert_eq!(parse(&reply(0x00, 0x00, 875), 0x01), None); // the receiver's own reply
        assert_eq!(parse(&[0x02, 0x01, 0x02, 0x00, 1], 0x01), None);
        let mut r = reply(0x01, 0x00, 875);
        r[2] = 0x06; // a notification
        assert_eq!(parse(&r, 0x01), None);
    }

    #[test]
    fn multipoint_receiver_hs80_and_harpoon() {
        // Recorded on HS80 RGB Wireless Gaming Receiver (0A6B) with HS80 + Harpoon RGB Wireless paired.
        assert_eq!(subdevice_targets(0x06), vec![0x09, 0x0A]);
        assert_eq!(subdevice_targets(0x02), vec![0x09]);
        assert_eq!(subdevice_targets(0x00), Vec::<u8>::new());

        let mut hs80_bat = reply(0x01, 0x00, 310);
        hs80_bat[0] = 0x01;
        assert_eq!(parse(&hs80_bat, 0x01).flatten().and_then(percent), Some(31));

        let mut harpoon_pid = reply(0x02, 0x00, HARPOON_PID as u32);
        harpoon_pid[0] = 0x01;
        assert_eq!(parse(&harpoon_pid, 0x02), Some(Some(HARPOON_PID as u32)));
        assert_eq!(mouse_name(HARPOON_PID as u32), "Corsair Harpoon RGB Wireless");

        let mut harpoon_bat = reply(0x02, 0x00, 610);
        harpoon_bat[0] = 0x01;
        assert_eq!(parse(&harpoon_bat, 0x02).flatten().and_then(percent), Some(61));

        let mut harpoon_chg = reply(0x02, 0x00, 2);
        harpoon_chg[0] = 0x01;
        assert_eq!(parse(&harpoon_chg, 0x02), Some(Some(2)));
    }

    #[test]
    fn levels() {
        assert_eq!(percent(875), Some(88));
        assert_eq!(percent(1000), Some(100));
        assert_eq!(percent(0), Some(0));
        assert_eq!(percent(1001), None);
    }

    #[test]
    fn mouse_protocol() {
        assert_eq!(mouse_request(0x09, PROP_BATTERY), [0x00, 0x09, 0x02, 0x0F]);
        // Recorded on a Scimitar Elite Wireless SE (receiver 2B00).
        assert_eq!(mouse_reply(&[0x01, 0x02, 0x00, 0x22, 0x2B, 0, 0], 0x09), Some(SCIMITAR_PID as u32));
        assert_eq!(mouse_reply(&[0x02, 0x02, 0x00, 0x5E, 0x1B, 0, 0], 0x0A), Some(HARPOON_PID as u32));
        assert_eq!(mouse_reply(&[0x01, 0x02, 0x00, 0xE8, 0x03, 0, 0], 0x09).and_then(percent), Some(100));
        assert_eq!(mouse_reply(&[0x01, 0x06, 0x00, 0x00, 0x00, 0, 0], 0x09), None); // reply to iCUE
        assert_eq!(mouse_reply(&[0x00, 0x02, 0x00, 0x00, 0x2B, 0, 0], 0x09), None); // the receiver
        assert_eq!(mouse_reply(&[0x00, 0x02, 0x00, 0x6E, 0x03, 0, 0], 0x08).and_then(percent), Some(88));
        assert_eq!(mouse_reply(&[0x01, 0x02, 0x05, 0xA5, 0xA5, 0, 0], 0x09), None);
    }

    #[test]
    fn foreign_reply_is_outvoted() {
        let run = |v: &[u32]| {
            let mut it = v.iter().copied();
            agreed(|| it.next())
        };
        assert_eq!(run(&[878, 878]), Some(878));
        assert_eq!(run(&[3, 878, 878]), Some(878));
        assert_eq!(run(&[878, 3, 878, 878]), Some(878));
        assert_eq!(run(&[1, 2, 3, 4, 4]), None);
        assert_eq!(run(&[878]), None);
    }
}
