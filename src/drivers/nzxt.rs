//! NZXT Lift Elite: receiver 1E71:2101 and USB cable 1E71:2129, vendor collection FFCA:0001.
//! Protocol from the supplied C++ BatteryHub src/drivers/nzxt.cpp, which credits HaloBattery
//! (providers/nzxt.py, MIT) and USB captures of NZXT CAM 4.76.5. Only telemetry is requested.
//! Output: 4E 02 81 00 B0 and zeros. On the tested 64-byte Windows collection, 4E is
//! the report ID; the C++ template's extra zero causes ERROR_INVALID_PARAMETER (87).
//! Retain the template's leading zero only for a 65-byte unnumbered collection.
//! Input (after HidDev strips report ID 0): 4E 02 97; percent at 17..19 (LE),
//! cell mV at 8..10 (BE), charging at byte 22 bit 0. Ignore the receiver's 4E E5 ACK.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};

const VID: u16 = 0x1E71;
const RECEIVER: u16 = 0x2101;
const CABLE: u16 = 0x2129;

fn request(len: u16) -> Option<Vec<u8>> {
    let offset = match len {
        64 => 0,
        65 => 1,
        _ => return None,
    };
    let mut r = vec![0; len as usize];
    r[offset..offset + 5].copy_from_slice(&[0x4E, 0x02, 0x81, 0x00, 0xB0]);
    Some(r)
}

fn parse(r: &[u8]) -> Option<(i32, i32, bool)> {
    if r.len() < 27 || r[..3] != [0x4E, 0x02, 0x97] {
        return None;
    }
    let percent = u16::from_le_bytes([r[17], r[18]]) as i32;
    if percent > 100 {
        return None;
    }
    let mv = u16::from_be_bytes([r[8], r[9]]) as i32;
    Some((percent, if (3000..=4500).contains(&mv) { mv } else { 0 }, r[22] & 1 != 0))
}

