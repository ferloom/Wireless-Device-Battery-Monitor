//! Per-device settings and the device menus. Each edition keeps them under its settings key, one
//! value per device key (stable across reconnects and restarts):
//!  * `Thresholds`: red warning level (DWORD, percent);
//!  * `Names`: name chosen with "Rename device" (REG_SZ; the device itself is not renamed);
//!  * `Warn5`: "Warn at 5%" (DWORD 0/1, default on);
//!  * `Hidden`: "Show on the taskbar" turned off (DWORD 1; the device is still read and listed in
//!    the menu, only the strip, its tooltip and the hover card skip it);
//!  * `Known`: the last status of every device seen (REG_SZ, a dashboard status line), so the
//!    dashboard can list devices that are not connected now but have saved history.
use std::collections::{HashMap, HashSet};

use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, HMENU, MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
};

use crate::device::{DeviceKind, DeviceStatus};
use crate::i18n::{n_, tr, trf};
use crate::install::{self as inst, HKCU};
use crate::ui::{MenuIcons, glyph};

/// The levels offered in the menus.
pub const THRESHOLDS: [i32; 7] = [50, 40, 30, 20, 15, 10, 5];
pub const DEFAULT_THRESHOLD: i32 = 15;
/// "Warn at 5%": shown once when the level falls to this, armed again at REARM_LEVEL or on power.
pub const CRITICAL_LEVEL: i32 = 5;
pub const REARM_LEVEL: i32 = 10;
/// Longest name accepted by "Rename device".
pub const NAME_MAX: usize = 48;

fn sub(settings_key: &str, name: &str) -> String {
    format!(r"{settings_key}\{name}")
}

/// Warning level for one device (the percentage at or below which it turns red).
pub fn threshold(settings_key: &str, device_key: &str) -> i32 {
    inst::open(HKCU, &sub(settings_key, "Thresholds"), false)
        .and_then(|k| k.get(device_key))
        .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        .map(|v| v as i32)
        .filter(|v| THRESHOLDS.contains(v))
        .unwrap_or(DEFAULT_THRESHOLD)
}

pub fn set_threshold(settings_key: &str, device_key: &str, value: i32) -> bool {
    inst::create(HKCU, &sub(settings_key, "Thresholds")).is_some_and(|k| k.set_dword(device_key, value as u32))
}

/// Low = discharging, a known level, at or below the device's warning level.
pub fn is_low(s: &DeviceStatus, threshold: i32) -> bool {
    !s.charging && !s.is_full() && s.connected && s.percent >= 0 && s.percent <= threshold
}

// ---- names -------------------------------------------------------------------------------------

