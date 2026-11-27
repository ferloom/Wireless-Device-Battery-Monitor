//! Steam Controller (2026) over its "Puck" receiver (VID 28DE, PID 1304 Proteus / 1305 Nereid).
//!
//! Listen only: nothing is ever written to the puck. SDL's HIDAPI driver for the Triton
//! controller (`SDL_hidapi_steam_triton.c` and `steam/controller_structs.h`, zlib) reads one
//! controller per interface 2..5 of the puck, on its vendor collection FF00:0001. The controller
//! pushes input report 0x43 (ID_TRITON_BATTERY_STATUS) with the charge state and the level in %;
//! reports 0x79 / 0x46 say a controller connected or disconnected, and 0x42 / 0x45 / 0x47 are
//! state reports that only mean the controller in that slot is switched on. SDL sends rumble,
//! IMU and a "lizard mode" setting over the same channel (re-sent every 3 s); sending any of that
//! here would change how the user's games see the controller, so this driver only reads.
//!
//! The battery report comes about every 2.5 s while the controller is awake (~0.4 Hz, measured
//! by CouchTurtle/sc2-research and eva-val/steambattery and confirmed on hardware in HaloBattery
//! #157). A quiet slot gets a short listen; a slot with a live controller is listened to for up
//! to 5 s, twice the cadence, and the read stops as soon as the battery report or a disconnect
//! arrives. The last level is kept, dimmed, while the puck is there. The older 2015 Steam
//! Controller dongle (1142) speaks a different protocol and is not covered.
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{Deadline, debug_log};

const VID: u16 = 0x28DE;
const PROTEUS: u16 = 0x1304;
const NEREID: u16 = 0x1305;
const PIDS: [u16; 2] = [PROTEUS, NEREID];
/// One controller slot per interface, as SDL matches them (the puck's own collection is on if6).
const SLOTS: [i32; 4] = [2, 3, 4, 5];
const VENDOR_PAGE: u16 = 0xFF00;
const VENDOR_USAGE: u16 = 0x0001;
const GAMEPAD_PAGE: u16 = 0x0001;
const GAMEPAD_USAGE: u16 = 0x0005;

const REPORT_BATTERY: u8 = 0x43;
/// 1 report id + sizeof(TritonBatteryStatus_t).
const BATTERY_LEN: usize = 15;
const REPORTS_WIRELESS: [u8; 2] = [0x79, 0x46];
const REPORTS_STATE: [u8; 3] = [0x42, 0x45, 0x47];
const WIRELESS_CONNECTED: u8 = 2;
const WIRELESS_DISCONNECTED: u8 = 1;
const CHARGE_DISCHARGING: u8 = 1;
const CHARGE_CHARGING: u8 = 2;
const CHARGE_DONE: u8 = 4;

const WINDOW_MS: u32 = 400;
const BATTERY_WINDOW_MS: u32 = 5000;
/// Safety valve against a pathological stream (5 s of ~266 Hz is ~1300 frames).
const MAX_REPORTS: usize = 4096;
const GRACE_SEC: u32 = 1800;

/// Slot key -> (level, charging, full), kept between polls.
static LAST: Mutex<Option<HashMap<String, (i32, bool, bool)>>> = Mutex::new(None);

fn last_map() -> std::sync::MutexGuard<'static, Option<HashMap<String, (i32, bool, bool)>>> {
    LAST.lock().unwrap_or_else(|e| e.into_inner())
}

/// Input report 0x43 -> (level %, charging, full). The charge state is EChargeState:
/// 1 discharging, 2 charging, 4 charging done; 0 and 3 mean nothing usable.
pub fn decode_battery(r: &[u8]) -> Option<(i32, bool, bool)> {
    if r.len() < BATTERY_LEN || r[0] != REPORT_BATTERY || r[2] > 100 {
        return None;
    }
    match r[1] {
        CHARGE_DISCHARGING => Some((r[2] as i32, false, false)),
        CHARGE_CHARGING => Some((r[2] as i32, true, false)),
        CHARGE_DONE => Some((r[2] as i32, true, true)),
        _ => None,
    }
}

/// The collection to listen on for one slot. The puck has several HID faces per interface:
/// prefer FF00:0001 (the slot's vendor channel), then another vendor collection, then the
/// gamepad face. SDL matches the interface alone, so all three can carry the report.
pub fn pick<'a>(colls: &'a [HidColl], pid: u16, iface: i32) -> Option<&'a HidColl> {
    fn rank(c: &HidColl) -> u8 {
        if c.page() == VENDOR_PAGE && c.usage() == VENDOR_USAGE {
            0
        } else if c.page() >= 0xFF00 {
            1
        } else if c.page() == GAMEPAD_PAGE && c.usage() == GAMEPAD_USAGE {
            2
        } else {
            3
        }
    }
    colls.iter()
        .filter(|c| c.vid == VID && c.pid == pid && c.iface == iface && c.in_len() > 0)
        .min_by_key(|c| (rank(c), c.path.len()))
}

