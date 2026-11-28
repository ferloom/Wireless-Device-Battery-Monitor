//! Ninjutso Sora V2 on its 1K 2.4 GHz receiver (1915:AE1C) or USB cable (1915:AE11), issue #479.
//!
//! Feature report 5 on the vendor collection FFA0:0001, 32 bytes. One read-only query is sent:
//!
//!   request  05 15 00 00 01 00 ...
//!   reply    05 15 .. .. .. .. .. .. .. LL CH FULL ONLINE ..
//!
//! LL = percent (0 means 100 %, as NinjaForce's own web app reads it), CH = charging, FULL = full,
//! ONLINE = mouse reachable (0 on the cable, so the cable always counts as connected). The reply is
//! fetched up to four times, as the bundled NinjaForce reader does: a sleeping mouse can answer
//! with ONLINE = 0 first. Nothing else is sent.
//!
//! Sources: NinjaForce's own page (out/renderer/assets/createSoraV2-*.js, `readState`), as also
//! implemented in Fan4Metal/Sora_tray (sora_tray.py) and Fan4Metal/mouse_tray
//! (mouse_tray/drivers/vendor/ninjutso.py); the cable's ONLINE = 0 reply was tested on hardware
//! and the same exchange is used by fuyue521/Wireless-Device-Battery-Monitor-CN (ninjutso.cpp).
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{hex4, sleep_ms};

const VID: u16 = 0x1915;
const RECEIVER: u16 = 0xAE1C;
const CABLE: u16 = 0xAE11;
const LEN: usize = 32;
const TRIES: usize = 4;
const GAP_MS: u64 = 90;
const REPORT_ID: u8 = 0x05;
const CMD: u8 = 0x15;

pub fn request() -> [u8; LEN] {
    let mut r = [0u8; LEN];
    r[0] = REPORT_ID;
    r[1] = CMD;
    r[4] = 0x01;
    r
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    Level(i32, bool, bool),
    Asleep,
}

/// A report-5 reply to our query. `cable` accepts ONLINE = 0, as the cable's reply does.
pub fn parse(r: &[u8], cable: bool) -> Option<Reading> {
    if r.len() < 13 || r[0] != REPORT_ID || r[1] != CMD || r[9] > 100 {
        return None;
    }
    if !cable && r[12] == 0 {
        return Some(Reading::Asleep);
    }
    Some(Reading::Level(if r[9] == 0 { 100 } else { r[9] as i32 }, r[10] != 0, r[11] != 0))
}

fn query(c: &HidColl, cable: bool) -> Option<Reading> {
    let d = HidDev::open_feature_only(c)?;
    let req = request();
    for attempt in 0..TRIES {
        if attempt > 0 {
            sleep_ms(GAP_MS);
        }
        if !d.set_feature_exact(&req) {
            return None;
        }
        sleep_ms(15);
        if let Some(v) = d.get_feature_exact(REPORT_ID, LEN).and_then(|r| parse(&r, cable)) {
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
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFFA0 && c.usage() == 0x0001 && c.feat_len() as usize >= LEN);
        // Cable and receiver are one mouse (one key): the cable's answer, read first, stands.
        let key = format!("ninjutso:{}:{}", hex4(VID).to_lowercase(), hex4(RECEIVER).to_lowercase());
        if ctx.out.iter().any(|o| o.key == key) {
            ctx.claim(VID, pid);
            continue;
        }
        let base = DeviceStatus {
            key,
            name: "Ninjutso Sora V2".into(),
            kind: DeviceKind::Mouse,
            via: if pid == CABLE { "USB cable" } else { "2.4 GHz receiver" }.into(),
            hw: Some((VID, pid)),
            grace_sec: 1800,
            ..Default::default()
        };
        let s = match colls.iter().find_map(|c| query(c, pid == CABLE)) {
            // On its cable the mouse is on power, also when the reply does not say so (#753).
            Some(Reading::Level(percent, charging, full)) => DeviceStatus { percent, charging: charging || (pid == CABLE && !full), full, ..base },
            Some(Reading::Asleep) => DeviceStatus { connected: false, ..base },
            None => continue,
        };
        ctx.add(s, VID, pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(level: u8, charging: u8, full: u8, online: u8) -> [u8; LEN] {
        let mut r = [0u8; LEN];
        r[0] = REPORT_ID;
        r[1] = CMD;
        r[9] = level;
        r[10] = charging;
        r[11] = full;
        r[12] = online;
        r
    }

    #[test]
    fn request_shape() {
        let r = request();
        assert_eq!(r[..5], [0x05, 0x15, 0x00, 0x00, 0x01]);
        assert!(r[5..].iter().all(|b| *b == 0));
    }

    #[test]
    fn replies() {
        assert_eq!(parse(&reply(66, 0, 0, 1), false), Some(Reading::Level(66, false, false)));
        // The web app reads 0 as a full battery; on the cable ONLINE is 0.
        assert_eq!(parse(&reply(0, 1, 1, 0), true), Some(Reading::Level(100, true, true)));
        assert_eq!(parse(&reply(66, 0, 0, 0), false), Some(Reading::Asleep));
    }

    #[test]
    fn rejects_other_reports() {
        assert_eq!(parse(&[], false), None); // empty reply
        let mut r = reply(50, 0, 0, 1);
        r[1] = 0x16;
        assert_eq!(parse(&r, false), None);
        r = reply(50, 0, 0, 1);
        r[9] = 101;
        assert_eq!(parse(&r, false), None);
        assert_eq!(parse(&reply(50, 0, 0, 1)[..12], false), None);
    }
}
