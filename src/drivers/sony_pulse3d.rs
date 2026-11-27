//! Sony PULSE 3D wireless headset (CFI-ZWH1) on its 2.4 GHz dongle `054C:0D5E` (issue #437).
//!
//! Protocol from HeadsetControl (MIT, lib/devices/sony_pulse_3d.hpp): interface 3 has a vendor
//! collection `FF01:0020`; the dongle never answers a query, it *pushes* an 8-byte status report
//! `B0` only when something changes (power, a button, the charger, a battery step about hourly):
//!
//!   B0 .. .. LL FF .. ..     LL = 0..100 in steps of 10, or 0x80 while charging
//!                            FF bit 0x08 = headset connected
//!
//! The output report `B1` is rejected by the dongle, so nothing is ever sent. The collection is
//! opened read-only, queued reports are drained, and the newest status is kept between reads; a
//! poll that hears nothing keeps showing the last level. The remaining bytes are not decoded.
use std::sync::Mutex;

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x054C;
const PID: u16 = 0x0D5E;
const STATUS_ID: u8 = 0xB0;
const BYTE_LEVEL: usize = 3;
const BYTE_FLAGS: usize = 4;
const LEVEL_CHARGING: u8 = 0x80;
const FLAG_CONNECTED: u8 = 0x08;
/// A report flood must not keep the poll thread busy.
const MAX_DRAIN_READS: usize = 64;
/// At start-up a level can be absent: wait briefly for the headset's next change report.
const FIRST_LISTEN_MS: u64 = 1500;
const DRAIN_MS: u64 = 50;

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
    *last = Some(Status {
        level: new.level.or(old.level),
        charging: new.charging,
        connected: new.connected,
    });
}

/// Drains what the dongle has already queued. `first` waits a little longer at start-up.
fn listen(c: &HidColl, first: bool) -> Option<Status> {
    let d = HidDev::open_read_only(c)?;
    let end = Deadline::in_ms(if first { FIRST_LISTEN_MS } else { DRAIN_MS });
    let mut last = None;
    for _ in 0..MAX_DRAIN_READS {
        if end.passed() {
            break;
        }
        match d.read(end.left_ms()) {
            Some(r) => {
                let before = last;
                remember(&mut last, &r);
                if first && last.is_some() && last != before {
                    break;
                }
            }
            None => break, // timeout: nothing else queued
        }
    }
    last
}

static LAST: Mutex<Option<Status>> = Mutex::new(None);

fn poll_all(ctx: &mut PollContext, first_listen: bool) {
    if ctx.is_claimed(VID, PID) || crate::device::is_ignored(VID, PID) {
        return;
    }
    let colls = ctx.find(VID, PID, |c| c.page() == 0xFF01 && c.usage() == 0x0020 && c.in_len() >= 8);
    if colls.is_empty() {
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    }
    let known = *LAST.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = colls.iter().find_map(|c| listen(c, first_listen && known.is_none()));
    let status = {
        let mut guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = fresh {
            *guard = Some(s);
        }
        guard.unwrap_or_default()
    };
    let s = DeviceStatus {
        key: "sony:pulse3d".into(),
        name: "Sony PULSE 3D".into(),
        kind: DeviceKind::Headset,
        via: "Wireless adapter".into(),
        hw: Some((VID, PID)),
        // The headset only sends a battery step about once an hour.
        grace_sec: 3600,
        connected: status.connected,
        percent: status.level.unwrap_or(-1),
        charging: status.charging,
        ..Default::default()
    };
    ctx.add(s, VID, PID);
}

pub fn poll(ctx: &mut PollContext) {
    poll_all(ctx, true);
}

/// Quick read: the status is cached, so this only drains reports already in the OS queue.
pub fn poll_quick(ctx: &mut PollContext) {
    poll_all(ctx, false);
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