#[derive(Clone, Copy, Default)]
struct Reading {
    battery: Option<(i32, bool, bool)>,
    state: bool,
    wireless: Option<u8>,
}

/// Reads one slot. A quiet slot costs WINDOW_MS; a slot with a live controller waits for the
/// battery report for up to BATTERY_WINDOW_MS. Nothing is written.
fn listen(c: &HidColl, wait_battery: bool) -> Option<Reading> {
    let d = HidDev::open_read_only(c)?;
    let long = Deadline::in_ms(BATTERY_WINDOW_MS as u64);
    let short = Deadline::in_ms(WINDOW_MS as u64);
    let mut r = Reading::default();
    let mut n = 0;
    while n < MAX_REPORTS {
        let live = wait_battery && (r.state || r.wireless == Some(WIRELESS_CONNECTED));
        let left = if live { &long } else { &short }.left_ms();
        if left == 0 {
            break;
        }
        let Some(frame) = d.read(left.min(300)) else { continue };
        n += 1;
        match frame.first().copied() {
            Some(REPORT_BATTERY) => {
                if let Some(v) = decode_battery(&frame) {
                    r.battery = Some(v);
                    break;
                }
            }
            Some(id) if REPORTS_STATE.contains(&id) => r.state = true,
            Some(id) if REPORTS_WIRELESS.contains(&id) && frame.len() >= 2 => r.wireless = Some(frame[1]),
            _ => {}
        }
        if r.wireless == Some(WIRELESS_DISCONNECTED) {
            break;
        }
    }
    Some(r)
}

/// One device entry for a slot, from this read and the value kept from earlier reads.
/// None means there is nothing to show yet.
fn status(key: &str, pid: u16, r: &Reading, last: Option<(i32, bool, bool)>) -> Option<DeviceStatus> {
    let base = DeviceStatus {
        key: key.into(),
        name: "Steam Controller".into(),
        kind: DeviceKind::Gamepad,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, pid)),
        grace_sec: GRACE_SEC,
        ..Default::default()
    };
    if r.wireless == Some(WIRELESS_DISCONNECTED) {
        return Some(DeviceStatus { connected: false, ..base });
    }
    let active = r.state || r.wireless == Some(WIRELESS_CONNECTED) || r.battery.is_some();
    if !active {
        // The puck is still plugged in: keep the last value dimmed, without a timer.
        let Some((level, _, _)) = last else { return None };
        return Some(DeviceStatus { percent: level, asleep: true, ..base });
    }
    let Some((level, charging, full)) = r.battery.or(last) else {
        return Some(DeviceStatus {
            level_text: crate::i18n::tr("waiting for battery level"),
            ..base
        });
    };
    Some(DeviceStatus { percent: level, charging, full, ..base })
}

fn poll_inner(ctx: &mut PollContext, full: bool) {
    let puck: Vec<HidColl> = ctx.colls.iter().filter(|c| c.vid == VID && PIDS.contains(&c.pid)).cloned().collect();
    if puck.is_empty() {
        // Only a full read sees every receiver: a quick read is filtered to a few hw ids.
        if full {
            *last_map() = None; // the puck was unplugged: forget its old levels
        }
        return;
    }
    let mut slots = Vec::new();
    for &pid in &PIDS {
        for iface in SLOTS {
            let Some(c) = pick(&puck, pid, iface) else { continue };
            let key = format!("steam:{pid:04x}:{iface}");
            slots.push((pid, iface, c.clone(), key));
        }
    }
    if slots.is_empty() {
        return;
    }
    let mut present = HashSet::new();
    for (pid, iface, c, key) in slots {
        present.insert(key.clone());
        debug_log(|| format!("[steam] slot {key} on iface {iface}, {:04X}:{:04X}", c.page(), c.usage()));
        let last = last_map().as_ref().and_then(|m| m.get(&key)).copied();
        let Some(r) = listen(&c, full) else {
            // Another app may hold the collection (Steam itself). Keep what we already know.
            if let Some(s) = status(&key, pid, &Reading::default(), last) {
                ctx.add(s, VID, pid);
            }
            continue;
        };
        if let Some(v) = r.battery {
            last_map().get_or_insert_with(HashMap::new).insert(key.clone(), v);
        }
        if r.wireless == Some(WIRELESS_DISCONNECTED) {
            if let Some(m) = last_map().as_mut() {
                m.remove(&key);
            }
        }
        let from = if r.wireless == Some(WIRELESS_DISCONNECTED) { None } else { last };
        if let Some(s) = status(&key, pid, &r, from) {
            ctx.add(s, VID, pid);
        }
    }
    if full && let Some(m) = last_map().as_mut() {
        m.retain(|k, _| present.contains(k));
    }
}

/// Full read: wait for the slow 0x43 report when a slot has a live controller.
pub fn poll(ctx: &mut PollContext) {
    poll_inner(ctx, true);
}

