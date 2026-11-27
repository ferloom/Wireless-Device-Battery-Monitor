//! Logitech G435 LIGHTSPEED headset, on both of its 2.4 GHz dongles.
//!
//! `046D:0ADF` (newer dongle): not HID++ - the dongle's vendor collection FF00:0001 (unnumbered
//! 64-byte reports) carries CRC-16 framed messages, and "PA" commands inside them reach the
//! headset's radio chip. Protocol from g435-battery-percentage (MIT,
//! github.com/portyporty/g435-battery-percentage, a tray app made from a capture on this headset),
//! as also used by TaskbarVitals (GPL-3.0, github.com/symNistaken/TaskbarVitals,
//! src/drivers/logitech_g435_0adf.cpp, where it was checked on hardware that a read does not
//! interrupt audio or the microphone); implemented here from those facts. Read every minute while
//! the headset answers, so charging shows up promptly; no reply means the headset is off or out of
//! range, and those reads back off to 15 minutes so they do not keep a sleeping headset awake.
//!
//! `046D:0ACB` (older dongle): no CRC framing and no FF00 collection - the battery is readable
//! only through the dongle's bootloader bridge on the vendor collection FFC0, report 6: enter
//! (06 01 03), "PA" commands wrapped in 06 04 03 <len16>, replies read back as feature report 6,
//! leave (06 08 03). Protocol from logitech-g435-battery (MIT,
//! github.com/deddroid/logitech-g435-battery), verified on this dongle. While the bridge is up
//! the dongle's own audio stops (~0.6 s per session), so a session is attempted only while the
//! dongle's playback *and* microphone are silent (WASAPI peak meters, checked right before it);
//! when they are not, the last known level keeps showing and the next chance is taken a minute
//! later. GET_BATTERY is never a session's first command: dongles still in "remote mode 0"
//! answer it with a junk voltage, and the reference reports the dongle hanging outright unless a
//! wireless command went first - the connect status runs first, and the mode is switched to 1
//! only when the first battery answer is implausible. Read at most every minute while the headset
//! answers and audio is silent; when it is off, back off to 15 minutes. The dongle stays hidden
//! from the rest of the driver chain (NEVER_TOUCH in device.rs); it is reached through
//! `device::colls_of`, which is what keeps other drivers from probing it. Charging is inferred from
//! a rising voltage (there is no charging flag in either protocol).
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x046D;
const PID_ADF: u16 = 0x0ADF;
const PID_ACB: u16 = 0x0ACB;
const CAT_OTA: u8 = 0x06;
const CAT_WIRELESS: u8 = 0x0E;
const GET_MODE: u8 = 0xE0;
const SET_MODE: u8 = 0xE1;
/// An answering (on, possibly charging) headset is read every minute: a charge of 1 % can happen
/// well inside the old 5 / 15 minute gaps, which made the level look stuck.
const QUERY_EVERY: Duration = Duration::from_secs(60);
const ACB_QUERY_EVERY: Duration = QUERY_EVERY;
/// A headset that does not answer is left alone for this long. Polling it every minute kept a
/// sleeping G435 awake (#399), so "off" now means a real backoff.
const OFF_QUERY_EVERY: Duration = Duration::from_secs(15 * 60);
/// After a read blocked by audio (or a failed one) the next try waits at least this long.
const ACB_RETRY: Duration = Duration::from_secs(60);
/// A rising voltage marks charging; an equal voltage keeps it for a while (a full battery still
/// charges), a falling one clears it. Hysteresis ignores small load-related wobble.
const MV_HYSTERESIS: i32 = 3;
const RISE_FLAT: Duration = Duration::from_secs(10 * 60);

/// Session init from the capture (OTA IOCTL_READ_MAX_LEN, sequence 1 inside).
const SESSION_INIT: [u8; 19] = [0x50, 0x41, 0x06, 0x01, 0x0D, 0x00, 0x25, 0x34, 0x12, 0x5A, 0x5A, 0x01, 0x00, 0x00, 0x00, 0xF0, 0x00, 0x00, 0x00];

