//! SinoWealth stock 2.4 GHz receiver (VID 0x258A, PID 0x002F): Redragon M602-KS and others.
//!
//! Protocol as used by BatteryTray (qu1ncky/BatteryTray, src/providers/devices/mice_platforms.rs):
//! feature report 5, two commands:
//!   ask(0x80): send `05 80`, wait 100 ms, read `05 80 01 ...` (byte 2 == 1 means the mouse is
//!   linked to the receiver);
//!   ask(0x90): send `05 90`, wait 100 ms, read `05 90 11 LL` (byte 2 == 0x11, byte 3 = level).
use std::time::Duration;

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::HidDev;
use crate::util::hex4;

const VID: u16 = 0x258A;
const PID: u16 = 0x002F;

pub fn poll(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID) {
        return;
    }
    let colls = ctx.find(VID, PID, |c| c.page() == 0xFF00 && c.feat_len() == 8);
    let Some(coll) = colls.first() else { return };
    let Some(dev) = HidDev::open(coll) else { return };

    // Link check.
    let mut req = [0u8; 8];
    req[0] = 0x05;
    req[1] = 0x80;
    if !dev.set_feature(&req) {
        return;
    }
    std::thread::sleep(Duration::from_millis(100));
    let Some(resp) = dev.get_feature(0x05) else { return };
    if !(resp.len() >= 3 && resp[0] == 0x05 && resp[1] == 0x80 && resp[2] == 0x01) {
        return; // mouse not linked / receiver present but empty
    }

    // Battery level.
    let mut req = [0u8; 8];
    req[0] = 0x05;
    req[1] = 0x90;
    if !dev.set_feature(&req) {
        return;
    }
    std::thread::sleep(Duration::from_millis(100));
    let Some(resp) = dev.get_feature(0x05) else { return };
    if !(resp.len() >= 4 && resp[0] == 0x05 && resp[1] == 0x90 && resp[2] == 0x11) {
        return;
    }
    let level = resp[3] as i32;

    let mut name = crate::hid::product_string(coll);
    if name.is_empty() {
        name = "Wireless mouse".into();
    }
    let s = DeviceStatus {
        key: format!("sinowealth:{}", hex4(PID)),
        name,
        via: "2.4 GHz receiver".into(),
        kind: DeviceKind::Mouse,
        percent: level,
        hw: Some((VID, PID)),
        ..Default::default()
    };
    ctx.add(s, VID, PID);
}