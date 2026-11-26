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

/// Earbuds: (left low, right low), each bud by its own level and charging state.
pub fn buds_low(s: &DeviceStatus, threshold: i32) -> Option<(bool, bool)> {
    let b = s.buds?;
    let low = |v: i32, c: bool| s.connected && !c && v >= 0 && v <= threshold;
    Some((low(b.left, b.left_charging), low(b.right, b.right_charging)))
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

// ---- "Red icon at the warning level" ----------------------------------------------------------

pub fn red_icon(settings_key: &str, device_key: &str) -> bool {
    inst::open(HKCU, &sub(settings_key, "RedIcon"), false).and_then(|k| k.get(device_key)).is_some_and(|v| v != "0x00000000")
}

pub fn set_red_icon(settings_key: &str, device_key: &str, on: bool) -> bool {
    inst::create(HKCU, &sub(settings_key, "RedIcon")).is_some_and(|k| k.set_dword(device_key, on as u32))
}

// ---- What hovering a device shows ----------------------------------------------------------------

/// What the mouse on a device brings up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HoverMode {
    /// The card (level, time left, chart) - the default.
    Card,
    /// Only a short tooltip with the name and level, as in the first versions.
    Tip,
    /// Nothing.
    Off,
}

fn dword(settings_key: &str, name: &str, device_key: &str) -> Option<u32> {
    let v = inst::open(HKCU, &sub(settings_key, name), false).and_then(|k| k.get(device_key))?;
    u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()
}

fn set_dword(settings_key: &str, name: &str, device_key: &str, v: u32) -> bool {
    inst::create(HKCU, &sub(settings_key, name)).is_some_and(|k| k.set_dword(device_key, v))
}

pub fn hover_mode(settings_key: &str, device_key: &str) -> HoverMode {
    match dword(settings_key, "Hover", device_key) {
        Some(1) => HoverMode::Tip,
        Some(2) => HoverMode::Off,
        _ => HoverMode::Card,
    }
}

pub fn set_hover_mode(settings_key: &str, device_key: &str, m: HoverMode) -> bool {
    set_dword(settings_key, "Hover", device_key, match m {
        HoverMode::Card => 0,
        HoverMode::Tip => 1,
        HoverMode::Off => 2,
    })
}

/// The card shows the battery chart (default on).
pub fn card_chart(settings_key: &str, device_key: &str) -> bool {
    dword(settings_key, "CardChart", device_key) != Some(0)
}

pub fn set_card_chart(settings_key: &str, device_key: &str, on: bool) -> bool {
    set_dword(settings_key, "CardChart", device_key, on as u32)
}

/// The card shows the time left (default on).
pub fn card_time(settings_key: &str, device_key: &str) -> bool {
    dword(settings_key, "CardTime", device_key) != Some(0)
}

pub fn set_card_time(settings_key: &str, device_key: &str, on: bool) -> bool {
    set_dword(settings_key, "CardTime", device_key, on as u32)
}

/// "Battery only": nothing is measured or worked out for the device (no history, statistics,
/// time left or chart), only its level is shown.
pub fn battery_only(settings_key: &str, device_key: &str) -> bool {
    dword(settings_key, "BatteryOnly", device_key).is_some_and(|v| v != 0)
}

pub fn set_battery_only(settings_key: &str, device_key: &str, on: bool) -> bool {
    set_dword(settings_key, "BatteryOnly", device_key, on as u32)
}

/// Phones, experimental: a rising level counts as charging (until it falls again), for phones
/// whose charging state BatteryHub cannot read.
pub fn rise_charging(settings_key: &str, device_key: &str) -> bool {
    dword(settings_key, "RiseCharging", device_key).is_some_and(|v| v != 0)
}

pub fn set_rise_charging(settings_key: &str, device_key: &str, on: bool) -> bool {
    set_dword(settings_key, "RiseCharging", device_key, on as u32)
}

// ---- "Icon colour" ---------------------------------------------------------------------------

/// Colour of a device's icon on the taskbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconColor {
    /// The taskbar's text colour (default).
    Taskbar,
    /// The colour of the device's brand (e.g. Razer green), when BatteryHub knows it.
    Brand,
    Rgb(u8, u8, u8),
}

/// Ready-made colours of the "Icon colour" menu.
pub const PRESETS: [(&str, (u8, u8, u8)); 6] = [
    (n_("Green"), (46, 204, 64)),
    (n_("Blue"), (0, 120, 215)),
    (n_("Cyan"), (0, 183, 195)),
    (n_("Purple"), (155, 89, 182)),
    (n_("Pink"), (232, 17, 123)),
    (n_("Orange"), (247, 99, 12)),
];