/// A name typed by the user, cleaned up: no control characters, single spaces, at most NAME_MAX
/// characters. None when nothing is left (the device's own name is used then).
pub fn clean_name(s: &str) -> Option<String> {
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(NAME_MAX).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Every name chosen with "Rename device": device key -> name.
pub fn names(settings_key: &str) -> HashMap<String, String> {
    inst::open(HKCU, &sub(settings_key, "Names"), false)
        .map(|k| k.values().into_iter().filter(|v| v.1 == 1).filter_map(|(n, _, d)| clean_name(&d).map(|d| (n, d))).collect())
        .unwrap_or_default()
}

/// Saves a name (None or an empty name goes back to the device's own name).
pub fn set_name(settings_key: &str, device_key: &str, name: Option<&str>) -> bool {
    match name.and_then(clean_name) {
        Some(n) => inst::create(HKCU, &sub(settings_key, "Names")).is_some_and(|k| k.set_string(device_key, &n)),
        None => inst::open(HKCU, &sub(settings_key, "Names"), true).is_none_or(|k| {
            k.delete_value(device_key);
            true
        }),
    }
}

// ---- "Warn at 5%" ------------------------------------------------------------------------------

pub fn warn_critical(settings_key: &str, device_key: &str) -> bool {
    inst::open(HKCU, &sub(settings_key, "Warn5"), false).and_then(|k| k.get(device_key)).is_none_or(|v| v != "0x00000000")
}

pub fn set_warn_critical(settings_key: &str, device_key: &str, on: bool) -> bool {
    inst::create(HKCU, &sub(settings_key, "Warn5")).is_some_and(|k| k.set_dword(device_key, on as u32))
}

// ---- "Show on the taskbar" --------------------------------------------------------------------

/// Keys of the devices hidden from the taskbar.
pub fn hidden(settings_key: &str) -> HashSet<String> {
    inst::open(HKCU, &sub(settings_key, "Hidden"), false)
        .map(|k| k.values().into_iter().filter(|(_, _, d)| d != "0x00000000").map(|(n, _, _)| n).collect())
        .unwrap_or_default()
}

pub fn set_hidden(settings_key: &str, device_key: &str, hide: bool) -> bool {
    if hide {
        inst::create(HKCU, &sub(settings_key, "Hidden")).is_some_and(|k| k.set_dword(device_key, 1))
    } else {
        inst::open(HKCU, &sub(settings_key, "Hidden"), true).is_none_or(|k| {
            k.delete_value(device_key);
            true
        })
    }
}

// ---- order on the taskbar ---------------------------------------------------------------------

/// Device keys in the order the user put them in (dragged on the strip, or "Move left / right").
pub fn order(settings_key: &str) -> Vec<String> {
    let mut v: Vec<(u32, String)> = inst::open(HKCU, &sub(settings_key, "Order"), false)
        .map(|k| k.values().into_iter().filter_map(|(n, _, d)| Some((u32::from_str_radix(d.trim_start_matches("0x"), 16).ok()?, n))).collect())
        .unwrap_or_default();
    v.sort();
    v.into_iter().map(|(_, n)| n).collect()
}

pub fn set_order(settings_key: &str, keys: &[String]) -> bool {
    let path = sub(settings_key, "Order");
    if let Some(k) = inst::open(HKCU, &path, true) {
        for (n, _, _) in k.values() {
            if !keys.contains(&n) {
                k.delete_value(&n);
            }
        }
    }
    inst::create(HKCU, &path).is_some_and(|k| keys.iter().enumerate().all(|(i, key)| k.set_dword(key, i as u32)))
}

/// Sorts `keys` (strip order) by the saved order; keys not saved keep their place after them.
pub fn apply_order(keys: &mut [String], saved: &[String]) {
    keys.sort_by_key(|k| saved.iter().position(|s| s == k).unwrap_or(usize::MAX));
}

/// The saved order after `a` and `b` swapped places: the current strip order first, then the
/// saved devices that are not connected now (they keep their relative order).
pub fn swapped(current: &[String], saved: &[String], a: &str, b: &str) -> Vec<String> {
    let mut v: Vec<String> = current.to_vec();
    if let (Some(i), Some(j)) = (v.iter().position(|k| k == a), v.iter().position(|k| k == b)) {
        v.swap(i, j);
    }
    v.extend(saved.iter().filter(|k| !current.contains(k)).cloned());
    v
}

/// The saved order after `a` moved to the place of `b` (the devices between shift by one): the
/// current strip order first, then the saved devices that are not connected now.
pub fn moved(current: &[String], saved: &[String], a: &str, b: &str) -> Vec<String> {
    let mut v: Vec<String> = current.to_vec();
    if let (Some(i), Some(j)) = (v.iter().position(|k| k == a), v.iter().position(|k| k == b)) {
        let k = v.remove(i);
        v.insert(j, k);
    }
    v.extend(saved.iter().filter(|k| !current.contains(k)).cloned());
    v
}

/// The saved order with devices seen for the first time in front (they keep their order).
pub fn with_new_first(saved: &[String], new: &[String]) -> Vec<String> {
    let fresh: Vec<String> = new.iter().filter(|k| !saved.contains(k)).cloned().collect();
    fresh.into_iter().chain(saved.iter().cloned()).collect()
}

// ---- forgetting devices that are gone ---------------------------------------------------------

/// A device not seen for this long is forgotten: its place, name and settings go (its
/// measurements are only kept for the same 7 days, see history::KEEP_SECS).
pub const FORGET_SECS: u64 = crate::history::KEEP_SECS;

/// Per-device settings, by subkey (a value per device key).
const DEVICE_SUBKEYS: [&str; 6] = ["Order", "Names", "Hidden", "Thresholds", "Warn5", "WidgetAnchors"];

/// Logical XAML insertion anchor for one peripheral in experimental Individual layout.
pub fn widget_anchor(settings_key: &str, device_key: &str) -> i32 {
    inst::open(HKCU, &sub(settings_key, "WidgetAnchors"), false)
        .and_then(|k| k.get(device_key))
        .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        .map(|v| v as i32)
        .filter(|v| (-4097..4096).contains(v))
        .unwrap_or(-1)
}

pub fn set_widget_anchor(settings_key: &str, device_key: &str, anchor: i32) -> bool {
    if !(-4097..4096).contains(&anchor) {
        return false;
    }
    inst::create(HKCU, &sub(settings_key, "WidgetAnchors")).is_some_and(|k| k.set_dword(device_key, anchor as u32))
}

fn seen_times(settings_key: &str) -> HashMap<String, u64> {
    inst::open(HKCU, &sub(settings_key, "Seen"), false)
        .map(|k| k.values().into_iter().filter_map(|(n, _, d)| Some((n, u32::from_str_radix(d.trim_start_matches("0x"), 16).ok()? as u64))).collect())
        .unwrap_or_default()
}

/// Every device seen so far: device key -> its last status line (dashboard::encode_status).
pub fn known(settings_key: &str) -> Vec<(String, String)> {
    inst::open(HKCU, &sub(settings_key, "Known"), false)
        .map(|k| k.values().into_iter().filter(|v| v.1 == 1).map(|(n, _, d)| (n, d)).collect())
        .unwrap_or_default()
}

/// Saves a device's status line for the dashboard's list of known devices.
pub fn set_known(settings_key: &str, device_key: &str, line: &str) -> bool {
    inst::create(HKCU, &sub(settings_key, "Known")).is_some_and(|k| k.set_string(device_key, line))
}

/// Remembers that these devices were connected at `now` (Unix time, UTC).
pub fn mark_seen(settings_key: &str, keys: &[String], now: u64) -> bool {
    inst::create(HKCU, &sub(settings_key, "Seen")).is_some_and(|k| keys.iter().all(|key| k.set_dword(key, now as u32)))
}

/// Forgets every device last seen more than FORGET_SECS before `now`; returns their keys. A device
/// with settings but no date yet (saved by an older version) is dated `now`.
pub fn forget_unseen(settings_key: &str, now: u64) -> Vec<String> {
    let seen = seen_times(settings_key);
    let mut known: HashSet<String> = HashSet::new();
    for name in DEVICE_SUBKEYS {
        if let Some(k) = inst::open(HKCU, &sub(settings_key, name), false) {
            known.extend(k.values().into_iter().map(|v| v.0));
        }
    }
    let undated: Vec<String> = known.into_iter().filter(|k| !seen.contains_key(k)).collect();
    if !undated.is_empty() {
        mark_seen(settings_key, &undated, now);
    }
    let gone: Vec<String> = seen.into_iter().filter(|(_, t)| now.saturating_sub(*t) > FORGET_SECS).map(|(k, _)| k).collect();
    if gone.is_empty() {
        return gone;
    }
    for name in DEVICE_SUBKEYS.iter().chain(std::iter::once(&"Seen")) {
        if let Some(k) = inst::open(HKCU, &sub(settings_key, name), true) {
            for key in &gone {
                k.delete_value(key);
            }
        }
    }
    gone
}

/// Which devices were already warned about: one warning when the level falls to 5% or below,
/// none on later readings, and a new one only after the device charged (on power, or back at
/// REARM_LEVEL or above).
#[derive(Default, Debug)]
pub struct CriticalWarnings {
    warned: HashSet<String>,
}

impl CriticalWarnings {
    /// Returns true when a warning should be shown for this reading now.
    pub fn check(&mut self, key: &str, level: i32, on_power: bool) -> bool {
        if level < 0 {
            return false; // unknown level: neither warn nor re-arm
        }
        if on_power || level >= REARM_LEVEL {
            self.warned.remove(key);
            return false;
        }
        level <= CRITICAL_LEVEL && self.warned.insert(key.to_string())
    }
}

// ---- menus -------------------------------------------------------------------------------------

/// Menu ids: BASE + device index * STRIDE + one of the item offsets below.
pub const MENU_BASE: usize = 1000;
const STRIDE: usize = 32;
const ITEM_DASHBOARD: usize = 0;
const ITEM_HISTORY: usize = 1;
const ITEM_STATS: usize = 2;
const ITEM_RENAME: usize = 3;
const ITEM_RESET_NAME: usize = 4;
const ITEM_WARN5: usize = 5;
const ITEM_SHOW: usize = 6;
const ITEM_THRESHOLD: usize = 8; // + index into THRESHOLDS
const ITEM_MOVE_BACK: usize = 20;
const ITEM_MOVE_ON: usize = 21;
const ITEM_RESET_HISTORY: usize = 22;
const _: () = assert!(ITEM_THRESHOLD + THRESHOLDS.len() <= ITEM_MOVE_BACK && ITEM_RESET_HISTORY < STRIDE);

/// What a device menu item asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceAction {
    /// Open the dashboard on a tab (0 overview, 1 history, 2 statistics).
    Dashboard(u8),
    Rename,
    ResetName,
    ToggleWarn5,
    ToggleShown,
    Threshold(i32),
    /// One place to the left (up on a vertical taskbar), or to the right (down).
    Move(bool),
    /// Delete every measurement of the device (after a warning).
    ResetHistory,
}

