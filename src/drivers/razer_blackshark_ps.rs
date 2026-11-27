//! Razer BlackShark V2 Pro for PlayStation on its 2.4 GHz dongle `1532:1014` (issue #414).
//!
//! Protocol from HeadsetControl (MIT, lib/devices/razer_blackshark_v2_pro_ps.hpp). Every frame is
//! 64 bytes, report id `02`, on the vendor collection `FF00:0001` (interface 3):
//!
//!   02 80 LL CC CC <payload>          request, CC CC = CRC-16/XMODEM of the zeroed frame
//!   ack on report 02: byte 1 = 'O' accepted, 'F' rejected (resent after 20 / 60 ms)
//!   02 LL 50 49 .. .. TT RR SS <data> reply, RR = 0x80 | sequence, SS = 0 on success
//!
//! A link query (`E6`, bit 0x08 = headset linked) is cheap. The headset's voltage (`31` on type
//! 06) is only available after switching the dongle to remote mode (`E1 01` on type 0E, plus the
//! firmware updater's read-max-length IOCTL) and the dongle is *always* switched back to local
//! mode afterwards. A remote dongle left by a killed process is reused and put back; a live
//! updater session is left alone. Headset-reported percent (`21`) and charge state (`2A`,
//! 1 = charging, 2 = full) come from customer frames (type 08) and are preferred over the voltage.
//!
//! Like the older G435 dongle (logitech_g435.rs), a session is only started when the headset's own
//! playback *and* microphone are at zero on the Windows mixer: switching mode interrupts audio.
//! Successful reads are cached for 5 minutes, an off headset for 15 minutes, and a blocked or
//! unanswered one for a minute. No command is ever guessed and no other device is touched.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{Deadline, sleep_ms};

const VID: u16 = 0x1532;
const PID: u16 = 0x1014;
const REPORT_ID: u8 = 0x02;
const FRAME_OUT: u8 = 0x80;
const ACK_OK: u8 = 0x4F;

const TYPE_OTA: u8 = 0x06;
const TYPE_LINK: u8 = 0x0E;
const FRAME_CUSTOM: u8 = 0x08;
const CUSTOM_REPLY: u8 = 0x01;
const FILTER_ESCAPE: i32 = 10;
const LINK_STATUS: u8 = 0xE6;
const GET_MODE: u8 = 0xE0;
const SET_MODE: u8 = 0xE1;
const MODE_LOCAL: u8 = 0x00;
const MODE_REMOTE: u8 = 0x01;
const LINK_HEADSET: u8 = 0x08;
const GET_BATTERY: u8 = 0x31;
const IND_BATTERY: u8 = 0x21;
const IND_CHARGING: u8 = 0x2A;
const CHARGE_NONE: u8 = 0x00;
const CHARGE_CHARGING: u8 = 0x01;
const CHARGE_FULL: u8 = 0x02;

/// Sent after every mode switch, as the vendor's firmware updater does.
const READ_MAX_LEN: [u8; 13] = [0x25, 0x34, 0x12, 0x5A, 0x5A, 0x01, 0x00, 0x00, 0x00, 0xF0, 0x00, 0x00, 0x00];

const QUERY_EVERY: Duration = Duration::from_secs(5 * 60);
const OFF_QUERY_EVERY: Duration = Duration::from_secs(15 * 60);
const RETRY: Duration = Duration::from_secs(60);
const MODE_SETTLE_MS: u64 = 300;

/// CRC-16/XMODEM (poly 1021, init 0).
fn crc16(data: &[u8]) -> u16 {
    crate::drivers::logitech_g435::crc16(data)
}

/// Wraps a payload in one 64-byte output frame.
pub fn frame(payload: &[u8]) -> [u8; 64] {
    let mut f = [0u8; 64];
    f[0] = REPORT_ID;
    f[1] = FRAME_OUT;
    f[2] = payload.len().min(59) as u8;
    let n = payload.len().min(59);
    f[5..5 + n].copy_from_slice(&payload[..n]);
    let crc = crc16(&f);
    f[3] = crc as u8;
    f[4] = (crc >> 8) as u8;
    f
}

