//! HyperX Pulsefire Haste Wireless on its 2.4 GHz dongle 03F0:028E (#517, #567, #777).
//!
//! Protocol as documented by OpenMouse mouse-protocol (AGPL-3.0,
//! github.com/OpenMouse-Project/mouse-protocol, src/hyperx/index.ts and src/drivers/hyperx/hid.ts);
//! implemented here from those facts. Vendor collection FF00:0001, 64-byte unnumbered reports:
//!
//!   request  51 00 ...
//!   reply    51 .. .. .. LL ST     LL = level 1..100 (0 or FF = not known),
//!                                  ST = 0 on battery, 1 charging, 2 full
//!
//! Only this read is sent. Not tried on hardware by BatteryHub.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x03F0;
const PID: u16 = 0x028E;
const CMD_BATTERY: u8 = 0x51;
const REPLY_MS: u64 = 500;

/// A reply (report id 0 already stripped): Some(None) = answered without a level.
fn parse(r: &[u8]) -> Option<Option<(i32, bool, bool)>> {
    if r.len() < 6 || r[0] != CMD_BATTERY || r[1..].iter().all(|&b| b == 0) {
        return None; // another report, or our own request read back
    }
    Some((1..=100).contains(&r[4]).then(|| (r[4] as i32, matches!(r[5], 1 | 2), r[5] == 2)))
}

fn read(c: &HidColl) -> Option<Option<(i32, bool, bool)>> {
    let d = HidDev::open(c)?;
    d.flush();
    if !d.write(&[0x00, CMD_BATTERY]) {
        return None;
    }
    let end = Deadline::in_ms(REPLY_MS);
    while !end.passed() {
        if let Some(v) = d.read(end.left_ms()).and_then(|r| parse(&r)) {
            return Some(v);
        }
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID) {
        return;
    }
    let colls = ctx.find(VID, PID, |c| c.page() == 0xFF00 && c.usage() == 0x0001 && c.in_len() == 65 && c.out_len() == 65);
    if colls.is_empty() {
        return;
    }
    let base = DeviceStatus {
        key: "hyperx:haste:028e".into(),
        name: "HyperX Pulsefire Haste Wireless".into(),
        kind: DeviceKind::Mouse,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, PID)),
        grace_sec: 1800,
        ..Default::default()
    };
    let s = match colls.iter().find_map(read) {
        Some(Some((percent, charging, full))) => DeviceStatus { percent, charging, full, ..base },
        _ => DeviceStatus { connected: false, ..base },
    };
    ctx.add(s, VID, PID);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(level: u8, state: u8) -> [u8; 64] {
        let mut r = [0u8; 64];
        r[..6].copy_from_slice(&[CMD_BATTERY, 0, 0, 0, level, state]);
        r
    }

    #[test]
    fn replies() {
        assert_eq!(parse(&reply(80, 0)), Some(Some((80, false, false))));
        assert_eq!(parse(&reply(50, 1)), Some(Some((50, true, false))));
        assert_eq!(parse(&reply(100, 2)), Some(Some((100, true, true))));
        assert_eq!(parse(&reply(0xFF, 1)), Some(None));
        assert_eq!(parse(&reply(0, 0)), None); // the request read back
        let mut other = reply(80, 0);
        other[0] = 0x50;
        assert_eq!(parse(&other), None);
    }
}
