//! Sony PlayStation wireless headsets whose 2.4 GHz dongle pushes the same 8-byte `B0` status
//! report on the vendor collection `FF01:0020`: PULSE 3D (CFI-ZWH1) on `054C:0D5E` (issue #437),
//! Gold wireless headset (CUHYA-0080, "Gold V2") on `12BA:0035` and Platinum wireless headset on
//! `12BA:0050`.
//!
//! Protocol from HeadsetControl (MIT, lib/devices/sony_pulse_3d.hpp) for the PULSE 3D and from
//! hid-playstation-headset (counter185, GPL-2.0, README) for the Gold / Platinum receivers; the
//! Gold's collection and reports checked with `--listen` on a CUHYA-0080. The dongle never answers
//! a query, it *pushes* a status report `B0` only when something changes (power, a button, volume,
//! the charger, a battery step about hourly):
//!
//!   B0 VV CC LL FF .. 11 00   VV = volume, CC = sound / chat balance
//!                             LL = 0..100, or 0x80 while charging
//!                             FF bit 0x08 = headset connected
//!
//! Windows drops input reports that arrive while no handle is open, and these arrive seconds or an
//! hour apart, so one small thread per dongle keeps the collection open read-only and records each
//! status; the polls only read what it recorded. The thread ends when the dongle is gone or no poll
//! has seen it for a while. Nothing is ever sent; at start the current input report `B0` is asked
//! for once over the control pipe (a read), which some dongles answer and the others ignore.
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};

const PAGE: u16 = 0xFF01;
const USAGE: u16 = 0x0020;
const STATUS_ID: u8 = 0xB0;
const BYTE_LEVEL: usize = 3;
const BYTE_FLAGS: usize = 4;
const LEVEL_CHARGING: u8 = 0x80;
const FLAG_CONNECTED: u8 = 0x08;
/// One blocking read; between reads the thread checks that it is still wanted.
const READ_MS: u32 = 2000;
/// Reads failing at once, in a row (the dongle was unplugged), before the thread gives up.
const MAX_FAILS: u32 = 10;
/// The thread ends when no poll has seen its dongle for this long.
const STALE: Duration = Duration::from_secs(120);
/// The PlayStation brand colour is keyed on Sony's vendor id, also for the licensed `12BA` dongles.
const SONY_VID: u16 = 0x054C;

struct Model {
    vid: u16,
    pid: u16,
    key: &'static str,
    name: &'static str,
}

const COUNT: usize = 3;
static MODELS: [Model; COUNT] = [
    Model { vid: 0x054C, pid: 0x0D5E, key: "sony:pulse3d", name: "Sony PULSE 3D" },
    Model { vid: 0x12BA, pid: 0x0035, key: "sony:gold-wireless", name: "Sony Gold Wireless" },
    Model { vid: 0x12BA, pid: 0x0050, key: "sony:platinum-wireless", name: "Sony Platinum Wireless" },
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct Status {
    level: Option<i32>,
    charging: bool,
    connected: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self { level: None, charging: false, connected: true }
    }
}

/// One model: its last status, the collection its listener holds and that listener's generation.
struct Slot {
    status: Option<Status>,
    path: Option<String>,
    generation: u32,
    seen: Option<Instant>,
}

impl Slot {
    const fn new() -> Self {
        Self { status: None, path: None, generation: 0, seen: None }
    }
}

static SLOTS: Mutex<[Slot; COUNT]> = Mutex::new([const { Slot::new() }; COUNT]);

fn slots() -> MutexGuard<'static, [Slot; COUNT]> {
    SLOTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// One status report. A charging report carries no level, so the caller merges it with the last one.
fn decode(r: &[u8]) -> Option<Status> {
    if r.len() < 8 || r[0] != STATUS_ID {
        return None;
    }
    let level = r[BYTE_LEVEL];
    let mut s = Status { connected: r[BYTE_FLAGS] & FLAG_CONNECTED != 0, ..Default::default() };
    if level == LEVEL_CHARGING {
        s.charging = true;
    } else if level <= 100 {
        s.level = Some(level as i32);
    }
    Some(s)
}

fn remember(last: &mut Option<Status>, r: &[u8]) {
    let Some(new) = decode(r) else { return };
    let old = last.unwrap_or_default();
    *last = Some(Status { level: new.level.or(old.level), charging: new.charging, connected: new.connected });
}

fn wanted(i: usize, generation: u32) -> bool {
    let g = slots();
    g[i].generation == generation && g[i].path.is_some() && g[i].seen.is_some_and(|t| t.elapsed() < STALE)
}

fn listen(i: usize, generation: u32, c: HidColl) {
    if let Some(d) = HidDev::open_read_only(&c) {
        if let Some(r) = d.get_input_report(STATUS_ID) {
            remember(&mut slots()[i].status, &r);
        }
        let mut fails = 0;
        while fails < MAX_FAILS && wanted(i, generation) {
            let started = Instant::now();
            match d.read(READ_MS) {
                Some(r) => {
                    fails = 0;
                    remember(&mut slots()[i].status, &r);
                }
                // A timeout takes the whole READ_MS; a read that fails at once means the handle is dead.
                None if started.elapsed() < Duration::from_millis(u64::from(READ_MS) / 2) => {
                    fails += 1;
                    std::thread::sleep(Duration::from_millis(500));
                }
                None => fails = 0,
            }
        }
    }
    let mut g = slots();
    if g[i].generation == generation {
        g[i].path = None;
    }
}

