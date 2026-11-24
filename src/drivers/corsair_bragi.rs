//! Corsair "Bragi" headset receivers: Virtuoso RGB Wireless (XT) and related models (VID 1B1C,
//! receivers 0A64, 0A62, 0A3E, 0A3D, 0A97), the Void Wireless v2 receiver 2A08 (issue #168, listed
//! by the original) and the HS80 RGB Wireless receiver 0A6B (#161; the original sends Bragi GETs to
//! every Corsair FF42 64-byte collection, which covers it). The Void family's older protocol is in
//! corsair.rs.
//!
//! Protocol as documented by the original C++ BatteryHub (GPL-3.0, src/drivers/corsair.cpp, which
//! lists these receivers and follows the Bragi GET framing OpenRGB uses for Corsair's newer
//! peripherals); implemented here from those facts. On the receiver's vendor collection (usage page FF42, 64-byte
//! input and output reports, report id 02):
//!
//!   GET      02 TT 02 PP                TT = target: 08 the receiver itself, 09 the paired headset
//!   reply    02 SS 02 ST V0 V1 V2 V3    SS = source (00 for target 08, 01 for a paired device),
//!                                       ST = status (00 ok), V = value (little endian)
//!   PP 0F    battery level, 0..1000 (tenths of a percent)
//!   PP 10    charge state: 1 charging, 3 full
//!
//! GET is read-only; the receiver is never switched into software mode. Other reports (e.g. the
//! receiver's own notifications) are skipped. A receiver whose headset does not answer is shown off.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{Deadline, hex4};

const VID: u16 = 0x1B1C;
const RECEIVERS: [u16; 7] = [0x0A64, 0x0A62, 0x0A3E, 0x0A3D, 0x0A97, 0x2A08, 0x0A6B];
const TARGET_HEADSET: u8 = 0x09;
pub const PROP_BATTERY: u8 = 0x0F;
pub const PROP_CHARGE: u8 = 0x10;
const REPLY_MS: u64 = 400;

pub fn request(target: u8, prop: u8) -> [u8; 4] {
    [0x02, target, 0x02, prop]
}

/// Value of a reply from `source` (report id included). Some(None) = the device answered with an
/// error status (e.g. no headset paired / switched off).
pub fn parse(r: &[u8], source: u8) -> Option<Option<u32>> {
    if r.len() < 8 || r[0] != 0x02 || r[1] != source || r[2] != 0x02 {
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

fn get(d: &HidDev, target: u8, prop: u8) -> Option<Option<u32>> {
    d.flush();
    if !d.write(&request(target, prop)) {
        return None;
    }
    let source = if target == 0x08 { 0x00 } else { 0x01 };
    let end = Deadline::in_ms(REPLY_MS);
    while !end.passed() {
        if let Some(v) = d.read(end.left_ms()).and_then(|r| parse(&r, source)) {
            return Some(v);
        }
    }
    None
}

struct Reading {
    percent: Option<i32>,
    charging: bool,
    full: bool,
}

/// None = this collection does not speak Bragi; Some(None) = receiver answered, headset off.
fn read(c: &HidColl) -> Option<Option<Reading>> {
    let d = HidDev::open(c)?;
    let mut level = get(&d, TARGET_HEADSET, PROP_BATTERY)?;
    // The first reading after the headset wakes up can be 0; ask again once.
    if level == Some(0) {
        level = get(&d, TARGET_HEADSET, PROP_BATTERY).flatten().or(level);
    }
    let Some(pct) = level.and_then(percent) else { return Some(None) };
    let charge = get(&d, TARGET_HEADSET, PROP_CHARGE).flatten();
    Some(Some(Reading { percent: Some(pct), charging: charge == Some(1), full: charge == Some(3) }))
}

pub fn poll(ctx: &mut PollContext) {
    for pid in RECEIVERS {
        if ctx.is_claimed(VID, pid) {
            continue;
        }
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFF42 && c.out_len() == 64 && c.in_len() == 64);
        let Some(found) = colls.iter().find_map(read) else { continue };
        let base = DeviceStatus {
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
            hw: Some((VID, pid)),
            grace_sec: 60,
            ..Default::default()
        };
        let s = match found {
            Some(r) => DeviceStatus { percent: r.percent.unwrap_or(-1), charging: r.charging, full: r.full, ..base },
            None => DeviceStatus { connected: false, ..base },
        };
        ctx.add(s, VID, pid);
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
    }

    #[test]
    fn replies() {
        assert_eq!(parse(&reply(0x01, 0x00, 875), 0x01), Some(Some(875)));
        assert_eq!(parse(&reply(0x01, 0x05, 0), 0x01), Some(None)); // error status
        assert_eq!(parse(&reply(0x00, 0x00, 875), 0x01), None); // the receiver's own reply
        assert_eq!(parse(&[0x02, 0x01, 0x02, 0x00, 1], 0x01), None);
        let mut r = reply(0x01, 0x00, 875);
        r[2] = 0x06; // a notification
        assert_eq!(parse(&r, 0x01), None);
    }

    #[test]
    fn levels() {
        assert_eq!(percent(875), Some(88));
        assert_eq!(percent(1000), Some(100));
        assert_eq!(percent(0), Some(0));
        assert_eq!(percent(1001), None);
    }
}