/// The brand colour of a vendor id, when known.
pub fn brand_rgb(vid: u16) -> Option<(u8, u8, u8)> {
    Some(match vid {
        0x1532 => (68, 214, 44),           // Razer green
        0x046D => (0, 184, 252),           // Logitech G blue
        0x1038 => (255, 82, 0),            // SteelSeries orange
        0x03F0 | 0x0951 => (227, 24, 55),  // HyperX red
        0x1B1C => (236, 232, 26),          // Corsair yellow
        0x0B05 => (255, 0, 41),            // ASUS ROG red
        0x054C => (0, 112, 209),           // PlayStation blue
        0x045E => (16, 124, 16),           // Xbox green
        _ => return None,
    })
}

/// The colour to draw the icon in; None = the taskbar's text colour.
pub fn icon_rgb(c: IconColor, vid: u16) -> Option<(u8, u8, u8)> {
    match c {
        IconColor::Taskbar => None,
        IconColor::Brand => brand_rgb(vid),
        IconColor::Rgb(r, g, b) => Some((r, g, b)),
    }
}

fn encode_color(c: IconColor) -> u32 {
    match c {
        IconColor::Taskbar => 0,
        IconColor::Brand => 1,
        IconColor::Rgb(r, g, b) => 0x0100_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32,
    }
}

fn decode_color(v: u32) -> IconColor {
    match v {
        1 => IconColor::Brand,
        v if v & 0x0100_0000 != 0 => IconColor::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
        _ => IconColor::Taskbar,
    }
}

pub fn icon_color(settings_key: &str, device_key: &str) -> IconColor {
    inst::open(HKCU, &sub(settings_key, "IconColor"), false)
        .and_then(|k| k.get(device_key))
        .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        .map(decode_color)
        .unwrap_or(IconColor::Taskbar)
}