fn is_custom(r: &[u8]) -> bool {
    r.len() == 16 && r[4] == FRAME_CUSTOM && r[10] == 0x04 && r[11] == 0x00 && matches!(r[13], 1 | 2) && r[14] == 0x01
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Reported {
    percent: Option<i32>,
    charge: Option<u8>,
}

/// Voltage -> % (the headset firmware's calibration record, last point estimated).
pub fn voltage_percent(mv: i32) -> i32 {
    const V: [i32; 11] = [3304, 3683, 3734, 3766, 3782, 3809, 3869, 3929, 3982, 4074, 4150];
    const P: [i32; 11] = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
    if mv <= V[0] {
        return 0;
    }
    if mv >= V[10] {
        return 100;
    }
    for i in 1..11 {
        if mv <= V[i] {
            return P[i - 1] + ((P[i] - P[i - 1]) * (mv - V[i - 1]) + (V[i] - V[i - 1]) / 2) / (V[i] - V[i - 1]);
        }
    }
    100
}

/// One conversation: sequence numbers and whatever the headset reported while we waited.
struct Session {
    seq: u8,
    reported: Reported,
}

impl Session {
    fn new() -> Self {
        // A random start makes a sequence collision with an older build unlikely.
        let seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos() % 127) as u8;
        Self { seq: seed, reported: Reported { percent: None, charge: None } }
    }

    /// Remembers a percent / charge state pushed unasked or returned as a customer reply. A GET
    /// returns the headset's raw figure, so it is filtered like the firmware does: only up while
    /// charging, only down off the charger, unless the jump is too large to be jitter.
    fn note(&mut self, r: &[u8]) {
        if !is_custom(r) {
            return;
        }
        let value = r[15];
        if r[12] == IND_BATTERY && value <= 100 {
            let on_charger = matches!(self.reported.charge, Some(CHARGE_CHARGING | CHARGE_FULL));
            let filtered = r[13] == CUSTOM_REPLY
                && self.reported.percent.is_some()
                && self.reported.charge.is_some()
                && (value as i32 - self.reported.percent.unwrap()).abs() <= FILTER_ESCAPE;
            self.reported.percent = Some(if filtered {
                if on_charger {
                    self.reported.percent.unwrap().max(value as i32)
                } else {
                    self.reported.percent.unwrap().min(value as i32)
                }
            } else {
                value as i32
            });
        } else if r[12] == IND_CHARGING {
            // 3 means "unknown" in the headset firmware; claiming it would steer the filter.
            self.reported.charge = (value <= CHARGE_FULL).then_some(value);
        }
    }

    /// Sends one command and waits for its reply; `custom_id` selects a customer GET reply.
    fn command(&mut self, d: &HidDev, typ: u8, body: &[u8], timeout_ms: u32, custom_id: Option<u8>) -> Option<Vec<u8>> {
        self.seq = self.seq % 127 + 1;
        let seq = self.seq;
        let expect = 0x80 | seq;
        let mut payload = vec![0x50, 0x41, typ, seq, (body.len() & 0xFF) as u8];
        if typ == TYPE_OTA {
            payload.push((body.len() >> 8) as u8);
        }
        payload.extend_from_slice(body);
        let out = frame(&payload);
        d.flush();
        let mut accepted = false;
        for copy in 0..3 {
            if copy > 0 {
                sleep_ms(if copy == 1 { 20 } else { 60 });
            }
            if !d.write(&out) {
                return None;
            }
            let ack = d.get_input_report(REPORT_ID)?;
            if ack.get(1) == Some(&ACK_OK) {
                accepted = true;
                break;
            }
        }
        if !accepted {
            return None;
        }
        let end = Deadline::in_ms(timeout_ms as u64);
        loop {
            if end.passed() {
                return None;
            }
            let r = d.read(end.left_ms())?;
            let n = r.len().min(2 + r[1] as usize);
            let r = &r[..n];
            if r.len() > 14 && r[0] == REPORT_ID && r[2] == 0x50 && r[3] == 0x49 && r[4] == 0x01 && r[12] == typ && r[13] == expect {
                return Some(r[14..].to_vec());
            }
            if r.first() == Some(&REPORT_ID) && r.get(2) == Some(&0x50) && r.get(3) == Some(&0x49) {
                self.note(r);
            }
            if let Some(id) = custom_id
                && is_custom(r)
                && r[12] == id
                && r[13] == CUSTOM_REPLY
            {
                return Some(r[12..].to_vec());
            }
        }
    }

    /// Link-layer GET, whose reply echoes the id at [11] and the value at [12].
    fn link(&mut self, d: &HidDev, id: u8) -> Option<u8> {
        let r = self.command(d, TYPE_LINK, &[id], 300, None)
            .or_else(|| self.command(d, TYPE_LINK, &[id], 1500, None))?;
        (r.len() >= 13 && r[0] == 0 && r[1] == 0x50 && r[2] == 0x49 && r[11] == id).then_some(r[12])
    }

    fn is_mode(&mut self, d: &HidDev, mode: u8) -> bool {
        self.link(d, GET_MODE) == Some(mode)
    }

    fn set_mode(&mut self, d: &HidDev, mode: u8) -> bool {
        let Some(r) = self.command(d, TYPE_LINK, &[SET_MODE, mode], 1500, None) else { return false };
        if r.first() != Some(&0) {
            return false;
        }
        sleep_ms(MODE_SETTLE_MS);
        self.command(d, TYPE_OTA, &READ_MAX_LEN, 2500, None).is_some()
    }

    /// A dongle already in remote mode may belong to another program (a firmware updater). A live
    /// session sends replies that every handle sees; a dongle left by a killed process is silent.
    fn foreign_session(&mut self, d: &HidDev) -> bool {
        let end = Deadline::in_ms(400);
        loop {
            if end.passed() {
                return false;
            }
            let Some(r) = d.read(end.left_ms()) else { return false };
            if r.len() < 2 {
                continue;
            }
            let r = &r[..r.len().min(2 + r[1] as usize)];
            if r.first() != Some(&REPORT_ID) || r.get(2) != Some(&0x50) || r.get(3) != Some(&0x49) {
                continue;
            }
            self.note(r);
            let reply = r.get(4) == Some(&0x01) && matches!(r.get(12), Some(&TYPE_OTA) | Some(&TYPE_LINK));
            if reply || (is_custom(r) && r[13] == CUSTOM_REPLY) {
                return true;
            }
        }
    }

    /// Always called after a remote-mode session, even when the read failed.
    fn restore_local(&mut self, d: &HidDev) -> bool {
        let mut local = self.is_mode(d, MODE_LOCAL);
        for _ in 0..3 {
            if local {
                break;
            }
            let _ = self.set_mode(d, MODE_LOCAL);
            local = self.is_mode(d, MODE_LOCAL);
        }
        local
    }

    fn voltage(&mut self, d: &HidDev) -> Option<i32> {
        for _ in 0..3 {
            let Some(r) = self.command(d, TYPE_OTA, &[GET_BATTERY], 1500, None) else { continue };
            if r.len() < 3 || r[0] != 0 {
                continue;
            }
            let mv = u16::from_le_bytes([r[1], r[2]]) as i32;
            if (2500..=4500).contains(&mv) {
                return Some(mv);
            }
        }
        None
    }

    fn custom_get(&mut self, d: &HidDev, id: u8) {
        // One retry: a lost reply would otherwise forget a charger that is there.
        for _ in 0..2 {
            if self.command(d, FRAME_CUSTOM, &[id, 0, 0], 500, Some(id)).is_some() {
                return;
            }
        }
    }

    fn drain(&mut self, d: &HidDev) {
        for _ in 0..64 {
            let Some(r) = d.read(0) else { return };
            self.note(&r);
        }
    }
}

