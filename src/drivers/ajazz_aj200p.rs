//! AJAZZ AJ200 / AJ200P "NL AI" mice on their 2.4 GHz receivers (VID 363C, e.g. 363C:ED0A).
//!
//! Protocol as documented by OpenMouse mouse-protocol (AGPL-3.0,
//! github.com/OpenMouse-Project/mouse-protocol, docs/ajazz-aj200p.md and src/ajazz/aj200p.ts;
//! hardware-checked there on the ED1C receiver only); implemented here from those facts. Vendor
//! collection FFA0:0002, 128-byte input and output reports:
//!
//!   request  0B 13 00 ...                  13 = read identity and battery
//!   reply    0A|0B 13 LEN 00 <payload>     00 = ok; payload [13] charging, [14] level %,
//!                                          [16] 1 = wireless
//!
//! Only this read is sent.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{self, HidColl, HidDev};
use crate::util::{Deadline, hex4};

const VID: u16 = 0x363C;
/// The receiver ids of the documented catalogue (wired ids are left out).
const RECEIVERS: [u16; 14] =
    [0xED00, 0xED05, 0xED08, 0xED0A, 0xED0C, 0xED1C, 0xED1E, 0xED26, 0xFC08, 0xFC0A, 0xFC0C, 0xFC1C, 0xFC1E, 0xFC26];
const REQUEST: [u8; 3] = [0x0B, 0x13, 0x00];
const LEN: u16 = 128;
const REPLY_MS: u64 = 500;

/// (level, charging) from an input report, report id first.
fn parse(r: &[u8]) -> Option<(i32, bool)> {
    let ok = r.len() >= 21 && matches!(r[0], 0x0A | 0x0B) && r[1] == 0x13 && r[2] >= 18 && r[3] == 0;
    // The payload starts at byte 4.
    (ok && (1..=100).contains(&r[18])).then(|| (r[18] as i32, r[20] != 1 && r[17] == 1))
}

fn read(c: &HidColl) -> Option<(i32, bool)> {
    let d = HidDev::open(c)?;
    d.flush();
    if !d.write(&REQUEST) {
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
    for pid in RECEIVERS {
        if ctx.is_claimed(VID, pid) {
            continue;
        }
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFFA0 && c.usage() == 0x0002 && c.in_len() == LEN && c.out_len() == LEN);
        if colls.is_empty() {
            continue;
        }
        let product = crate::report::clean_name(&hid::product_string(&colls[0]));
        let base = DeviceStatus {
            key: format!("aj200p:{}", hex4(pid)),
            name: if product.is_empty() || product == "?" { "AJAZZ AJ200 mouse".into() } else { product },
            kind: DeviceKind::Mouse,
            via: "2.4 GHz receiver".into(),
            hw: Some((VID, pid)),
            grace_sec: 1800,
            ..Default::default()
        };
        let s = match colls.iter().find_map(read) {
            Some((percent, charging)) => DeviceStatus { percent, charging, ..base },
            None => DeviceStatus { connected: false, ..base },
        };
        ctx.add(s, VID, pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        let mut r = [0u8; 128];
        r[..4].copy_from_slice(&[0x0A, 0x13, 55, 0x00]);
        (r[18], r[20]) = (64, 1);
        assert_eq!(parse(&r), Some((64, false)));
        (r[17], r[20]) = (1, 0); // on the cable
        assert_eq!(parse(&r), Some((64, true)));
        r[3] = 1; // an error status
        assert_eq!(parse(&r), None);
        r[3] = 0;
        r[18] = 0;
        assert_eq!(parse(&r), None);
        r[18] = 64;
        r[1] = 0x17; // another command's reply
        assert_eq!(parse(&r), None);
    }
}
