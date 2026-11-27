//! Rapoo mice that push their battery level without being asked (VID 24AE):
//!
//! * VT9 PRO on its older 1K 2.4 GHz receiver (PID 185A). Nothing is sent: the receiver pushes
//!   input report `BB B0 42 40 06 01 15` by itself about every 3 seconds on the vendor collection
//!   FF00:0002 (7-byte input / 7-byte output). The last byte is the battery level in percent: in
//!   issue #374 the receiver reported `15` (21) while Rapoo's software showed 21 %, and at 37 %
//!   the same receiver sent `25`, with every other byte unchanged. None of the other bytes are
//!   interpreted (no charging state is documented there).
//! * VT3 Max Gen2 on its 2.4 GHz receiver (PID 1417, issue #424). The same collection is 19 bytes
//!   and pushes report `07` every 6-10 s while idle, faster while active:
//!   `07 20 00 20 03 20 03 00 4C 00 03 01 01 17 46 00 00 01 B8` was captured at 76 %. Byte 8 is
//!   the percent, bytes 13..14 the mouse's USB pid (4617) in little endian; byte 7 = 0 means the
//!   level is valid (the reporter also saw 2 while charging, which Rapoo A HUB confirmed). The
//!   layout is from pedro3z0/rapoo-software-linux (docs/PROTOCOL.md, "Live status report"); the
//!   reply and the charging meaning were confirmed by the owner on Windows and in a local build.
//!
//! Both collections are only read, for a few seconds per poll: a mouse that is switched off sends
//! nothing, so the last level stays shown (dimmed after the grace time) instead of a made-up value.
//! A poll that hears nothing does not claim the device, so no flicker is introduced while Rapoo A
//! HUB is open.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{Deadline, hex4, sleep_ms};

const VID: u16 = 0x24AE;
const VT9: u16 = 0x185A;
const VT3: u16 = 0x1417;

const HEADER: [u8; 2] = [0xBB, 0xB0];
const REPORT_LEN: usize = 7;
/// The heartbeat comes every ~3 s; wait a little longer so one is always caught.
const LISTEN_MS: u64 = 3500;

const STATUS_ID: u8 = 0x07;
const STATUS_LEN: usize = 19;
/// The VT3's report comes every ~6-10 s when idle, so a full read waits longer; the quick read
/// must stay short (it runs every 10 s) and only notices a mouse that just woke up.
const STATUS_LISTEN_MS: u64 = 11_000;
const STATUS_QUICK_MS: u64 = 1_500;
const VT3_MOUSE_PID: u16 = 0x4617;

/// Battery level from one legacy input report, if it is the receiver's status report.
pub fn decode(r: &[u8]) -> Option<i32> {
    (r.len() == REPORT_LEN && r[..2] == HEADER && (1..=100).contains(&r[6])).then(|| r[6] as i32)
}

/// `(percent, charging)` from a VT3 Max Gen2 status report.
pub fn decode_status(r: &[u8]) -> Option<(i32, bool)> {
    if r.len() != STATUS_LEN || r[0] != STATUS_ID || !matches!(r[7], 0 | 2) || r[8] > 100 {
        return None;
    }
    // The receiver id alone is shared by other Rapoo firmware; the pid in the frame identifies
    // the mouse behind it (the capture has 17 46 = 0x4617).
    (u16::from_le_bytes([r[13], r[14]]) == VT3_MOUSE_PID).then_some((r[8] as i32, r[7] == 2))
}

fn listen<T>(c: &HidColl, ms: u64, parse: impl Fn(&[u8]) -> Option<T>) -> Option<T> {
    let d = HidDev::open_read_only(c)?;
    let end = Deadline::in_ms(ms);
    while !end.passed() {
        match d.read(end.left_ms()) {
            Some(r) => {
                if let Some(value) = parse(&r) {
                    return Some(value);
                }
            }
            None => sleep_ms(20), // timeout or read error: do not spin
        }
    }
    None
}

