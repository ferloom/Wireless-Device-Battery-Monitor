//! EWEADN X23 Pro wireless mouse on its 2.4 GHz receiver (VID 04F3, PID 026F, "X23 Pro Receiver").
//!
//! Protocol as documented by eweadn-battery-tray (MIT, github.com/17iii/eweadn-battery-tray,
//! EweadnBattery.cs) and verified on real 04F3:026F hardware: on interface 1, collection
//! 000A:0000 (32-byte input report) passively pushes a status report roughly every 5 s with no
//! write needed:
//!
//!   04 01 02 02 LL ...   LL = battery level 0..=100 (%)
//!
//! No charging flag is present in this report. Read-only: nothing is written to the receiver.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x04F3;
const PID: u16 = 0x026F;
const REPORT: u8 = 0x04;
const LEN: u16 = 32;
/// Receiver pushes a status report about every 5 s while the mouse is awake.
const WAIT_MS: u64 = 6000;

/// Battery percentage from one 32-byte status input report (report ID included).
pub fn decode(r: &[u8]) -> Option<i32> {
    (r.len() >= 5 && r[0] == REPORT && r[1..4].iter().any(|&b| b != 0) && r[4] <= 100).then(|| r[4] as i32)
}

fn read_level(c: &HidColl) -> Option<i32> {
    let d = HidDev::open_read_only(c)?;
    let end = Deadline::in_ms(WAIT_MS);
    loop {
        let left = end.left_ms();
        if left == 0 {
            return None;
        }
        let Some(r) = d.read(left) else { continue };
        if let Some(level) = decode(&r) {
            return Some(level);
        }
    }
}

pub fn poll(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID) {
        return;
    }
    let colls = ctx.find(VID, PID, |c| c.page() == 0x000A && c.in_len() == LEN);
    if colls.is_empty() {
        return;
    }
    let base = DeviceStatus {
        key: "eweadn:026f".into(),
        name: "EWEADN X23 Pro".into(),
        kind: DeviceKind::Mouse,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, PID)),
        grace_sec: 1800,
        ..Default::default()
    };
    let s = match colls.iter().find_map(read_level) {
        Some(level) => DeviceStatus { percent: level, ..base },
        None => DeviceStatus { connected: false, ..base },
    };
    ctx.add(s, VID, PID);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_report() {
        let mut r = vec![0u8; 32];
        r[..10].copy_from_slice(&[0x04, 0x01, 0x02, 0x02, 0x3C, 0x00, 0x00, 0x00, 0x01, 0x11]);
        assert_eq!(decode(&r), Some(60));
        r[4] = 100;
        assert_eq!(decode(&r), Some(100));
        r[4] = 0;
        assert_eq!(decode(&r), Some(0));
    }

    #[test]
    fn rejects_invalid_and_empty_reports() {
        let mut r = vec![0u8; 32];
        r[0] = REPORT;
        // All-zero placeholder returned by GetInputReport before a push arrives.
        assert_eq!(decode(&r), None);
        // Out-of-range level.
        r[1] = 0x01;
        r[4] = 101;
        assert_eq!(decode(&r), None);
        // Wrong report ID.
        r[0] = 0x03;
        r[4] = 50;
        assert_eq!(decode(&r), None);
        // Too short.
        assert_eq!(decode(&[0x04, 0x01, 0x02, 0x02]), None);
    }
}