fn query(c: &HidColl) -> Option<(i32, i32, bool)> {
    let request = request(c.out_len())?;
    let d = HidDev::open(c)?;
    d.flush();
    if !d.write(&request) {
        return None;
    }
    for _ in 0..10 {
        // Unlike the C++ wrapper, read returns None for both timeouts and errors.
        if let Some(r) = d.read(150)
            && let Some(value) = parse(&r)
        {
            return Some(value);
        }
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    poll_with(ctx, query);
}

fn poll_with(ctx: &mut PollContext, mut read: impl FnMut(&HidColl) -> Option<(i32, i32, bool)>) {
    let mut found = None;
    for pid in [CABLE, RECEIVER] {
        if ctx.is_claimed(VID, pid) {
            continue;
        }
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFFCA && c.usage() == 1);
        if colls.is_empty() {
            continue;
        }
        ctx.claim(VID, pid);
        if found.is_none() {
            found = colls.iter().find_map(|c| read(c).map(|value| (pid, value)));
        }
    }
    // No fresh result: the taskbar's missing-device logic retains the last measurement.
    // Returning an unknown/asleep entry here would overwrite it with -1 on every timeout.
    let Some((pid, (percent, millivolts, charging))) = found else { return };
    ctx.out.push(DeviceStatus {
        key: "nzxt:2101".into(),
        name: "NZXT Lift Elite".into(),
        kind: DeviceKind::Mouse,
        via: if pid == CABLE { "USB cable" } else { "2.4 GHz receiver" }.into(),
        vid: VID,
        hw: Some((VID, pid)),
        grace_sec: 1800,
        percent,
        millivolts,
        charging,
        ..Default::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(percent: u16, mv: u16, flags: u8) -> [u8; 64] {
        let mut r = [0; 64];
        r[..3].copy_from_slice(&[0x4E, 0x02, 0x97]);
        r[17..19].copy_from_slice(&percent.to_le_bytes());
        r[8..10].copy_from_slice(&mv.to_be_bytes());
        r[22] = flags;
        r
    }

    fn context() -> PollContext {
        PollContext {
            colls: [RECEIVER, CABLE].into_iter().map(|pid| HidColl { vid: VID, pid, usage_page: 0xFFCA, usage_id: 1, ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn telemetry_frames() {
        let q = request(65).unwrap();
        assert_eq!(&q[..6], &[0, 0x4E, 2, 0x81, 0, 0xB0]);
        assert!(q[6..].iter().all(|&b| b == 0));
        let q = request(64).unwrap();
        assert_eq!(q.len(), 64);
        assert_eq!(&q[..5], &[0x4E, 2, 0x81, 0, 0xB0]);
        assert!(q[5..].iter().all(|&b| b == 0));
        assert!(request(63).is_none());
        assert_eq!(parse(&reply(77, 3950, 2)), Some((77, 3950, false)));
        assert_eq!(parse(&reply(100, 4200, 3)), Some((100, 4200, true)));
        assert_eq!(parse(&reply(0, 2999, 0)), Some((0, 0, false)));
        assert_eq!(parse(&reply(50, 4501, 0)), Some((50, 0, false)));
    }

    #[test]
    fn rejects_ack_echo_truncation_and_invalid_percent() {
        let r = reply(50, 3800, 0);
        for len in 0..27 {
            assert_eq!(parse(&r[..len]), None);
        }
        let mut ack = r;
        ack[1] = 0xE5;
        assert_eq!(parse(&ack), None);
        assert_eq!(parse(&request(64).unwrap()), None);
        assert_eq!(parse(&reply(101, 3800, 0)), None);
        assert_eq!(parse(&reply(256, 3800, 0)), None);
    }

    #[test]
    fn cable_wins_without_duplicate_or_receiver_query() {
        let mut ctx = context();
        poll_with(&mut ctx, |c| {
            assert_eq!(c.pid, CABLE);
            Some((80, 4100, true))
        });
        assert_eq!(ctx.out.len(), 1);
        assert_eq!(ctx.out[0].hw, Some((VID, CABLE)));
        assert_eq!(ctx.out[0].via, "USB cable");
        assert!(ctx.is_claimed(VID, RECEIVER) && ctx.is_claimed(VID, CABLE));
    }

    #[test]
    fn receiver_fallback_keeps_identity() {
        let mut ctx = context();
        poll_with(&mut ctx, |c| (c.pid == RECEIVER).then_some((42, 3700, false)));
        assert_eq!(ctx.out[0].key, "nzxt:2101");
        assert_eq!(ctx.out[0].hw, Some((VID, RECEIVER)));
        assert_eq!(ctx.out[0].percent, 42);
        assert_eq!(ctx.out[0].via, "2.4 GHz receiver");
    }

    #[test]
    fn captured_lift_elite_reply() {
        // Actual receiver response with CAM closed: 42%, 3837 mV, on battery.
        let r = [0x4E, 2, 0x97, 0, 1, 0x40, 1, 0xCE, 0x0E, 0xFD, 0xFF, 0x40, 1, 0x86, 0, 0x0B, 0, 0x2A, 0, 0x64, 0, 0x17, 0, 8, 0, 0x40, 1];
        assert_eq!(parse(&r), Some((42, 3837, false)));
    }

    #[test]
    fn silence_does_not_overwrite_last_value_and_other_interfaces_are_ignored() {
        let mut ctx = context();
        poll_with(&mut ctx, |_| None);
        assert!(ctx.out.is_empty());
        assert!(ctx.is_claimed(VID, RECEIVER));
        let mut ctx = context();
        ctx.colls[0].usage_page = 1;
        ctx.colls[1].usage_id = 2;
        poll_with(&mut ctx, |_| panic!("must not query unrelated collections"));
        assert!(ctx.out.is_empty() && ctx.claimed.is_empty());
    }
}