fn dbg(msg: &str) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("BATTERYHUB_G435_DEBUG").is_some()) {
        eprintln!("[g435] {msg}");
    }
}

/// CRC-16/XMODEM (poly 1021, init 0).
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// One 64-byte frame.
pub fn frame(cmd: u8, body: &[u8]) -> [u8; 64] {
    let mut f = [0u8; 64];
    f[0] = cmd;
    f[1] = body.len() as u8;
    let n = body.len().min(60);
    f[4..4 + n].copy_from_slice(&body[..n]);
    let crc = crc16(&f);
    f[2] = (crc & 0xFF) as u8;
    f[3] = (crc >> 8) as u8;
    f
}

pub fn pa(category: u8, seq: u8, len: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x50, 0x41, category, seq, len];
    v.extend_from_slice(payload);
    v
}

/// Battery voltage from a GET_BATTERY reply; None when absent or not a plausible Li-Po voltage.
pub fn parse_mv(r: &[u8]) -> Option<i32> {
    r.windows(8).find_map(|w| {
        (w[0] == 0x00 && w[1] == 0x07 && w[2] == 0x00 && w[3] == 0x06 && w[5] == 0x00)
            .then(|| w[6] as i32 | (w[7] as i32) << 8)
            .filter(|mv| (2500..=4500).contains(mv))
    })
}

/// Voltage -> % (g435-battery-percentage's curve), piecewise linear.
pub fn percent(mv: i32) -> i32 {
    const V: [i32; 16] = [4200, 4150, 4100, 4050, 4000, 3950, 3900, 3850, 3800, 3750, 3700, 3650, 3600, 3550, 3500, 3400];
    const P: [i32; 16] = [100, 95, 90, 85, 80, 75, 70, 62, 55, 45, 35, 25, 15, 10, 5, 0];
    if mv >= V[0] {
        return 100;
    }
    for i in 1..16 {
        if mv >= V[i] {
            return P[i] + ((P[i - 1] - P[i]) * (mv - V[i]) + (V[i - 1] - V[i]) / 2) / (V[i - 1] - V[i]);
        }
    }
    0
}

pub fn poll(ctx: &mut PollContext) {
    poll_adf(ctx);
    poll_acb(ctx);
}

// =========================== newer dongle: 046D:0ADF ===========================

/// Sends one frame (report id 0 first) and reads for `wait_ms`; with `want_mv`, stops at a battery reply.
fn transact(d: &HidDev, cmd: u8, body: &[u8], wait_ms: u64, want_mv: bool) -> Option<Option<i32>> {
    let f = frame(cmd, body);
    let mut out = Vec::with_capacity(65);
    out.push(0x00);
    out.extend_from_slice(&f);
    d.flush();
    if !d.write(&out) {
        return None;
    }
    let end = Deadline::in_ms(wait_ms);
    while !end.passed() {
        if let Some(r) = d.read(end.left_ms())
            && want_mv
            && let Some(mv) = parse_mv(&r)
        {
            return Some(Some(mv));
        }
    }
    Some(None)
}

fn read_mv(c: &HidColl) -> Option<Option<i32>> {
    let d = HidDev::open(c)?;
    transact(&d, 0x84, &[0x01], 50, false)?;
    let mut seq: u8 = 2;
    let mut next = |cat: u8, len: u8, payload: &[u8]| {
        let cmd = pa(cat, seq, len, payload);
        seq = seq % 127 + 1;
        cmd
    };
    let steps = (|| {
        transact(&d, 0x85, &[0x01], 50, false)?;
        transact(&d, 0x80, &SESSION_INIT, 450, false)?;
        transact(&d, 0x80, &next(CAT_WIRELESS, 1, &[GET_MODE]), 120, false)?;
        transact(&d, 0x80, &next(CAT_WIRELESS, 2, &[SET_MODE, 0x01]), 120, false)?;
        transact(&d, 0x80, &next(CAT_WIRELESS, 1, &[GET_MODE]), 120, false)?;
        transact(&d, 0x80, &next(CAT_OTA, 1, &[0x00, 0x31]), 250, true)
    })();
    // The session is always closed once it was opened.
    let _ = transact(&d, 0x85, &[0x02], 50, false);
    Some(steps.flatten())
}

