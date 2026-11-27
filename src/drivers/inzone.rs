//! Sony INZONE H5 (054C:0EBF) and H9 II (054C:0FA8) headsets on their 2.4 GHz dongles.
//!
//! Protocol as documented by HeadsetControl (GPL-3.0, github.com/Sapd/HeadsetControl,
//! lib/devices/protocols/sony_inzone_protocol.hpp and sony_inzone_h5.hpp); implemented here from
//! those facts. The reporter of issue #328 read the battery this way on their own H5, where the
//! channel is the vendor collection FF04:0001 (HeadsetControl names FF04:0002), so both are asked.
//! Report 02, 64 bytes, a Sony vendor HCI command:
//!
//!   request  02 0C 01 00 FC 08 96 C3 41 04 01 TT TT CS        battery GET to the receiver
//!            TT = transaction id (LE), CS = sum of bytes 6..12
//!   reply    02 LL 04 FF .. 00 96 C3 1x 04 10|A0 TT TT CH PP .. CS
//!            CH = charger (non-zero: charging), PP = percent (FF: headset offline)
//!            CS = sum of bytes 5..LL (at byte LL + 1)
//!
//! Only this read is sent.
use std::sync::atomic::{AtomicU16, Ordering};

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x054C;
const H5_PID: u16 = 0x0EBF;
const H9_II_PID: u16 = 0x0FA8;
const REPORT: u8 = 0x02;
const LEN: usize = 64;
const EID_BATTERY: u8 = 0x04;
const GET: u8 = 0x01;
const RET: u8 = 0x10;
const NTFY_ACTIVE: u8 = 0xA0;
const TO_RECEIVER: u8 = 0x41;
const REPLY_MS: u64 = 1500;

static TID: AtomicU16 = AtomicU16::new(2);

/// The next transaction id; 0 and 1 are skipped (the dongle's own pushes carry 1).
fn next_tid() -> u16 {
    let t = TID.fetch_add(1, Ordering::Relaxed);
    if t <= 1 {
        TID.store(3, Ordering::Relaxed);
        2
    } else {
        t
    }
}

pub fn request(tid: u16) -> [u8; LEN] {
    let mut b = [0u8; LEN];
    b[..11].copy_from_slice(&[REPORT, 12, 0x01, 0x00, 0xFC, 8, 0x96, 0xC3, TO_RECEIVER, EID_BATTERY, GET]);
    b[11] = tid as u8;
    b[12] = (tid >> 8) as u8;
    b[13] = b[6..=12].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    b
}

/// (percent, charging) from the battery reply to `tid`; Some(None) when the headset is offline.
pub fn parse(b: &[u8], tid: u16) -> Option<Option<(i32, bool)>> {
    if b.len() < 16 || b[0] != REPORT || b[2] != 0x04 || b[3] != 0xFF || b[5] != 0 || b[6..8] != [0x96, 0xC3] || b[8] >> 4 != 1 {
        return None;
    }
    let n = b[1] as usize;
    if !(12..=LEN - 2).contains(&n) || n + 1 >= b.len() {
        return None;
    }
    if b[5..=n].iter().fold(0u8, |a, &x| a.wrapping_add(x)) != b[n + 1] {
        return None;
    }
    if b[9] != EID_BATTERY || (b[10] != RET && b[10] != NTFY_ACTIVE) || u16::from_le_bytes([b[11], b[12]]) != tid || n < 14 {
        return None;
    }
    let (charger, percent) = (b[13], b[14]);
    match percent {
        0xFF => Some(None),
        p if p <= 100 => Some(Some((p as i32, charger != 0))),
        _ => None,
    }
}

fn read(c: &HidColl) -> Option<Option<(i32, bool)>> {
    let d = HidDev::open(c)?;
    d.flush();
    let tid = next_tid();
    if !d.write(&request(tid)) {
        return None;
    }
    let end = Deadline::in_ms(REPLY_MS);
    while !end.passed() {
        if let Some(v) = d.read(end.left_ms()).and_then(|r| parse(&r, tid)) {
            return Some(v);
        }
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    poll_model(ctx, H5_PID, "sony:inzone-h5", "Sony INZONE H5");
    poll_model(ctx, H9_II_PID, "sony:inzone-h9-ii", "Sony INZONE H9 II");
}

fn poll_model(ctx: &mut PollContext, pid: u16, key: &str, name: &str) {
    if ctx.is_claimed(VID, pid) {
        return;
    }
    let colls = ctx.find(VID, pid, |c| c.page() == 0xFF04 && c.in_len() as usize == LEN && c.out_len() as usize == LEN);
    if colls.is_empty() {
        return;
    }
    let base = DeviceStatus {
        key: key.into(),
        name: name.into(),
        kind: DeviceKind::Headset,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, pid)),
        ..Default::default()
    };
    let s = match colls.iter().find_map(read) {
        Some(Some((level, charging))) => DeviceStatus { percent: level, charging, ..base },
        _ => DeviceStatus { connected: false, ..base },
    };
    ctx.add(s, VID, pid);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(tid: u16, charger: u8, percent: u8) -> Vec<u8> {
        let mut b = vec![0u8; LEN];
        let n = 14;
        b[..11].copy_from_slice(&[REPORT, n as u8, 0x04, 0xFF, 10, 0x00, 0x96, 0xC3, 0x14, EID_BATTERY, RET]);
        b[11] = tid as u8;
        b[12] = (tid >> 8) as u8;
        b[13] = charger;
        b[14] = percent;
        b[n + 1] = b[5..=n].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b
    }

    #[test]
    fn h9_ii_power_status() {
        // Power-status reply captured from a connected H9 II on FF04:0001.
        let reply = [0x02, 0x0E, 0x04, 0xFF, 0x0B, 0x00, 0x96, 0xC3, 0x14, 0x04, 0x10, 0x2A, 0x00, 0x00, 0x24, 0xCF];
        assert_eq!(parse(&reply, 42), Some(Some((36, false))));
    }

    #[test]
    fn frames() {
        let r = request(0x0102);
        assert_eq!(r[..14], [0x02, 0x0C, 0x01, 0x00, 0xFC, 0x08, 0x96, 0xC3, 0x41, 0x04, 0x01, 0x02, 0x01, 0xA2]);
        assert_eq!(parse(&reply(7, 0, 100), 7), Some(Some((100, false))));
        assert_eq!(parse(&reply(7, 1, 55), 7), Some(Some((55, true))));
        assert_eq!(parse(&reply(7, 0, 0xFF), 7), Some(None));
        // Another transaction, a broken checksum, our own request read back.
        assert_eq!(parse(&reply(7, 0, 50), 8), None);
        let mut bad = reply(7, 0, 50);
        bad[15] ^= 1;
        assert_eq!(parse(&bad, 7), None);
        assert_eq!(parse(&request(7), 7), None);
    }
}