/// Appends one submenu per device ("Name (32%, on battery)"), in three sections: information
/// (dashboard, history, statistics), management (rename) and warnings (red warning level, warn
/// at 5%). Every item has an icon. `renamed` holds the keys that have a name of the user's.
pub fn append_device_menus(
    menu: HMENU,
    devices: &[DeviceStatus],
    settings_key: &str,
    renamed: &HashSet<String>,
    hidden: &HashSet<String>,
    vertical: bool,
    icons: &mut MenuIcons,
) {
    unsafe {
        for (i, d) in devices.iter().enumerate() {
            let Ok(sub) = CreatePopupMenu() else { continue };
            MenuIcons::check_or_icon(sub);
            let id = |item: usize| MENU_BASE + i * STRIDE + item;
            let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, 0, &hs(&format!("{} - {}", d.name, d.charge_text())));
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            for (item, label, g) in [
                (ITEM_DASHBOARD, n_("Dashboard..."), glyph::DASHBOARD),
                (ITEM_HISTORY, n_("Battery history"), glyph::HISTORY),
                (ITEM_STATS, n_("Statistics"), glyph::STATISTICS),
            ] {
                let _ = AppendMenuW(sub, MF_STRING, id(item), &hs(&tr(label)));
                icons.set(sub, id(item) as u32, false, g);
            }
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let _ = AppendMenuW(sub, MF_STRING, id(ITEM_RENAME), &hs(&tr("Rename device...")));
            icons.set(sub, id(ITEM_RENAME) as u32, false, glyph::RENAME);
            if renamed.contains(&d.key) {
                let _ = AppendMenuW(sub, MF_STRING, id(ITEM_RESET_NAME), &hs(&tr("Use the device's own name")));
                icons.set(sub, id(ITEM_RESET_NAME) as u32, false, glyph::RESET);
            }
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            // Red warning level.
            if let Ok(levels) = CreatePopupMenu() {
                let current = threshold(settings_key, &d.key);
                for (j, t) in THRESHOLDS.iter().enumerate() {
                    let check = if *t == current { MF_CHECKED } else { MF_UNCHECKED };
                    let def = if *t == DEFAULT_THRESHOLD { format!("  ({})", tr("default")) } else { String::new() };
                    let _ = AppendMenuW(levels, MF_STRING | check, id(ITEM_THRESHOLD + j), &hs(&format!("{t}%{def}")));
                }
                let n = windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(sub));
                let _ = AppendMenuW(
                    sub,
                    MF_STRING | MF_POPUP,
                    levels.0 as usize,
                    &hs(&trf("Red warning at or below {level}%", &[("level", &current.to_string())])),
                );
                icons.set(sub, n as u32, true, glyph::WARNING);
            }
            let check = if warn_critical(settings_key, &d.key) { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(sub, MF_STRING | check, id(ITEM_WARN5), &hs(&tr("Warn at 5% (notification)")));
            icons.set(sub, id(ITEM_WARN5) as u32, false, glyph::BELL);
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let shown = !hidden.contains(&d.key);
            let check = if shown { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(sub, MF_STRING | check, id(ITEM_SHOW), &hs(&tr("Show on the taskbar")));
            icons.set(sub, id(ITEM_SHOW) as u32, false, glyph::VIEW);
            // Place on the strip (the devices can also be dragged there).
            let (back, on) = if vertical {
                ((n_("Move up"), glyph::UP), (n_("Move down"), glyph::DOWN))
            } else {
                ((n_("Move left"), glyph::LEFT), (n_("Move right"), glyph::RIGHT))
            };
            for (item, (label, g), enabled) in [(ITEM_MOVE_BACK, back, i > 0), (ITEM_MOVE_ON, on, i + 1 < devices.len())] {
                let _ = AppendMenuW(sub, MF_STRING | if enabled { MF_ENABLED } else { MF_GRAYED }, id(item), &hs(&tr(label)));
                icons.set(sub, id(item) as u32, false, g);
            }
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let _ = AppendMenuW(sub, MF_STRING, id(ITEM_RESET_HISTORY), &hs(&tr("Reset measurements...")));
            icons.set(sub, id(ITEM_RESET_HISTORY) as u32, false, glyph::DELETE);

            let n = windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(menu));
            let label = if hidden.contains(&d.key) {
                format!("{}  ({}, {})", d.name, d.charge_text(), tr("hidden"))
            } else {
                format!("{}  ({})", d.name, d.charge_text())
            };
            let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, sub.0 as usize, &hs(&label));
            icons.set(menu, n as u32, true, if d.kind == DeviceKind::Earbuds { crate::ui::glyph::EARBUDS } else { d.kind.glyph() });
        }
    }
}

