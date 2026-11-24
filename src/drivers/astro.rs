//! Logitech ASTRO A50 Gen 5 on its 046D:0B1C base station.
//!
//! Battery-only request on the vendor collection FF32:0074 (interface 8), with 64-byte reports
//! and report ID 02: `02 0C 03 00 06 0C ...`. The response carries percent in byte 6 and charging
//! in byte 8. Protocol validated by the original C++ BatteryHub and HeadsetControl. Read-only.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x046D;
const PID: u16 = 0x0B1C;
const LEN: usize = 64;
const REPLY_MS: u64 = 1000;

pub fn request() -> [u8; LEN] {
    let mut r = [0u8; LEN];
    r[..6].copy_from_slice(&[0x02, 0x0C, 0x03, 0, 0x06, 0x0C]);
    r
}

/// Percentage and charging state. None means the report is unrelated, incomplete, or invalid.
pub fn parse(r: &[u8]) -> Option<(i32, bool)> {
    if r.len() < 9
        || r[0] != 0x02
        || r[1] != 0x0C
        || r[2] < 6
        || (r[2] as usize + 3) > r.len()
        || r[3] != 0
        || r[4] != 0x06
        || !matches!(r[5], 0 | 0x0C)
        || r[6] > 100
    {
        return None;
    }
    Some((r[6] as i32, r[8] != 0))
}

fn query(c: &HidColl) -> Option<(i32, bool)> {
    let d = HidDev::open(c)?;
    d.flush();
    if !d.write(&request()) {
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
    let colls = ctx.find(VID, PID, |c| c.page() == 0xFF32 && c.usage() == 0x0074 && c.iface == 8 && c.in_len() as usize == LEN && c.out_len() as usize == LEN);
    if colls.is_empty() {
        return;
    }
    // Keep the generic Logitech driver from probing this recognised Astro collection.
    ctx.claim(VID, PID);
    let Some((percent, charging)) = colls.iter().find_map(query) else {
        return;
    };
    ctx.add(
        DeviceStatus {
            key: "astro:0b1c".into(),
            name: "Logitech Astro A50 Gen 5".into(),
            kind: DeviceKind::Headset,
            via: "USB base station".into(),
            percent,
            charging,
            grace_sec: 1800,
            ..Default::default()
        },
        VID,
        PID,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(percent: u8, charging: u8) -> [u8; LEN] {
        let mut r = [0u8; LEN];
        r[..9].copy_from_slice(&[0x02, 0x0C, 6, 0, 0x06, 0x0C, percent, 0, charging]);
        r
    }

    #[test]
    fn battery_request_is_read_only_and_exact() {
        let r = request();
        assert_eq!(&r[..6], &[0x02, 0x0C, 0x03, 0, 0x06, 0x0C]);
        assert!(r[6..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parses_percent_and_charge() {
        assert_eq!(parse(&reply(63, 0)), Some((63, false)));
        assert_eq!(parse(&reply(100, 1)), Some((100, true)));
        assert_eq!(parse(&reply(0, 0)), Some((0, false)));
    }

    #[test]
    fn rejects_invalid_or_unrelated_reports() {
        let r = reply(50, 1);
        assert_eq!(parse(&r[..8]), None);
        assert_eq!(parse(&[]), None); // disconnected/no reply
        let mut bad = r;
        bad[6] = 101;
        assert_eq!(parse(&bad), None);
        let mut bad = r;
        bad[2] = 5;
        assert_eq!(parse(&bad), None);
        let mut bad = r;
        bad[5] = 1;
        assert_eq!(parse(&bad), None);
        let mut bad = r;
        bad[1] = 0x0D; // unrelated mixer/volume event
        assert_eq!(parse(&bad), None);
    }
}