pub fn set_icon_color(settings_key: &str, device_key: &str, c: IconColor) -> bool {
    inst::create(HKCU, &sub(settings_key, "IconColor")).is_some_and(|k| k.set_dword(device_key, encode_color(c)))
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

/// "Lock device positions" is on (the devices' Move left / right are greyed).
pub static ORDER_LOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
const DEVICE_SUBKEYS: [&str; 12] =
    ["Order", "Names", "Hidden", "Thresholds", "Warn5", "RedIcon", "IconColor", "Hover", "CardChart", "CardTime", "BatteryOnly", "RiseCharging"];

/// Deletes every setting BatteryHub keeps for one device (name, order, warning levels, colours,
/// hover, when it was last seen and what it was): "Delete device" in the dashboard.
pub fn forget_device(settings_key: &str, device_key: &str) {
    for name in DEVICE_SUBKEYS.iter().chain(["Seen", "Known"].iter()) {
        if let Some(k) = inst::open(HKCU, &sub(settings_key, name), true) {
            k.delete_value(device_key);
        }
    }
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
const STRIDE: usize = 48;
const ITEM_DASHBOARD: usize = 0;
const ITEM_HISTORY: usize = 1;
const ITEM_STATS: usize = 2;
const ITEM_RENAME: usize = 3;
const ITEM_RESET_NAME: usize = 4;
const ITEM_WARN5: usize = 5;
const ITEM_SHOW: usize = 6;
const ITEM_RED_ICON: usize = 7;
const ITEM_THRESHOLD: usize = 8; // + index into THRESHOLDS
const ITEM_MOVE_BACK: usize = 20;
const ITEM_MOVE_ON: usize = 21;
const ITEM_RESET_HISTORY: usize = 22;
const ITEM_COLOR_TASKBAR: usize = 23;
const ITEM_COLOR_BRAND: usize = 24;
const ITEM_COLOR_CUSTOM: usize = 25;
const ITEM_COLOR_PRESET: usize = 26; // + index into PRESETS
const ITEM_HOVER_CARD: usize = 32;
const ITEM_HOVER_TIP: usize = 33;
const ITEM_HOVER_OFF: usize = 34;
const ITEM_CARD_CHART: usize = 35;
const ITEM_CARD_TIME: usize = 36;
const ITEM_BATTERY_ONLY: usize = 37;
const ITEM_RISE_CHARGING: usize = 38;
const _: () = assert!(ITEM_COLOR_PRESET + PRESETS.len() <= ITEM_HOVER_CARD && ITEM_BATTERY_ONLY < STRIDE);
const _: () = assert!(ITEM_THRESHOLD + THRESHOLDS.len() <= ITEM_MOVE_BACK && ITEM_RESET_HISTORY < STRIDE);

/// What a device menu item asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceAction {
    /// Open the dashboard on a tab (0 overview, 1 history, 2 statistics).
    Dashboard(u8),
    Rename,
    ResetName,
    ToggleWarn5,
    ToggleRedIcon,
    /// Icon colour; CustomColor asks for one first.
    IconColor(IconColor),
    CustomColor,
    ToggleShown,
    Hover(HoverMode),
    ToggleCardChart,
    ToggleCardTime,
    ToggleBatteryOnly,
    ToggleRiseCharging,
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
            let only = battery_only(settings_key, &d.key);
            for (item, label, g) in [
                (ITEM_DASHBOARD, n_("Dashboard..."), glyph::DASHBOARD),
                (ITEM_HISTORY, n_("Battery history"), glyph::HISTORY),
                (ITEM_STATS, n_("Statistics"), glyph::STATISTICS),
            ] {
                let grey = if only && item != ITEM_DASHBOARD { MF_GRAYED } else { MF_ENABLED };
                let _ = AppendMenuW(sub, MF_STRING | grey, id(item), &hs(&tr(label)));
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
            let check = if red_icon(settings_key, &d.key) { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(sub, MF_STRING | check, id(ITEM_RED_ICON), &hs(&tr("Red icon at the warning level")));
            icons.set(sub, id(ITEM_RED_ICON) as u32, false, glyph::WARNING);
            // Icon colour: the taskbar's, the brand's, a ready-made one or the user's own.
            if let Ok(colors) = CreatePopupMenu() {
                let current = icon_color(settings_key, &d.key);
                let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
                let brand = if brand_rgb(d.vid).is_some() { MF_ENABLED } else { MF_GRAYED };
                let _ = AppendMenuW(colors, MF_STRING | check(current == IconColor::Taskbar), id(ITEM_COLOR_TASKBAR), &hs(&tr("Taskbar colour")));
                let _ = AppendMenuW(colors, MF_STRING | brand | check(current == IconColor::Brand), id(ITEM_COLOR_BRAND), &hs(&tr("Brand colour")));
                let _ = AppendMenuW(colors, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                let mut preset = false;
                for (j, (name, (r, g, b))) in PRESETS.iter().enumerate() {
                    let on = current == IconColor::Rgb(*r, *g, *b);
                    preset |= on;
                    let _ = AppendMenuW(colors, MF_STRING | check(on), id(ITEM_COLOR_PRESET + j), &hs(&tr(name)));
                }
                let _ = AppendMenuW(colors, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                let own = matches!(current, IconColor::Rgb(..)) && !preset;
                let _ = AppendMenuW(colors, MF_STRING | check(own), id(ITEM_COLOR_CUSTOM), &hs(&tr("Custom colour...")));
                let n = windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(sub));
                let _ = AppendMenuW(sub, MF_STRING | MF_POPUP, colors.0 as usize, &hs(&tr("Icon colour")));
                icons.set(sub, n as u32, true, glyph::COLOR);
            }
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let shown = !hidden.contains(&d.key);
            let check = if shown { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(sub, MF_STRING | check, id(ITEM_SHOW), &hs(&tr("Show on the taskbar")));
            icons.set(sub, id(ITEM_SHOW) as u32, false, glyph::VIEW);
            // What hovering the device shows, and what its card holds.
            if let Ok(hover) = CreatePopupMenu() {
                let mode = hover_mode(settings_key, &d.key);
                let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
                for (item, label, m) in [
                    (ITEM_HOVER_CARD, n_("Card with details"), HoverMode::Card),
                    (ITEM_HOVER_TIP, n_("Only the name and level (tooltip)"), HoverMode::Tip),
                    (ITEM_HOVER_OFF, n_("Nothing"), HoverMode::Off),
                ] {
                    let _ = AppendMenuW(hover, MF_STRING | check(mode == m), id(item), &hs(&tr(label)));
                }
                let _ = AppendMenuW(hover, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                let card = if mode == HoverMode::Card && !only { MF_ENABLED } else { MF_GRAYED };
                let chart = card_chart(settings_key, &d.key) && !only;
                let time = card_time(settings_key, &d.key) && !only;
                let _ = AppendMenuW(hover, MF_STRING | card | check(chart), id(ITEM_CARD_CHART), &hs(&tr("Battery chart on the card")));
                let _ = AppendMenuW(hover, MF_STRING | card | check(time), id(ITEM_CARD_TIME), &hs(&tr("Time left on the card")));
                let n = windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(sub));
                let _ = AppendMenuW(sub, MF_STRING | MF_POPUP, hover.0 as usize, &hs(&tr("On hover")));
                icons.set(sub, n as u32, true, glyph::INFO);
            }
            let check = if only { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(sub, MF_STRING | check, id(ITEM_BATTERY_ONLY), &hs(&tr("Battery only (no history, statistics or forecast)")));
            icons.set(sub, id(ITEM_BATTERY_ONLY) as u32, false, glyph::STATISTICS);
            // Phones only: their charging state is often unknown.
            if d.kind == DeviceKind::Phone {
                let check = if rise_charging(settings_key, &d.key) { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(sub, MF_STRING | check, id(ITEM_RISE_CHARGING), &hs(&tr("Rising level means charging (experimental)")));
                icons.set(sub, id(ITEM_RISE_CHARGING) as u32, false, glyph::REFRESH);
            }
            // Place on the strip (the devices can also be dragged there).
            let (back, on) = if vertical {
                ((n_("Move up"), glyph::UP), (n_("Move down"), glyph::DOWN))
            } else {
                ((n_("Move left"), glyph::LEFT), (n_("Move right"), glyph::RIGHT))
            };
            // Not while "Lock device positions" is on.
            let free = !ORDER_LOCKED.load(std::sync::atomic::Ordering::Relaxed);
            for (item, (label, g), enabled) in [(ITEM_MOVE_BACK, back, free && i > 0), (ITEM_MOVE_ON, on, free && i + 1 < devices.len())] {
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
        ITEM_RED_ICON => DeviceAction::ToggleRedIcon,
        ITEM_COLOR_TASKBAR => DeviceAction::IconColor(IconColor::Taskbar),
        ITEM_COLOR_BRAND => DeviceAction::IconColor(IconColor::Brand),
        ITEM_COLOR_CUSTOM => DeviceAction::CustomColor,
        i if (ITEM_COLOR_PRESET..ITEM_COLOR_PRESET + PRESETS.len()).contains(&i) => {
            let (r, g, b) = PRESETS[i - ITEM_COLOR_PRESET].1;
            DeviceAction::IconColor(IconColor::Rgb(r, g, b))
        }
        ITEM_SHOW => DeviceAction::ToggleShown,
        ITEM_HOVER_CARD => DeviceAction::Hover(HoverMode::Card),
        ITEM_HOVER_TIP => DeviceAction::Hover(HoverMode::Tip),
        ITEM_HOVER_OFF => DeviceAction::Hover(HoverMode::Off),
        ITEM_CARD_CHART => DeviceAction::ToggleCardChart,
        ITEM_CARD_TIME => DeviceAction::ToggleCardTime,
        ITEM_BATTERY_ONLY => DeviceAction::ToggleBatteryOnly,
        ITEM_RISE_CHARGING => DeviceAction::ToggleRiseCharging,
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
        assert_eq!(decode_menu_id(MENU_BASE + 2 * STRIDE + ITEM_HOVER_TIP), Some((2, DeviceAction::Hover(HoverMode::Tip))));
        assert_eq!(decode_menu_id(MENU_BASE + STRIDE + ITEM_BATTERY_ONLY), Some((1, DeviceAction::ToggleBatteryOnly)));
        assert_eq!(decode_menu_id(MENU_BASE + ITEM_RED_ICON), Some((0, DeviceAction::ToggleRedIcon)));
        assert_eq!(decode_menu_id(MENU_BASE + ITEM_THRESHOLD + THRESHOLDS.len()), None);
    }

    #[test]
    fn icon_colors() {
        for c in [IconColor::Taskbar, IconColor::Brand, IconColor::Rgb(0, 0, 0), IconColor::Rgb(12, 34, 56)] {
            assert_eq!(decode_color(encode_color(c)), c);
        }
        assert_eq!(icon_rgb(IconColor::Brand, 0x1532), Some((68, 214, 44)));
        assert_eq!(icon_rgb(IconColor::Brand, 0x3554), None);
        assert_eq!(decode_menu_id(MENU_BASE + ITEM_COLOR_PRESET + 1), Some((0, DeviceAction::IconColor(IconColor::Rgb(0, 120, 215)))));
        assert_eq!(decode_menu_id(MENU_BASE + STRIDE + ITEM_COLOR_CUSTOM), Some((1, DeviceAction::CustomColor)));
    }

    #[test]
    fn each_bud_low_on_its_own() {
        let mut s = DeviceStatus { connected: true, percent: 10, ..Default::default() };
        s.buds = Some(crate::device::Buds { left: 100, right: 10, left_charging: false, right_charging: false, case: None });
        assert_eq!(buds_low(&s, 15), Some((false, true)));
        s.buds = Some(crate::device::Buds { left: 8, right: -1, left_charging: true, right_charging: false, case: None });
        assert_eq!(buds_low(&s, 15), Some((false, false))); // charging, and not reported
        s.connected = false;
        s.buds = Some(crate::device::Buds { left: 5, right: 5, left_charging: false, right_charging: false, case: None });
        assert_eq!(buds_low(&s, 15), Some((false, false)));
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
