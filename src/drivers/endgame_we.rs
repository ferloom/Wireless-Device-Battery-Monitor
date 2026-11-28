//! Endgame Gear OP1we on its 2.4 GHz receiver 3367:1961 (#621). The 4K dongle is endgame_4k.rs.
//!
//! Protocol as documented by OpenMouse mouse-protocol (AGPL-3.0,
//! github.com/OpenMouse-Project/mouse-protocol, src/endgame-gear/wireless.ts and
//! src/drivers/endgame/egg-we-hid.ts); implemented here from those facts. The command frame is
//! CompX's GetPower (08 04 .. checksum, see compx.rs), sent as a feature report to the collection
//! FF02:0002; the answer is an input report on the FF01 collection:
//!
//!   reply    09 04 00 .. .. .. LL CH     00 = status ok, LL = level, CH = non-zero while charging
//!
//! Only this read is sent. Not tried on hardware by BatteryHub.
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::drivers::compx::{CMD_POWER, request};
use crate::hid::{HidColl, HidDev};
use crate::util::Deadline;

const VID: u16 = 0x3367;
const PID: u16 = 0x1961;
const NOTIFY: u8 = 0x09;
const LEN: u16 = 17;
const REPLY_MS: u64 = 500;

/// (level, charging) from an input report, report id first.
fn parse(r: &[u8]) -> Option<(i32, bool)> {
    (r.len() >= 8 && r[..3] == [NOTIFY, CMD_POWER, 0] && r[6] <= 100).then(|| (r[6] as i32, r[7] != 0))
}

fn read(command: &HidColl, notify: &HidColl) -> Option<(i32, bool)> {
    // Listening before asking: the answer must not arrive without a handle open for it.
    let rx = HidDev::open_read_only(notify).or_else(|| HidDev::open(notify))?;
    let tx = HidDev::open_feature_only(command).or_else(|| HidDev::open(command))?;
    rx.flush();
    if !tx.set_feature(&request(CMD_POWER)) {
        return None;
    }
    let end = Deadline::in_ms(REPLY_MS);
    while !end.passed() {
        if let Some(v) = rx.read(end.left_ms()).and_then(|r| parse(&r)) {
            return Some(v);
        }
    }
    None
}

pub fn poll(ctx: &mut PollContext) {
    if ctx.is_claimed(VID, PID) {
        return;
    }
    let commands = ctx.find(VID, PID, |c| c.page() == 0xFF02 && c.usage() == 0x0002 && c.feat_len() == LEN);
    let notifies = ctx.find(VID, PID, |c| c.page() == 0xFF01 && c.in_len() == LEN);
    let (Some(command), Some(notify)) = (commands.first(), notifies.first()) else { return };
    let base = DeviceStatus {
        key: "endgame:1961".into(),
        name: "Endgame Gear OP1we".into(),
        kind: DeviceKind::Mouse,
        via: "2.4 GHz receiver".into(),
        hw: Some((VID, PID)),
        grace_sec: 1800,
        ..Default::default()
    };
    let s = match read(command, notify) {
        Some((percent, charging)) => DeviceStatus { percent, charging, ..base },
        None => DeviceStatus { connected: false, ..base },
    };
    ctx.add(s, VID, PID);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        let mut r = [0u8; 17];
        r[..8].copy_from_slice(&[0x09, 0x04, 0x00, 0, 0, 0, 73, 1]);
        assert_eq!(parse(&r), Some((73, true)));
        r[2] = 1; // an error status
        assert_eq!(parse(&r), None);
        r[2] = 0;
        r[6] = 101;
        assert_eq!(parse(&r), None);
        assert_eq!(parse(&request(CMD_POWER)), None); // report 08, not the notification
    }
}