#[derive(Clone, Copy)]
struct Charge {
    charging: bool,
    rise_at: Instant,
}

impl Default for Charge {
    fn default() -> Self {
        Self { charging: false, rise_at: Instant::now() }
    }
}

/// Charging is not reported by the headset: infer it from the voltage trend. A rise means it is
/// on the charger, a drop means it is not; equal readings keep the state briefly (a full battery
/// stops rising while still charging).
fn charge_from_mv(prev: Option<i32>, mv: Option<i32>, old: Charge) -> Charge {
    let Some(mv) = mv else {
        return Charge::default();
    };
    match prev {
        Some(p) if mv > p + MV_HYSTERESIS => Charge { charging: true, rise_at: Instant::now() },
        Some(p) if mv + MV_HYSTERESIS < p => Charge { charging: false, rise_at: Instant::now() },
        _ => Charge { charging: old.charging && old.rise_at.elapsed() < RISE_FLAT, rise_at: old.rise_at },
    }
}

#[derive(Clone, Copy)]
struct Last {
    at: Instant,
    mv: Option<i32>,
    charge: Charge,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

fn poll_adf(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID_ADF) {
        return;
    }
    let colls = ctx.find(VID, PID_ADF, |c| c.page() == 0xFF00 && c.usage() == 0x0001 && c.in_len() == 65 && c.out_len() == 65);
    if colls.is_empty() {
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    }
    let cached = *LAST.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = cached.filter(|l| l.at.elapsed() < if l.mv.is_some() { QUERY_EVERY } else { OFF_QUERY_EVERY });
    let (mv, charge) = match fresh {
        Some(l) => (l.mv, l.charge),
        None => {
            let Some(mv) = colls.iter().find_map(read_mv) else { return };
            let charge = charge_from_mv(cached.and_then(|l| l.mv), mv, cached.map(|l| l.charge).unwrap_or_default());
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Last { at: Instant::now(), mv, charge });
            (mv, charge)
        }
    };
    let base = DeviceStatus {
        key: "g435:0adf".into(),
        name: "Logitech G435".into(),
        kind: DeviceKind::Headset,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, PID_ADF)),
        grace_sec: 60,
        ..Default::default()
    };
    let s = match mv {
        Some(mv) => DeviceStatus {
            percent: percent(mv),
            millivolts: mv,
            charging: charge.charging,
            full: charge.charging && percent(mv) >= 100,
            ..base
        },
        None => DeviceStatus { connected: false, ..base },
    };
    ctx.add(s, VID, PID_ADF);
}

// =========================== older dongle: 046D:0ACB ===========================

const ENTER: [u8; 2] = [0x01, 0x03];
const LEAVE: [u8; 2] = [0x08, 0x03];
const PA_CONNECT: [u8; 6] = [0x50, 0x41, 0x0E, 0x01, 0x01, 0xE3];
const PA_BATTERY: [u8; 7] = [0x50, 0x41, 0x06, 0x02, 0x01, 0x00, 0x31];
const PA_SET_MODE: [u8; 7] = [0x50, 0x41, 0x0E, 0x03, 0x02, 0xE1, 0x01];
const PA_BATTERY_2: [u8; 7] = [0x50, 0x41, 0x06, 0x04, 0x01, 0x00, 0x31];

fn bridge_report(len: usize, body: &[u8]) -> Option<Vec<u8>> {
    if len <= body.len() {
        return None;
    }
    let mut report = vec![0; len];
    report[0] = 0x06;
    report[1..1 + body.len()].copy_from_slice(body);
    Some(report)
}