/// Decodes a menu id from `append_device_menus` into (device index, action).
pub fn decode_menu_id(id: usize) -> Option<(usize, DeviceAction)> {
    let rel = id.checked_sub(MENU_BASE)?;
    let (dev, item) = (rel / STRIDE, rel % STRIDE);
    let action = match item {
        ITEM_DASHBOARD => DeviceAction::Dashboard(0),
        ITEM_HISTORY => DeviceAction::Dashboard(1),
        ITEM_STATS => DeviceAction::Dashboard(2),
        ITEM_RENAME => DeviceAction::Rename,
        ITEM_RESET_NAME => DeviceAction::ResetName,
        ITEM_WARN5 => DeviceAction::ToggleWarn5,
        ITEM_SHOW => DeviceAction::ToggleShown,
        ITEM_MOVE_BACK => DeviceAction::Move(false),
        ITEM_MOVE_ON => DeviceAction::Move(true),
        ITEM_RESET_HISTORY => DeviceAction::ResetHistory,
        i if (ITEM_THRESHOLD..ITEM_MOVE_BACK).contains(&i) => DeviceAction::Threshold(*THRESHOLDS.get(i - ITEM_THRESHOLD)?),
        _ => return None,
    };
    Some((dev, action))
}

fn hs(s: &str) -> windows::core::HSTRING {
    windows::core::HSTRING::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_round_trip() {
        for dev in 0..5 {
            for (j, t) in THRESHOLDS.iter().enumerate() {
                assert_eq!(decode_menu_id(MENU_BASE + dev * STRIDE + ITEM_THRESHOLD + j), Some((dev, DeviceAction::Threshold(*t))));
            }
            assert_eq!(decode_menu_id(MENU_BASE + dev * STRIDE + ITEM_RENAME), Some((dev, DeviceAction::Rename)));
            assert_eq!(decode_menu_id(MENU_BASE + dev * STRIDE + ITEM_STATS), Some((dev, DeviceAction::Dashboard(2))));
            assert_eq!(decode_menu_id(MENU_BASE + dev * STRIDE + ITEM_SHOW), Some((dev, DeviceAction::ToggleShown)));
        }
        assert_eq!(decode_menu_id(3), None);
        assert_eq!(decode_menu_id(MENU_BASE + 7), None);
    }

    #[test]
    fn critical_warning_once_and_rearmed_after_charging() {
        let mut w = CriticalWarnings::default();
        assert!(!w.check("m", 12, false));
        assert!(!w.check("m", 6, false));
        assert!(w.check("m", 5, false)); // reached 5%: warn
        assert!(!w.check("m", 5, false)); // same level again: no repeat
        assert!(!w.check("m", 3, false)); // falling further: still once
        assert!(!w.check("m", 7, false)); // a bounce under 10% does not re-arm
        assert!(!w.check("m", 4, false));
        assert!(!w.check("m", -1, false)); // unknown level changes nothing
        assert!(!w.check("m", 4, true)); // plugged in: re-armed, no warning while on power
        assert!(w.check("m", 5, false)); // unplugged and still low: warned again
        assert!(!w.check("k", 9, false));
        assert!(w.check("k", 1, false)); // first reading already below 5%
        assert!(!w.check("k", 50, false));
        assert!(w.check("k", 5, false));
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  My   mouse \n"), Some("My mouse".into()));
        assert_eq!(clean_name("\t\r\n "), None);
        assert_eq!(clean_name(&"x".repeat(100)).unwrap().chars().count(), NAME_MAX);
        assert_eq!(clean_name("Słuchawki 🎧"), Some("Słuchawki 🎧".into()));
    }

    #[test]
    fn names_survive_in_the_registry() {
        let sk = r"Software\BatteryHub-Test-Names";
        assert!(set_name(sk, "logi:unit:ABC", Some("Office mouse")));
        assert_eq!(names(sk).get("logi:unit:ABC").map(String::as_str), Some("Office mouse"));
        assert!(set_name(sk, "logi:unit:ABC", None));
        assert!(!names(sk).contains_key("logi:unit:ABC"));
        assert!(warn_critical(sk, "x")); // default on
        assert!(hidden(sk).is_empty());
        assert!(set_hidden(sk, "x", true) && set_hidden(sk, "y", true));
        assert_eq!(hidden(sk), HashSet::from(["x".to_string(), "y".to_string()]));
        assert!(set_hidden(sk, "x", false));
        assert_eq!(hidden(sk), HashSet::from(["y".to_string()]));
        assert!(set_warn_critical(sk, "x", false));
        assert!(!warn_critical(sk, "x"));
        assert!(inst::delete_tree(HKCU, sk));
    }

    #[test]
    fn new_devices_first_and_dragged_moves() {
        let k = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // New ones in front, in the order they came; known ones keep their saved place.
        assert_eq!(with_new_first(&k(&["a", "b"]), &k(&["x", "a", "y"])), k(&["x", "y", "a", "b"]));
        // Dragged from the first place to the third: the two between shift left.
        assert_eq!(moved(&k(&["a", "b", "c", "d"]), &k(&["gone"]), "a", "c"), k(&["b", "c", "a", "d", "gone"]));
        assert_eq!(moved(&k(&["a", "b", "c"]), &[], "c", "a"), k(&["c", "a", "b"]));
    }

    #[test]
    fn saved_order_and_swaps() {
        let k = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut keys = k(&["a", "b", "c", "new"]);
        apply_order(&mut keys, &k(&["c", "gone", "a", "b"]));
        assert_eq!(keys, k(&["c", "a", "b", "new"]));
        // c and new swap; "gone" (not connected) stays saved after them.
        let saved = swapped(&keys, &k(&["c", "gone", "a", "b"]), "c", "new");
        assert_eq!(saved, k(&["new", "a", "b", "c", "gone"]));
        assert_eq!(decode_menu_id(MENU_BASE + STRIDE + ITEM_MOVE_BACK), Some((1, DeviceAction::Move(false))));
        assert_eq!(decode_menu_id(MENU_BASE + ITEM_RESET_HISTORY), Some((0, DeviceAction::ResetHistory)));
    }
}