#[derive(Clone, Copy)]
struct Reading {
    percent: i32,
    charging: bool,
    full: bool,
    mv: i32,
}

#[derive(Clone, Copy)]
enum State {
    Reading(Reading),
    Offline,
    /// Audio was flowing, or the dongle did not answer: keep the last level and retry later.
    Unknown,
}

fn read_one(c: &HidColl) -> Option<State> {
    let d = HidDev::open(c)?;
    let mut s = Session::new();
    s.drain(&d);
    let link = s.link(&d, LINK_STATUS)?;
    if link & LINK_HEADSET == 0 {
        // With the headset off, a dongle left in remote mode by a killed process still ought to be
        // put back; a live updater session is left alone.
        if s.link(&d, GET_MODE)? == MODE_REMOTE && !s.foreign_session(&d) {
            let _ = s.restore_local(&d);
        }
        return Some(State::Offline);
    }
    // Local is the dongle's normal mode. A remote one is either left over from a killed process
    // (safe to reuse, and put back here) or another program's live session (leave it alone).
    let mode = s.link(&d, GET_MODE)?;
    if mode != MODE_LOCAL && mode != MODE_REMOTE {
        return None;
    }
    if mode == MODE_REMOTE && s.foreign_session(&d) {
        return None;
    }
    if mode == MODE_LOCAL && (!s.set_mode(&d, MODE_REMOTE) || !s.is_mode(&d, MODE_REMOTE)) {
        // The switch may have taken effect even when its follow-up IOCTL failed.
        let _ = s.restore_local(&d);
        return None;
    }
    let mv = s.voltage(&d);
    if mv.is_some() {
        s.custom_get(&d, IND_CHARGING);
        s.custom_get(&d, IND_BATTERY);
    }
    // If the switch back fails the dongle is left in remote mode: treat it as a failed read so the
    // next try puts it back, and do not trust a level obtained from a stuck session.
    if !s.restore_local(&d) {
        return None;
    }
    let mv = mv?;
    let charge = s.reported.charge.unwrap_or(CHARGE_NONE);
    Some(State::Reading(Reading {
        percent: s.reported.percent.unwrap_or_else(|| voltage_percent(mv)),
        charging: charge == CHARGE_CHARGING,
        full: charge == CHARGE_FULL,
        mv,
    }))
}

