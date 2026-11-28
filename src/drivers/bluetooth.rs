//! Bluetooth fallback: Windows keeps the level it reads over HFP (classic) or the GATT Battery
//! Service (LE) in a device property, the same value Settings > Bluetooth & devices shows.
//! Fast Pair earbuds get their left / right / case split from BLE broadcasts.
use std::collections::{BTreeMap, BTreeSet};

use windows::Win32::Devices::DeviceAndDriverInstallation::{CM_DEVNODE_STATUS_FLAGS, CM_Get_DevNode_Status, CM_PROB, CR_SUCCESS, DN_HAS_PROBLEM};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName, DEVPKEY_DeviceContainer_Category};

use super::moto_buds;
use super::sdi::{BT_BATTERY, CLASS_OF_DEVICE, DevSet, IS_CONNECTED, container_of_interface};
use crate::ble;
use crate::device::{DeviceKind, DeviceStatus, PollContext};
use crate::hid;
use crate::util::{guess_kind, sleep_ms};

/// Bluetooth address from a main device node id ("BTHENUM\DEV_6CFBED129847\...").
fn bt_address(inst: &str) -> Option<u64> {
    let p = inst.find("\\DEV_")? + 5;
    let hex = inst.get(p..p + 12)?;
    u64::from_str_radix(hex, 16).ok()
}

fn kind_from_class_of_device(cod: u32) -> DeviceKind {
    let (major, minor) = ((cod >> 8) & 0x1F, (cod >> 2) & 0x3F);
    match major {
        0x02 => DeviceKind::Phone,
        0x04 => DeviceKind::Headset,
        0x05 if minor & 0x0F == 0x01 || minor & 0x0F == 0x02 => DeviceKind::Gamepad,
        0x05 if minor >> 4 == 1 => DeviceKind::Keyboard,
        0x05 if minor >> 4 == 2 => DeviceKind::Mouse,
        _ => DeviceKind::Other,
    }
}

fn kind_from_category(c: &str) -> DeviceKind {
    if c == "Communication.Phone" || c.starts_with("Communication.Phone.") {
        DeviceKind::Phone
    } else if c == "Input.Keyboard" {
        DeviceKind::Keyboard
    } else if c == "Input.Mouse" {
        DeviceKind::Mouse
    } else if c == "Input.Gaming" {
        DeviceKind::Gamepad
    } else if c.starts_with("Communication.Headset") || c == "Audio.Headphones" {
        DeviceKind::Headset
    } else {
        DeviceKind::Other
    }
}

fn strip_service_suffix(n: String) -> String {
    for suf in [" Hands-Free AG", " Hands-Free", " Avrcp Transport", " Stereo", " Audio Gateway"] {
        if n.len() > suf.len() && n.ends_with(suf) {
            return n[..n.len() - suf.len()].to_string();
        }
    }
    n
}

/// Portable speakers (e.g. the JBL Go 3, which only speaks A2DP/AVRCP) do not give Windows a
/// battery level. Show them anyway, as a speaker with an unknown ("?") level, instead of dropping
/// them just because there is no level property.
fn is_portable_speaker(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("jbl go 3") || n.contains("jbl go3") || n.contains("speaker") || n.contains("głośnik")
}

/// Vendor drivers (AirPods, ...) also say "Bluetooth"; dedupe them by name, our own entries by key.
/// A row that is off says nothing about this connection: a receiver whose keyboard switched to
/// Bluetooth reports itself off under the same name (#818).
fn is_duplicate(candidate: &DeviceStatus, existing: &[DeviceStatus]) -> bool {
    existing.iter().any(|o| o.key == candidate.key || (o.connected && !o.key.starts_with("bt:") && o.name == candidate.name))
}