fn poll_all(ctx: &mut PollContext, quick: bool) {
    if !ctx.is_claimed(VID, VT9) {
        let colls = ctx.find(VID, VT9, |c| c.page() == 0xFF00 && c.usage() == 0x0002 && c.in_len() as usize == REPORT_LEN);
        if let Some(level) = colls.iter().find_map(|c| listen(c, LISTEN_MS, decode)) {
            ctx.add(
                DeviceStatus {
                    key: format!("rapoo:{}", hex4(VT9).to_lowercase()),
                    name: "Rapoo VT9 PRO".into(),
                    kind: DeviceKind::Mouse,
                    via: "2.4 GHz receiver".into(),
                    hw: Some((VID, VT9)),
                    grace_sec: 1800,
                    percent: level,
                    ..Default::default()
                },
                VID,
                VT9,
            );
        }
    }
    if ctx.is_claimed(VID, VT3) {
        return;
    }
    let colls = ctx.find(VID, VT3, |c| c.page() == 0xFF00 && c.usage() == 0x0002 && c.in_len() as usize == STATUS_LEN);
    let window = if quick { STATUS_QUICK_MS } else { STATUS_LISTEN_MS };
    let Some((level, charging)) = colls.iter().find_map(|c| listen(c, window, decode_status)) else { return };
    ctx.add(
        DeviceStatus {
            key: format!("rapoo:{}", hex4(VT3).to_lowercase()),
            name: "Rapoo VT3 Max Gen2".into(),
            kind: DeviceKind::Mouse,
            via: "2.4 GHz receiver".into(),
            hw: Some((VID, VT3)),
            grace_sec: 1800,
            percent: level,
            charging,
            ..Default::default()
        },
        VID,
        VT3,
    );
}

/// Full poll: wait long enough for the VT3's idle heartbeat.
pub fn poll(ctx: &mut PollContext) {
    poll_all(ctx, false);
}

/// Quick poll (offline / asleep devices): only a short listen, so the 10 s timer is not blocked.
pub fn poll_quick(ctx: &mut PollContext) {
    poll_all(ctx, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(level: u8) -> [u8; REPORT_LEN] {
        [0xBB, 0xB0, 0x42, 0x40, 0x06, 0x01, level]
    }

    fn status(level: u8, charge: u8) -> [u8; STATUS_LEN] {
        let mut r = [0u8; STATUS_LEN];
        r.copy_from_slice(&[0x07, 0x20, 0x00, 0x20, 0x03, 0x20, 0x03, charge, level, 0x00, 0x03, 0x01, 0x01, 0x17, 0x46, 0x00, 0x00, 0x01, 0xB8]);
        r
    }

    #[test]
    fn captured_levels() {
        // Issue #374: Rapoo's software showed 21 % for this frame, and later 37 % for `.. 25`.
        assert_eq!(decode(&frame(21)), Some(21));
        assert_eq!(decode(&frame(0x25)), Some(37));
    }

    #[test]
    fn rejects_other_reports() {
        assert_eq!(decode(&[0xB0, 0x42]), None); // the shorter collection
        assert_eq!(decode(&[0xBB, 0xB0, 0x42, 0x40, 0x06, 0x01]), None); // truncated
        assert_eq!(decode(&frame(0)), None);
        assert_eq!(decode(&frame(101)), None);
        let mut r = frame(50);
        r[0] = 0xBA;
        assert_eq!(decode(&r), None);
        r = frame(50);
        r[1] = 0xB1;
        assert_eq!(decode(&r), None);
    }

    #[test]
    fn vt3_captured_frame() {
        // Issue #424: the owner's frame, captured at 76 % on a VT3 Max Gen2 / 2.4 GHz receiver.
        let captured = [0x07u8, 0x20, 0x00, 0x20, 0x03, 0x20, 0x03, 0x00, 0x4C, 0x00, 0x03, 0x01, 0x01, 0x17, 0x46, 0x00, 0x00, 0x01, 0xB8];
        assert_eq!(decode_status(&captured), Some((76, false)));
        assert_eq!(decode_status(&status(100, 2)), Some((100, true)));
    }

    #[test]
    fn vt3_rejects_other_frames() {
        assert_eq!(decode_status(&frame(50)), None); // the VT9's 7-byte report
        let mut r = status(50, 0);
        r[0] = 0x06; // command report, not the status push
        assert_eq!(decode_status(&r), None);
        r = status(50, 0);
        r[7] = 0x01; // level not marked valid
        assert_eq!(decode_status(&r), None);
        r = status(50, 0);
        r[8] = 101;
        assert_eq!(decode_status(&r), None);
        r = status(50, 0);
        r[14] = 0x00; // not the mouse we know
        assert_eq!(decode_status(&r), None);
        assert_eq!(decode_status(&status(50, 0)[..18]), None); // short read
    }
}
