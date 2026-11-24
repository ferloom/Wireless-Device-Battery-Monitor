//! Mice on CompX's "report 8" protocol (VID 3554, CompX's ODM id, ATK's 373B and Pulsar's 3710): Pulsar X2 V2 /
//! X2H / X2A / Xlite V3, XS-1 TenZ, Kysona M600, ATK VXE R1 SE+, VXE Dragonfly R1 Pro (Max) and the LAMZU
//! Atlantis / Thorn generation on the "LAMZU 4K Receiver". Each product id is listed explicitly;
//! nothing else on these vendor ids gets a frame (3554:FA09 / FB40 are AULA keyboards, aula.rs).
//!
//! Protocol and ids as documented by the Linux kernel's hid-pulsar driver (GPL-2.0+,
//! drivers/hid/hid-pulsar.c and hid-ids.h, formerly hid-kysona), vxe-r1-pro-max-battery (MIT,
//! github.com/S1berla/vxe-r1-pro-max-battery, docs/protocol.md, verified on 3554:F58A) and
//! HaloBattery (MIT, github.com/HeyOkay/HaloBattery, providers/compx.py and providers/lamzu.py for
//! the LAMZU 3554:F510 / F50F ids, from OpenMouse-Project/mouse-protocol's Atlantis notes), and as
//! used by the original C++ BatteryHub (GPL-3.0, src/drivers/kysona.cpp); implemented here from
//! those facts. On interface 1, vendor collection FF02:0002 (17-byte input and output report 08):
//!
//!   request  08 CC 00 ... 00 SS          SS (byte 16) = 0x55 - sum(bytes 0..15)
//!   cmd 03   reply [6] = 1 when the mouse is linked to the receiver, 0 when it is not
//!   cmd 04   reply [6] = level 0..100, [7] = 1 on USB power (charging), [8..9] = mV (big endian)
//!
//! A reply counts only with the echoed command and a valid checksum. Read-only: no setting is
//! written. A mouse that is off or asleep is left out (the app keeps its last level, dimmed).
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const LEN: usize = 17;
const REPORT: u8 = 0x08;
pub const CMD_STATUS: u8 = 0x03;
pub const CMD_POWER: u8 = 0x04;
const REPLY_MS: u64 = 300;
/// Nothing answered at all (receiver without this protocol, or link down): ask again after this.
const SILENT_QUERY_EVERY: Duration = Duration::from_secs(60);

struct Product {
    vid: u16,
    pid: u16,
    name: &'static str,
    /// Receiver and cable of one mouse share the group, so a charging mouse is listed once.
    group: &'static str,
    cable: bool,
}

const fn p(vid: u16, pid: u16, name: &'static str, group: &'static str, cable: bool) -> Product {
    Product { vid, pid, name, group, cable }
}

const PRODUCTS: [Product; 14] = [
    p(0x3554, 0xF508, "Pulsar mouse", "pulsar1k", false),
    p(0x3554, 0xF509, "Pulsar mouse", "pulsar4k", false),
    p(0x3554, 0xF507, "Pulsar mouse", "pulsar-cable", true),
    p(0x3554, 0xF57C, "Kysona M600", "m600", false),
    // Shared by the Kysona M600 v2, Redragon K1NG Pro / M913 and Taidu TSG808 (issue #87).
    p(0x3554, 0xF5D5, "Wireless mouse", "m600v2", false),
    p(0x3554, 0xF57D, "Kysona M600", "m600", true),
    p(0x373B, 0x1085, "VXE R1 SE+", "r1se", false),
    p(0x3554, 0xF58F, "VXE R1 SE+", "r1se", true),
    p(0x3554, 0xF58A, "VXE Dragonfly R1", "r1pro", false),
    p(0x3554, 0xF58C, "VXE Dragonfly R1", "r1pro", true),
    // Six LAMZU models (Atlantis OG V2 / Mini / Mini Pro, Thorn, Maya, Paro) share these ids.
    p(0x3554, 0xF510, "LAMZU mouse", "lamzu4k", false),
    p(0x3554, 0xF50F, "LAMZU mouse", "lamzu4k", true),
    // Pulsar XS-1 8K Gen.2 receiver and its wired TenZ identity; both use report 8.
    p(0x3710, 0x5406, "Pulsar TenZ", "pulsar-xs1", false),
    // 3710:3415 / interface 1 / FF02:0002 (17-byte reports), verified with the original BatteryHub.
    p(0x3710, 0x3415, "Pulsar TenZ", "pulsar-xs1", true),
];

