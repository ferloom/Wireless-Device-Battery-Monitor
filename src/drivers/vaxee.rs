//! VAXEE wireless mice on their 2.4 GHz receivers (VID 3057, e.g. XE V2, #653).
//!
//! Protocol of the public VAXEE Control Center, as documented by OpenMouse mouse-protocol
//! (AGPL-3.0, github.com/OpenMouse-Project/mouse-protocol, docs/vaxee-testing.md and
//! src/vaxee/index.ts); implemented here from those facts. Vendor collection FF05:0001, feature
//! report 0E (63 bytes):
//!
//!   request  0E A5 CC 01 01 00 ...     CC = command, 01 = read, 01 = reply length
//!   reply    [0E] A5 CC 03 01 NN VV    after about 100 ms
//!   CC 0B    battery in steps of 5 %
//!   CC 10    non-zero while charging
//!
//! Only these two reads are sent. Not tried on hardware, there or here.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{self, HidColl, HidDev};
use crate::util::{hex4, sleep_ms};

const VID: u16 = 0x3057;
/// The receiver ids of the Control Center's list (wired ids are left out).
const RECEIVERS: [u16; 5] = [0x0005, 0x1001, 0x1002, 0x2001, 0x2002];
const REPORT: u8 = 0x0E;
const CMD_BATTERY: u8 = 0x0B;
const CMD_CHARGING: u8 = 0x10;
const PAUSE_MS: u64 = 100;

fn request(cmd: u8) -> [u8; 5] {
    [REPORT, 0xA5, cmd, 0x01, 0x01]
}

/// The value byte of the reply to `cmd`, with or without the leading report id.
fn parse(r: &[u8], cmd: u8) -> Option<u8> {
    let r = r.strip_prefix(&[REPORT]).unwrap_or(r);
    (r.len() >= 6 && r[..4] == [0xA5, cmd, 0x03, 0x01] && r[4] >= 1).then(|| r[5])
}

fn ask(d: &HidDev, cmd: u8) -> Option<u8> {
    if !d.set_feature(&request(cmd)) {
        return None;
    }
    sleep_ms(PAUSE_MS);
    parse(&d.get_feature(REPORT)?, cmd)
}

/// (level, charging). A level of 0 counts as no mouse linked, not as an empty battery.
fn read(c: &HidColl) -> Option<(i32, bool)> {
    let d = HidDev::open(c).or_else(|| HidDev::open_feature_only(c))?;
    let steps = ask(&d, CMD_BATTERY).filter(|s| (1..=20).contains(s))?;
    Some((steps as i32 * 5, ask(&d, CMD_CHARGING).is_some_and(|v| v != 0)))
}

pub fn poll(ctx: &mut PollContext) {
    for pid in RECEIVERS {
        if ctx.is_claimed(VID, pid) {
            continue;
        }
        let colls = ctx.find(VID, pid, |c| c.page() == 0xFF05 && c.usage() == 0x0001 && c.feat_len() == 64);
        if colls.is_empty() {
            continue;
        }
        let product = crate::report::clean_name(&hid::product_string(&colls[0]));
        let base = DeviceStatus {
            key: format!("vaxee:{}", hex4(pid)),
            name: if product.is_empty() || product == "?" { "VAXEE mouse".into() } else { product },
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
    fn frames() {
        assert_eq!(request(CMD_BATTERY), [0x0E, 0xA5, 0x0B, 0x01, 0x01]);
        assert_eq!(parse(&[0x0E, 0xA5, 0x0B, 0x03, 0x01, 0x01, 17], CMD_BATTERY), Some(17));
        assert_eq!(parse(&[0xA5, 0x10, 0x03, 0x01, 0x01, 1], CMD_CHARGING), Some(1));
        assert_eq!(parse(&request(CMD_BATTERY), CMD_BATTERY), None); // the request read back
        assert_eq!(parse(&[0x0E, 0xA5, 0x10, 0x03, 0x01, 0x01, 1], CMD_BATTERY), None); // another command
        assert_eq!(parse(&[0x0E, 0xA5, 0x0B, 0x03, 0x01, 0x00, 17], CMD_BATTERY), None); // no data
    }
}