/// Quick read (every 10 s for a known, asleep controller): only the short window, so it never
/// holds up the tray. A quick read can be filtered to a few hw ids, hence it never clears the
/// cache just because this product is absent from its list.
pub fn poll_quick(ctx: &mut PollContext) {
    poll_inner(ctx, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coll(pid: u16, iface: i32, page: u16, usage: u16) -> HidColl {
        HidColl { vid: VID, pid, iface, usage_page: page, usage_id: usage, input_len: 54, path: format!("p{pid:x}-{iface}-{page:x}-{usage:x}"), ..Default::default() }
    }

    #[test]
    fn battery_report() {
        // The frames confirmed on hardware in HaloBattery #157.
        assert_eq!(decode_battery(&[0x43, 1, 0x5F, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Some((95, false, false)));
        assert_eq!(decode_battery(&[0x43, 4, 0x64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Some((100, true, true)));
        assert_eq!(decode_battery(&[0x43, 2, 40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Some((40, true, false)));
    }

    #[test]
    fn bad_battery_frames_are_refused() {
        let good = [0x43, 1, 50, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        for state in [0, 3, 5, 0xFF] {
            let mut r = good;
            r[1] = state;
            assert_eq!(decode_battery(&r), None, "state {state}");
        }
        let mut r = good;
        r[0] = 0x42; // a state report has no level
        assert_eq!(decode_battery(&r), None);
        r[0] = 0x43;
        r[2] = 101;
        assert_eq!(decode_battery(&r), None);
        assert_eq!(decode_battery(&good[..14]), None);
        assert_eq!(decode_battery(&[]), None);
    }

    #[test]
    fn slot_picks_vendor_usage_and_ignores_other_interfaces() {
        let cols = vec![
            coll(PROTEUS, 6, VENDOR_PAGE, 0x0002), // the puck itself, not a slot
            coll(PROTEUS, 3, GAMEPAD_PAGE, GAMEPAD_USAGE),
            coll(PROTEUS, 3, VENDOR_PAGE, VENDOR_USAGE),
            coll(NEREID, 3, VENDOR_PAGE, VENDOR_USAGE),
        ];
        let c = pick(&cols, PROTEUS, 3).unwrap();
        assert_eq!((c.page(), c.usage()), (VENDOR_PAGE, VENDOR_USAGE));
        assert!(pick(&cols, PROTEUS, 2).is_none());
        assert!(pick(&cols, 0x1142, 3).is_none()); // the 2015 dongle
    }

    #[test]
    fn fallback_to_a_vendor_or_gamepad_collection() {
        let cols = vec![coll(PROTEUS, 2, VENDOR_PAGE, 0x0002), coll(PROTEUS, 2, GAMEPAD_PAGE, GAMEPAD_USAGE)];
        assert_eq!(pick(&cols, PROTEUS, 2).unwrap().usage(), 0x0002);
        let cols = vec![coll(PROTEUS, 2, GAMEPAD_PAGE, GAMEPAD_USAGE)];
        assert_eq!(pick(&cols, PROTEUS, 2).unwrap().usage(), GAMEPAD_USAGE);
    }

    #[test]
    fn status_from_a_live_controller() {
        let r = Reading { battery: Some((95, false, false)), state: true, wireless: None };
        let s = status("steam:1304:2", PROTEUS, &r, None).unwrap();
        assert_eq!((s.key.as_str(), s.percent, s.charging, s.full, s.connected, s.asleep), ("steam:1304:2", 95, false, false, true, false));
        assert_eq!(s.kind, DeviceKind::Gamepad);
        assert_eq!(s.hw, Some((VID, PROTEUS)));
        assert_eq!(s.via, "2.4 GHz receiver");
    }

    #[test]
    fn status_without_a_level_waits_and_quiet_slot_keeps_it_dimmed() {
        let live = Reading { state: true, ..Reading::default() };
        let s = status("steam:1304:2", PROTEUS, &live, None).unwrap();
        assert_eq!(s.percent, -1);
        assert!(!s.level_text.is_empty());
        assert!(s.connected && !s.asleep);

        let quiet = Reading::default();
        let s = status("steam:1304:2", PROTEUS, &quiet, Some((80, true, false))).unwrap();
        assert_eq!((s.percent, s.asleep, s.charging, s.connected), (80, true, false, true));

        assert!(status("steam:1304:2", PROTEUS, &quiet, None).is_none());
    }

    #[test]
    fn disconnect_removes_the_icon() {
        let r = Reading { battery: Some((80, false, false)), wireless: Some(WIRELESS_DISCONNECTED), ..Reading::default() };
        let s = status("steam:1304:2", PROTEUS, &r, Some((80, false, false))).unwrap();
        assert!(!s.connected);
    }

    #[test]
    fn charging_done_is_full() {
        let r = Reading { battery: Some((100, true, true)), wireless: Some(WIRELESS_CONNECTED), ..Reading::default() };
        let s = status("steam:1304:2", PROTEUS, &r, None).unwrap();
        assert!(s.is_full());
    }
}
