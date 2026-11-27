//! AJAZZ AJ139 V2 MC mouse on its 2.4 GHz receiver (VID A8A5, PID 2255) or USB cable (A8A4:2255). The
//! product id is shared with other AJAZZ models on the same chip, so the mouse gets a generic name.
//! The VGN Dragonfly 3 Pro's receiver (391D:1A03) answers the same battery read on the same
//! collection, as tested on the mouse by PromiseTzZ (issue #272); it gets only that read.
//!
//! Protocol as documented by ajazz-aj139-macos (MIT, github.com/werquess/ajazz-aj139-macos,
//! PROTOCOL.md, "checked against a real device") and as used by the original C++ BatteryHub (GPL-3.0,
//! src/drivers/misc_mice.cpp); implemented here from those facts. Vendor collection FF01:0010,
//! unnumbered 64-byte output / input reports:
//!
//!   request  55 30 A5 2E 2E 00 ...    command 30 (battery), read (A5), checksum = low byte of the
//!                                     sum of bytes 4..63 (2E), length 2E
//!   reply    AA 30 .. .. .. .. .. .. LL PP    LL = level %, PP non-zero on USB power
//!
//! On receiver collections with 64-byte input/output reports, the F0 30 battery query is tried when
//! the existing query fails or gets no valid reply.
//! Only this read is sent; the configuration-write command and the Bluetooth OTA service are never
//! touched. A level of 0 from the receiver means the mouse is off or out of range.
//!
//! Note: The Smailwolf RS6 (FYA6-3311 firmware V4.0.3) reports a hardcoded 100% battery with
//! charge_flag=0 regardless of actual charge state, both online and offline. The official app
//! hides the battery UI for this reason. We check the online status (cmd 0xED) before reading
//! battery to avoid reporting stale data when the mouse is sleeping (online=1).
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid::{HidColl, HidDev};
use crate::util::{Deadline, hex4};

const PID: u16 = 0x2255;
const RECEIVER_VID: u16 = 0xA8A5;
const CABLE_VID: u16 = 0xA8A4;
/// (VID, PID, cable, name)
const DEVICES: [(u16, u16, bool, &str); 3] = [
    (RECEIVER_VID, PID, false, "AJAZZ wireless mouse"),
    (CABLE_VID, PID, true, "AJAZZ wireless mouse"),
    (0x391D, 0x1A03, false, "VGN Dragonfly 3 Pro"),
];
const CMD_BATTERY: u8 = 0x30;
const CMD_ONLINE: u8 = 0xED;
const WAIT_MS: u64 = 700;

/// The output report, report ID 0 first.
pub fn request() -> [u8; 65] {
    let mut r = [0u8; 65];
    let p = &mut r[1..];
    p[..5].copy_from_slice(&[0x55, CMD_BATTERY, 0xA5, 0x00, 0x2E]);
    p[3] = p[4..].iter().fold(0u8, |a, &x| a.wrapping_add(x));
    r
}

fn freq_request() -> [u8; 64] {
    let mut r = [0u8; 64];
    r[..8].copy_from_slice(&[0xF0, CMD_BATTERY, 0xA5, 0x0B, 0x2E, 0x01, 0x01, 0x01]);
    r
}

fn online_request() -> [u8; 64] {
    let mut r = [0u8; 64];
    r[..3].copy_from_slice(&[0xF0, CMD_ONLINE, 0x01]);
    r
}

fn decode_online(r: &[u8]) -> Option<u8> {
    // Response: F0 ED 01 XX ... where XX = 2 (online/awake), 1 (offline/sleeping), 0 (wired)
    if r.len() >= 4 && r[0] == 0xF0 && r[1] == CMD_ONLINE && r[2] == 0x01 {
        Some(r[3])
    } else {
        None
    }
}

/// Battery reply (64 bytes, no report ID): Some((level, on USB power)).
pub fn decode(r: &[u8]) -> Option<(i32, bool)> {
    (r.len() >= 10 && r[0] == 0xAA && r[1] == CMD_BATTERY && r[8] <= 100).then(|| (r[8] as i32, r[9] != 0))
}

fn freq_checksum(r: &[u8]) -> u8 {
    r[4..].iter().fold(0u8, |a, &x| a.wrapping_add(x))
}

fn decode_freq_reply(r: &[u8]) -> Option<(i32, bool)> {
    if r.len() != 64
        || r[0] != 0xF0
        || r[1] != CMD_BATTERY
        || r[2] != 0xA5
        || r[3] != freq_checksum(r)
        || r[4] != 0x2E
        || r[5..8] != [0x01, 0x01, 0x01]
        || r[8] > 100
        || r[9] > 1
    {
        return None;
    }
    Some((r[8] as i32, r[9] == 1))
}