fn apply_fast_pair(s: &mut DeviceStatus, b: &ble::FastPairBattery) {
    let (mut dis, mut chg) = (-1, -1);
    for (p, c) in [(b.left, b.left_chg), (b.right, b.right_chg)] {
        if p < 0 {
            continue;
        }
        let dst = if c { &mut chg } else { &mut dis };
        if *dst < 0 || p < *dst {
            *dst = p;
        }
    }
    if dis >= 0 || chg >= 0 {
        s.percent = if dis >= 0 { dis } else { chg };
        s.charging = dis < 0;
    }
    let mut parts = Vec::new();
    for (label, p, c) in [("L", b.left, b.left_chg), ("R", b.right, b.right_chg), ("case", b.case, b.case_chg)] {
        if p >= 0 {
            parts.push(format!("{label} {p}%{}", if c { " ⚡" } else { "" }));
        }
    }
    s.detail = parts.join(", ");
}

struct Found {
    pct: u8,
    name: String,
    connected: bool,
}

/// A hash of which Bluetooth devices are connected right now. Connecting or disconnecting a
/// Bluetooth device (e.g. AirPods) does not add or remove device nodes, so Windows sends no
/// device-change message; comparing this signature every few seconds catches it instead.
pub fn connection_signature() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for e in ["BTHENUM", "BTHLE"] {
        let Some(set) = DevSet::present(Some(e)) else { continue };
        for n in set.nodes() {
            if n.instance_id.starts_with("BTHENUM\\DEV_") || n.instance_id.starts_with("BTHLE\\DEV_") {
                n.instance_id.hash(&mut h);
                n.boolean(&IS_CONNECTED).hash(&mut h);
            }
        }
    }
    h.finish()
}

