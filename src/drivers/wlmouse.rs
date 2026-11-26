//! WLMOUSE mice (VID 36A7: Beast X, Beast X Mini, Mini Pro, Max) over their 2.4 GHz receivers.
//!
//! Protocol from HaloBattery (providers/wlmouse.py, MIT), which credits mouse-battery-tray (MIT).
//! The vendor collection (usage page FFFF, 65-byte feature report, report ID 0) answers a status
//! query:
//!
//!   request  (feature SET)  00 | 00 00 02 02 00 83 | 00 ... (65 bytes)
//!   reply    (feature GET)  A1|A2 ?? ?? 02 00 83 CC NN ...
//!            CC  charging (non-zero)
//!            NN  battery level 0..100 %
//!
//! The reply is found by scanning for `02 00 83` preceded by A1/A2 three bytes earlier, so a
//! leading report-ID byte does not matter. The receiver needs a moment to fetch the answer from the
//! mouse. No reply means the mouse is off or asleep.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{self, HidColl, HidDev};
use crate::util::{hex4, sleep_ms};

const VID: u16 = 0x36A7;
const QUERY: [u8; 7] = [0x00, 0x00, 0x00, 0x02, 0x02, 0x00, 0x83];

fn model(pid: u16) -> Option<&'static str> {
    Some(match pid {
        0xA887 => "WLmouse Beast X",
        0xA885 => "WLmouse Beast X Mini",
        0xA868 => "WLmouse Beast X Mini Pro",
        0xA880 => "WLmouse Beast X Max",
        _ => return None,
    })
}

/// Finds the status reply in a feature report: Some((level, charging)).
pub fn parse(r: &[u8]) -> Option<(i32, bool)> {
    (5..r.len().saturating_sub(2)).find_map(|i| {
        let hit = r[i] == 0x83 && r[i - 1] == 0x00 && r[i - 2] == 0x02 && matches!(r[i - 5], 0xA1 | 0xA2);
        (hit && r[i + 2] <= 100).then(|| (r[i + 2] as i32, r[i + 1] != 0))
    })
}

fn query(c: &HidColl) -> Option<(i32, bool)> {
    let d = HidDev::open(c).or_else(|| HidDev::open_feature_only(c))?;
    if !d.set_feature(&QUERY) {
        return None;
    }
    // The receiver relays the query to the mouse; poll for the answer for up to ~0.8 s.
    for _ in 0..16 {
        sleep_ms(50);
        if let Some(v) = d.get_feature(0).and_then(|r| parse(&r)) {
            return Some(v);
        }
    }
    None
}

/// The mouse a USB-cable id belongs to: its cable and its receiver are one device (one key).
/// Only pairs seen on hardware (#4: the Beast X Mini's cable is `A886`, its receiver `A885`).
fn receiver_of(pid: u16) -> Option<u16> {
    match pid {
        0xA886 => Some(0xA885),
        _ => None,
    }
}

pub fn poll(ctx: &mut PollContext) {
    // One entry per mouse: on its cable and its receiver at the same time it is read once, the
    // cable's answer first (it also says whether it charges).
    let mut found_by: Vec<(DeviceStatus, u16)> = Vec::new();
    for (vid, pid) in ctx.products(VID) {
        let mut colls = ctx.find(vid, pid, |c| c.page() == 0xFFFF && c.feat_len() >= 64);
        if colls.is_empty() {
            continue;
        }
        colls.sort_by_key(|c| c.usage() != 0);
        let cable = receiver_of(pid);
        let mouse = cable.unwrap_or(pid);
        let name = model(mouse).map(String::from).unwrap_or_else(|| {
            let n = crate::report::clean_name(&hid::product_string(&colls[0]));
            if n.is_empty() || n == "?" { "WLmouse mouse".into() } else { n }
        });
        let found = colls.iter().find_map(query);
        // Both ids are ours, also the one whose answer is not kept.
        ctx.claim(vid, pid);
        let base = DeviceStatus {
            key: format!("wlmouse:{}", hex4(mouse)),
            name,
            kind: DeviceKind::Mouse,
            via: if cable.is_some() { "USB cable".into() } else { "2.4 GHz receiver".into() },
            hw: Some((vid, pid)),
            grace_sec: 60,
            ..Default::default()
        };
        let s = match found {
            Some((level, charging)) => DeviceStatus { percent: level, charging, ..base },
            None => DeviceStatus { connected: false, ..base },
        };
        match found_by.iter_mut().find(|(o, _)| o.key == s.key) {
            // The other one already answered (the cable before the receiver): keep the better.
            Some(slot) => {
                if (s.connected && !slot.0.connected) || (s.connected && cable.is_some()) {
                    *slot = (s, pid);
                }
            }
            None => found_by.push((s, pid)),
        }
    }
    for (s, pid) in found_by {
        ctx.add(s, VID, pid);
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn cable_and_receiver_are_one_mouse() {
        assert_eq!(super::receiver_of(0xA886), Some(0xA885));
        assert_eq!(super::receiver_of(0xA885), None);
    }

    #[test]
    fn reply() {
        let mut r = vec![0u8; 65];
        r[..8].copy_from_slice(&[0xA1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00, 0x4D]);
        assert_eq!(parse(&r), Some((77, false)));
        // With a leading report-ID byte, charging.
        let mut r = vec![0u8; 65];
        r[..9].copy_from_slice(&[0x00, 0xA2, 0x00, 0x02, 0x02, 0x00, 0x83, 0x01, 0x32]);
        assert_eq!(parse(&r), Some((50, true)));
    }

    #[test]
    fn not_a_reply() {
        // Our own query echoed back, and a level out of range.
        let mut q = vec![0u8; 65];
        q[1..7].copy_from_slice(&[0x00, 0x00, 0x02, 0x02, 0x00, 0x83]);
        assert_eq!(parse(&q), None);
        let mut r = vec![0u8; 65];
        r[..8].copy_from_slice(&[0xA1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00, 0xFF]);
        assert_eq!(parse(&r), None);
    }
}