#[derive(Clone, Copy)]
struct Last {
    at: Instant,
    state: State,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

fn interval(state: State) -> Duration {
    match state {
        State::Reading(_) => QUERY_EVERY,
        State::Offline => OFF_QUERY_EVERY,
        State::Unknown => RETRY,
    }
}

fn poll_all(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID) || crate::device::is_ignored(VID, PID) {
        return;
    }
    let colls = ctx.find(VID, PID, |c| c.page() == 0xFF00 && c.usage() == 0x0001 && c.in_len() == 64 && c.out_len() == 64);
    if colls.is_empty() {
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    }
    // The lock spans the gate and the session: a full and a quick read must not talk at once.
    let mut guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let cached = *guard;
    let fresh = cached.filter(|l| l.at.elapsed() < interval(l.state));
    let state = match fresh {
        Some(l) => l.state,
        None => {
            // Switching mode interrupts the headset's audio; only do it at a silent mixer.
            let state = if crate::audio::silent("blackshark") {
                colls.iter().find_map(read_one).unwrap_or(State::Unknown)
            } else {
                State::Unknown
            };
            *guard = Some(Last { at: Instant::now(), state });
            state
        }
    };
    drop(guard);
    let base = DeviceStatus {
        key: "razer:blackshark_v2pro_ps".into(),
        name: "Razer BlackShark V2 Pro (PS)".into(),
        kind: DeviceKind::Headset,
        via: "Razer receiver".into(),
        hw: Some((VID, PID)),
        grace_sec: 600,
        ..Default::default()
    };
    let s = match state {
        State::Reading(r) => DeviceStatus { percent: r.percent, charging: r.charging, full: r.full, millivolts: r.mv, ..base },
        State::Offline => DeviceStatus { connected: false, ..base },
        State::Unknown => base,
    };
    ctx.add(s, VID, PID);
}

pub fn poll(ctx: &mut PollContext) {
    poll_all(ctx);
}

/// The quick read reuses the same cache: it must never start a mode switch every 10 seconds.
pub fn poll_quick(ctx: &mut PollContext) {
    poll_all(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_carry_the_crc_of_the_zeroed_frame() {
        const CRC_CHECK: u16 = 0x31C3;
        assert_eq!(crc16(b"123456789"), CRC_CHECK);
        let f = frame(&[0x50, 0x41, 0x0E, 0x01, 0x01, 0xE6]);
        assert_eq!((f[0], f[1], f[2], f[5]), (REPORT_ID, FRAME_OUT, 6, 0x50));
        let mut z = f;
        z[3] = 0;
        z[4] = 0;
        assert_eq!(crc16(&z), u16::from_le_bytes([f[3], f[4]]));
    }

    #[test]
    fn customer_frames_are_the_only_indications() {
        let mut r = vec![0u8; 16];
        r[0] = REPORT_ID;
        r[4] = FRAME_CUSTOM;
        r[10] = 0x04;
        r[13] = 2; // notification
        r[14] = 0x01;
        r[12] = IND_BATTERY;
        r[15] = 87;
        assert!(is_custom(&r));
        let mut s = Session::new();
        s.note(&r);
        r[12] = IND_CHARGING;
        r[15] = CHARGE_CHARGING;
        s.note(&r);
        assert_eq!(s.reported, Reported { percent: Some(87), charge: Some(CHARGE_CHARGING) });
        r[13] = 0; // not a reply or a notification
        s.note(&r);
        assert_eq!(s.reported.charge, Some(CHARGE_CHARGING));
        r[13] = 2; // a charger fault means "unknown", not a level to trust
        r[15] = 3;
        s.note(&r);
        assert_eq!(s.reported.charge, None);
    }

    #[test]
    fn raw_get_is_filtered_like_the_firmware() {
        let mut r = vec![0u8; 16];
        r[0] = REPORT_ID;
        r[4] = FRAME_CUSTOM;
        r[10] = 0x04;
        r[13] = 2; // notification: the filtered figure
        r[14] = 0x01;
        r[12] = IND_BATTERY;
        r[15] = 80;
        let mut s = Session::new();
        s.note(&r);
        r[12] = IND_CHARGING;
        r[15] = CHARGE_NONE;
        s.note(&r);
        r[12] = IND_BATTERY;
        r[13] = CUSTOM_REPLY; // GET: the raw figure, within jitter
        r[15] = 84;
        s.note(&r);
        assert_eq!(s.reported.percent, Some(80)); // off the charger it only falls
        r[15] = 95; // a real jump is taken as-is
        s.note(&r);
        assert_eq!(s.reported.percent, Some(95));
    }

    #[test]
    fn voltage_curve() {
        assert_eq!(voltage_percent(0), 0);
        assert_eq!(voltage_percent(3304), 0);
        assert_eq!(voltage_percent(4150), 100);
        assert_eq!(voltage_percent(5000), 100);
        assert_eq!(voltage_percent(3982), 80);
        assert!(voltage_percent(3809) >= 49 && voltage_percent(3809) <= 51);
    }
}
