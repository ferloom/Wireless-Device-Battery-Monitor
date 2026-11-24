//! MCHOSE G3 V2 (Pro) mouse on its 2.4 GHz receiver (VID 3837, PID 4245) or USB cable (3837:418C).
//!
//! Protocol as documented by MCHOSE-Monitor (MIT, github.com/Jstdi001/MCHOSE-Monitor, verified on a
//! G3 V2) and as used by the original C++ BatteryHub (GPL-3.0, src/drivers/mchose_g3.cpp);
//! implemented here from those facts. Vendor collection FF01:0010, unnumbered 64-byte output /
//! input reports (the same framing as the AJAZZ AJ139, ajazz_aj139.rs):
//!
//!   request  55 30 A5 0B 2E 01 01 01 00 ...    command 30 (battery), read (A5)
//!   reply    AA 30 .. .. .. .. .. .. LL CC     LL = level 1..100, CC = 1 while charging
//!
//! Other reports are skipped. Only this read is sent. With both the cable and the receiver plugged
//! in, the mouse is listed once (cable first).
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x3837;
const CABLE: u16 = 0x418C;
const RECEIVER: u16 = 0x4245;
const KEY: &str = "mchose:g3v2";
const WAIT_MS: u64 = 1000;

/// The output report, report ID 0 first.
pub fn request() -> [u8; 65] {
    let mut r = [0u8; 65];
    r[1..9].copy_from_slice(&[0x55, 0x30, 0xA5, 0x0B, 0x2E, 0x01, 0x01, 0x01]);
    r
}

/// Battery reply (no report ID): Some((level, charging)).
pub fn decode(r: &[u8]) -> Option<(i32, bool)> {
    (r.len() >= 10 && r[0] == 0xAA && r[1] == 0x30 && (1..=100).contains(&r[8])).then(|| (r[8] as i32, r[9] == 1))
}

fn read(c: &HidColl) -> Option<(i32, bool)> {
    let d = HidDev::open(c)?;
    d.flush();
    if !d.write(&request()) {
        return None;
    }
    let end = Deadline::in_ms(WAIT_MS);
    while !end.passed() {
        if let Some(v) = d.read(end.left_ms()).and_then(|r| decode(&r)) {
            return Some(v);
        }
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    for pid in [CABLE, RECEIVER] {
        if ctx.is_claimed(VID, pid) {
            continue;
        }
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFF01 && c.usage() == 0x0010 && c.out_len() >= 65 && c.in_len() >= 65);
        if colls.is_empty() {
            continue;
        }
        if ctx.out.iter().any(|o| o.key == KEY) {
            ctx.claim(VID, pid); // already read over the cable
            continue;
        }
        let cable = pid == CABLE;
        let base = DeviceStatus {
            key: KEY.into(),
            name: "MCHOSE G3 V2".into(),
            kind: DeviceKind::Mouse,
            via: if cable { "USB cable" } else { "2.4 GHz receiver" }.into(),
            hw: Some((VID, pid)),
            grace_sec: 1800,
            ..Default::default()
        };
        let s = match colls.iter().find_map(read) {
            Some((level, charging)) => DeviceStatus { percent: level, charging: charging || cable, ..base },
            None if cable => continue, // the receiver may still answer
            None => DeviceStatus { connected: false, ..base },
        };
        ctx.add(s, VID, pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_frame() {
        assert_eq!(request()[..9], [0x00, 0x55, 0x30, 0xA5, 0x0B, 0x2E, 0x01, 0x01, 0x01]);
        assert!(request()[9..].iter().all(|&b| b == 0));
    }

    #[test]
    fn battery_reply() {
        let mut r = vec![0u8; 64];
        r[..2].copy_from_slice(&[0xAA, 0x30]);
        r[8] = 64;
        assert_eq!(decode(&r), Some((64, false)));
        r[9] = 1;
        assert_eq!(decode(&r), Some((64, true)));
        r[8] = 0;
        assert_eq!(decode(&r), None);
        r[8] = 50;
        r[1] = 0xFA; // unsolicited report
        assert_eq!(decode(&r), None);
    }
}