/// One bridge message on report 6, then the 06 5A / 06 FE acknowledgement from the input pipe.
fn bridge_msg(d: &HidDev, body: &[u8]) -> bool {
    let Some(out) = bridge_report(d.coll().out_len() as usize, body) else { return false };
    d.flush();
    if !d.write(&out) {
        return false;
    }
    let end = Deadline::in_ms(900);
    while !end.passed() {
        if let Some(r) = d.read(end.left_ms())
            && r.len() >= 2
            && r[0] == 0x06
            && (r[1] == 0x5A || r[1] == 0xFE)
        {
            return true;
        }
    }
    false
}

/// A "PA" command to the headset's radio chip, wrapped for the bridge.
fn bridge_pa(d: &HidDev, cmd: &[u8]) -> bool {
    let len = cmd.len() as u16;
    let mut body = vec![0x04, 0x03, len as u8, (len >> 8) as u8];
    body.extend_from_slice(cmd);
    bridge_msg(d, &body)
}

/// The bridge parks replies in feature report 6; FF in byte 1 means "nothing yet".
fn bridge_frame(d: &HidDev, wait_ms: u64) -> Option<Vec<u8>> {
    let end = Deadline::in_ms(wait_ms);
    loop {
        if let Some(f) = d.get_feature(0x06)
            && f.len() > 1
            && f[1] != 0xFF
        {
            return Some(f);
        }
        if end.passed() {
            return None;
        }
        crate::util::sleep_ms(25);
    }
}

/// Drops pending replies (some commands also produce status frames).
fn bridge_drain(d: &HidDev, max: usize) {
    for _ in 0..max {
        match d.get_feature(0x06) {
            Some(f) if f.len() > 1 && f[1] != 0xFF => {}
            _ => return,
        }
    }
}

fn find(f: &[u8], pat: &[u8]) -> Option<usize> {
    f.windows(pat.len()).position(|w| w == pat)
}

/// Millivolts from a GET_BATTERY reply, if it is a plausible Li-Po voltage.
fn acb_parse_mv(f: &[u8]) -> Option<i32> {
    find(f, &[0x07, 0x00, 0x06]).and_then(|i| {
        let w = f.get(i..i + 8)?;
        (w[3] & 0x80 != 0 && w[4] == 0x00).then(|| w[5] as i32 | (w[6] as i32) << 8).filter(|mv| (2500..=4600).contains(mv))
    })
}

/// Waits for a battery reply. Dongles in remote mode 0 answer with a junk voltage: implausible
/// values pass through as None so the caller can switch the mode and ask again.
fn acb_battery(d: &HidDev) -> Option<i32> {
    let end = Deadline::in_ms(1500);
    loop {
        let f = bridge_frame(d, end.left_ms() as u64)?;
        if let Some(mv) = acb_parse_mv(&f) {
            return Some(mv);
        }
        if end.passed() {
            return None;
        }
    }
}

/// One bridge conversation, without enter/leave. Some(Some(mv)) = level read, Some(None) = the
/// headset is off (its connect status answers 0), None = no usable answer.
fn acb_session(d: &HidDev) -> Option<Option<i32>> {
    // Connect status first: GET_BATTERY must not be the first command (see module doc).
    if !bridge_pa(d, &PA_CONNECT) {
        return None;
    }
    let end = Deadline::in_ms(1500);
    let connected = loop {
        let f = bridge_frame(d, end.left_ms() as u64)?;
        if let Some(i) = find(&f, &[0x02, 0x00, 0xE3]) {
            break f.get(i + 3).copied().unwrap_or(0) != 0;
        }
        if end.passed() {
            return None;
        }
    };
    bridge_drain(d, 4);
    if !connected {
        return Some(None);
    }
    if !bridge_pa(d, &PA_BATTERY) {
        return None;
    }
    let mv = acb_battery(d);
    let mv = match mv {
        Some(mv) => Some(mv),
        None => {
            if !bridge_pa(d, &PA_SET_MODE) {
                return None;
            }
            bridge_drain(d, 2);
            if !bridge_pa(d, &PA_BATTERY_2) {
                return None;
            }
            acb_battery(d)
        }
    };
    mv.map(Some)
}