/// Starts a listener on `c` unless one already holds it.
fn ensure_listener(i: usize, c: &HidColl) {
    let generation = {
        let mut g = slots();
        g[i].seen = Some(Instant::now());
        if g[i].path.as_deref() == Some(c.path.as_str()) {
            return;
        }
        g[i].generation = g[i].generation.wrapping_add(1);
        g[i].path = Some(c.path.clone());
        g[i].generation
    };
    let c = c.clone();
    let started = std::thread::Builder::new().name(format!("{} listener", MODELS[i].key)).stack_size(64 * 1024).spawn(move || listen(i, generation, c)).is_ok();
    if !started {
        let mut g = slots();
        if g[i].generation == generation {
            g[i].path = None;
        }
    }
}

fn stop_listener(i: usize, forget: bool) {
    let mut g = slots();
    g[i].path = None;
    if forget {
        g[i].status = None;
    }
}

fn poll_model(ctx: &mut PollContext, i: usize) {
    let m = &MODELS[i];
    if ctx.is_claimed(m.vid, m.pid) || crate::device::is_ignored(m.vid, m.pid) {
        stop_listener(i, false);
        return;
    }
    let Some(c) = ctx.find(m.vid, m.pid, |c| c.page() == PAGE && c.usage() == USAGE && c.in_len() >= 8).into_iter().next() else {
        stop_listener(i, true);
        return;
    };
    ensure_listener(i, &c);
    let status = slots()[i].status.unwrap_or_default();
    let s = DeviceStatus {
        key: m.key.into(),
        name: m.name.into(),
        kind: DeviceKind::Headset,
        via: "Wireless adapter".into(),
        vid: SONY_VID,
        hw: Some((m.vid, m.pid)),
        // The headset only sends a battery step about once an hour.
        grace_sec: 3600,
        connected: status.connected,
        percent: status.level.unwrap_or(-1),
        charging: status.charging,
        ..Default::default()
    };
    ctx.add(s, m.vid, m.pid);
}

pub fn poll(ctx: &mut PollContext) {
    for i in 0..COUNT {
        poll_model(ctx, i);
    }
}

/// Quick read: the listener threads keep the status current, so this is the same as a full poll.
pub fn poll_quick(ctx: &mut PollContext) {
    poll(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(level: u8, flags: u8) -> Vec<u8> {
        vec![STATUS_ID, 0, 0, level, flags, 0, 0, 0]
    }

    #[test]
    fn decodes_only_the_status_report() {
        assert_eq!(decode(&report(70, FLAG_CONNECTED)), Some(Status { level: Some(70), charging: false, connected: true }));
        assert_eq!(decode(&report(80, 0)), Some(Status { level: Some(80), charging: false, connected: false }));
        assert_eq!(decode(&report(LEVEL_CHARGING, FLAG_CONNECTED)), Some(Status { level: None, charging: true, connected: true }));
        assert_eq!(decode(&[0xB1, 0, 0, 70, FLAG_CONNECTED, 0, 0, 0]), None); // the rejected output id
        assert_eq!(decode(&report(70, FLAG_CONNECTED)[..7]), None);
    }

    #[test]
    fn gold_receiver_reports() {
        // Recorded with --listen on a CUHYA-0080: volume 5, balance 100 / 90, level 1, flags 6C (connected).
        assert_eq!(decode(&[0xB0, 0x05, 0x64, 0x01, 0x6C, 0x13, 0x11, 0x00]), Some(Status { level: Some(1), charging: false, connected: true }));
        assert_eq!(decode(&[0xB0, 0x05, 0x5A, 0x01, 0x6C, 0x14, 0x11, 0x00]), Some(Status { level: Some(1), charging: false, connected: true }));
        // Sample from hid-playstation-headset: charging, flags 2D (VSS, connected, mode 2).
        assert_eq!(decode(&[0xB0, 0x01, 0x08, 0x80, 0x2D, 0x14, 0x11, 0x00]), Some(Status { level: None, charging: true, connected: true }));
    }

    #[test]
    fn charging_keeps_the_last_level() {
        let mut last = Some(Status { level: Some(40), charging: false, connected: true });
        remember(&mut last, &report(LEVEL_CHARGING, FLAG_CONNECTED));
        assert_eq!(last, Some(Status { level: Some(40), charging: true, connected: true }));
        remember(&mut last, &report(50, FLAG_CONNECTED));
        assert_eq!(last, Some(Status { level: Some(50), charging: false, connected: true }));
        remember(&mut last, &report(70, 0));
        assert_eq!(last, Some(Status { level: Some(70), charging: false, connected: false }));
    }
}