fn supported_collection(c: &HidColl) -> bool {
    let min_len = if c.vid == CABLE_VID { 65 } else { 64 };
    DEVICES.iter().any(|&(v, p, _, _)| c.vid == v && c.pid == p)
        && c.page() == 0xFF01
        && c.usage() == 0x0010
        && c.out_len() >= min_len
        && c.in_len() >= min_len
}

fn uses_freq_query(c: &HidColl, receiver: bool) -> bool {
    receiver && c.vid == RECEIVER_VID && c.pid == PID && c.page() == 0xFF01 && c.usage() == 0x0010 && c.out_len() == 64 && c.in_len() == 64
}

fn should_try_freq_query(eligible: bool, legacy_request_sent: bool, legacy_wait_expired: bool) -> bool {
    eligible && (!legacy_request_sent || legacy_wait_expired)
}

fn read(c: &HidColl, receiver: bool) -> Option<(i32, bool)> {
    let d = HidDev::open(c)?;
    d.flush();
    let f0_eligible = uses_freq_query(c, receiver);
    
    // For receiver, check online status first (cmd 0xED) to avoid stale battery when mouse is sleeping
    if receiver && f0_eligible {
        if d.write(&online_request()) {
                let end = Deadline::in_ms(WAIT_MS);
            while !end.passed() {
                if let Some(r) = d.read(end.left_ms()) {
                    if let Some(online) = decode_online(&r) {
                        // online=2 means mouse is awake/online; 1 = sleeping/offline; 0 = wired
                        if online != 2 {
                            return None; // mouse is sleeping, don't report stale battery
                        }
                        break;
                    }
                }
            }
        }
    }
    
    let legacy_request_sent = d.write(&request());
    let mut request_sent = legacy_request_sent;
    let mut freq_attempted = false;
    if should_try_freq_query(f0_eligible, legacy_request_sent, false) {
        freq_attempted = true;
        request_sent |= d.write(&freq_request());
    }
    if !request_sent {
        return None;
    }
    let mut end = Deadline::in_ms(WAIT_MS);
    loop {
        while !end.passed() {
            if let Some(r) = d.read(end.left_ms()) {
                if let Some(v) = decode(&r) {
                    return Some(v);
                }
                if let Some(v) = decode_freq_reply(&r) {
                    return Some(v);
                }
            }
        }
        if !freq_attempted && should_try_freq_query(f0_eligible, legacy_request_sent, true) {
            freq_attempted = true;
            if d.write(&freq_request()) {
                end = Deadline::in_ms(WAIT_MS);
                continue;
            }
        }
        break;
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    for (vid, pid, cable, name) in DEVICES {
        if ctx.is_claimed(vid, pid) {
            continue;
        }
        let colls = ctx.find(vid, pid, supported_collection);
        if colls.is_empty() {
            continue;
        }
        let base = DeviceStatus {
            key: format!("ajazz139:{}:{}", hex4(vid), hex4(pid)),
            name: name.into(),
            kind: DeviceKind::Mouse,
            via: if cable { "USB cable" } else { "2.4 GHz receiver" }.into(),
            hw: Some((vid, pid)),
            grace_sec: 1800,
            ..Default::default()
        };
        let s = match colls.iter().find_map(|c| read(c, !cable)) {
            Some((level, power)) if cable || level > 0 => DeviceStatus { percent: level, charging: power || cable, ..base },
            _ if cable => continue, // the cable alone gives nothing to show
            _ => DeviceStatus { connected: false, ..base },
        };
        ctx.add(s, vid, pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_frame() {
        assert_eq!(request()[..7], [0x00, 0x55, 0x30, 0xA5, 0x2E, 0x2E, 0x00]);
        assert!(request()[7..].iter().all(|&b| b == 0));
    }

    #[test]
    fn battery_reply() {
        let mut r = vec![0u8; 64];
        r[..3].copy_from_slice(&[0xAA, 0x30, 0xA5]);
        r[8] = 76;
        assert_eq!(decode(&r), Some((76, false)));
        r[9] = 1;
        assert_eq!(decode(&r), Some((76, true)));
    }

    #[test]
    fn other_reports() {
        let mut r = vec![0u8; 64];
        r[..2].copy_from_slice(&[0xAA, 0xFA]);
        r[8] = 50;
        assert_eq!(decode(&r), None);
        r[1] = 0x30;
        r[8] = 0xFF;
        assert_eq!(decode(&r), None);
        assert_eq!(decode(&request()[1..]), None); // our own request read back
    }

    #[test]
    fn collection_size_boundary() {
        let mut c = HidColl { vid: RECEIVER_VID, pid: PID, usage_page: 0xFF01, usage_id: 0x0010, input_len: 64, output_len: 64, ..Default::default() };
        assert!(supported_collection(&c));

        c.input_len = 63;
        assert!(!supported_collection(&c));
        c.input_len = 64;
        c.output_len = 63;
        assert!(!supported_collection(&c));
    }

    #[test]
    fn cable_requires_65_byte_collection() {
        let mut c = HidColl { vid: CABLE_VID, pid: PID, usage_page: 0xFF01, usage_id: 0x0010, input_len: 64, output_len: 64, ..Default::default() };
        assert!(!supported_collection(&c));

        c.input_len = 65;
        c.output_len = 65;
        assert!(supported_collection(&c));
    }

    fn freq_reply(percent: u8, charging: u8) -> Vec<u8> {
        let mut r = vec![0u8; 64];
        r[..8].copy_from_slice(&[0xF0, 0x30, 0xA5, 0x00, 0x2E, 0x01, 0x01, 0x01]);
        r[8] = percent;
        r[9] = charging;
        r[3] = freq_checksum(&r);
        r
    }

    #[test]
    fn freq_request_frame() {
        let r = freq_request();
        assert_eq!(&r[..8], &[0xF0, 0x30, 0xA5, 0x0B, 0x2E, 0x01, 0x01, 0x01]);
        assert!(r[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn freq_battery_reply() {
        assert_eq!(decode_freq_reply(&freq_reply(100, 0)), Some((100, false)));
        assert_eq!(decode_freq_reply(&freq_reply(37, 1)), Some((37, true)));
    }

    #[test]
    fn freq_battery_reply_rejects_invalid_reports() {
        let mut r = freq_reply(50, 0);
        r[0] = 0xF1;
        assert_eq!(decode_freq_reply(&r), None);

        let mut r = freq_reply(50, 0);
        r[1] = 0x31;
        assert_eq!(decode_freq_reply(&r), None);

        let mut r = freq_reply(50, 0);
        r[2] = 0xA4;
        assert_eq!(decode_freq_reply(&r), None);

        let mut r = freq_reply(50, 0);
        r[4] = 0x2D;
        assert_eq!(decode_freq_reply(&r), None);

        assert_eq!(decode_freq_reply(&freq_reply(101, 0)), None);
        assert_eq!(decode_freq_reply(&freq_reply(50, 2)), None);
        assert_eq!(decode_freq_reply(&freq_reply(50, 0)[..63]), None);
    }

    #[test]
    fn freq_battery_reply_rejects_mismatched_response_fields() {
        for (index, value) in [(3, 0x94), (5, 0), (6, 0), (7, 0)] {
            let mut r = freq_reply(50, 0);
            r[index] = value;
            assert_eq!(decode_freq_reply(&r), None, "unexpected response field at byte {index}");
        }
    }

    #[test]
    fn f0_query_requires_receiver_64_byte_collection() {
        let mut c = HidColl { vid: RECEIVER_VID, pid: PID, usage_page: 0xFF01, usage_id: 0x0010, input_len: 64, output_len: 64, ..Default::default() };
        assert!(uses_freq_query(&c, true));
        assert!(!uses_freq_query(&c, false));

        c.vid = CABLE_VID;
        assert!(!uses_freq_query(&c, true));
        c.vid = RECEIVER_VID;
        c.input_len = 65;
        assert!(!uses_freq_query(&c, true));
        c.input_len = 64;
        c.output_len = 65;
        assert!(!uses_freq_query(&c, true));
        c.output_len = 64;
        c.pid = 0x2256;
        assert!(!uses_freq_query(&c, true));
        c.pid = PID;
        c.usage_page = 0xFF02;
        assert!(!uses_freq_query(&c, true));
        c.usage_page = 0xFF01;
        c.usage_id = 0x0011;
        assert!(!uses_freq_query(&c, true));
    }

    #[test]
    fn f0_receiver_keeps_generic_name() {
        let mut ctx = PollContext::default();
        ctx.colls.push(HidColl {
            path: String::new(),
            vid: RECEIVER_VID,
            pid: PID,
            usage_page: 0xFF01,
            usage_id: 0x0010,
            input_len: 64,
            output_len: 64,
            ..Default::default()
        });

        poll(&mut ctx);

        assert_eq!(ctx.out[0].name, "AJAZZ wireless mouse");
    }

    #[test]
    fn freq_fallback_waits_for_legacy_timeout() {
        assert!(should_try_freq_query(true, false, false));
        assert!(!should_try_freq_query(true, true, false));
        assert!(should_try_freq_query(true, true, true));
        assert!(!should_try_freq_query(false, true, true));
    }
}