/// Runs one bridge session and always leaves the bridge afterwards: while it is up, the dongle's
/// audio is dead. If leaving fails, it is retried on a fresh handle - a stuck bridge would cost
/// the user their audio until replug.
fn acb_read(c: &HidColl) -> Option<Option<i32>> {
    let d = HidDev::open(c)?;
    if !bridge_msg(&d, &ENTER) {
        dbg("enter: no ack");
        return None;
    }
    let res = acb_session(&d);
    if !bridge_msg(&d, &LEAVE) {
        dbg("leave: no ack, retrying");
        for _ in 0..2 {
            crate::util::sleep_ms(150);
            if let Some(d2) = HidDev::open(c)
                && bridge_msg(&d2, &LEAVE)
            {
                break;
            }
        }
    }
    res
}

// ---- the audio gate: never cut a stream that is actually flowing ---------------------------
//
// While the 0ACB dongle is in the bridge its audio is stopped, so a session is only safe when
// its own endpoints are silent. Shared with the Razer PS dongle (see src/audio.rs).
fn audio_busy() -> bool {
    !crate::audio::silent("g435")
}

// ---- poll for the older dongle -------------------------------------------------------------

#[derive(Clone, Copy)]
struct LastAcb {
    at: Instant,
    /// Some(Some(mv)) = level, Some(None) = headset off, None = unknown (audio was flowing, or
    /// no answer) - the last known level keeps showing while audio flows.
    mv: Option<Option<i32>>,
    charge: Charge,
}

static LAST_ACB: Mutex<Option<LastAcb>> = Mutex::new(None);