pub fn checksum(b: &[u8]) -> u8 {
    b[..LEN - 1].iter().fold(0x55u8, |a, &x| a.wrapping_sub(x))
}

pub fn request(cmd: u8) -> [u8; LEN] {
    let mut r = [0u8; LEN];
    r[0] = REPORT;
    r[1] = cmd;
    r[LEN - 1] = checksum(&r);
    r
}

fn valid(r: &[u8], cmd: u8) -> bool {
    r.len() >= LEN && r[0] == REPORT && r[1] == cmd && r[LEN - 1] == checksum(r)
}

/// Reply to cmd 03: Some(mouse linked).
pub fn parse_status(r: &[u8]) -> Option<bool> {
    (valid(r, CMD_STATUS) && r[6] <= 1).then(|| r[6] == 1)
}

/// Reply to cmd 04: Some((level, on power, millivolts)).
pub fn parse_power(r: &[u8]) -> Option<(i32, bool, i32)> {
    (valid(r, CMD_POWER) && r[6] <= 100 && r[7] <= 1).then(|| (r[6] as i32, r[7] == 1, ((r[8] as i32) << 8) | r[9] as i32))
}

fn query<T>(d: &HidDev, cmd: u8, parse: fn(&[u8]) -> Option<T>) -> Option<T> {
    d.flush();
    if !d.write(&request(cmd)) {
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

enum Reading {
    Power(i32, bool, i32),
    /// The receiver answered: no mouse linked, or linked but the mouse did not answer.
    Away,
}

fn read(c: &HidColl) -> Option<Reading> {
    let d = HidDev::open(c)?;
    let linked = query(&d, CMD_STATUS, parse_status);
    if linked == Some(false) {
        return Some(Reading::Away);
    }
    match query(&d, CMD_POWER, parse_power) {
        Some((level, power, mv)) => Some(Reading::Power(level, power, mv)),
        None => linked.map(|_| Reading::Away),
    }
}

static NEXT_TRY: Mutex<Option<HashMap<(u16, u16), Instant>>> = Mutex::new(None);

fn key(prod: &Product) -> String {
    format!("compx:{}", prod.group)
}

fn presence_anchor(ctx: &PollContext, prod: &Product, vid: u16, pid: u16) -> (u16, u16) {
    if prod.cable && vid == 0x3710 && ctx.colls.iter().any(|c| c.vid == vid && c.pid == 0x5406) { (vid, 0x5406) } else { (vid, pid) }
}

fn add_or_merge(ctx: &mut PollContext, mut s: DeviceStatus, prod: &Product, vid: u16, pid: u16) {
    if let Some(existing) = ctx.out.iter_mut().find(|o| o.key == s.key) {
        if prod.cable && vid == 0x3710 {
            existing.percent = s.percent;
            existing.millivolts = s.millivolts;
            existing.charging = s.charging;
            existing.via = std::mem::take(&mut s.via);
        } else {
            existing.charging |= s.charging;
        }
        ctx.claim(vid, pid);
    } else {
        ctx.add(s, vid, pid);
    }
}

fn battery_collection(vid: u16, c: &HidColl) -> bool {
    if vid == 0x3710 {
        c.page() == 0xFF02 && c.usage() == 0x0002 && c.iface == 1 && c.in_len() as usize == LEN && c.out_len() as usize == LEN
    } else {
        c.page() >= 0xFF00 && c.in_len() as usize == LEN && c.out_len() as usize == LEN
    }
}

pub fn poll(ctx: &mut PollContext) {
    for prod in &PRODUCTS {
        let (vid, pid) = (prod.vid, prod.pid);
        if ctx.is_claimed(vid, pid) {
            continue;
        }
        let mut colls = ctx.find(vid, pid, |c| battery_collection(vid, c));
        if colls.is_empty() {
            continue;
        }
        // FF02:0002 on interface 1 is the battery channel; other 17-byte vendor collections come after.
        colls.sort_by_key(|c| !(c.page() == 0xFF02 && c.usage() == 0x0002 && c.iface == 1));
        let due = {
            let mut g = NEXT_TRY.lock().unwrap_or_else(|e| e.into_inner());
            g.get_or_insert_with(HashMap::new).get(&(vid, pid)).is_none_or(|t| Instant::now() >= *t)
        };
        if !due {
            continue;
        }
        let reading = colls.iter().find_map(read);
        {
            let mut g = NEXT_TRY.lock().unwrap_or_else(|e| e.into_inner());
            let m = g.get_or_insert_with(HashMap::new);
            if reading.is_none() {
                m.insert((vid, pid), Instant::now() + SILENT_QUERY_EVERY);
            } else {
                m.remove(&(vid, pid));
            }
        }
        let Some(Reading::Power(level, power, mv)) = reading else {
            if reading.is_some() {
                ctx.claim(vid, pid); // off or asleep: the app keeps the last level
            }
            continue;
        };
        let logical_key = key(prod);
        // Pulsar's reply byte 7 is authoritative for both USB and radio. Other cable drivers may
        // omit that bit, so preserve their established cable-means-powered behavior.
        let charging = if vid == 0x3710 { power } else { power || prod.cable };
        // Keep the receiver as the presence anchor when it is also plugged in. That gives the
        // last-known reading its grace period if the mouse briefly reconnects after cable removal.
        let s = DeviceStatus {
            key: logical_key,
            name: prod.name.into(),
            kind: DeviceKind::Mouse,
            via: if prod.cable { "USB cable" } else { "2.4 GHz receiver" }.into(),
            percent: level,
            charging,
            millivolts: mv,
            hw: Some(presence_anchor(ctx, prod, vid, pid)),
            grace_sec: 1800,
            ..Default::default()
        };
        add_or_merge(ctx, s, prod, vid, pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bytes: &[u8]) -> Vec<u8> {
        let mut f = vec![0u8; LEN];
        f[..bytes.len()].copy_from_slice(bytes);
        f[LEN - 1] = checksum(&f);
        f
    }

    #[test]
    fn requests() {
        // vxe-r1-pro-max-battery docs/protocol.md
        let r = request(CMD_STATUS);
        assert_eq!(r[..2], [0x08, 0x03]);
        assert_eq!(r[16], 0x4A);
        assert!(r[2..16].iter().all(|&b| b == 0));
        assert_eq!(request(CMD_POWER)[16], 0x49);
    }

    #[test]
    fn status_reply() {
        // Online reply recorded on 3554:F58A (vxe-r1-pro-max-battery).
        let r = [0x08, 0x03, 0, 0, 0, 0x01, 0x01, 0xC5, 0x1B, 0x07, 0, 0, 0, 0, 0, 0, 0x61];
        assert_eq!(parse_status(&r), Some(true));
        assert_eq!(parse_status(&frame(&[0x08, 0x03, 0, 0, 0, 0x01, 0x00])), Some(false));
        let mut bad = r;
        bad[16] = 0x62;
        assert_eq!(parse_status(&bad), None);
    }

    #[test]
    fn power_reply() {
        // 95 %, on battery, 0x101A mV (kernel layout; the doc's example has a mistyped checksum).
        let r = frame(&[0x08, 0x04, 0, 0, 0, 0x02, 0x5F, 0x00, 0x10, 0x1A]);
        assert_eq!(parse_power(&r), Some((95, false, 4122)));
        assert_eq!(parse_power(&frame(&[0x08, 0x04, 0, 0, 0, 0x02, 0x40, 0x01, 0x10, 0x60])), Some((64, true, 4192)));
        assert_eq!(parse_power(&frame(&[0x08, 0x04, 0, 0, 0, 0x02, 0x65, 0x00])), None); // 101 %
        assert_eq!(parse_power(&frame(&[0x08, 0x03, 0, 0, 0, 0x02, 0x40, 0x00])), None); // other command
        assert_eq!(parse_power(&r[..10]), None);
    }

    #[test]
    fn only_listed_ids() {
        assert!(PRODUCTS.iter().all(|p| !(p.vid == 0x3554 && (p.pid == 0xFA09 || p.pid == 0xFB40))));
        assert!(PRODUCTS.iter().any(|p| p.pid == 0xF58C && p.cable));
        assert!(PRODUCTS.iter().any(|p| p.pid == 0xF510 && !p.cable));
        assert!(PRODUCTS.iter().any(|p| p.vid == 0x3710 && p.pid == 0x5406 && p.name == "Pulsar TenZ"));
        assert!(PRODUCTS.iter().any(|p| p.vid == 0x3710 && p.pid == 0x3415 && p.cable));
    }

    #[test]
    fn pulsar_uses_only_its_validated_battery_collection() {
        let coll =
            |iface, page, usage| HidColl { iface, usage_page: page, usage_id: usage, input_len: LEN as u16, output_len: LEN as u16, ..Default::default() };
        assert!(battery_collection(0x3710, &coll(1, 0xFF02, 0x0002)));
        assert!(!battery_collection(0x3710, &coll(2, 0xFF02, 0x0002)));
        assert!(!battery_collection(0x3710, &coll(1, 0xFF03, 0x0002)));
        assert!(!battery_collection(0x3710, &coll(1, 0xFF02, 0x0003)));
        assert!(battery_collection(0x3554, &coll(2, 0xFF03, 0x0002)));
    }

    #[test]
    fn pulsar_cable_and_receiver_keep_one_identity_and_charge_state() {
        let receiver = PRODUCTS.iter().find(|p| p.vid == 0x3710 && p.pid == 0x5406).unwrap();
        let cable = PRODUCTS.iter().find(|p| p.vid == 0x3710 && p.pid == 0x3415).unwrap();
        assert_eq!(key(receiver), key(cable));

        let mut ctx = PollContext::default();
        ctx.colls.push(HidColl { vid: receiver.vid, pid: receiver.pid, ..Default::default() });
        ctx.add(
            DeviceStatus {
                key: key(receiver),
                name: receiver.name.into(),
                percent: 95,
                charging: false,
                via: "2.4 GHz receiver".into(),
                hw: Some((receiver.vid, receiver.pid)),
                ..Default::default()
            },
            receiver.vid,
            receiver.pid,
        );
        assert_eq!(presence_anchor(&ctx, cable, cable.vid, cable.pid), (receiver.vid, receiver.pid));
        add_or_merge(
            &mut ctx,
            DeviceStatus { key: key(cable), name: cable.name.into(), percent: 95, charging: true, via: "USB cable".into(), ..Default::default() },
            cable,
            cable.vid,
            cable.pid,
        );
        assert_eq!(ctx.out.len(), 1);
        assert_eq!(ctx.out[0].key, "compx:pulsar-xs1");
        assert_eq!(ctx.out[0].via, "USB cable");
        assert!(ctx.out[0].charging);
        assert_eq!(ctx.out[0].hw, Some((receiver.vid, receiver.pid)));
        assert!(ctx.is_claimed(cable.vid, cable.pid));
    }
}
