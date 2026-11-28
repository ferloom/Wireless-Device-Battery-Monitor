//! AJAZZ 8K mice (e.g. AJ159 APEX) and the Angry Miao AM Infinity 8K on the 2.4 GHz receiver
//! 3151:5007 ("AJAZZ 2.4G 8K", issue #471).
//!
//! Protocol as documented by HaloBattery (MIT, github.com/HeyOkay/HaloBattery,
//! providers/am_infinity.py), from AJAZZ Control Center (GPL-3.0, github.com/Aiacos/ajazz-control-center,
//! its AJ-series mouse battery notes), which reads this receiver on its own AJ159 APEX; implemented
//! here from those facts. The receiver copies the mouse's charge into a status feature report only
//! after the vendor's status poll, sent to the control collection FFFF:0002:
//!
//!   request  feature report 00:  00 F7 00 ... (65 bytes)
//!   reply    feature report 05:  05 00 00 LL ...   LL = level, 0 = link not up yet
//!
//! A non-zero byte between the report id and the level is junk a reconnect can return and is
//! refused. No charging flag. Only this poll is sent.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::sleep_ms;

const VID: u16 = 0x3151;
const PID: u16 = 0x5007;
const POLL: u8 = 0xF7;
const STATUS_REPORT: u8 = 0x05;
const SETTLE_MS: u64 = 30;
const TRIES: u64 = 5;

pub fn request() -> [u8; 2] {
    [0x00, POLL]
}

/// The level from a status report (report id in byte 0).
pub fn parse(r: &[u8]) -> Option<i32> {
    if r.len() < 4 || r[0] != STATUS_REPORT || r[1] != 0 || r[2] != 0 || r[3] == 0 {
        return None;
    }
    Some(r[3].min(100) as i32)
}

fn read(c: &HidColl) -> Option<i32> {
    let d = HidDev::open_feature_only(c)?;
    // The vendor app polls about once a second, so the first answer can still be the empty one:
    // ask a few times, waiting a little longer each time (about 0.45 s in all).
    for attempt in 1..=TRIES {
        if !d.set_feature(&request()) {
            return None;
        }
        sleep_ms(SETTLE_MS * attempt);
        if let Some(level) = d.get_feature(STATUS_REPORT).and_then(|r| parse(&r)) {
            return Some(level);
        }
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID) {
        return;
    }
    let colls = ctx.find(VID, PID, |c| c.page() == 0xFFFF && c.usage() == 0x0002 && c.feat_len() >= 64);
    if colls.is_empty() {
        return;
    }
    let base = DeviceStatus {
        key: "ajazz8k:5007".into(),
        name: "AJAZZ 8K mouse".into(),
        kind: DeviceKind::Mouse,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, PID)),
        grace_sec: 300,
        ..Default::default()
    };
    let s = match colls.iter().find_map(read) {
        Some(level) => DeviceStatus { percent: level, ..base },
        None => DeviceStatus { connected: false, ..base },
    };
    ctx.add(s, VID, PID);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_report() {
        assert_eq!(parse(&[0x05, 0x00, 0x00, 0x64, 0x01, 0x01, 0x01, 0x02]), Some(100));
        assert_eq!(parse(&[0x05, 0x00, 0x00, 0x2A]), Some(42));
        // Link not up yet, junk after a reconnect, another report.
        assert_eq!(parse(&[0x05, 0x00, 0x00, 0x00]), None);
        assert_eq!(parse(&[0x05, 0x00, 0x07, 0x40]), None);
        assert_eq!(parse(&[0x00, 0xF7, 0x00, 0x00]), None);
        assert_eq!(request(), [0x00, 0xF7]);
    }
}
