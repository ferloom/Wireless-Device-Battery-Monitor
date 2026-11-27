//! Rapoo VT9 PRO mouse on its older 1K 2.4 GHz receiver (VID 24AE, PID 185A).
//!
//! Nothing is sent: the receiver pushes input report `BB B0 42 40 06 01 15` by itself about every
//! 3 seconds on the vendor collection FF00:0002 (7-byte input / 7-byte output). The last byte is the
//! battery level in percent: in issue #374 the receiver reported `15` (21) while Rapoo's software
//! showed 21 %, and at 37 % the same receiver sent `25`, with every other byte unchanged. The other
//! bytes are not interpreted (no charging state is documented), and none of the receiver's other
//! collections is used.
//!
//! The collection is only read, for a few seconds per poll: a mouse that is switched off sends
//! nothing, so the last level stays shown (dimmed after the grace time) instead of a made-up value.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{Deadline, sleep_ms};

const VID: u16 = 0x24AE;
const PID: u16 = 0x185A;
const HEADER: [u8; 2] = [0xBB, 0xB0];
const REPORT_LEN: usize = 7;
/// The heartbeat comes every ~3 s; wait a little longer so one is always caught.
const LISTEN_MS: u64 = 3500;

/// Battery level from one input report, if it is the receiver's status report.
pub fn decode(r: &[u8]) -> Option<i32> {
    (r.len() == REPORT_LEN && r[..2] == HEADER && (1..=100).contains(&r[6])).then(|| r[6] as i32)
}

fn listen(c: &HidColl) -> Option<i32> {
    let d = HidDev::open_read_only(c)?;
    let end = Deadline::in_ms(LISTEN_MS);
    while !end.passed() {
        match d.read(end.left_ms()) {
            Some(r) => {
                if let Some(level) = decode(&r) {
                    return Some(level);
                }
            }
            None => sleep_ms(20), // timeout or read error: do not spin
        }
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID) {
        return;
    }
    let colls = ctx.find(VID, PID, |c| c.page() == 0xFF00 && c.usage() == 0x0002 && c.in_len() as usize == REPORT_LEN);
    let Some(level) = colls.iter().find_map(listen) else { return };
    ctx.add(
        DeviceStatus {
            key: "rapoo:185a".into(),
            name: "Rapoo VT9 PRO".into(),
            kind: DeviceKind::Mouse,
            via: "2.4 GHz receiver".into(),
            hw: Some((VID, PID)),
            grace_sec: 1800,
            percent: level,
            ..Default::default()
        },
        VID,
        PID,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(level: u8) -> [u8; REPORT_LEN] {
        [0xBB, 0xB0, 0x42, 0x40, 0x06, 0x01, level]
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
}
