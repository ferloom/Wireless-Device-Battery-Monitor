//! Keychron Nape Pro trackball (ZMK firmware) on the Link-KM 2.4 GHz receiver (3434:D026) or on its
//! USB cable (3434:0440).
//!
//! Protocol as documented by nape_pro (github.com/nvalis/nape_pro, docs/protocol-reference.md,
//! checked on a real D026), KeychronNapeBatteryWidget (github.com/gtun/KeychronNapeBatteryWidget)
//! and NapeBar (github.com/ky0209/NapeBar), and as used by the original C++ BatteryHub (GPL-3.0,
//! src/drivers/keychron.cpp); implemented here from those facts. VIA-style raw HID collection
//! FF60:0061, unnumbered 32-byte reports:
//!
//!   B2      ->  B2 .. then three slots at bytes 2 / 7 / 12: vid BE16, pid BE16, status (1 = linked)
//!                (answered by the receiver itself; a Nape Pro reports pid 4004)
//!   A7 31   ->  A7 31 LL CC        LL = level 0..100, CC non-zero while charging
//!
//! Replies are matched on the echoed request; unsolicited notifications are skipped. A7 31 is only
//! sent when a Nape Pro is linked in a receiver slot. Other devices on the Link-KM (Keychron keyboards
//! and mice) are not read this way.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x3434;
const LINK_KM: u16 = 0xD026;
const CABLE: u16 = 0x0440;
const NAPE_IN_SLOT: u16 = 0x4004;
const REPLY_MS: u64 = 600;

/// Whether the reply to B2 lists a linked Nape Pro: None when it is not a slot reply.
pub fn nape_linked(r: &[u8]) -> Option<bool> {
    if r.len() < 17 || r[0] != 0xB2 {
        return None;
    }
    Some([2usize, 7, 12].iter().any(|&o| u16::from_be_bytes([r[o + 2], r[o + 3]]) == NAPE_IN_SLOT && r[o + 4] == 1))
}

/// Reply to A7 31: Some((level, charging)).
pub fn decode_battery(r: &[u8]) -> Option<(i32, bool)> {
    (r.len() >= 4 && r[0] == 0xA7 && r[1] == 0x31 && r[2] <= 100).then(|| (r[2] as i32, r[3] != 0))
}

fn query<T>(d: &HidDev, cmd: &[u8], parse: fn(&[u8]) -> Option<T>) -> Option<T> {
    let mut out = [0u8; 33];
    out[1..=cmd.len()].copy_from_slice(cmd);
    d.flush();
    if !d.write(&out) {
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

/// None: no answer at all; Some(None): the receiver answered, no Nape Pro linked.
fn read(c: &HidColl, receiver: bool) -> Option<Option<(i32, bool)>> {
    let d = HidDev::open(c)?;
    if receiver && !query(&d, &[0xB2], nape_linked)? {
        return Some(None);
    }
    query(&d, &[0xA7, 0x31], decode_battery).map(Some)
}

pub fn poll(ctx: &mut PollContext) {
    for pid in [CABLE, LINK_KM] {
        if ctx.is_claimed(VID, pid) {
            continue;
        }
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFF60 && c.usage() == 0x0061 && c.out_len() == 33);
        let receiver = pid == LINK_KM;
        let Some(found) = colls.iter().find_map(|c| read(c, receiver)) else { continue };
        // No Nape Pro on this receiver: leave it to the other Keychron code (nothing to show).
        let Some((level, charging)) = found else { continue };
        if ctx.out.iter().any(|o| o.key == "keychron:nape") {
            ctx.claim(VID, pid);
            continue;
        }
        let s = DeviceStatus {
            key: "keychron:nape".into(),
            name: "Keychron Nape Pro".into(),
            kind: DeviceKind::Mouse,
            via: if receiver { "2.4 GHz receiver" } else { "USB cable" }.into(),
            hw: Some((VID, pid)),
            percent: level,
            charging: charging || !receiver,
            grace_sec: 1800,
            ..Default::default()
        };
        ctx.add(s, VID, pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots() {
        let mut r = vec![0u8; 32];
        r[0] = 0xB2;
        // slot 2 (bytes 7..11): 3434:4004, linked
        r[7..12].copy_from_slice(&[0x34, 0x34, 0x40, 0x04, 0x01]);
        assert_eq!(nape_linked(&r), Some(true));
        r[11] = 0;
        assert_eq!(nape_linked(&r), Some(false));
        r[0] = 0xA7;
        assert_eq!(nape_linked(&r), None);
    }

    #[test]
    fn battery() {
        let mut r = vec![0u8; 32];
        r[..4].copy_from_slice(&[0xA7, 0x31, 73, 0]);
        assert_eq!(decode_battery(&r), Some((73, false)));
        r[3] = 1;
        assert_eq!(decode_battery(&r), Some((73, true)));
        r[2] = 101;
        assert_eq!(decode_battery(&r), None);
        r[..2].copy_from_slice(&[0xA7, 0x30]);
        assert_eq!(decode_battery(&r), None);
    }
}