pub fn poll(ctx: &mut PollContext) {
    // Only the Bluetooth enumerators: listing every device node on the system (all classes) was the
    // most expensive part of a periodic read.
    let sets = ["BTHENUM", "BTHLE", "BTHLEDEVICE"].map(|e| DevSet::present(Some(e)));
    if sets.iter().all(Option::is_none) {
        return;
    }
    let mut container_name: BTreeMap<String, String> = BTreeMap::new();
    let mut container_cod: BTreeMap<String, u32> = BTreeMap::new();
    let mut container_cat: BTreeMap<String, String> = BTreeMap::new();
    let mut container_conn: BTreeMap<String, bool> = BTreeMap::new();
    let mut container_addr: BTreeMap<String, u64> = BTreeMap::new();
    // LE addresses first: a phone's Battery Service is read over LE.
    let mut container_le: BTreeMap<String, u64> = BTreeMap::new();
    let mut battery: BTreeMap<String, Found> = BTreeMap::new();
    let mut speakers: BTreeSet<String> = BTreeSet::new();
    // Moto Buds already read over their own service (moto_buds.rs), whatever Windows calls them.
    let mut moto: BTreeSet<String> = BTreeSet::new();

    // Only the classic and LE root nodes supply connection state and phone addresses.
    let [classic, le, services] = &sets;
    let mut complete = classic.is_some() && le.is_some();
    let roots = classic.iter().chain(le).flat_map(|set| {
        let (nodes, ok) = set.nodes_with_status();
        complete &= ok;
        nodes
    });
    for n in roots.chain(services.iter().flat_map(DevSet::nodes)) {
        let inst = &n.instance_id;
        if !(inst.starts_with("BTHENUM\\") || inst.starts_with("BTHLE\\") || inst.starts_with("BTHLEDEVICE\\")) {
            continue;
        }
        let cid = n.container().unwrap_or_else(|| inst.clone());
        let name = n.string(&DEVPKEY_Device_FriendlyName).or_else(|| n.string(&DEVPKEY_Device_DeviceDesc)).unwrap_or_default();
        if inst.starts_with("BTHENUM\\DEV_") || inst.starts_with("BTHLE\\DEV_") {
            if is_portable_speaker(&name) {
                speakers.insert(cid.clone());
            }
            container_name.insert(cid.clone(), name.clone());
            if let Some(a) = bt_address(inst) {
                let key = moto_buds::key(a);
                if ctx.out.iter().any(|o| o.key == key) {
                    moto.insert(cid.clone());
                }
                container_addr.insert(cid.clone(), a);
                if inst.starts_with("BTHLE\\") {
                    container_le.insert(cid.clone(), a);
                }
            }
            if let Some(cod) = n.u32(&CLASS_OF_DEVICE) {
                container_cod.insert(cid.clone(), cod);
            }
            if let Some(cat) = n.string(&DEVPKEY_DeviceContainer_Category) {
                container_cat.insert(cid.clone(), cat);
            }
            if let Some(conn) = n.boolean(&IS_CONNECTED) {
                // Classic and LE nodes can share one container: connected if either is.
                *container_conn.entry(cid.clone()).or_insert(false) |= conn;
            }
        }
        let Some(pct) = n.byte(&BT_BATTERY) else { continue };
        let conn = n.boolean(&IS_CONNECTED).unwrap_or(true);
        let (mut st, mut prob) = (CM_DEVNODE_STATUS_FLAGS(0), CM_PROB(0));
        if unsafe { CM_Get_DevNode_Status(&mut st, &mut prob, n.data.DevInst, 0) } == CR_SUCCESS && st.0 & DN_HAS_PROBLEM.0 != 0 {
            continue;
        }
        if pct > 100 {
            continue;
        }
        if let Some(prev) = battery.get(&cid)
            && prev.connected
            && !conn
        {
            continue; // prefer a live node
        }
        battery.insert(cid, Found { pct, name: strip_service_suffix(name), connected: conn });
    }
    drop(sets);
    if complete {
        // Missing connection properties are inconclusive. Keep such devices eligible.
        let active = container_addr.iter().chain(&container_le)
            .filter(|(cid, _)| container_conn.get(*cid) != Some(&false))
            .map(|(_, &addr)| addr);
        ble::retain_gatt_links(active);
    }

    // Physical devices a driver above already reported over a HID interface on the BT link itself
    // (or Moto Buds' own service).
    ctx.bt_containers.extend(moto.iter().cloned()); // the generic fallback leaves them out too
    let mut reported = moto;
    let mut hid_ids: BTreeMap<String, (u16, u16)> = BTreeMap::new();
    for c in &ctx.colls {
        let Some(cont) = container_of_interface(&c.path) else { continue };
        if ctx.is_claimed(c.vid, c.pid) {
            reported.insert(cont.clone());
        }
        hid_ids.insert(cont, (c.vid, c.pid));
    }

    // Phones: many (iPhones among them) offer the standard Battery Service over Bluetooth LE. It is
    // read first, also when Windows has a level: Windows keeps the level a phone sent over HFP until
    // it sends another, which can stay old for a long time (e.g. at 15 % while it charges at 30 %).
    // Windows' level stays the fallback. Connected phones only, read once per poll.
    for (cid, name) in &container_name {
        if reported.contains(cid) || !container_conn.get(cid).copied().unwrap_or(false) {
            continue;
        }
        let phone = container_cod.get(cid).is_some_and(|&c| kind_from_class_of_device(c) == DeviceKind::Phone)
            || container_cat.get(cid).is_some_and(|c| kind_from_category(c) == DeviceKind::Phone);
        if !phone {
            continue;
        }
        let Some(&addr) = container_le.get(cid).or_else(|| container_addr.get(cid)) else { continue };
        if let Some(pct) = ble::read_gatt_battery(addr) {
            battery.insert(cid.clone(), Found { pct, name: strip_service_suffix(name.clone()), connected: true });
        }
    }

    let (mut watching, mut waited) = (false, false);
    // A speaker without a battery property still gets an entry (with an unknown level).
    let cids: BTreeSet<&String> = battery.keys().chain(speakers.iter()).collect();
    for cid in cids {
        if reported.contains(cid) {
            continue;
        }
        let found = battery.get(cid);
        let connected = container_conn.get(cid).copied().unwrap_or_else(|| found.map(|f| f.connected).unwrap_or(speakers.contains(cid)));
        if !connected {
            continue;
        }
        let mut s = DeviceStatus { key: format!("bt:{cid}"), ..Default::default() };
        s.name = container_name.get(cid).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| found.map(|f| f.name.clone()).unwrap_or_default());
        if s.name.is_empty() {
            s.name = "Bluetooth device".into();
        }
        if is_duplicate(&s, &ctx.out) {
            continue;
        }
        s.via = "Bluetooth".into();
        s.percent = found.map(|f| f.pct as i32).unwrap_or(-1);
        s.kind = container_cod.get(cid).map(|&c| kind_from_class_of_device(c)).unwrap_or_default();
        if s.kind == DeviceKind::Other
            && let Some(cat) = container_cat.get(cid)
        {
            s.kind = kind_from_category(cat);
        }
        if let Some(&(vid, pid)) = hid_ids.get(cid) {
            s.vid = vid;
            if s.kind == DeviceKind::Other {
                s.kind = hid::kind_from_collections(&ctx.colls, vid, pid);
            }
        }
        if s.kind == DeviceKind::Other {
            s.kind = guess_kind(&s.name);
        }
        if speakers.contains(cid) {
            s.kind = DeviceKind::Speaker; // a portable box is not a headset, even with CoD audio
        } else if s.kind == DeviceKind::Headset
            && let Some(&addr) = container_addr.get(cid)
        {
            let mut just_started = false;
            if !watching {
                watching = true;
                just_started = ble::ensure_watcher();
            }
            let mut got = ble::fast_pair_lookup(addr);
            if got.is_none() && just_started && !waited {
                for _ in 0..15 {
                    sleep_ms(200);
                    got = ble::fast_pair_lookup(addr);
                    if got.is_some() {
                        break;
                    }
                }
            }
            waited |= just_started;
            if let Some(b) = got {
                apply_fast_pair(&mut s, &b);
            }
        }
        ctx.out.push(s);
        ctx.bt_containers.insert(cid.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_connected_row_of_the_same_name_hides_the_bluetooth_one() {
        let row = |key: &str, connected| DeviceStatus { key: key.into(), name: "DSV2 Pro".into(), connected, ..Default::default() };
        let bt = row("bt:{1}", true);
        assert!(is_duplicate(&bt, &[row("razer:0290", true)]));
        assert!(!is_duplicate(&bt, &[row("razer:0290", false)]));
        assert!(is_duplicate(&bt, &[row("bt:{1}", false)])); // our own entry
        assert!(!is_duplicate(&bt, &[row("bt:{2}", true)])); // another device of the same model
    }

    #[test]
    fn portable_speakers_are_detected() {
        assert!(is_portable_speaker("JBL Go 3"));
        assert!(is_portable_speaker("JBL Go 3 Stereo"));
        assert!(is_portable_speaker("JBL GO3"));
        assert!(is_portable_speaker("My Bluetooth speaker"));
        assert!(is_portable_speaker("Głośnik JBL"));
        assert!(!is_portable_speaker("JBL Quantum 800"));
        assert!(!is_portable_speaker("JBL Flip 5"));
    }

    #[test]
    fn audio_class_is_a_speaker_only_for_boxes() {
        assert_eq!(kind_from_class_of_device(0x00240414), DeviceKind::Headset); // CoD audio
        assert_eq!(guess_kind("JBL Go 3"), DeviceKind::Other); // name alone does not say it
        assert_eq!(guess_kind("JBL Go 3 Speaker"), DeviceKind::Speaker);
        assert_eq!(DeviceKind::Speaker.glyph(), '\u{E7F5}');
        assert_eq!(DeviceKind::Speaker.label_en(), crate::i18n::n_("Speaker"));
    }
}