fn poll_acb(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID_ACB) || crate::device::is_ignored(VID, PID_ACB) {
        return;
    }
    let colls: Vec<HidColl> = crate::device::colls_of(VID, PID_ACB).into_iter().filter(|c| c.page() == 0xFFC0 && c.feat_len() >= 64).collect();
    if colls.is_empty() {
        *LAST_ACB.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    }
    // The lock spans the gate and the session: the dongle must never serve two sessions at once
    // (full and quick poll can run on two threads).
    let mut guard = LAST_ACB.lock().unwrap_or_else(|e| e.into_inner());
    let cached = *guard;
    let fresh = cached.filter(|l| {
        l.at.elapsed()
            < match l.mv {
                Some(Some(_)) => ACB_QUERY_EVERY,
                Some(None) => OFF_QUERY_EVERY,
                None => ACB_RETRY,
            }
    });
    let (mv, charge) = match fresh {
        Some(l) => (l.mv, l.charge),
        None => {
            let (mv, charge) = if audio_busy() {
                // No session while the dongle's audio is in use: keep the last level and state.
                (cached.map(|l| l.mv).unwrap_or(None), cached.map(|l| l.charge).unwrap_or_default())
            } else {
                let mv = colls.iter().find_map(acb_read);
                match mv {
                    Some(Some(mv)) => dbg(&format!("read: {mv} mV, {}%", percent(mv))),
                    Some(None) => dbg("read: headset off"),
                    None => dbg("read: no answer"),
                }
                let charge = charge_from_mv(cached.and_then(|l| l.mv).flatten(), mv.flatten(), cached.map(|l| l.charge).unwrap_or_default());
                (mv, charge)
            };
            *guard = Some(LastAcb { at: Instant::now(), mv, charge });
            (mv, charge)
        }
    };
    drop(guard);
    let base = DeviceStatus {
        key: "g435:0acb".into(),
        name: "Logitech G435".into(),
        kind: DeviceKind::Headset,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, PID_ACB)),
        grace_sec: 60,
        ..Default::default()
    };
    let s = match mv {
        Some(Some(mv)) => DeviceStatus {
            percent: percent(mv),
            millivolts: mv,
            charging: charge.charging,
            full: charge.charging && percent(mv) >= 100,
            ..base
        },
        Some(None) => DeviceStatus { connected: false, ..base },
        None => base,
    };
    ctx.add(s, VID, PID_ACB);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_report_checks_output_capacity() {
        assert_eq!(bridge_report(0, &[]), None);
        assert_eq!(bridge_report(2, &[1, 2]), None);
        assert_eq!(bridge_report(3, &[1, 2]), Some(vec![6, 1, 2]));
        assert_eq!(bridge_report(5, &[1, 2]), Some(vec![6, 1, 2, 0, 0]));
    }

    #[test]
    fn charging_is_inferred_from_the_voltage_trend() {
        let now = Instant::now();
        let flat = Charge { charging: false, rise_at: now };
        assert!(!charge_from_mv(None, Some(3900), flat).charging);
        let rising = charge_from_mv(Some(3900), Some(3920), flat);
        assert!(rising.charging);
        // Small wobble while on the battery does not look like charging.
        assert!(!charge_from_mv(Some(3900), Some(3902), flat).charging);
        // A flat voltage keeps the state briefly (full battery), then it expires.
        assert!(charge_from_mv(Some(3920), Some(3920), rising).charging);
        let old = Charge { charging: true, rise_at: now - RISE_FLAT - Duration::from_secs(1) };
        assert!(!charge_from_mv(Some(4200), Some(4200), old).charging);
        assert!(!charge_from_mv(Some(3920), Some(3880), rising).charging);
        assert!(!charge_from_mv(Some(3920), None, rising).charging);
    }

    #[test]
    fn frames() {
        assert_eq!(crc16(b"123456789"), 0x31C3); // CRC-16/XMODEM check value
        let f = frame(0x84, &[0x01]);
        assert_eq!(&f[..2], &[0x84, 0x01]);
        assert_eq!(f[4], 0x01);
        let mut z = f;
        z[2] = 0;
        z[3] = 0;
        assert_eq!(crc16(&z), u16::from_le_bytes([f[2], f[3]]));
        assert_eq!(pa(CAT_OTA, 7, 1, &[0x00, 0x31]), vec![0x50, 0x41, 0x06, 7, 1, 0x00, 0x31]);
    }

    #[test]
    fn replies_and_curve() {
        let mut r = vec![0u8; 64];
        r[10..18].copy_from_slice(&[0x00, 0x07, 0x00, 0x06, 0x09, 0x00, 0x68, 0x0F]); // 3944 mV
        assert_eq!(parse_mv(&r), Some(3944));
        r[16] = 0x10;
        r[17] = 0x00; // 16 mV: not a voltage
        assert_eq!(parse_mv(&r), None);
        assert_eq!(percent(4250), 100);
        assert_eq!(percent(3300), 0);
        assert_eq!(percent(3800), 55);
        assert!(percent(3944) > 70 && percent(3944) < 75);
    }

    /// Frames captured from a real 0ACB dongle: a mode-0 battery answer is junk (234 mV), the
    /// answer after switching to remote mode 1 holds the level (4050 mV).
    #[test]
    fn acb_replies() {
        let junk = [0x06u8, 0x50, 0x49, 0x01, 0xC0, 0x39, 0x84, 0x2D, 0x02, 0x07, 0x00, 0x06, 0x82, 0x00, 0xEA, 0x00, 0x00, 0x00];
        assert_eq!(acb_parse_mv(&junk), None);
        let real = [0x06u8, 0x50, 0x49, 0x01, 0xC3, 0xBC, 0xFB, 0x17, 0x02, 0x07, 0x00, 0x06, 0x84, 0x00, 0xD2, 0x0F, 0x00, 0x00];
        assert_eq!(acb_parse_mv(&real), Some(4050));
        assert_eq!(percent(4050), 85);
        let status = [0x06u8, 0x50, 0x49, 0x0E, 0xC2, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0xE3, 0x01];
        let i = find(&status, &[0x02, 0x00, 0xE3]).unwrap();
        assert_eq!(status[i + 3], 1);
    }
}
