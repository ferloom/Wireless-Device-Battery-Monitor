//! BatteryHub Taskbar. Runs in the background (the installer can add it to autostart)
//! and draws "🎧 25%  🖱 32%" straight onto the taskbar, left of the notification area, so no
//! hovering is needed. Levels reload every minute. Left click does nothing; right click opens the
//! menu (warning levels, refresh options, exit). When the taskbar cannot host the strip (vertical
//! taskbar, unknown Explorer layout) it falls back to one notification-area icon per device.
#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Instant;

use batteryhub::cardfx::{BlurScratch, CardBg, CardContent, Layer, compose_card};
use batteryhub::device::Missing;
use batteryhub::i18n::{n_, tr, trf};
use batteryhub::settings;
use batteryhub::stats::fmt_hours;
use batteryhub::ui::{self, rgb, split};
use batteryhub::{DeviceKind, DeviceStatus};
use windows::Win32::Foundation::{COLORREF, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::Controls::{
    TOOLTIPS_CLASSW, TTF_ABSOLUTE, TTF_TRACK, TTM_ADDTOOLW, TTM_GETBUBBLESIZE, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

const WM_POLL_DONE: u32 = WM_APP + 2;
const WM_TRAY: u32 = WM_APP + 3;
const WM_QUICK_DONE: u32 = WM_APP + 4;
const WM_UPDATE_CHECKED: u32 = WM_APP + 5;
const WM_UPDATE_DOWNLOADED: u32 = WM_APP + 6;
const WM_SHOW_UPDATED: u32 = WM_APP + 7;
/// tray_probe found where a secondary taskbar's notification area begins.
const WM_TRAY_PROBED: u32 = WM_APP + 8;
/// A pair of same-named connections was found: ask once whether to join them.
const WM_SAME_NAME_PROMPT: u32 = WM_APP + 10;
/// Another window came to the front: copies over a full-screen window step aside at once.
const WM_FOREGROUND: u32 = WM_APP + 20;
/// A background write of the history finished (wparam: 1 = written).
const WM_HISTORY_SAVED: u32 = WM_APP + 9;
// Not exported by the windows crate's WindowsAndMessaging module.
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_POLL: usize = 1;
const TIMER_PLACE: usize = 2;
const TIMER_DEVCHANGE: usize = 3;
const TIMER_BTWATCH: usize = 4;
const TIMER_QUICK: usize = 5;
const TIMER_AUTO_UPDATE: usize = 6;
/// "Update finished" message shortly after the first start of an updated version.
const TIMER_UPDATED: usize = 7;
/// Frames of the hover card's fade / slide (~60 per second while it runs).
const TIMER_CARD: usize = 20;
/// Removes the low-battery notification's temporary icon.
const TIMER_NOTIFY: usize = 21;
/// While the mouse is off the strip with the card open: is it on the card, or gone?
const TIMER_AWAY: usize = 22;
/// Devices sliding to their new places on the strip.
const TIMER_SLIDE: usize = 23;
/// Frames of a strip popping in as it appears on a monitor, or out as it leaves one.
const TIMER_MIRROR_IN: usize = 24;
/// Frames of the widget gliding to a new place or height (GLIDE_MS).
const TIMER_GLIDE: usize = 25;
/// "Widget position: left": the taskbar's left end checked again soon after a window opens or
/// closes (its task button comes or goes), LEFT_PROBES times.
const TIMER_LEFT_PROBE: usize = 26;
/// Frames of the charging animation (green light passing over charging devices).
const TIMER_CHARGE: usize = 27;
const TIMER_BADGE: usize = 28;
/// Frames of the ring's charging plug / unplug animation.
const TIMER_RING_CHARGE: usize = 29;
const CHARGE_FRAME_MS: u32 = 40;
/// Length of the show / hide animation of the connection badges.
const BADGE_MS: f32 = 220.0;
/// How long the top of the ring moves aside for the charging bolt (and back on unplug).
const RING_CHARGE_MS: f32 = 1000.0;
/// Fraction of the ring removed at the top while it is on power.
const RING_GAP: f32 = 0.2;
/// One pass of the charging animation over a device.
const CHARGE_SWEEP_MS: f32 = 1800.0;
const LEFT_PROBES: u8 = 4;
/// How old the taskbar's items may be when checking room for the widget: next to the icons they
/// move all the time; next to the clock only a window opening or closing changes the room (the
/// shell hook marks the answer stale then), so this is only a fallback.
const ROOM_LEFT_AGE: std::time::Duration = std::time::Duration::from_secs(2);
const ROOM_AGE: std::time::Duration = std::time::Duration::from_secs(60);
/// Length of that glide.
const GLIDE_MS: f32 = 260.0;
/// Length of that pop-in.
const MIRROR_IN_MS: f32 = 320.0;
/// How long a device takes to slide to its new place (ms).
const SLIDE_MS: f32 = 220.0;
/// An item appearing on the strip first opens its space (the others move aside), then pops in;
/// one leaving pops out, then its space closes. Durations in ms.
const SPACE_MS: f32 = 200.0;
const POP_MS: f32 = 260.0;
/// Items appearing together pop in one after another, this far apart (ms).
const STAGGER_MS: u64 = 70;
/// Key of the "no devices found" glyph on the strip.
const NONE_KEY: &str = "\u{0}none";
/// "Rising level means charging": a phone whose level stays the same this long is not charging.
const RISE_FLAT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// The refresh glyph's item while a refresh started from the menu runs.
const REFRESH_KEY: &str = "\u{0}refresh";

/// An item appearing on or leaving the strip.
struct Presence {
    leaving: bool,
    /// When it started (later than now while it waits for its turn).
    at: Instant,
}

impl Presence {
    /// (share of its width it takes 0-1, how far it has popped in 0-1, finished).
    fn frame(&self) -> (f32, f32, bool) {
        let t = self.at.elapsed().as_secs_f32() * 1000.0;
        let ease = |v: f32| {
            let v = v.clamp(0.0, 1.0);
            v * v * (3.0 - 2.0 * v)
        };
        if self.leaving {
            (1.0 - ease((t - POP_MS) / SPACE_MS), 1.0 - (t / POP_MS).clamp(0.0, 1.0), t >= POP_MS + SPACE_MS)
        } else {
            (ease(t / SPACE_MS), ((t - SPACE_MS) / POP_MS).clamp(0.0, 1.0), t >= SPACE_MS + POP_MS)
        }
    }

    /// Turns around halfway (a device that comes back while leaving, or goes while appearing),
    /// from about where it is now.
    fn turn(&mut self, leaving: bool) {
        if self.leaving == leaving {
            return;
        }
        let (f, p, _) = self.frame();
        let t = match (leaving, p > 0.0) {
            (true, true) => POP_MS * (1.0 - p),
            (true, false) => POP_MS + SPACE_MS * (1.0 - f),
            (false, true) => SPACE_MS + POP_MS * p,
            (false, false) => SPACE_MS * f,
        };
        self.leaving = leaving;
        self.at = Instant::now().checked_sub(std::time::Duration::from_secs_f32(t / 1000.0)).unwrap_or_else(Instant::now);
    }
}

/// One item on the strip this frame.
#[derive(Clone)]
struct Item {
    key: String,
    /// The device (None for the "no devices found" glyph).
    dev: Option<DeviceStatus>,
    /// Its place among the visible devices (hit-testing); None while it leaves.
    vis: Option<usize>,
    /// Share of its width it takes, and how far it has popped in.
    f: f32,
    p: f32,
}
/// Notification-area id of the low-battery notification (device icons use hashes of their keys).
const NOTIFY_UID: u32 = 0x5BA7;
/// The app's own notification-area icon, shown while no device is and the empty-strip icon is off.
const APP_UID: u32 = 0x5BA8;
const CARD_SHOW_MS: f32 = 160.0;
const CARD_HIDE_MS: f32 = 110.0;
/// How long the mouse may be off both the strip and the card before the card closes.
const CARD_AWAY_MS: u128 = 300;
/// Old and new card blending when the mouse moves to another device (like the taskbar's previews).
const CARD_CROSS_MS: f32 = 130.0;
/// Strongest blur of the card's content during its animations (px at 100 % scaling).
const BLUR_MAX: f32 = 6.0;
/// Automatic update check this long after start-up; when the network is not up yet (e.g. right
/// at sign-in) it is tried again every AUTO_UPDATE_RETRY_MS, at most AUTO_UPDATE_RETRIES times.
const AUTO_UPDATE_DELAY_MS: u32 = 2_000;
const AUTO_UPDATE_RETRY_MS: u32 = 15_000;
const AUTO_UPDATE_RETRIES: u32 = 8;
/// The right-click menu is open or being built: the strip shows no hover card, and — more
/// importantly — must not pop out, hide or move while it owns the menu. A transient "no room"
/// answer used to hide the owner, which dismissed the menu and left the widget seemingly dead
/// until the next placement tick put it back.
static MENU_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Guards `MENU_OPEN` for the whole life of one menu: ignores a second right-click while a menu is
/// already up (nested menus also confused the owner-drawn menu style) and always clears the flag,
/// even if opening the menu returns early.
struct MenuGuard;

impl MenuGuard {
    fn enter() -> Option<Self> {
        (!MENU_OPEN.swap(true, std::sync::atomic::Ordering::SeqCst)).then_some(Self)
    }
}

impl Drop for MenuGuard {
    fn drop(&mut self) {
        MENU_OPEN.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

static AUTO_UPDATE_TRIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// How often receivers whose device is off or asleep are checked again.
const QUICK_MS: u32 = 10_000;
const WM_DEVICECHANGE_MSG: u32 = 0x0219;
const DBT_DEVNODES_CHANGED: usize = 0x0007;
const DBT_DEVICEARRIVAL: usize = 0x8000;
const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;
/// Seconds the strip may be impossible to place before falling back to notification-area icons.
const FALLBACK_AFTER_TICKS: u32 = 3;
/// A tray-icon change lasting one tick must not notify or switch modes (#609).
const CROWDED_CONFIRM_TICKS: u32 = 3;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
const PBT_APMSUSPEND: usize = 0x0004;
/// Battery refresh intervals offered in the menu, in minutes.
const INTERVALS: [(u32, &str); 5] = [(1, n_("1 minute")), (5, n_("5 minutes")), (10, n_("10 minutes")), (30, n_("30 minutes")), (60, n_("1 hour"))];
/// 30..34: menu ids 10 / 11 belong to ID_ICONS_MODE / ID_VERSIONS (issue #171).
const ID_INTERVAL_BASE: usize = 30;
const ID_ON_CHANGE: usize = 20;
const MIN_GRACE_SEC: u64 = 150;
const ID_EXIT: usize = 1;
const ID_REFRESH_DEVICES: usize = 2;
const ID_REFRESH_LEVELS: usize = 3;
const ID_CHECK_UPDATES: usize = 4;
const ID_REQUEST: usize = 5;
const ID_AUTOSTART: usize = 6;
const ID_AUTO_UPDATE: usize = 7;
const ID_BACKGROUND: usize = 8;
const ID_SILENT_UPDATE: usize = 9;
const ID_ICONS_MODE: usize = 10;
const ID_VERSIONS: usize = 11;
const ID_DASHBOARD: usize = 12;
const ID_COMPACT: usize = 13;
const _: () = assert!(ID_INTERVAL_BASE > ID_ON_CHANGE && ID_INTERVAL_BASE + INTERVALS.len() <= ID_MONITOR_ALL, "menu ids overlap");
/// "Show widget on": all monitors, main monitor only, or ID_MONITOR_FIRST + i for one other monitor.
const ID_MONITOR_ALL: usize = 40;
const ID_MONITOR_MAIN: usize = 41;
const ID_MONITOR_FIRST: usize = 42;
const ID_MONITOR_GONE: usize = 73;
/// "Position on the taskbar": ID_POSITION_FIRST + i for POSITION_OFFSETS[i].
const ID_POSITION_FIRST: usize = 80;
const ID_POSITION_CUSTOM: usize = 95;
/// 10 px further left / right (any value, not only the listed ones).
const ID_POSITION_LEFT: usize = 96;
const ID_POSITION_RIGHT: usize = 97;
/// "Widget position": next to the clock (right) or next to the taskbar icons (left).
const ID_SIDE_RIGHT: usize = 98;
const ID_SIDE_LEFT: usize = 99;
/// "Widget position: next to the icons": right after the task buttons, wherever they are.
const ID_SIDE_ICONS: usize = 118;
/// "Widget position: Before the icons" (centred taskbar icons only).
const ID_SIDE_BEFORE: usize = 119;
/// "Position on the taskbar: Change position": the widget follows the mouse until a click.
const ID_POSITION_MOVE: usize = 211;
/// "Mirror other widgets": every taskbar's widget alike, or each with its own place and order.
const ID_MIRRORING: usize = 212;
/// "Lock device positions": no dragging or moving the devices on the strip.
const ID_LOCK_ORDER: usize = 213;
/// "Charging animation" and "Ring style".
const ID_CHARGE_ANIM: usize = 214;
const ID_RING: usize = 215;
/// "Show connection icons": a small dongle / Bluetooth / Wi-Fi badge next to each device.
const ID_CONNECTION_ICONS: usize = 216;

/// Widget positions (registry WidgetLeft): next to the clock, at the left end, right after the
/// taskbar icons, right before them (centred icons), or where it was put with the mouse.
const SIDE_RIGHT: u32 = 0;
const SIDE_LEFT: u32 = 1;
const SIDE_ICONS: u32 = 2;
const SIDE_BEFORE: u32 = 3;
const SIDE_FREE: u32 = 4;
/// "Help": the GitHub page and ID_HELP_RELEASE_FIRST + i for the latest releases.
const ID_HELP_GITHUB: usize = 200;
const ID_HELP_RELEASE_FIRST: usize = 201;
const HELP_RELEASES: usize = 5;
const ID_HELP_DISCORD: usize = 207;
/// The project's Discord server (a permanent invite).
const DISCORD_URL: &str = "https://discord.gg/aVp2E7a9tb";
/// "BatteryHub is free and will stay free": opens the page for a tip.
const ID_HELP_FREE: usize = 208;
const SUPPORT_URL: &str = "https://revolut.me/p2ps";
/// "Show the icon when no device is connected".
const ID_SHOW_EMPTY: usize = 210;
const POSITION_MAX: u32 = 1000;
const POSITION_OFFSETS: [u32; 11] = [0, 20, 40, 60, 80, 100, 120, 140, 160, 180, 200];
/// "Height on the taskbar": ID_HEIGHT_FIRST + i for HEIGHT_OFFSETS[i] (px, + = down).
const ID_HEIGHT_FIRST: usize = 100;
const ID_HEIGHT_CUSTOM: usize = 115;
const ID_HEIGHT_UP: usize = 116;
const ID_HEIGHT_DOWN: usize = 117;
const HEIGHT_MAX: i32 = 40;
const HEIGHT_OFFSETS: [i32; 7] = [-15, -10, -5, 0, 5, 10, 15];
/// "Language": Windows' language, create a new one, open the folder, or ID_LANG_FIRST + i.
const ID_LANG_AUTO: usize = 120;
const ID_LANG_NEW: usize = 121;
const ID_LANG_FOLDER: usize = 122;
const ID_LANG_FIRST: usize = 130;
const ID_LANG_LAST: usize = 199;
const _: () = assert!(
    ID_POSITION_RIGHT < ID_HEIGHT_FIRST && ID_HEIGHT_DOWN < ID_LANG_AUTO && ID_HEIGHT_CUSTOM < ID_LANG_AUTO && ID_LANG_LAST < settings::MENU_BASE,
    "menu ids overlap"
);

pub const MAIN_CLASS: windows::core::PCWSTR = w!("BatteryHub.Taskbar.Main");
const STRIP_CLASS: windows::core::PCWSTR = w!("BatteryHub.Taskbar.Strip");
const SETTINGS_KEY: &str = batteryhub::install::TASKBAR.settings_key;
/// Folder name of this edition's data (%LOCALAPPDATA%\BatteryHub\Taskbar).
const EDITION: &str = batteryhub::install::TASKBAR.short;

/// BH_PROFILE (measurements only): time spent on the hover card, written to the debug log
/// (%TEMP%h2.log, with BH_DEBUG) each time the card closes.
mod prof {
    use std::sync::atomic::{AtomicU64, Ordering};
    pub static FRAMES: AtomicU64 = AtomicU64::new(0);
    pub static COMPOSE_US: AtomicU64 = AtomicU64::new(0);
    pub static PRESENT_US: AtomicU64 = AtomicU64::new(0);
    pub static RENDERS: AtomicU64 = AtomicU64::new(0);
    pub static RENDER_US: AtomicU64 = AtomicU64::new(0);
    pub fn add(c: &AtomicU64, v: u64) {
        c.fetch_add(v, Ordering::Relaxed);
    }
    pub fn report() {
        if std::env::var_os("BH_PROFILE").is_none() {
            return;
        }
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let f = g(&FRAMES).max(1);
        super::dbg_log(&format!(
            "card profile: {} frames, compose {} us/frame, present {} us/frame, {} renders ({} us each)",
            g(&FRAMES),
            g(&COMPOSE_US) / f,
            g(&PRESENT_US) / f,
            g(&RENDERS),
            g(&RENDER_US) / g(&RENDERS).max(1)
        ));
    }
}

/// BH_DEMO (diagnostics and measurements only): two made-up devices on the strip, so the hover
/// card can be tried and measured without devices at hand. They are never written to the history.
fn demo_devices() -> Vec<DeviceStatus> {
    if std::env::var_os("BH_DEMO").is_none() {
        return Vec::new();
    }
    let dev = |key: &str, name: &str, kind: DeviceKind, percent: i32, charging: bool| DeviceStatus {
        key: key.into(),
        name: name.into(),
        via: "Demo (BH_DEMO)".into(),
        kind,
        percent,
        charging,
        ..Default::default()
    };
    vec![dev("demo:headset", "Demo headset", DeviceKind::Headset, 38, true), dev("demo:mouse", "Demo mouse with a longer name", DeviceKind::Mouse, 64, false)]
}

/// A dashboard window is open (it shows the history file, so that is kept current).
fn dashboard_open() -> bool {
    batteryhub::dashboard::any_open()
}

fn history_path() -> std::path::PathBuf {
    batteryhub::history::History::path(EDITION)
}

/// A notification-area icon to show: device key, text in the icon, colour, level, tooltip.
type TrayIcon = (String, String, COLORREF, i32, String);

struct Tracked {
    status: DeviceStatus,
    seen: Instant,
    /// The name the device reports (status.name is the user's name for it, if it has one).
    reported: String,
}

/// Ring power states, in the order the bolt colour moves through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RingPower {
    Off,
    Charging,
    Full,
}

/// One ring animation: the state it came from, the state it is going to, and when it started.
#[derive(Clone, Copy)]
struct RingAnim {
    at: Instant,
    from: RingPower,
    to: RingPower,
}

/// Blend two COLORREFs (stored as 0x00BBGGRR) channel by channel.
fn mix_color(a: COLORREF, b: COLORREF, t: f32) -> COLORREF {
    let (ar, ag, ab) = split(a);
    let (br, bg, bb) = split(b);
    let mix = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8 };
    rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

#[derive(Default)]
struct App {
    main: HWND,
    strip: HWND,
    taskbar: HWND,
    devices: BTreeMap<String, Tracked>,
    order: Vec<String>,
    polling: bool,
    loaded: bool,
    /// A refresh started from the menu: show the ↻ indicator until it finishes.
    manual: bool,
    /// "Refresh devices": forget every known device (no grace period) on the next result.
    forget: bool,
    /// Devices whose status line is saved for the dashboard this session: (name, kind, via).
    known_saved: HashMap<String, (String, u8, String)>,
    /// Whether the devices were enumerated at least once (the first read always enumerates).
    scanned: bool,
    /// The next poll was triggered by a device connecting / disconnecting: devices that are gone
    /// and have no sleep grace period disappear at once instead of after MIN_GRACE_SEC.
    devchange: bool,
    /// The main taskbar's widget: its render, hit ranges and animations.
    mv: View,
    /// The copies on other monitors (and the hidden ones kept ready), each rendered for its own
    /// taskbar: keyed by strip window.
    views: HashMap<isize, View>,
    /// Fonts for the other monitors' DPI scales (the main one's are `fonts`).
    fonts_more: Vec<Fonts>,
    /// Icon fonts of the ring style, per DPI scale.
    ring_fonts: Vec<(f32, HFONT)>,
    /// Strip the device being dragged belongs to.
    drag_on: HWND,
    /// Widget position (SIDE_*) and, for SIDE_FREE, its left edge (px at 100 %, from the
    /// taskbar's left end).
    side: u32,
    free_x: i32,
    /// "Mirror other widgets" (default on): every taskbar's widget has the main one's place and
    /// order. Off: each has its own (App.views[..].lay).
    mirroring: bool,
    /// "Lock device positions": the devices can't be dragged or moved.
    lock_order: bool,
    /// "Charging animation": a green light passes over charging devices (since charge_t0).
    charge_anim: bool,
    charge_t0: ChargeClock,
    /// "Ring style": each device as its icon inside a ring filled to its level.
    ring: bool,
    /// Ring power transitions: from which bolt colour to which, including 100% (blue) and unplug.
    ring_charge: HashMap<String, RingAnim>,
    /// Show a small connection badge (dongle, Bluetooth, Wi-Fi, cable) next to each device.
    connection_icons: bool,
    /// Toggling the badges: when it started and whether they are appearing (else disappearing).
    badge_at: Option<(Instant, bool)>,
    /// "Change position" running: (strip, where the mouse holds it, its layout before).
    moving: Option<(HWND, i32, Layout)>,
    /// Strip the right-click menu was opened on (its widget's settings are the ones changed).
    menu_on: HWND,
    /// Copies that lost their room during placement: they pop out (place_mirror_at).
    pop_out: RefCell<Vec<HWND>>,
    /// When each taskbar without room was last said (one notification per monitor, at most every
    /// 10 minutes).
    short_said: HashMap<isize, Instant>,
    /// Consecutive ticks each taskbar has been short, before saying so (#609).
    short_ticks: HashMap<isize, u32>,
    last_place: (RECT, RECT, i32),
    /// Main strip uses a topmost popup owned by `Shell_TrayWnd` (e.g. when Start11's SDTaskbar
    /// sits inside `Shell_TrayWnd` and fights child-window Z-order).
    strip_popup: bool,
    /// Main taskbar's notification area, to notice when it changes (see tray_probe).
    main_tray: RECT,
    /// Every copy on another monitor knows its place: the strips appear on all taskbars together.
    mirrors_ready: bool,
    /// Strips popping in or out (strip, start, leaving): copies on other monitors, and the main
    /// strip when the widget moves to or from another monitor. A copy that left is destroyed, the
    /// main strip hidden, once it has popped out.
    mirror_in: Vec<(HWND, Instant, bool)>,
    /// Hidden copies kept ready on taskbars that don't show the widget (taskbar, strip). A window
    /// created at the moment it is needed loses its first updates (its pop-in stalled, or it
    /// stayed blank), so a copy is made ahead of time and reused when the widget comes back.
    parked: Vec<(HWND, HWND)>,
    /// Hover tooltip (tracking tooltip owned by the main window) and what it shows right now.
    tip: HWND,
    /// Hover card (device, level, last 24 hours) and the levels it charts.
    card: HWND,
    card_anim: CardAnim,
    /// Rendered card contents of the last two devices shown (moving back and forth between two
    /// devices reuses them instead of drawing the text and chart again).
    card_cache: Vec<Rc<CardContent>>,
    /// The card's bitmap, kept while the card is open (one allocation, not one per frame).
    card_surface: Option<Surface>,
    /// Scratch buffers of the card's blur, kept while the card is open.
    blur_scratch: BlurScratch,
    /// Hour picked on the card's chart with the mouse (device key, time): its level and change
    /// are shown above the chart.
    card_scrub: Option<(String, u64)>,
    /// Top and bottom of the card's chart (card pixels), to map the mouse to a time.
    card_chart_y: (i32, i32),
    /// Since when the mouse is on neither the strip nor the card: the card closes shortly after,
    /// so the mouse can move from the strip onto it.
    card_away: Option<Instant>,
    /// A card opened by a click: (device key, strip). Another click or the menu closes it; the
    /// mouse leaving both the strip and the card closes it only in the "Away" mode (#439).
    card_pinned: Option<(String, HWND)>,
    /// Which of the two ways above the pinned card uses (the default is "click elsewhere").
    card_dismiss: settings::CardDismiss,
    /// Mouse hook while a "click elsewhere" card is open.
    click_hook: Option<HHOOK>,
    /// The left button moved far enough to be a drag, not a click.
    drag_moved: bool,
    /// Fonts of the strip and the card for the current DPI scale.
    fonts: Option<Fonts>,
    history: batteryhub::history::History,
    /// Last time the history was written.
    history_saved: Option<Instant>,
    tip_for: Option<usize>,
    tracking: bool,
    taskbar_created: u32,
    /// (taskbar client height, DPI, light theme) the strip was last rendered for; a change (e.g.
    /// after leaving full screen, a DPI or theme change) triggers a fresh render.
    painted_for: (i32, u32, bool),
    /// Notification-area icons instead of the strip (the taskbar cannot host it right now).
    fallback: bool,
    /// The taskbar is on the left or right edge: the strip is a column (icon over level, devices
    /// stacked) above the notification area, and hit ranges are along y.
    vertical: bool,
    /// Consecutive seconds the strip could not be placed.
    fail_ticks: u32,
    /// Fallback icons: device key -> (icon id, icon handle).
    tray: BTreeMap<String, (u32, HICON)>,
    /// Last Bluetooth connection signature (connect / disconnect detection).
    bt_sig: u64,
    /// A device change arrived while a read was running: read again as soon as it finishes.
    pending_change: bool,
    /// Receivers / cables (vid, pid) whose device reported "off" in the last read.
    offline_hw: HashSet<(u16, u16)>,
    /// Light taskbar theme and the optional extra gap (TaskbarOffset), read once and again only
    /// when Windows reports a settings change - not from the registry every second.
    light: bool,
    offset: u32,
    /// Up / down shift of the widget in px (TaskbarOffsetY, + = down).
    offset_y: i32,
    /// The height shift gliding to offset_y: (shift it started from, when).
    y_glide: Option<(f32, Instant)>,
    /// The strip gliding to a new place (taskbar client coordinates): (from, to, when).
    glide: Option<(POINT, POINT, Instant)>,
    /// Copies on other monitors gliding to a new place (screen coordinates): (copy, from, to, when).
    mirror_glides: RefCell<Vec<(HWND, POINT, POINT, Instant)>>,
    /// Checks of the taskbar's left end left after a window opened or closed.
    left_probes: u8,
    /// Per taskbar: whether the widget fits there without covering a task button (false: hidden
    /// there). Taskbars missing here have room as far as known.
    rooms: RefCell<HashMap<isize, bool>>,
    /// No taskbar the widget is shown on has room: the levels are notification-area icons.
    crowded: bool,
    /// Consecutive ticks with the opposite room answer, before switching modes (#609).
    crowded_ticks: u32,
    /// Devices whose last reading dropped to 0 % at once, not yet confirmed (see doubt_zero).
    zero_once: HashSet<String>,
    /// Phones with "Rising level means charging": (last level, charging as worked out, when the
    /// level last changed).
    rising: HashMap<String, (i32, bool, Instant)>,
    /// When "not enough room" was last said (once per crowding, at most every 10 minutes).
    crowded_said: Option<Instant>,
    /// "SHELLHOOK" message: windows opening and closing.
    shell_msg: u32,
    /// Copies of the strip on the taskbars of other monitors ("Show widget on"): (taskbar, strip).
    mirrors: Vec<(HWND, HWND)>,
    /// Strip the tooltip belongs to (the main strip or a mirror).
    tip_on: HWND,
    /// Strip under the mouse: drawn with the taskbar's hover plate.
    hovered: HWND,
    /// "Show widget on": "all", "main" or the id of one other monitor (cached setting).
    choice: String,
    /// The widget is shown only on another monitor: the main taskbar's strip is hidden.
    primary_hidden: bool,
    /// Plate behind the widget for transparent taskbars (cached setting).
    background: bool,
    /// "Show as notification-area icons": Windows places and orders the icons (no strip).
    icons_mode: bool,
    /// "Widget position: left": next to the taskbar icons instead of the clock (horizontal taskbars).
    align_left: bool,
    /// With align_left: right after the last task button (also centred ones), not at the left end.
    after_icons: bool,
    /// "Show the icon when no device is connected" turned off: nothing on the strip then, and the
    /// app's own notification-area icon (APP_UID) holds the menu.
    hide_empty: bool,
    app_icon: Option<HICON>,
    compact: bool,
    /// Version the setup just updated from (shown once in "Update finished").
    updated_from: Option<String>,
    /// Names chosen with "Rename device" (device key -> name).
    names: std::collections::HashMap<String, String>,
    /// Connections joined into one device (#644): secondary key -> key that keeps the readings.
    merged: std::collections::HashMap<String, String>,
    /// Same-named pair found in the latest read, waiting for the one-time offer on the UI thread.
    duplicate_prompt: Option<(String, String)>,
    /// Keys of the devices with "Show on the taskbar" turned off.
    hidden: HashSet<String>,
    /// "Warn at 5%" state per device.
    critical: settings::CriticalWarnings,
    /// Icon of the low-battery notification while it is shown.
    notify_icon: Option<HICON>,
    /// Registered message: a device was renamed.
    names_msg: u32,
    /// Device order chosen by the user (dragged on the strip, or "Move left / right").
    saved_order: Vec<String>,
    /// Key of the device being dragged along the strip (left button held).
    drag: Option<String>,
    /// While dragging: the mouse along the strip, and where in the device it was grabbed.
    drag_at: i32,
    grab: i32,
    /// The order changed since the last render: devices slide from where they were drawn (handed
    /// to every widget at its next render).
    reordered: bool,
    /// When each device was last written as seen, and when unseen devices were last forgotten.
    seen_saved: std::collections::HashMap<String, u64>,
    forgot_at: u64,
    /// Items appearing on or leaving the strip, and the items of the last render (with the
    /// device as it was, so one that left can still pop out).
    presence: std::collections::HashMap<String, Presence>,
    shown: Vec<(String, Option<DeviceStatus>)>,
}

/// One taskbar's widget: what it shows, where its devices are, and how they move. Every taskbar's
/// widget is rendered on its own, for that taskbar's DPI, height and orientation (a copy of the
/// main one's pixels came out at the wrong scale on a monitor with other scaling).
#[derive(Default)]
struct View {
    /// The last render (premultiplied), and the same on the hover plate.
    px: Vec<u32>,
    lit: Vec<u32>,
    size: SIZE,
    /// Ranges of the devices along the strip (x, or y on a vertical taskbar): (from, to, device index).
    hits: Vec<(i32, i32, usize)>,
    /// Where each device starts along the strip in the last render: its place, and where it was
    /// drawn (moving devices are drawn away from their place).
    starts: HashMap<String, i32>,
    drawn_at: HashMap<String, f32>,
    /// Devices sliding to a new place: distance still to go when it started, and when.
    slides: HashMap<String, (f32, Instant)>,
    /// The order changed since this widget's last render.
    reordered: bool,
    /// Length of the strip in the last render (width, or height on a vertical taskbar).
    strip_len: i32,
    /// DPI scale and orientation it was rendered for.
    scale: f32,
    vertical: bool,
    /// Its own place and order ("Mirror other widgets" off); None: the main widget's.
    lay: Option<Layout>,
}

/// Where a widget sits on its taskbar and in which order it shows the devices.
#[derive(Clone, Default, PartialEq)]
struct Layout {
    side: u32,
    /// Gap left of the notification area (px at 100 %), with SIDE_RIGHT.
    offset: u32,
    /// Up / down shift (px at 100 %, + = down).
    offset_y: i32,
    /// SIDE_FREE: left edge (px at 100 %) from the taskbar's left end.
    free_x: i32,
    /// Saved device order.
    order: Vec<String>,
}

/// Registry key of the widget on the monitor of taskbar `bar` (its own layout).
fn widget_key(bar: HWND) -> String {
    format!("{SETTINGS_KEY}\\Widgets\\{}", monitor_id(bar).replace(['\\', '/'], "#"))
}

/// The own layout saved for the widget on `bar`'s monitor, if there is one.
fn load_layout(bar: HWND) -> Option<Layout> {
    let key = widget_key(bar);
    let side = batteryhub::util::reg_get_dword(&key, "WidgetLeft")?;
    Some(Layout {
        side,
        offset: batteryhub::util::reg_get_dword(&key, "TaskbarOffset").unwrap_or(0),
        offset_y: batteryhub::util::reg_get_dword(&key, "TaskbarOffsetY").unwrap_or(0) as i32,
        free_x: batteryhub::util::reg_get_dword(&key, "WidgetFreeX").unwrap_or(0) as i32,
        order: settings::order(&key),
    })
}

fn save_layout(key: &str, lay: &Layout) {
    if let Some(k) = batteryhub::install::create(batteryhub::install::HKCU, key) {
        k.set_dword("WidgetLeft", lay.side);
        k.set_dword("TaskbarOffset", lay.offset);
        k.set_dword("TaskbarOffsetY", lay.offset_y as u32);
        k.set_dword("WidgetFreeX", lay.free_x as u32);
    }
}

/// Windows 11 taskbar icons are centred (Settings > Personalization > Taskbar > Taskbar
/// behaviors > Taskbar alignment; centred unless set to left).
fn taskbar_centred() -> bool {
    batteryhub::util::reg_get_dword(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "TaskbarAl").is_none_or(|v| v != 0)
}

/// Start of the charging animation's clock.
struct ChargeClock(Instant);

impl Default for ChargeClock {
    fn default() -> Self {
        ChargeClock(Instant::now())
    }
}

impl std::ops::Deref for ChargeClock {
    type Target = Instant;
    fn deref(&self) -> &Instant {
        &self.0
    }
}

/// What a widget is rendered for: its taskbar's DPI scale, orientation, and height (horizontal)
/// or width (vertical).
#[derive(Clone, Copy)]
struct Target {
    s: f32,
    vertical: bool,
    extent: i32,
    /// Up / down shift drawn now (px at 100 %).
    shift_y: f32,
}

/// The render `px` on the taskbar's hover plate: a rounded, slightly lighter (dark taskbar) or
/// darker (light taskbar) rectangle behind the content, like the taskbar's own buttons.
fn lit_pixels(px: &[u32], size: SIZE, s: f32, light: bool) -> Vec<u32> {
    let (w, h) = (size.cx, size.cy);
    let mut plate = Canvas { w, h, px: vec![0; (w * h).max(0) as usize] };
    let (color, alpha) = if light { (rgb(0, 0, 0), 14) } else { (rgb(255, 255, 255), 21) };
    plate.round_rect((4.0 * s).round() as i32, (4.0 * s).round() as i32, color, alpha);
    px.iter()
        .zip(&plate.px)
        .map(|(&c, &p)| {
            // Content over the plate, both premultiplied.
            let inv = 255 - (c >> 24);
            let ch = |sh: u32| (((c >> sh) & 0xFF) + ((p >> sh) & 0xFF) * inv / 255).min(255);
            ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0)
        })
        .collect()
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
    /// Receivers being read by the running quick read.
    static QUICK_IDS: RefCell<Vec<(u16, u16)>> = const { RefCell::new(Vec::new()) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().map(|mut a| f(&mut a)))
}

/// The time picked on the card's chart for a mouse at `t`: the nearest whole hour (local time)
/// within 12 hours of now, or now itself.
fn snap_hour(t: f64, now: u64) -> u64 {
    let l = batteryhub::dashboard::local(now);
    let base = now - (l.minute as u64 * 60 + l.second as u64);
    let (lo, hi) = (now.saturating_sub(12 * 3600), now + 12 * 3600);
    (-13i64..=13)
        .map(|k| (base as i64 + k * 3600) as u64)
        .filter(|h| (lo..=hi).contains(h))
        .chain(std::iter::once(now))
        .min_by(|a, b| (*a as f64 - t).abs().total_cmp(&(*b as f64 - t).abs()))
        .unwrap_or(now)
}

/// A change of level for the card: "−2%", "+5%", "−0.4%" (tenths below 1 %, with the language's
/// decimal separator), "±0%".
fn fmt_change(d: f64) -> String {
    let a = d.abs();
    if a < 0.05 {
        return "±0%".into(); // rounds to 0.0
    }
    let num = if a < 0.95 { batteryhub::i18n::decimal(a, 1) } else { format!("{a:.0}") };
    if d < 0.0 { format!("−{num}%") } else { format!("+{num}%") }
}

// ---- drawing -----------------------------------------------------------------------------------

/// Premultiplied BGRA canvas.
struct Canvas {
    w: i32,
    h: i32,
    px: Vec<u32>,
}

impl Canvas {
    /// Filled rounded rectangle [x0, x1) x [y0, y1).
    #[allow(clippy::too_many_arguments)]
    fn fill_rrect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, radius: i32, c: COLORREF, a: u32) {
        let r = radius.max(0).min((x1 - x0) / 2).min((y1 - y0) / 2);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = if x < x0 + r {
                    x0 + r - x
                } else if x >= x1 - r {
                    x - (x1 - r - 1)
                } else {
                    0
                };
                let dy = if y < y0 + r {
                    y0 + r - y
                } else if y >= y1 - r {
                    y - (y1 - r - 1)
                } else {
                    0
                };
                if dx * dx + dy * dy <= r * r {
                    self.blend(x, y, c, a);
                }
            }
        }
    }

    /// Filled circle.
    fn disc(&mut self, cx: i32, cy: i32, r: i32, c: COLORREF) {
        let r = r.max(1);
        for y in cy - r..=cy + r {
            for x in cx - r..=cx + r {
                if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r {
                    self.blend(x, y, c, 255);
                }
            }
        }
    }

    /// Straight line of thickness `t` (horizontal and vertical lines are what the chart needs).
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, t: i32, c: COLORREF) {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        let half = (t / 2).max(0);
        for i in 0..=steps {
            let x = x0 + (x1 - x0) * i / steps;
            let y = y0 + (y1 - y0) * i / steps;
            for oy in -half..t - half {
                for ox in -half..t - half {
                    self.blend(x + ox, y + oy, c, 255);
                }
            }
        }
    }

    /// Dashed straight line (the card's forecast): dashes and gaps of `dash` px along it.
    #[allow(clippy::too_many_arguments)]
    fn dashed(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, t: i32, c: COLORREF, dash: i32) {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        let dash = dash.max(2);
        let half = (t / 2).max(0);
        for i in 0..=steps {
            if (i / dash) % 2 == 1 {
                continue;
            }
            let x = x0 + (x1 - x0) * i / steps;
            let y = y0 + (y1 - y0) * i / steps;
            for oy in -half..t - half {
                for ox in -half..t - half {
                    self.blend(x + ox, y + oy, c, 255);
                }
            }
        }
    }

    /// Text mask at (x, y).
    fn draw(&mut self, m: &Mask, x: i32, y: i32, c: COLORREF, alpha: u32) {
        for yy in 0..m.h {
            for xx in 0..m.w {
                let a = m.a[(yy * m.w + xx) as usize] as u32 * alpha / 255;
                self.blend(x + xx, y + yy, c, a);
            }
        }
    }

    /// Fills the canvas with a rounded rectangle, `inset` from the top and bottom edge.
    fn round_rect(&mut self, inset: i32, radius: i32, c: COLORREF, a: u32) {
        let (w, h, r) = (self.w, self.h, radius.max(1));
        for y in inset..h - inset {
            for x in 0..w {
                let dx = if x < r {
                    r - x
                } else if x >= w - r {
                    x - (w - r - 1)
                } else {
                    0
                };
                let dy = if y < inset + r {
                    inset + r - y
                } else if y >= h - inset - r {
                    y - (h - inset - r - 1)
                } else {
                    0
                };
                if dx * dx + dy * dy <= r * r {
                    self.blend(x, y, c, a);
                }
            }
        }
    }

    /// Draws a user's full-colour icon: premultiplied ARGB pixels, source-over with `alpha`.
    fn draw_rgba(&mut self, px: &[u32], w: i32, x: i32, y: i32, alpha: u32) {
        if w <= 0 || alpha == 0 {
            return;
        }
        let h = (px.len() / w as usize) as i32;
        for yy in 0..h {
            for xx in 0..w {
                let s = px[(yy * w + xx) as usize];
                let sa = ((s >> 24) & 0xFF) * alpha / 255;
                if sa == 0 {
                    continue;
                }
                let (dx, dy) = (x + xx, y + yy);
                if dx < 0 || dy < 0 || dx >= self.w || dy >= self.h {
                    continue;
                }
                let d = &mut self.px[(dy * self.w + dx) as usize];
                let inv = 255 - sa;
                let ch = |shift: u32| ((s >> shift) & 0xFF) as u32 * alpha / 255;
                let (r, g, b) = (
                    ch(16) + ((*d >> 16) & 0xFF) * inv / 255,
                    ch(8) + ((*d >> 8) & 0xFF) * inv / 255,
                    ch(0) + (*d & 0xFF) * inv / 255,
                );
                let na = sa + ((*d >> 24) & 0xFF) * inv / 255;
                *d = na << 24 | r << 16 | g << 8 | b;
            }
        }
    }

    fn blend(&mut self, x: i32, y: i32, c: COLORREF, a: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || a == 0 {
            return;
        }
        let (r, g, b) = split(c);
        let d = &mut self.px[(y * self.w + x) as usize];
        let inv = 255 - a;
        let ch = |src: u8, shift: u32| ((src as u32 * a + ((*d >> shift) & 0xFF) * inv) / 255) & 0xFF;
        let na = (a + ((*d >> 24) & 0xFF) * inv / 255).min(255);
        *d = na << 24 | ch(r, 16) << 16 | ch(g, 8) << 8 | ch(b, 0);
    }
}

/// Ink coverage of `text`, cropped to the drawn pixels. Same technique as the original widget:
/// ClearType text, white on black (black on white for a light taskbar), coverage = RGB average.
struct Mask {
    w: i32,
    h: i32,
    a: Vec<u8>,
}

fn mask(text: &str, font: HFONT, dark_on_light: bool) -> Mask {
    unsafe {
        let dc = CreateCompatibleDC(None);
        let old_f = SelectObject(dc, HGDIOBJ(font.0));
        let wtext: Vec<u16> = text.encode_utf16().collect();
        let mut sz = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &wtext, &mut sz);
        let (w, h) = (sz.cx + 8, sz.cy + 8);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let mut out = Mask { w: 0, h: 0, a: Vec::new() };
        if let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old_b = SelectObject(dc, HGDIOBJ(bmp.0));
            let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
            px.fill(if dark_on_light { 0xFFFF_FFFF } else { 0 });
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, if dark_on_light { rgb(0, 0, 0) } else { rgb(255, 255, 255) });
            let _ = TextOutW(dc, 4, 4, &wtext);
            let _ = GdiFlush();
            let cover = |v: u32| -> u8 {
                let c = ((v & 0xFF) + ((v >> 8) & 0xFF) + ((v >> 16) & 0xFF)) / 3;
                (if dark_on_light { 255 - c } else { c }) as u8
            };
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1, -1);
            for y in 0..h {
                for x in 0..w {
                    if cover(px[(y * w + x) as usize]) != 0 {
                        x0 = x0.min(x);
                        x1 = x1.max(x);
                        y0 = y0.min(y);
                        y1 = y1.max(y);
                    }
                }
            }
            if x1 >= 0 {
                out.w = x1 - x0 + 1;
                out.h = y1 - y0 + 1;
                out.a = Vec::with_capacity((out.w * out.h) as usize);
                for y in 0..out.h {
                    for x in 0..out.w {
                        out.a.push(cover(px[((y0 + y) * w + x0 + x) as usize]));
                    }
                }
            }
            SelectObject(dc, old_b);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
        }
        SelectObject(dc, old_f);
        let _ = DeleteDC(dc);
        out
    }
}

/// Creates a ClearType font, falling back when the preferred face is not installed (GDI would
/// otherwise silently substitute another face).
fn make_font(px: i32, weight: i32, face: &str, fallback: &str, fallback_px: i32) -> HFONT {
    for (f, size) in [(face, px), (fallback, if fallback_px > 0 { fallback_px } else { px })] {
        unsafe {
            let h = CreateFontW(
                -size,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                &windows::core::HSTRING::from(f),
            );
            if f == fallback {
                return h;
            }
            let dc = CreateCompatibleDC(None);
            let o = SelectObject(dc, HGDIOBJ(h.0));
            let mut got = [0u16; 64];
            GetTextFaceW(dc, Some(&mut got));
            SelectObject(dc, o);
            let _ = DeleteDC(dc);
            if batteryhub::util::from_wide(&got).eq_ignore_ascii_case(f) {
                return h;
            }
            let _ = DeleteObject(HGDIOBJ(h.0));
        }
    }
    unreachable!()
}

/// Earbuds icon (drawn, no font has one): two buds outlined, `h` px tall. A bud that is not
/// reported (in its case, or off) is grey, like a sleeping device.
fn buds_mask(h: i32, left_on: bool, right_on: bool) -> Mask {
    let n = h.max(8);
    Mask { w: n, h: n, a: batteryhub::gui::earbuds_icon(n, left_on, right_on) }
}

/// A premultiplied BGRA image (w x h) scaled by `k` around its centre, box-blurred with radius
/// `blur` (twice, close to a Gaussian) and faded to `fade`: one frame of a copy popping in.
fn pop_pixels(src: &[u32], w: usize, h: usize, k: f32, blur: usize, fade: f32) -> Vec<u32> {
    if src.len() != w * h || w == 0 || h == 0 {
        return src.to_vec();
    }
    let ch = |p: u32, i: u32| ((p >> (8 * i)) & 0xFF) as f32;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let at = |x: i32, y: i32| if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 { 0 } else { src[y as usize * w + x as usize] };
    let mut img = vec![[0f32; 4]; w * h];
    for y in 0..h {
        let sy = cy + (y as f32 + 0.5 - cy) / k - 0.5;
        let (y0, fy) = (sy.floor() as i32, sy - sy.floor());
        for x in 0..w {
            let sx = cx + (x as f32 + 0.5 - cx) / k - 0.5;
            let (x0, fx) = (sx.floor() as i32, sx - sx.floor());
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            let o = &mut img[y * w + x];
            for (i, v) in o.iter_mut().enumerate() {
                let i = i as u32;
                let top = ch(a, i) * (1.0 - fx) + ch(b, i) * fx;
                let bottom = ch(c, i) * (1.0 - fx) + ch(d, i) * fx;
                *v = top * (1.0 - fy) + bottom * fy;
            }
        }
    }
    if blur > 0 {
        let r = blur as i32;
        let mut tmp = vec![[0f32; 4]; w.max(h)];
        for horizontal in [true, false, true, false] {
            let (lines, len) = if horizontal { (h, w) } else { (w, h) };
            for line in 0..lines {
                let idx = |i: usize| if horizontal { line * w + i } else { i * w + line };
                for i in 0..len {
                    tmp[i] = img[idx(i)];
                }
                for i in 0..len as i32 {
                    let mut sum = [0f32; 4];
                    for j in (i - r).max(0)..=(i + r).min(len as i32 - 1) {
                        for (s, v) in sum.iter_mut().zip(tmp[j as usize]) {
                            *s += v;
                        }
                    }
                    img[idx(i as usize)] = sum.map(|v| v / (2 * r + 1) as f32);
                }
            }
        }
    }
    img.iter()
        .map(|p| {
            let c = |i: usize| ((p[i] * fade).round().clamp(0.0, 255.0) as u32) << (8 * i);
            c(0) | c(1) | c(2) | c(3)
        })
        .collect()
}

/// `top` over `bottom` with `gap` px between, both centred.
fn stack_masks(top: &Mask, bottom: &Mask, gap: i32) -> Mask {
    let w = top.w.max(bottom.w);
    let h = top.h + gap + bottom.h;
    let mut a = vec![0u8; (w * h) as usize];
    for (m, y0) in [(top, 0), (bottom, top.h + gap)] {
        let x0 = (w - m.w) / 2;
        for y in 0..m.h {
            let src = &m.a[(y * m.w) as usize..((y + 1) * m.w) as usize];
            let at = ((y0 + y) * w + x0) as usize;
            a[at..at + m.w as usize].copy_from_slice(src);
        }
    }
    Mask { w, h, a }
}

/// Bilinear scaling of a mask by `k`.
fn scale_mask(m: &Mask, k: f32) -> Mask {
    let w = ((m.w as f32 * k).round() as i32).max(1);
    let h = ((m.h as f32 * k).round() as i32).max(1);
    let at = |x: i32, y: i32| if x < 0 || y < 0 || x >= m.w || y >= m.h { 0.0 } else { m.a[(y * m.w + x) as usize] as f32 };
    let mut a = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let sy = (y as f32 + 0.5) / k - 0.5;
        let (y0, fy) = (sy.floor() as i32, sy - sy.floor());
        for x in 0..w {
            let sx = (x as f32 + 0.5) / k - 0.5;
            let (x0, fx) = (sx.floor() as i32, sx - sx.floor());
            let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
            let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
            a.push((top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8);
        }
    }
    Mask { w, h, a }
}

/// A solid lightning bolt. The font glyph is only an outline and all but disappears at the size
/// of a taskbar badge, so the shape is drawn from a polygon instead (3x3 subsamples per pixel).
fn bolt_mask(size: i32) -> Mask {
    let n = size.max(1);
    let pts: [(f32, f32); 7] = [(0.58, 0.0), (0.08, 0.56), (0.44, 0.56), (0.22, 1.0), (0.96, 0.40), (0.58, 0.40), (0.82, 0.0)];
    let inside = |px: f32, py: f32| {
        let mut hit = false;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let ((xi, yi), (xj, yj)) = (pts[i], pts[j]);
            if (yi > py) != (yj > py) {
                let at = (xj - xi) * (py - yi) / (yj - yi) + xi;
                if px < at {
                    hit = !hit;
                }
            }
            j = i;
        }
        hit
    };
    let mut a = vec![0u8; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let mut hits = 0u32;
            for sy in 0..3 {
                for sx in 0..3 {
                    let px = (x as f32 + (sx as f32 + 0.5) / 3.0) / n as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / 3.0) / n as f32;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            a[(y * n + x) as usize] = (hits * 255 / 9) as u8;
        }
    }
    Mask { w: n, h: n, a }
}

/// Draws a piece of an item popping in (`p` 0 to 1) or out (1 to 0): it grows from half its size
/// around the item's centre `c` with a small overshoot, fades in, and is blurred while it moves
/// (strongest at the start), like the strip's other motion.
#[allow(clippy::too_many_arguments)]
fn draw_popping(cv: &mut Canvas, m: &Mask, at: (i32, i32), c: (f32, f32), p: f32, s: f32, color: COLORREF, alpha: u32) {
    if p <= 0.0 {
        return;
    }
    let (c1, c3) = (1.70158f32, 2.70158f32);
    let back = 1.0 + c3 * (p - 1.0).powi(3) + c1 * (p - 1.0).powi(2);
    let k = (0.5 + 0.5 * back).max(0.05);
    let sm = scale_mask(m, k);
    let blur = ((1.0 - p) * (1.0 - p) * 4.0 * s).round() as i32;
    let sm = if blur > 0 { blur_mask(&blur_mask(&sm, blur, true), blur, false) } else { sm };
    // Where the piece's centre goes, scaled towards the item's centre.
    let (mx, my) = (at.0 as f32 + m.w as f32 / 2.0, at.1 as f32 + m.h as f32 / 2.0);
    let (nx, ny) = (c.0 + (mx - c.0) * k, c.1 + (my - c.1) * k);
    let fade = p * p * (3.0 - 2.0 * p);
    cv.draw(&sm, (nx - sm.w as f32 / 2.0).round() as i32, (ny - sm.h as f32 / 2.0).round() as i32, color, (alpha as f32 * fade).round() as u32);
}

/// Box blur (twice, close to a Gaussian) of a mask along x or y; the result is 2 * r larger on
/// that axis.
fn blur_mask(m: &Mask, r: i32, horizontal: bool) -> Mask {
    let (w, h) = if horizontal { (m.w + 2 * r, m.h) } else { (m.w, m.h + 2 * r) };
    let mut a = vec![0u32; (w * h) as usize];
    for y in 0..m.h {
        for x in 0..m.w {
            let (tx, ty) = if horizontal { (x + r, y) } else { (x, y + r) };
            a[(ty * w + tx) as usize] = m.a[(y * m.w + x) as usize] as u32;
        }
    }
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: i32, i: i32| if horizontal { (line * w + i) as usize } else { (i * w + line) as usize };
    let mut tmp = vec![0u32; len as usize];
    for _ in 0..2 {
        for line in 0..lines {
            for i in 0..len {
                tmp[i as usize] = a[at(line, i)];
            }
            for i in 0..len {
                let (lo, hi) = ((i - r).max(0), (i + r).min(len - 1));
                let sum: u32 = (lo..=hi).map(|j| tmp[j as usize]).sum();
                a[at(line, i)] = sum / (2 * r + 1) as u32;
            }
        }
    }
    Mask { w, h, a: a.into_iter().map(|v| v.min(255) as u8).collect() }
}

/// Segoe Fluent Icons glyphs used for the small connection icons (E702 is the Bluetooth logo;
/// E71E would show a magnifying glass).
const GLYPH_WIFI: char = '\u{E701}';
const GLYPH_BLUETOOTH: char = '\u{E702}';
const GLYPH_USB: char = '\u{E88E}';
const GLYPH_HID: char = '\u{E772}';

/// Which single icon stands for the link. LIGHTSPEED is a Logitech transport rather than a Windows
/// connection: it gets the Wi-Fi icon with a small bolt on it. Cable and Bluetooth still win when
/// that is how the device is actually read.
fn connection_badge(via: &str, name: &str, vid: Option<u16>) -> Option<(char, bool)> {
    let v = via.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    let icon = if v.contains("bluetooth") {
        (GLYPH_BLUETOOTH, false)
    } else if v.contains("wi-fi") || v.contains("wifi") {
        (GLYPH_WIFI, false)
    } else if v.contains("usb") {
        (GLYPH_USB, false)
    } else if v.contains("hid") {
        (GLYPH_HID, false)
    } else if n.contains("lightspeed") || v.contains("logitech") || vid == Some(0x046D) {
        (GLYPH_WIFI, true)
    } else if v.contains("2.4") {
        (GLYPH_WIFI, false)
    } else {
        return None;
    };
    Some(icon)
}

/// One grey for every connection: the badge is a hint, not a second colour code.
const BADGE_GRAY: COLORREF = rgb(150, 150, 150);

/// `top` over `base` at `(x, y)`, taking the stronger alpha of the two.
fn overlay_mask(base: &Mask, top: &Mask, x: i32, y: i32) -> Mask {
    let mut a = base.a.clone();
    for yy in 0..top.h {
        for xx in 0..top.w {
            let (tx, ty) = (x + xx, y + yy);
            if tx < 0 || ty < 0 || tx >= base.w || ty >= base.h {
                continue;
            }
            let i = (ty * base.w + tx) as usize;
            a[i] = a[i].max(top.a[(yy * top.w + xx) as usize]);
        }
    }
    Mask { w: base.w, h: base.h, a }
}

/// The icon after the level: one grey glyph, no text, no plate or background.
fn badge_piece(dev: Option<usize>, via: &str, name: &str, vid: Option<u16>, icon_font: HFONT, light: bool, alpha: u32, gap_after: i32) -> Option<Piece> {
    let (glyph, bolt) = connection_badge(via, name, vid)?;
    let mut m = mask(&glyph.to_string(), icon_font, light);
    if bolt {
        // Dim the Wi-Fi arcs so the solid bolt is unmistakably the foreground mark.
        for a in &mut m.a {
            *a = (*a as f32 * 0.45).round() as u8;
        }
        let b = bolt_mask((m.h as f32 * 0.95).round() as i32);
        m = overlay_mask(&m, &b, (m.w - b.w) / 2, (m.h - b.h) / 2);
    }
    if m.a.iter().all(|&a| a == 0) {
        return None;
    }
    let mut p = Piece::new(dev, m, BADGE_GRAY, alpha * 190 / 255, gap_after);
    p.glow = false;
    Some(p)
}

/// Opacity / pop of a badge `ms` after the toggle: eased, and mirrored when hiding.
fn badge_progress(ms: f32, showing: bool) -> f32 {
    let p = (ms / BADGE_MS).clamp(0.0, 1.0);
    let eased = p * p * (3.0 - 2.0 * p);
    if showing { eased } else { 1.0 - eased }
}

/// A device's own ICO / PNG on the strip (#476), in place of the built-in glyph.
fn custom_piece(dev: Option<usize>, key: &str, max: i32, color: COLORREF, alpha: u32, gap_after: i32) -> Option<Piece> {
    let path = settings::icon_file(SETTINGS_KEY, key)?;
    let sprite = batteryhub::custom_icon::load(&path, max.max(1) as u32)?;
    Some(rgba_piece(dev, sprite, color, alpha, gap_after))
}

/// A full-colour piece. Its mask is the image's shape and `color` the colour of its glow.
fn rgba_piece(dev: Option<usize>, sprite: batteryhub::custom_icon::Sprite, color: COLORREF, alpha: u32, gap_after: i32) -> Piece {
    let m = Mask { w: sprite.w, h: sprite.h, a: sprite.px.iter().map(|&p| (p >> 24) as u8).collect() };
    let mut p = Piece::new(dev, m, color, alpha, gap_after);
    p.rgba = Some(sprite.px);
    p
}

/// Premultiplied ARGB pixels of a tinted mask, for composing a custom icon into a ring (#476).
fn rgba_mask(m: &Mask, color: COLORREF, alpha: u32) -> Vec<u32> {
    let (r, g, b) = ((color.0 & 0xFF) as u32, ((color.0 >> 8) & 0xFF) as u32, ((color.0 >> 16) & 0xFF) as u32);
    m.a.iter()
        .map(|&a| {
            let a = a as u32 * alpha / 255;
            a << 24 | (r * a / 255) << 16 | (g * a / 255) << 8 | b * a / 255
        })
        .collect()
}

/// Source-over of premultiplied ARGB pixels at `(x, y)` into a `w`-wide canvas.
fn overlay_rgba(base: &mut [u32], w: i32, src: &[u32], sw: i32, x: i32, y: i32) {
    if sw <= 0 {
        return;
    }
    let sh = (src.len() / sw as usize) as i32;
    for yy in 0..sh {
        for xx in 0..sw {
            let s = src[(yy * sw + xx) as usize];
            let sa = (s >> 24) & 0xFF;
            if sa == 0 {
                continue;
            }
            let (dx, dy) = (x + xx, y + yy);
            if dx < 0 || dy < 0 || dx >= w || dy * w + dx >= base.len() as i32 {
                continue;
            }
            let d = &mut base[(dy * w + dx) as usize];
            let inv = 255 - sa;
            let ch = |shift: u32| (s >> shift) & 0xFF;
            let (r, g, b) = (
                ch(16) + ((*d >> 16) & 0xFF) * inv / 255,
                ch(8) + ((*d >> 8) & 0xFF) * inv / 255,
                ch(0) + (*d & 0xFF) * inv / 255,
            );
            let na = sa + ((*d >> 24) & 0xFF) * inv / 255;
            *d = na << 24 | r << 16 | g << 8 | b;
        }
    }
}

/// The pixels of a ring with a user's icon in its middle, fully opaque: the piece's own alpha
/// dims them once (asleep devices).
fn compose_ring(base: &Mask, arc: &Mask, sprite: &batteryhub::custom_icon::Sprite, base_color: COLORREF, arc_color: COLORREF) -> Vec<u32> {
    let d = base.w;
    let mut px = rgba_mask(base, base_color, 255);
    overlay_rgba(&mut px, d, &rgba_mask(arc, arc_color, 255), d, 0, 0);
    overlay_rgba(&mut px, d, &sprite.px, sprite.w, (d - sprite.w) / 2, (d - sprite.h) / 2);
    px
}

/// A user's icon inside the battery ring (#476), composed into one full-colour piece.
fn custom_ring_piece(dev: Option<usize>, key: &str, d: i32, thick: f32, frac: f32, gap: f32, base_mul: f32, bolt_open: f32, base_color: COLORREF, arc_color: COLORREF, alpha: u32, gap_after: i32) -> Option<Piece> {
    let path = settings::icon_file(SETTINGS_KEY, key)?;
    let sprite = batteryhub::custom_icon::load(&path, (d / 2).max(1) as u32)?;
    let center = Mask { w: sprite.w, h: sprite.h, a: vec![255; (sprite.w * sprite.h) as usize] };
    let (base, mut arc) = ring_masks_with(&center, d, thick, frac, gap, base_mul);
    if bolt_open > 0.0 {
        let bolt = ring_bolt_mask(&center, d, thick, gap, bolt_open);
        for (v, a) in arc.a.iter_mut().zip(&bolt.a) {
            *v = (*v).max(*a);
        }
    }
    let px = compose_ring(&base, &arc, &sprite, base_color, arc_color);
    let m = Mask { w: d, h: d, a: vec![255; (d * d) as usize] };
    let mut p = Piece::new(dev, m, COLORREF(0), alpha, gap_after);
    p.rgba = Some(px);
    Some(p)
}

fn device_icon(d: &DeviceStatus, font: HFONT, light: bool) -> Mask {
    if d.kind != DeviceKind::Earbuds {
        return mask(&d.kind.glyph().to_string(), font, light);
    }
    let g = mask(&DeviceKind::Headset.glyph().to_string(), font, light);
    let (l, r) = d.buds.map(|b| (b.left >= 0, b.right >= 0)).unwrap_or((true, true));
    // Rows with ink in the headset glyph: the earbuds take the same top and bottom.
    let ink: Vec<i32> = (0..g.h).filter(|&y| (0..g.w).any(|x| g.a[(y * g.w + x) as usize] > 60)).collect();
    let (Some(&top), Some(&bottom)) = (ink.first(), ink.last()) else { return buds_mask(g.h, l, r) };
    let (ink_top, ink_bottom) = batteryhub::gui::EARBUDS_INK;
    let n = (((bottom - top + 1) as f32) / (ink_bottom - ink_top)).round() as i32;
    let b = buds_mask(n, l, r);
    let dy = top - (ink_top * n as f32).round() as i32;
    let mut a = vec![0u8; (b.w * g.h) as usize];
    for y in 0..b.h {
        let ty = y + dy;
        if (0..g.h).contains(&ty) {
            a[(ty * b.w) as usize..((ty + 1) * b.w) as usize].copy_from_slice(&b.a[(y * b.w) as usize..((y + 1) * b.w) as usize]);
        }
    }
    Mask { w: b.w, h: g.h, a }
}

struct Piece {
    /// Index of the device this piece belongs to (for hover hit-testing), None for others.
    dev: Option<usize>,
    m: Mask,
    color: COLORREF,
    alpha: u32,
    gap_after: i32,
    /// Part of the piece drawn in another colour (same size as `m`, drawn at the same place):
    /// earbuds whose two buds are coloured apart, or a ring's filled part.
    second: Option<(Mask, COLORREF)>,
    /// Drawn with a soft glow in its colour ("Glowing icon").
    glow: bool,
    /// A user-chosen full-colour icon instead of a tinted mask (#476).
    rgba: Option<Vec<u32>>,
}

impl Piece {
    fn new(dev: Option<usize>, m: Mask, color: COLORREF, alpha: u32, gap_after: i32) -> Self {
        Piece { dev, m, color, alpha, gap_after, second: None, glow: false, rgba: None }
    }

    /// The piece's masks with their colours.
    fn layers(&self) -> impl Iterator<Item = (&Mask, COLORREF)> {
        std::iter::once((&self.m, self.color)).chain(self.second.as_ref().map(|(m, c)| (m, *c)))
    }

    /// The same piece in two colours: `first` before column (or row) `at`, `second` from it on.
    fn split(dev: Option<usize>, m: Mask, at: i32, rows: bool, colors: (COLORREF, COLORREF), alpha: u32, gap_after: i32) -> Self {
        if colors.0 == colors.1 {
            return Piece::new(dev, m, colors.0, alpha, gap_after);
        }
        let (a, b) = split_mask(&m, at, rows);
        Piece { dev, m: a, color: colors.0, alpha, gap_after, second: Some((b, colors.1)), glow: false, rgba: None }
    }
}

/// Ring style: a `d` square with the ring (only the part above `percent`, faint) and `icon` in its
/// middle, and a second with the ring's part filled clockwise from the top up to `percent`.
#[cfg(test)]
fn ring_masks(icon: &Mask, d: i32, thick: f32, percent: i32) -> (Mask, Mask) {
    ring_masks_with(icon, d, thick, percent.clamp(0, 100) as f32 / 100.0, 0.0, 1.0)
}

/// The icon stays on whole pixels (placed by its ink, since glyphs sit off-centre in their font
/// cell); the ring is centred on that same ink to a fraction of a pixel. Shared with the bolt so
/// both land on the same centre.
fn ring_geometry(icon: &Mask, d: i32) -> (f32, f32, i32, i32) {
    let (mut x0, mut y0, mut x1, mut y1) = (icon.w, icon.h, -1, -1);
    for yy in 0..icon.h {
        for xx in 0..icon.w {
            if icon.a[(yy * icon.w + xx) as usize] > 8 {
                (x0, y0, x1, y1) = (x0.min(xx), y0.min(yy), x1.max(xx), y1.max(yy));
            }
        }
    }
    let half = d as f32 / 2.0;
    let (mx, my) = if x1 >= x0 { ((x0 + x1 + 1) as f32 / 2.0, (y0 + y1 + 1) as f32 / 2.0) } else { (icon.w as f32 / 2.0, icon.h as f32 / 2.0) };
    let (ox, oy) = ((half - mx).round() as i32, (half - my).round() as i32);
    (ox as f32 + mx, oy as f32 + my, ox, oy)
}

/// Hover=3 was the old "Card on click" choice. It is now a separate setting, so the old value
/// simply means the ordinary hover card.
fn hover_card_mode(mode: settings::HoverMode) -> settings::HoverMode {
    if mode == settings::HoverMode::Click { settings::HoverMode::Card } else { mode }
}

/// "Card on click" moves the card (and the short tooltip) out of hover: passing the mouse over
/// the device must not open anything.
fn hover_shows_popup(mode: settings::HoverMode, card_click: bool) -> bool {
    !card_click && hover_card_mode(mode) != settings::HoverMode::Off
}

/// A card opened by hover always closes when the mouse leaves. A clicked card does too, unless its
/// setting asks for the Control Center behaviour (it then waits for a click elsewhere).
fn closes_when_away(pinned: bool, mode: settings::CardDismiss) -> bool {
    !pinned || mode == settings::CardDismiss::Away
}

/// Map a battery level onto the part of the circle that is still visible after the top `gap` is
/// opened: 87% means 87% of the *remaining* ring, not 87% of the whole circle.
fn ring_fill_fraction(level: f32, gap: f32) -> f32 {
    let gap = gap.clamp(0.0, 0.5);
    gap / 2.0 + level.clamp(0.0, 1.0) * (1.0 - gap)
}

/// The same ring, with `frac` of the circle filled and `gap` (a fraction of the circle, centred on
/// the top) removed. The ends of the *filled* arc get round caps, so opening it looks like the arc
/// is being pushed apart rather than cut off. `base_mul` fades the faint unfilled remainder out
/// while on power, so a 30% device does not keep looking like a whole circle.
fn ring_masks_with(icon: &Mask, d: i32, thick: f32, frac: f32, gap: f32, base_mul: f32) -> (Mask, Mask) {
    let n = (d * d).max(0) as usize;
    let (mut base, mut arc) = (vec![0u8; n], vec![0u8; n]);
    let (cx, cy, ox, oy) = ring_geometry(icon, d);
    // Every ring the same size; half a pixel of room for its centre to move.
    let half = d as f32 / 2.0;
    let ro = half - 1.0;
    let ri = ro - thick;
    let frac = frac.clamp(0.0, 1.0);
    let gap = gap.clamp(0.0, 0.5);
    for y in 0..d {
        for x in 0..d {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let r = (dx * dx + dy * dy).sqrt();
            let cover = (ro - r + 0.5).clamp(0.0, 1.0) * (r - ri + 0.5).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            // 0 at the top, growing clockwise.
            let mut turn = dx.atan2(-dy) / std::f32::consts::TAU;
            if turn < 0.0 {
                turn += 1.0;
            }
            if gap > 0.0 && (turn < gap / 2.0 || turn > 1.0 - gap / 2.0) {
                continue; // the top segment moves aside
            }
            let i = (y * d + x) as usize;
            if turn <= frac && frac > 0.0 {
                arc[i] = (cover * 255.0) as u8;
            } else {
                base[i] = (cover * 255.0 * 0.28 * base_mul.clamp(0.0, 1.0)) as u8;
            }
        }
    }
    if gap > 0.0 && frac > gap / 2.0 {
        // Round caps only where the filled arc actually begins and ends. A cap at the left end of
        // the gap would put bright pixels on the unfilled side of a 30% ring.
        let (rm, cap) = ((ro + ri) / 2.0, (thick / 2.0).max(0.5));
        let last = frac.min(1.0 - gap / 2.0);
        for turn in [gap / 2.0, last] {
            let a = turn * std::f32::consts::TAU;
            let (ex, ey) = (cx + a.sin() * rm, cy - a.cos() * rm);
            for y in 0..d {
                for x in 0..d {
                    let dist = ((x as f32 + 0.5 - ex).powi(2) + (y as f32 + 0.5 - ey).powi(2)).sqrt();
                    let cover = (cap - dist + 0.5).clamp(0.0, 1.0);
                    if cover > 0.0 {
                        let i = (y * d + x) as usize;
                        arc[i] = arc[i].max((cover * 255.0) as u8);
                    }
                }
            }
        }
    }
    for yy in 0..icon.h {
        for xx in 0..icon.w {
            let (tx, ty) = (ox + xx, oy + yy);
            if (0..d).contains(&tx) && (0..d).contains(&ty) {
                let i = (ty * d + tx) as usize;
                base[i] = base[i].max(icon.a[(yy * icon.w + xx) as usize]);
            }
        }
    }
    (Mask { w: d, h: d, a: base }, Mask { w: d, h: d, a: arc })
}

/// The solid charging bolt in the gap at the top of a ring. It uses the ring's own centre and the
/// same angle as the removed arc, so it lands exactly in the opening; `open` fades it in / out.
fn ring_bolt_mask(icon: &Mask, d: i32, thick: f32, gap: f32, open: f32) -> Mask {
    let size = ((d as f32 * 0.30).round() as i32).max(5);
    let drawn = bolt_mask(size);
    // The bolt is slanted, so its font-free polygon is not centred in its cell. Crop to the ink
    // and centre *that*, otherwise it looks shifted to one side of the gap.
    let (mut bx0, mut by0, mut bx1, mut by1) = (size, size, -1, -1);
    for y in 0..size {
        for x in 0..size {
            if drawn.a[(y * size + x) as usize] > 8 {
                (bx0, by0, bx1, by1) = (bx0.min(x), by0.min(y), bx1.max(x), by1.max(y));
            }
        }
    }
    let raw = if bx1 >= bx0 {
        let (w, h) = (bx1 - bx0 + 1, by1 - by0 + 1);
        let mut a = Vec::with_capacity((w * h) as usize);
        for y in by0..=by1 {
            a.extend_from_slice(&drawn.a[(y * size + bx0) as usize..=(y * size + bx1) as usize]);
        }
        Mask { w, h, a }
    } else {
        return Mask { w: d, h: d, a: vec![0; (d * d).max(0) as usize] };
    };
    let mut a = vec![0u8; (d * d).max(0) as usize];
    let (cx, cy, _, _) = ring_geometry(icon, d);
    let rm = (d as f32 / 2.0 - 1.0) - thick / 2.0;
    // Middle of the removed segment, then centre the bolt's ink there on x and y.
    let gy = cy - rm * (gap * std::f32::consts::PI).cos() - (d as f32 * 0.07).round() - 1.0;
    let x0 = (cx - raw.w as f32 / 2.0).round() as i32;
    // One pixel above the cell is allowed: the top of the bolt is its thinnest tip, so clipping
    // that row is what actually lifts the visible bolt by 1 px at 100% DPI (where a plain 0 clamp
    // kept it pinned to the top edge).
    let y0 = (gy - raw.h as f32 / 2.0).round().clamp(-1.0, (d - raw.h) as f32) as i32;
    for yy in 0..raw.h {
        for xx in 0..raw.w {
            let (tx, ty) = (x0 + xx, y0 + yy);
            if tx < 0 || ty < 0 || tx >= d || ty >= d {
                continue;
            }
            a[(ty * d + tx) as usize] = (raw.a[(yy * raw.w + xx) as usize] as f32 * open).round().clamp(0.0, 255.0) as u8;
        }
    }
    Mask { w: d, h: d, a }
}

/// The soft glow around a piece ("Glowing icon"): `m` blurred both ways by `r` and made brighter.
/// It is `2 r` wider and higher than `m`.
fn glow_mask(m: &Mask, r: i32) -> Mask {
    let g = blur_mask(&blur_mask(m, r, true), r, false);
    Mask { w: g.w, h: g.h, a: g.a.into_iter().map(|v| (v as u32 * 2).min(255) as u8).collect() }
}

/// Charging animation: a band of `color` light passing over `from..to` (along x, or y when
/// `vertical`) of what is drawn in `cv`, `t` (0 to 1) of the way.
fn charge_sweep(cv: &mut Canvas, from: i32, to: i32, vertical: bool, color: COLORREF, t: f32) {
    let band = ((to - from) as f32 * 0.35).max(6.0);
    let pos = from as f32 - band + (to - from) as f32 * t + 2.0 * band * t;
    let (r, g, b) = split(color);
    let (w, h) = (cv.w, cv.h);
    for y in 0..h {
        for x in 0..w {
            let along = if vertical { y } else { x };
            if along < from || along >= to {
                continue;
            }
            let d = (along as f32 + 0.5 - pos) / band;
            let f = (-d * d).exp() * 0.85;
            if f < 0.02 {
                continue;
            }
            let p = &mut cv.px[(y * w + x) as usize];
            let a = (*p >> 24) & 0xFF;
            if a < 8 {
                continue;
            }
            let mix = |cur: u32, tint: u8| ((cur as f32) * (1.0 - f) + (tint as u32 * a / 255) as f32 * f).round().min(a as f32) as u32;
            *p = a << 24 | mix((*p >> 16) & 0xFF, r) << 16 | mix((*p >> 8) & 0xFF, g) << 8 | mix(*p & 0xFF, b);
        }
    }
}

/// Two masks of `m`'s size: the ink before column (or row) `at`, and the ink from it on.
fn split_mask(m: &Mask, at: i32, rows: bool) -> (Mask, Mask) {
    let mut a = m.a.clone();
    let mut b = m.a.clone();
    for y in 0..m.h {
        for x in 0..m.w {
            let i = (y * m.w + x) as usize;
            if (if rows { y } else { x }) < at {
                b[i] = 0;
            } else {
                a[i] = 0;
            }
        }
    }
    (Mask { w: m.w, h: m.h, a }, Mask { w: m.w, h: m.h, a: b })
}

/// Every font the strip and the card draw with, for one DPI scale (creating a font and checking
/// its face costs a device context each time, so they are made once, not per render).
struct Fonts {
    scale: f32,
    icons: HFONT,
    bolt: HFONT,
    text: HFONT,
    badge: HFONT,
    badge_icon: HFONT,
    card_icon: HFONT,
    card_name: HFONT,
    card_big: HFONT,
    card_small: HFONT,
}

impl Fonts {
    fn new(s: f32) -> Self {
        let px = |v: f32| (v * s).round() as i32;
        Fonts {
            scale: s,
            icons: make_font(px(16.0), 400, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0),
            bolt: make_font(px(11.0), 400, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0),
            text: make_font(px(13.0), 400, "Segoe UI Variable Text", "Segoe UI", px(12.0)),
            badge: make_font(px(9.0), 600, "Segoe UI Variable Text", "Segoe UI", px(8.0)),
            badge_icon: make_font(px(10.0), 400, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0),
            card_icon: make_font(px(16.0), 400, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0),
            card_name: make_font(px(15.0), 600, "Segoe UI Variable Text", "Segoe UI", 0),
            card_big: make_font(px(30.0), 600, "Segoe UI Variable Display", "Segoe UI", 0),
            card_small: make_font(px(12.0), 400, "Segoe UI Variable Text", "Segoe UI", 0),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for f in [self.icons, self.bolt, self.text, self.badge, self.badge_icon, self.card_icon, self.card_name, self.card_big, self.card_small] {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
    }
}

impl App {
    /// Creates the fonts for the current DPI scale when there are none yet or the scale changed.
    fn ensure_fonts(&mut self) {
        let s = self.scale();
        if self.fonts.as_ref().map(|f| f.scale) != Some(s) {
            self.fonts = Some(Fonts::new(s));
        }
    }

    /// Device `i` of the strip (connected devices in strip order), without copying the list.
    fn visible_at(&self, i: usize) -> Option<&DeviceStatus> {
        self.order.iter().filter_map(|k| self.devices.get(k)).map(|t| &t.status).filter(|s| s.connected && !self.hidden.contains(&s.key)).nth(i)
    }

    fn scale(&self) -> f32 {
        let dpi = unsafe { GetDpiForWindow(self.taskbar) };
        if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
    }

    /// Connected devices shown on the taskbar ("Show on the taskbar" not turned off).
    fn visible(&self) -> Vec<DeviceStatus> {
        self.connected().into_iter().filter(|s| !self.hidden.contains(&s.key)).collect()
    }

    /// Every connected device, hidden ones too (the right-click menu lists them all).
    fn connected(&self) -> Vec<DeviceStatus> {
        self.order.iter().filter_map(|k| self.devices.get(k)).map(|t| t.status.clone()).filter(|s| s.connected).collect()
    }

    /// Renders the strip and hands it to the layered window.
    fn paint(&mut self) {
        if self.badge_at.is_some_and(|(at, _)| at.elapsed().as_secs_f32() * 1000.0 >= BADGE_MS) {
            self.badge_at = None;
        }
        if self.crowded && !self.fallback {
            self.sync_tray();
        }
        if self.fallback {
            self.sync_tray();
            return;
        }
        if self.strip.0.is_null() {
            return;
        }
        self.painted_for = self.render_target();
        let devices = self.visible();
        let items = self.strip_items(&devices);
        if std::mem::take(&mut self.reordered) {
            self.mv.reordered = true;
            for v in self.views.values_mut() {
                v.reordered = true;
            }
        }
        // The main taskbar's widget.
        let target = self.bar_target(self.taskbar);
        let mut mv = std::mem::take(&mut self.mv);
        let before = mv.size;
        self.render(&mut mv, &items, target, self.strip);
        let resized = before != mv.size;
        self.mv = mv;
        if !self.popping(self.strip) {
            self.push(self.strip);
        }
        // Every copy on another monitor: rendered for its own taskbar.
        let copies: Vec<(HWND, HWND)> = self.mirrors.iter().chain(self.parked.iter()).copied().collect();
        self.views.retain(|k, _| copies.iter().any(|c| c.1.0 as isize == *k));
        let mut grown = Vec::new();
        for (bar, strip) in copies {
            let key = strip.0 as isize;
            let own = self.own_layout(strip);
            let mut v = self.views.remove(&key).unwrap_or_default();
            let before = v.size;
            let mut target = self.bar_target(bar);
            if own {
                // Its own place and order ("Mirror other widgets" off).
                if v.lay.is_none() {
                    v.lay = Some(load_layout(bar).unwrap_or_else(|| self.main_layout()));
                }
                let lay = v.lay.clone().unwrap_or_default();
                target.shift_y = lay.offset_y as f32;
                let mut order = self.order.clone();
                settings::apply_order(&mut order, &lay.order);
                let mut list = items.clone();
                list.sort_by_key(|it| order.iter().position(|k| *k == it.key).unwrap_or(usize::MAX));
                self.render(&mut v, &list, target, strip);
            } else {
                v.lay = None;
                self.render(&mut v, &items, target, strip);
            }
            let resized = before != v.size;
            self.views.insert(key, v);
            // A new size: its placement below gives it the image together with its new place.
            if resized && self.mirrors.iter().any(|m| m.1 == strip) {
                grown.push(strip);
            } else if !self.popping(strip) {
                self.push(strip);
            }
        }
        self.sync_slide_timer();
        self.sync_charge_timer();
        self.prune_ring_charge();
        self.sync_ring_charge_timer();
        self.sync_badge_timer();
        let animating = self.drag.is_some() || self.sliding() || !self.presence.is_empty() || self.y_glide.is_some() || self.charging_shown() || self.ring_charge_animating() || self.badge_at.is_some();
        if resized || !animating {
            self.last_place.2 = -1; // force a reposition with the new width
            self.place();
        }
        // The copies keep their left edge when their width changes: move them too, so their right
        // edge stays next to the clock (not only at the next timer tick).
        for i in 0..self.mirrors.len() {
            if !animating || grown.contains(&self.mirrors[i].1) {
                self.place_mirror_at(i);
            }
        }
    }

    /// DPI scale, orientation and size of the taskbar `bar` (what its widget is rendered for).
    fn bar_target(&self, bar: HWND) -> Target {
        let dpi = unsafe { GetDpiForWindow(bar) };
        let s = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let mut c = RECT::default();
        unsafe {
            let _ = GetClientRect(bar, &mut c);
        }
        let (w, h) = (c.right - c.left, c.bottom - c.top);
        let vertical = if bar == self.taskbar { self.vertical } else { h > w };
        Target { s, vertical, extent: if vertical { w } else { h }, shift_y: self.shown_offset_y() }
    }

    /// The fonts of the strip (icons, bolt, text, connection icon) for DPI scale `s`.
    fn strip_fonts(&mut self, s: f32) -> (HFONT, HFONT, HFONT, HFONT) {
        let all = |f: &Fonts| (f.icons, f.bolt, f.text, f.badge_icon);
        if s == self.scale() {
            self.ensure_fonts();
            return self.fonts.as_ref().map(all).unwrap_or_default();
        }
        if !self.fonts_more.iter().any(|f| f.scale == s) {
            if self.fonts_more.len() >= 4 {
                self.fonts_more.remove(0);
            }
            self.fonts_more.push(Fonts::new(s));
        }
        self.fonts_more.iter().find(|f| f.scale == s).map(all).unwrap_or_default()
    }

    /// Icon font of the ring style for DPI scale `s` (kept per scale).
    fn ring_font(&mut self, s: f32) -> HFONT {
        if let Some(&(_, f)) = self.ring_fonts.iter().find(|(k, _)| *k == s) {
            return f;
        }
        let f = make_font((13.0 * s).round() as i32, 400, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0);
        self.ring_fonts.push((s, f));
        f
    }

    /// The pieces of every item (icon, level, bolt) drawn for DPI scale `s`.
    fn pieces(&mut self, items: &[Item], s: f32, vertical: bool) -> Vec<Piece> {
        let px = |v: f32| (v * s).round() as i32;
        let light = self.light;
        // Same colours, fonts and spacing as the original C++ widget.
        let fg = if light { rgb(0, 0, 0) } else { rgb(255, 255, 255) };
        let red = ui::low_color(light);
        let (icons, bolt, text, badge_icon_font) = self.strip_fonts(s);
        // The drops are drawn for a short while after the menu toggle even when they are being
        // turned off, so they fade out instead of vanishing in one frame. A fade never scales or
        // blurs the tiny mark (which made it disappear in earlier builds).
        let badge_step = self.badge_at.map(|(at, showing)| badge_progress(at.elapsed().as_secs_f32() * 1000.0, showing));
        let badge_fade = if self.connection_icons {
            badge_step.unwrap_or(1.0)
        } else {
            badge_step.unwrap_or(0.0)
        };
        // Ring style: the icon a size between the strip's icons and the bolt, to fit inside the ring.
        let ring_font = if self.ring { self.ring_font(s) } else { bolt };
        let mut pieces: Vec<Piece> = Vec::new();
        for (i, it) in items.iter().enumerate() {
            let Some(d) = &it.dev else {
                if it.key == REFRESH_KEY {
                    // Refresh started from the menu: a small refresh glyph until it finishes.
                    pieces.push(Piece::new(Some(i), mask("\u{E72C}", bolt, light), fg, 170, 0));
                } else {
                    // Battery-unknown glyph, so there is always something to right-click.
                    pieces.push(Piece::new(Some(i), mask("\u{E996}", icons, light), fg, 110, 0));
                }
                continue;
            };
            let alpha = if d.asleep { 120 } else { 255 };
            // Red at or below this device's own warning level (right-click menu, default 15%);
            // earbuds by each bud's own level. The icon too when the device's menu asks for it.
            let threshold = settings::threshold(SETTINGS_KEY, &d.key);
            let low = settings::is_low(d, threshold);
            let buds_low = settings::buds_low(d, threshold);
            let red_icon = settings::red_icon(SETTINGS_KEY, &d.key);
            let tint = |on: bool| if on { red } else { fg };
            // The icon in its own colour (menu: Icon colour), red over it when asked for.
            let own = settings::icon_rgb(settings::icon_color(SETTINGS_KEY, &d.key), d.vid).map(|(r, g, b)| rgb(r, g, b)).unwrap_or(fg);
            let icon_tint = |on: bool| if red_icon && on { red } else { own };
            // Lightning bolt on power: green while charging, blue once it reached 100%.
            let bolt_color = ui::bolt_color(d, light);
            let mark = bolt_color.is_some();
            let glow = settings::glow(SETTINGS_KEY, &d.key);
            if self.ring && d.percent < 0 {
                // Ring style without a level: the faint ring alone would read as 0%, so the "?"
                // (or the coarse level) stands next to it. No filled arc, bolt or power colour.
                let dpx = px(28.0);
                let thick = (2.5 * s).max(2.0);
                let ring_gap_after = px(4.0);
                let icon = device_icon(d, ring_font, light);
                let p = custom_ring_piece(Some(i), &d.key, dpx, thick, 0.0, 0.0, 1.0, 0.0, own, own, alpha, ring_gap_after)
                    .unwrap_or_else(|| {
                        let (base, _) = ring_masks_with(&icon, dpx, thick, 0.0, 0.0, 1.0);
                        let mut p = Piece::new(Some(i), base, own, alpha, ring_gap_after);
                        p.glow = glow;
                        p
                    });
                pieces.push(p);
                let gap = if badge_fade > 0.0 { px(2.0) } else { 0 };
                pieces.push(Piece::new(Some(i), mask(&d.level(), text, light), fg, alpha, gap));
                if badge_fade > 0.0 {
                    let a = (alpha as f32 * badge_fade).round() as u32;
                    if let Some(b) = badge_piece(Some(i), &d.via, &d.name, d.hw.map(|h| h.0), badge_icon_font, light, a, 0) {
                        pieces.push(b);
                    }
                }
                continue;
            }
            if self.ring && d.percent >= 0 {
                // Ring style: the icon inside a ring filled clockwise to the level. On power the
                // top segment moves aside (with round ends), the unfilled remainder fades away and
                // a bolt takes the gap; unplugging plays the same animation backwards.
                let on_power = d.is_charging() || d.is_full();
                let now = Instant::now();
                let (open, fill, animating) = self.ring_charge_state(&d.key, on_power, d.percent, now);
                let base_color = if low { red } else { own };
                // Green while charging, blue at 100%, and a blue -> normal fade on unplug.
                let ring_color = self.ring_color(&d.key, d, light, base_color, now, animating);
                if !animating {
                    self.ring_charge.remove(&d.key);
                }
                let dpx = px(28.0);
                let thick = (2.5 * s).max(2.0);
                let gap = RING_GAP * open;
                // Space belongs to the ring piece: the layout adds `gap_after` *after* a piece,
                // so putting it on the last (badge) piece would never move anything.
                let ring_gap_after = px(4.0);
                // The gap takes part of the circle away, so the level has to be spread over what
                // is left. Otherwise 87% covers 96% of the visible ring and looks full.
                let draw_frac = ring_fill_fraction(fill, gap);
                let icon = device_icon(d, ring_font, light);
                let p = custom_ring_piece(Some(i), &d.key, dpx, thick, draw_frac, gap, 1.0, open, icon_tint(low), ring_color, alpha, ring_gap_after)
                    .unwrap_or_else(|| {
                        let (base, mut arc) = ring_masks_with(&icon, dpx, thick, draw_frac, gap, 1.0);
                        if open > 0.0 {
                            let bolt = ring_bolt_mask(&icon, dpx, thick, gap, open);
                            for (v, a) in arc.a.iter_mut().zip(&bolt.a) {
                                *v = (*v).max(*a);
                            }
                        }
                        let mut p = Piece::new(Some(i), base, icon_tint(low), alpha, ring_gap_after);
                        p.second = Some((arc, ring_color));
                        p.glow = glow;
                        p
                    });
                pieces.push(p);
                if badge_fade > 0.0 {
                    let a = (alpha as f32 * badge_fade).round() as u32;
                    if let Some(b) = badge_piece(Some(i), &d.via, &d.name, d.hw.map(|h| h.0), badge_icon_font, light, a, 0) {
                        pieces.push(b);
                    }
                }
                continue;
            }
            let mut icon_piece = match custom_piece(Some(i), &d.key, px(24.0), icon_tint(low), alpha, px(5.0)) {
                Some(p) => p,
                None => {
                    let icon = device_icon(d, icons, light);
                    match buds_low {
                        // Each bud of the drawn earbuds in its own colour (left bud in the left half).
                        Some((l, r)) if d.kind == DeviceKind::Earbuds => {
                            let half = icon.w / 2;
                            Piece::split(Some(i), icon, half, false, (icon_tint(l), icon_tint(r)), alpha, px(5.0))
                        }
                        _ => Piece::new(Some(i), icon, icon_tint(low), alpha, px(5.0)),
                    }
                }
            };
            icon_piece.glow = glow;
            pieces.push(icon_piece);
            // The level is followed immediately by the connection label, then the bolt.
            let gap = if mark || badge_fade > 0.0 { px(2.0) } else { 0 };
            let both = d.buds.filter(|b| d.percent >= 0 && b.left >= 0 && b.right >= 0);
            let piece = match both {
                // Earbuds with both buds out: each bud's own level, each red on its own. On a
                // vertical taskbar the two don't fit side by side: left over right.
                Some(b) => {
                    let (l, r) = buds_low.unwrap_or((low, low));
                    let (ml, mr) = (mask(&format!("{}%", b.left), text, light), mask(&format!("{}%", b.right), text, light));
                    if vertical {
                        let at = ml.h;
                        Piece::split(Some(i), stack_masks(&ml, &mr, px(1.0)), at, true, (tint(l), tint(r)), alpha, gap)
                    } else {
                        let m = mask(&format!("{}% {}%", b.left, b.right), text, light);
                        let at = ml.w + (m.w - ml.w - mr.w).max(0) / 2;
                        Piece::split(Some(i), m, at, false, (tint(l), tint(r)), alpha, gap)
                    }
                }
                None => {
                    let t = if !d.level_text.is_empty() && d.percent < 0 {
                        d.level_text.clone()
                    } else if d.percent >= 0 {
                        format!("{}%", d.percent)
                    } else {
                        "?".into()
                    };
                    Piece::new(Some(i), mask(&t, text, light), tint(low), alpha, gap)
                }
            };
            pieces.push(piece);
            if badge_fade > 0.0 {
                let a = (alpha as f32 * badge_fade).round() as u32;
                if let Some(b) = badge_piece(Some(i), &d.via, &d.name, d.hw.map(|h| h.0), badge_icon_font, light, a, px(2.0)) {
                    pieces.push(b);
                }
            }
            if let Some(c) = bolt_color {
                pieces.push(Piece::new(Some(i), mask("\u{E945}", bolt, light), c, alpha, if badge_fade > 0.0 { px(2.0) } else { 0 }));
            }
        }
        pieces
    }

    /// Renders one widget for `t` into `v` (pixels, size, hit ranges, animations). `strip` is its
    /// window (the device held by the mouse is drawn there only).
    fn render(&mut self, v: &mut View, items: &[Item], t: Target, strip: HWND) {
        let pieces = self.pieces(items, t.s, t.vertical);
        let drag_here = self.drag.is_some() && strip == self.drag_on;
        if t.vertical {
            let w = t.extent.clamp((32.0 * t.s).round() as i32, (160.0 * t.s).round() as i32);
            self.render_column(v, &pieces, items, w, t.s, drag_here);
        } else {
            self.render_row(v, &pieces, items, t.extent, t.s, t.shift_y, drag_here);
        }
        v.scale = t.s;
        v.vertical = t.vertical;
        v.lit = lit_pixels(&v.px, v.size, t.s, self.light);
    }

    /// Horizontal taskbar: the devices side by side, `bar_h` high.
    #[allow(clippy::too_many_arguments)]
    fn render_row(&mut self, v: &mut View, pieces: &[Piece], items: &[Item], bar_h: i32, s: f32, shift_y: f32, drag_here: bool) {
        let px = |x: f32| (x * s).round() as i32;
        let light = self.light;
        let h = bar_h.clamp(px(24.0), px(64.0));
        // The padding comes and goes with the first item and the last one.
        let fmax = items.iter().map(|it| it.f).fold(0.0, f32::max);
        let pad = px(8.0) as f32 * fmax;
        let dev_gap = px(14.0) as f32;
        // Lay out: each item at its full width, taking only its share of it (the ones after it
        // follow at once), so a device appearing makes room before it pops in.
        let n = items.len();
        let compact = self.compact && pieces.iter().all(|p| p.m.h * 2 + px(1.0) <= h);
        let mut xs = vec![0i32; pieces.len()];
        let mut ys = vec![None; pieces.len()];
        let mut ranges: Vec<(i32, i32)> = vec![(0, 0); n];
        let mut x = pad;
        let mut k = 0;
        for i in 0..n {
            if i > 0 {
                x += dev_gap * items[i - 1].f.min(items[i].f);
            }
            let start = x.round() as i32;
            let mut cx = 0;
            let first = k;
            while k < pieces.len() && pieces[k].dev == Some(i) {
                xs[k] = start + cx;
                cx += pieces[k].m.w + if pieces.get(k + 1).is_some_and(|q| q.dev == Some(i)) { pieces[k].gap_after } else { 0 };
                k += 1;
            }
            if compact && k > first + 1 {
                let icon = &pieces[first];
                let row_start = icon.m.w + icon.gap_after;
                let row_w = cx - row_start;
                let row_h = pieces[first + 1..k].iter().map(|p| p.m.h).max().unwrap_or(0);
                // Compact layout puts the small pieces under the icon: keep a connection mark
                // clear of the ring there too.
                let gap = if self.ring { px(10.0) } else { px(1.0) };
                let y = (h - icon.m.h - gap - row_h) / 2;
                cx = icon.m.w.max(row_w);
                xs[first] = start + (cx - icon.m.w) / 2;
                ys[first] = Some(y);
                for j in first + 1..k {
                    xs[j] += (cx - row_w) / 2 - row_start;
                    ys[j] = Some(y + icon.m.h + gap + (row_h - pieces[j].m.h) / 2);
                }
            }
            ranges[i] = (start, start + cx);
            x += cx as f32 * items[i].f;
        }
        for xk in xs.iter_mut().skip(k) {
            x += px(10.0) as f32 * fmax.min(if n > 0 { 1.0 } else { 0.0 });
            *xk = x.round() as i32;
            x += pieces[k].m.w as f32;
        }
        let w = ((x + pad).round() as i32).max(1);
        // Alpha 1 everywhere: fully transparent pixels of a layered window let clicks fall through.
        let mut cv = Canvas { w, h, px: vec![0x0100_0000; (w * h) as usize] };
        if self.background {
            // Rounded plate in the taskbar's own colour: keeps the text readable on a transparent
            // or restyled taskbar (e.g. TranslucentTB, Windhawk's Taskbar Styler).
            let plate = if light { rgb(243, 243, 243) } else { rgb(32, 32, 32) };
            cv.round_rect(px(4.0), px(6.0), plate, 235);
        }
        // Each visible device owns its pieces plus half of the gap around them.
        v.hits.clear();
        for (i, it) in items.iter().enumerate() {
            if let Some(vi) = it.vis {
                let from = v.hits.last().map(|h| h.1).unwrap_or(0);
                v.hits.push((from, ranges[i].1 + (dev_gap / 2.0) as i32, vi));
            }
        }
        let keys: Vec<String> = items.iter().map(|it| it.key.clone()).collect();
        let moves = self.motion(v, &keys, &ranges, w, s, drag_here);
        // The dragged device last, over the others.
        let held = |p: &Piece| p.dev.is_some_and(|i| moves[i].2 < 255);
        for pass in [false, true] {
            for ((p, &x), &y) in pieces.iter().zip(&xs).zip(&ys) {
                if held(p) != pass {
                    continue;
                }
                let (off, blur, alpha) = p.dev.map(|i| moves[i]).unwrap_or((0, 0, 255));
                let y0 = (y.unwrap_or((h - p.m.h) / 2) + px(shift_y)).clamp(0, (h - p.m.h).max(0));
                if p.glow && p.dev.is_some_and(|i| items[i].p >= 1.0) {
                    let r = px(3.0).max(1);
                    cv.draw(&glow_mask(&p.m, r), x + off - r, y0 - r, p.color, p.alpha * alpha / 255);
                }
                if let Some(rgba) = &p.rgba {
                    cv.draw_rgba(rgba, p.m.w, x + off, y0, p.alpha * alpha / 255);
                    continue;
                }
                for (m, color) in p.layers() {
                    if let Some(i) = p.dev
                        && items[i].p < 1.0
                    {
                        let c = ((ranges[i].0 + ranges[i].1) as f32 / 2.0 + off as f32, h as f32 / 2.0);
                        draw_popping(&mut cv, m, (x + off, y0), c, items[i].p, s, color, p.alpha * alpha / 255);
                    } else if blur > 0 {
                        let b = blur_mask(m, blur, true);
                        cv.draw(&b, x + off - blur, y0, color, p.alpha * alpha / 255);
                    } else {
                        cv.draw(m, x + off, y0, color, p.alpha * alpha / 255);
                    }
                }
            }
        }
        if let Some(last) = v.hits.last_mut() {
            last.1 = w;
        }
        self.charge_light(&mut cv, items, &ranges, &moves, false);
        v.size = SIZE { cx: w, cy: h };
        v.px = cv.px;
    }

    /// "Charging animation": the light passing over every charging device that is fully shown.
    fn charge_light(&self, cv: &mut Canvas, items: &[Item], ranges: &[(i32, i32)], moves: &[(i32, i32, u32)], vertical: bool) {
        if !self.charge_anim {
            return;
        }
        let t = (self.charge_t0.elapsed().as_secs_f32() * 1000.0 % CHARGE_SWEEP_MS) / CHARGE_SWEEP_MS;
        for (i, it) in items.iter().enumerate() {
            if let Some(d) = &it.dev
                && d.is_charging()
                && !d.is_full()
                && it.p >= 1.0
            {
                let off = moves.get(i).map_or(0, |m| m.0);
                charge_sweep(cv, ranges[i].0 + off, ranges[i].1 + off, vertical, ui::charging_color(self.light), t);
            }
        }
    }

    /// TIMER_BADGE runs for one pop of the connection badges, after the menu toggle.
    fn sync_badge_timer(&self) {
        unsafe {
            if self.badge_at.is_some() {
                SetTimer(Some(self.main), TIMER_BADGE, 15, None);
            } else {
                let _ = KillTimer(Some(self.main), TIMER_BADGE);
            }
        }
    }

    /// Starts the ring's animation when the new read `s` changes the device's power state.
    fn track_ring_power(&mut self, s: &DeviceStatus, now: Instant) {
        if s.percent < 0 {
            // No level, no gauge: nothing to animate.
            self.ring_charge.remove(&s.key);
            return;
        }
        let was = self.devices.get(&s.key).map(|t| Self::ring_power(&t.status)).unwrap_or(RingPower::Off);
        let now_power = Self::ring_power(s);
        if was != now_power {
            let from = self.ring_charge.get(&s.key).map(|a| a.to).unwrap_or(was);
            self.ring_charge.insert(s.key.clone(), RingAnim { at: now, from, to: now_power });
        }
    }

    /// Colour of the ring and the bolt in its gap while on power.
    fn ring_power_color(d: &DeviceStatus, light: bool) -> COLORREF {
        ui::bolt_color(d, light).unwrap_or_else(|| ui::charging_color(light))
    }

    fn ring_power(d: &DeviceStatus) -> RingPower {
        if d.is_full() {
            RingPower::Full
        } else if d.is_charging() {
            RingPower::Charging
        } else {
            RingPower::Off
        }
    }

    fn ring_anim_color(state: RingPower, light: bool, base: COLORREF) -> COLORREF {
        match state {
            RingPower::Off => base,
            RingPower::Charging => ui::charging_color(light),
            RingPower::Full => ui::full_color(light),
        }
    }

    /// Ring colour during a transition. From 100% on unplug it goes straight from blue to the
    /// normal icon colour; charging green is only an endpoint when it actually was the start state.
    fn ring_color(&self, key: &str, d: &DeviceStatus, light: bool, base: COLORREF, now: Instant, animating: bool) -> COLORREF {
        let Some(a) = self.ring_charge.get(key).filter(|_| animating) else {
            return if Self::ring_power(d) == RingPower::Off { base } else { Self::ring_power_color(d, light) };
        };
        let p = (now.saturating_duration_since(a.at).as_secs_f32() * 1000.0 / RING_CHARGE_MS).clamp(0.0, 1.0);
        let eased = p * p * (3.0 - 2.0 * p);
        mix_color(Self::ring_anim_color(a.from, light, base), Self::ring_anim_color(a.to, light, base), eased)
    }

    /// `(open, fill, animating)` for a ring: `open` 0..1 moves the top segment aside and fades in
    /// the bolt. `fill` is always the reported level — the gap does not turn 95% into 100%.
    fn ring_charge_state(&self, key: &str, on_power: bool, percent: i32, now: Instant) -> (f32, f32, bool) {
        let actual = (percent.clamp(0, 100) as f32) / 100.0;
        match self.ring_charge.get(key) {
            None => (if on_power { 1.0 } else { 0.0 }, actual, false),
            Some(a) => {
                let p = (now.saturating_duration_since(a.at).as_secs_f32() * 1000.0 / RING_CHARGE_MS).clamp(0.0, 1.0);
                let eased = p * p * (3.0 - 2.0 * p);
                // Only plugging / unplugging moves the arc. Charging -> full is a colour-only fade.
                let open = match (a.from, a.to) {
                    (RingPower::Off, RingPower::Off) => 0.0,
                    (RingPower::Off, _) => eased,
                    (_, RingPower::Off) => 1.0 - eased,
                    _ => 1.0,
                };
                (open, actual, p < 1.0)
            }
        }
    }

    fn prune_ring_charge(&mut self) {
        let now = Instant::now();
        self.ring_charge.retain(|_, a| now.saturating_duration_since(a.at).as_secs_f32() * 1000.0 < RING_CHARGE_MS);
    }

    fn ring_charge_animating(&self) -> bool {
        let now = Instant::now();
        self.ring_charge.values().any(|a| now.saturating_duration_since(a.at).as_secs_f32() * 1000.0 < RING_CHARGE_MS)
    }

    fn sync_ring_charge_timer(&self) {
        unsafe {
            if self.ring_charge_animating() {
                SetTimer(Some(self.main), TIMER_RING_CHARGE, 15, None);
            } else {
                let _ = KillTimer(Some(self.main), TIMER_RING_CHARGE);
            }
        }
    }

    /// The charging animation runs while it is on and some shown device is charging.
    fn charging_shown(&self) -> bool {
        self.charge_anim && !self.fallback && self.visible().iter().any(|d| d.is_charging() && !d.is_full())
    }

    fn sync_charge_timer(&self) {
        unsafe {
            if self.charging_shown() {
                SetTimer(Some(self.main), TIMER_CHARGE, CHARGE_FRAME_MS, None);
            } else {
                let _ = KillTimer(Some(self.main), TIMER_CHARGE);
            }
        }
    }

    /// Vertical taskbar: each device as its icon over its level (the bolt beside the level when it
    /// fits), devices stacked, as wide as the taskbar. Hit ranges are rows (y).
    fn render_column(&mut self, v: &mut View, pieces: &[Piece], items: &[Item], w: i32, s: f32, drag_here: bool) {
        let light = self.light;
        let px = |x: f32| (x * s).round() as i32;
        let sizes: Vec<(Option<usize>, i32, i32)> = pieces.iter().map(|p| (p.dev, p.m.w, p.m.h)).collect();
        let (pad, dev_gap) = (px(8.0), px(12.0));
        let (placed, full_h, rows) = column_layout(&sizes, w, pad, px(2.0), dev_gap, px(2.0));
        let n = items.len();
        let mut ranges: Vec<(i32, i32)> = vec![(i32::MAX, 0); n];
        for (p, &(_, y, show)) in pieces.iter().zip(&placed) {
            if let (Some(i), true) = (p.dev, show) {
                ranges[i] = (ranges[i].0.min(y), ranges[i].1.max(y + p.m.h));
            }
        }
        // What each item gives back of its height while it appears or leaves: the items below it
        // move up by what the ones above gave back (the column stays on the notification area).
        let fmax = items.iter().map(|it| it.f).fold(0.0, f32::max);
        let mut gone = (1.0 - fmax) * pad as f32;
        let mut shift = vec![0i32; n];
        for i in 0..n {
            shift[i] = gone.round() as i32;
            gone += (1.0 - items[i].f) * (ranges[i].1 - ranges[i].0).max(0) as f32;
            if i + 1 < n {
                gone += (1.0 - items[i].f.min(items[i + 1].f)) * dev_gap as f32;
            }
        }
        let tail = gone.round() as i32;
        let h = (full_h - tail - ((1.0 - fmax) * pad as f32).round() as i32).max(1);
        for (i, r) in ranges.iter_mut().enumerate() {
            *r = (r.0 - shift[i], r.1 - shift[i]);
        }
        let ys: Vec<i32> = pieces.iter().zip(&placed).map(|(p, &(_, y, _))| y - p.dev.map(|i| shift[i]).unwrap_or(tail)).collect();
        let mut hits: Vec<(i32, i32, usize)> = rows.iter().filter_map(|&(a, b, i)| items[i].vis.map(|vi| (a - shift[i], b - shift[i], vi))).collect();
        if let Some(first) = hits.first_mut() {
            first.0 = 0;
        }
        if let Some(last) = hits.last_mut() {
            last.1 = h;
        }
        let mut cv = Canvas { w, h, px: vec![0x0100_0000; (w * h) as usize] };
        if self.background {
            let plate = if light { rgb(243, 243, 243) } else { rgb(32, 32, 32) };
            cv.round_rect(0, px(6.0), plate, 235);
        }
        let keys: Vec<String> = items.iter().map(|it| it.key.clone()).collect();
        let moves = self.motion(v, &keys, &ranges, h, s, drag_here);
        let held = |p: &Piece| p.dev.is_some_and(|i| moves[i].2 < 255);
        for pass in [false, true] {
            for ((p, &(x, _, show)), &y) in pieces.iter().zip(&placed).zip(&ys) {
                if !show || held(p) != pass {
                    continue;
                }
                let (off, blur, alpha) = p.dev.map(|i| moves[i]).unwrap_or((0, 0, 255));
                if p.glow && p.dev.is_some_and(|i| items[i].p >= 1.0) {
                    let r = px(3.0).max(1);
                    cv.draw(&glow_mask(&p.m, r), x - r, y + off - r, p.color, p.alpha * alpha / 255);
                }
                if let Some(rgba) = &p.rgba {
                    cv.draw_rgba(rgba, p.m.w, x, y + off, p.alpha * alpha / 255);
                    continue;
                }
                for (m, color) in p.layers() {
                    if let Some(i) = p.dev
                        && items[i].p < 1.0
                    {
                        let c = (w as f32 / 2.0, (ranges[i].0 + ranges[i].1) as f32 / 2.0 + off as f32);
                        draw_popping(&mut cv, m, (x, y + off), c, items[i].p, s, color, p.alpha * alpha / 255);
                    } else if blur > 0 {
                        cv.draw(&blur_mask(m, blur, false), x, y + off - blur, color, p.alpha * alpha / 255);
                    } else {
                        cv.draw(m, x, y + off, color, p.alpha * alpha / 255);
                    }
                }
            }
        }
        self.charge_light(&mut cv, items, &ranges, &moves, true);
        v.hits = hits;
        v.size = SIZE { cx: w, cy: h };
        v.px = cv.px;
    }

    /// The widget of `strip` (the main taskbar's or a copy's).
    fn view(&self, strip: HWND) -> Option<&View> {
        if strip == self.strip { Some(&self.mv) } else { self.views.get(&(strip.0 as isize)) }
    }

    fn view_mut(&mut self, strip: HWND) -> Option<&mut View> {
        if strip == self.strip { Some(&mut self.mv) } else { self.views.get_mut(&(strip.0 as isize)) }
    }

    /// Whether `strip` is drawn for a vertical taskbar.
    fn strip_vertical(&self, strip: HWND) -> bool {
        self.view(strip).map_or(self.vertical, |v| v.vertical)
    }

    /// The main widget's place and order (every widget's with "Mirror other widgets" on).
    fn main_layout(&self) -> Layout {
        Layout { side: self.side, offset: self.offset, offset_y: self.offset_y, free_x: self.free_x, order: self.saved_order.clone() }
    }

    /// `strip` is a copy with its own place and order ("Mirror other widgets" off).
    fn own_layout(&self, strip: HWND) -> bool {
        !self.mirroring && strip != self.strip && self.mirrors.iter().chain(self.parked.iter()).any(|m| m.1 == strip)
    }

    /// Place and order of the widget in `strip`.
    fn layout_of(&self, strip: HWND) -> Layout {
        if self.own_layout(strip)
            && let Some(lay) = self.view(strip).and_then(|v| v.lay.clone())
        {
            return lay;
        }
        if self.own_layout(strip) {
            return load_layout(self.bar_of(strip)).unwrap_or_else(|| self.main_layout());
        }
        self.main_layout()
    }

    /// Taskbar of `strip` (the main one for the main strip or an unknown window).
    fn bar_of(&self, strip: HWND) -> HWND {
        self.mirrors.iter().chain(self.parked.iter()).find(|m| m.1 == strip).map_or(self.taskbar, |m| m.0)
    }

    /// Changes the place and order of the widget in `strip` (every widget's while they mirror the
    /// main one), saved when `save`, and moves the widgets there.
    fn change_layout(&mut self, strip: HWND, save: bool, f: impl FnOnce(&mut Layout)) {
        let before = self.layout_of(strip);
        let mut lay = before.clone();
        f(&mut lay);
        if self.own_layout(strip) {
            let bar = self.bar_of(strip);
            if save {
                save_layout(&widget_key(bar), &lay);
                settings::set_order(&widget_key(bar), &lay.order);
            }
            let reorder = lay.order != before.order;
            if let Some(v) = self.view_mut(strip) {
                v.lay = Some(lay.clone());
                v.reordered |= reorder;
            }
        } else {
            if save {
                save_dword("WidgetLeft", lay.side);
                save_dword("WidgetFreeX", lay.free_x as u32);
                for (name, v) in [("TaskbarOffset", lay.offset), ("TaskbarOffsetY", lay.offset_y as u32)] {
                    if v == 0 {
                        batteryhub::util::reg_delete_value(SETTINGS_KEY, name);
                    } else {
                        save_dword(name, v);
                    }
                }
            }
            if lay.offset_y != self.offset_y {
                self.y_glide = Some((self.shown_offset_y(), Instant::now()));
                unsafe {
                    SetTimer(Some(self.main), TIMER_GLIDE, 15, None);
                }
            }
            self.side = lay.side;
            self.align_left = lay.side != SIDE_RIGHT;
            self.after_icons = lay.side == SIDE_ICONS;
            self.offset = lay.offset;
            self.offset_y = lay.offset_y;
            self.free_x = lay.free_x;
        }
        if lay.side != before.side {
            // A new place: the taskbar's items are checked again at once (not a minute-old answer).
            batteryhub::tray_probe::left_refresh_soon();
        }
        self.last_place = Default::default();
        self.paint();
        self.place();
        self.sync_mirrors();
    }

    /// "Change position": the widget in `strip` follows the mouse along its taskbar until a click.
    fn begin_move(&mut self, strip: HWND) {
        let strip = if self.view(strip).is_some() { strip } else { self.strip };
        if strip.0.is_null() || self.strip_vertical(strip) {
            return;
        }
        let before = self.layout_of(strip);
        let (mut rc, mut pt) = (RECT::default(), POINT::default());
        unsafe {
            let _ = GetWindowRect(strip, &mut rc);
            let _ = GetCursorPos(&mut pt);
        }
        // Held where the mouse is on it, or by its middle (the menu was elsewhere).
        let grab = if pt.x >= rc.left && pt.x < rc.right { pt.x - rc.left } else { (rc.right - rc.left) / 2 };
        self.moving = Some((strip, grab, before));
        self.hide_tip();
        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(strip);
        }
        self.move_step();
    }

    /// While moving: the widget's left edge follows the mouse (within its taskbar).
    fn move_step(&mut self) {
        let Some((strip, grab, _)) = self.moving.clone() else { return };
        let bar = self.bar_of(strip);
        let s = self.view(strip).map_or(1.0, |v| v.scale);
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let mut p = POINT { x: pt.x - grab, y: pt.y };
        unsafe {
            let _ = ScreenToClient(bar, &mut p);
        }
        let x = (p.x.max(0) as f32 / s).round() as i32;
        if self.layout_of(strip).side == SIDE_FREE && self.layout_of(strip).free_x == x {
            return;
        }
        self.change_layout(strip, false, |l| {
            l.side = SIDE_FREE;
            l.free_x = x;
        });
    }

    /// A click ends "Change position": the place is kept (`keep`) or the old one restored.
    fn end_move(&mut self, keep: bool) {
        let Some((strip, _, before)) = self.moving.take() else { return };
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        let now = self.layout_of(strip);
        self.change_layout(strip, true, |l| *l = if keep { now } else { before });
    }

    /// Left edge (client x of `bar`) of a widget `w` wide on the horizontal taskbar `bar` (screen
    /// rectangle `br`, notification area from screen x `right`) with layout `lay`. None while
    /// the taskbar's items are not known yet (WM_TRAY_PROBED places it once they are).
    #[allow(clippy::too_many_arguments)]
    fn widget_left(&self, bar: HWND, br: RECT, right: i32, w: i32, s: f32, lay: &Layout) -> Option<i32> {
        let to_client = |x: i32| {
            let mut p = POINT { x, y: br.top };
            unsafe {
                let _ = ScreenToClient(bar, &mut p);
            }
            p.x
        };
        let gap = (6.0 * s) as i32;
        // Before the icons needs centred icons: at the left end otherwise.
        let side = if lay.side == SIDE_BEFORE && !taskbar_centred() { SIDE_LEFT } else { lay.side };
        let exclude = || screen_spans(bar, &foreign_widgets(bar));
        use batteryhub::tray_probe as tp;
        Some(match side {
            SIDE_LEFT => to_client(tp::left_content_end(bar, br, self.main, WM_TRAY_PROBED)?) + gap,
            SIDE_ICONS => to_client(tp::icons_end(bar, br, right, &exclude(), self.main, WM_TRAY_PROBED)?) + gap,
            SIDE_BEFORE => (to_client(tp::icons_start(bar, br, right, &exclude(), self.main, WM_TRAY_PROBED)?) - w - gap).max(0),
            SIDE_FREE => ((lay.free_x as f32 * s).round() as i32).clamp(0, (to_client(right) - w - (2.0 * s) as i32).max(0)),
            _ => {
                let mut left = to_client(right) - w - (2.0 * s) as i32;
                // Optional extra gap (DPI-independent px), e.g. to sit next to another taskbar widget.
                if lay.offset > 0 {
                    left -= (lay.offset as f32 * s) as i32;
                }
                // Other programs' widgets in the taskbar: sit left of them instead of on top.
                avoid_widgets(bar, left, w, (4.0 * s) as i32)
            }
        })
    }

    /// Some widget has a device sliding to a new place.
    fn sliding(&self) -> bool {
        !self.mv.slides.is_empty() || self.views.values().any(|v| !v.slides.is_empty())
    }

    /// TIMER_SLIDE runs while devices slide, appear or leave on any widget.
    fn sync_slide_timer(&self) {
        unsafe {
            if !self.sliding() && self.presence.is_empty() {
                let _ = KillTimer(Some(self.main), TIMER_SLIDE);
            } else {
                SetTimer(Some(self.main), TIMER_SLIDE, 15, None);
            }
        }
    }

    /// Shows or removes the hover plate when the mouse enters or leaves a strip.
    fn set_hovered(&mut self, on: HWND) {
        if on == self.hovered {
            return;
        }
        let old = std::mem::replace(&mut self.hovered, on);
        for strip in [old, on] {
            if !strip.0.is_null() && is_our_strip(strip) {
                self.push(strip);
            }
        }
    }

    /// Hands the last render to a strip window (the main strip or a mirror).
    fn push(&self, strip: HWND) {
        let Some(v) = self.view(strip) else { return };
        let px = if strip == self.hovered && v.lit.len() == v.px.len() { &v.lit } else { &v.px };
        self.push_px_at(strip, px, v.size, None);
    }



    /// Like push_px, and moves the window to `at` (screen) in the same step: a copy that changes
    /// size and place shows the new image in the new place at once, never for a frame in between.
    fn push_px_at(&self, strip: HWND, px: &[u32], size: SIZE, at: Option<POINT>) {
        let (w, h) = (size.cx, size.cy);
        if strip.0.is_null() || px.len() != (w * h) as usize {
            return;
        }
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
                let ob = SelectObject(mem, HGDIOBJ(bmp.0));
                let src = POINT::default();
                let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                let _ = UpdateLayeredWindow(strip, Some(screen), at.as_ref().map(|p| p as *const POINT), Some(&size), Some(mem), Some(&src), COLORREF(0), Some(&bf), ULW_ALPHA);
                SelectObject(mem, ob);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }

    // ---- taskbar placement ---------------------------------------------------------------------

    /// What the strip has to be rendered for right now: taskbar height, DPI and theme.
    fn render_target(&self) -> (i32, u32, bool) {
        let mut client = RECT::default();
        unsafe {
            let _ = GetClientRect(self.taskbar, &mut client);
            ((client.right - client.left) << 12 | (client.bottom - client.top), GetDpiForWindow(self.taskbar), self.light)
        }
    }

    /// Creates the strip as a child of the taskbar (again, after Explorer restarted). Returns false
    /// when the taskbar cannot host it right now: no taskbar / notification area (Explorer is
    /// restarting or uses an unknown layout), or a vertical taskbar.
    fn ensure_strip(&mut self) -> bool {
        unsafe {
            // The strip is only valid while it still is a window and a child of a live taskbar; a
            // handle that outlived Explorer can be reused by an unrelated window.
            let alive = !self.strip.0.is_null()
                && IsWindow(Some(self.strip)).as_bool()
                && IsWindow(Some(self.taskbar)).as_bool()
                && has_sdtaskbar(self.taskbar) == self.strip_popup
                && if self.strip_popup {
                    GetWindow(self.strip, GW_OWNER).is_ok_and(|p| p == self.taskbar)
                } else {
                    GetParent(self.strip).is_ok_and(|p| p == self.taskbar)
                };
            if alive && MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
                // Don't tear down the menu's owner because of a momentary layout change.
                return true;
            }
            if !alive {
                if is_our_strip(self.strip) {
                    let _ = DestroyWindow(self.strip);
                }
                self.strip = HWND::default();
                self.strip_popup = false;
                let Ok(taskbar) = FindWindowW(w!("Shell_TrayWnd"), None) else {
                    dbg_log("no Shell_TrayWnd");
                    return false;
                };
                if FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), None).is_err() {
                    dbg_log("no TrayNotifyWnd");
                    return false;
                }
                self.taskbar = taskbar;
            }
            let mut bar = RECT::default();
            let _ = GetWindowRect(self.taskbar, &mut bar);
            let vertical = bar.bottom - bar.top > bar.right - bar.left;
            if vertical != self.vertical {
                // Turned between horizontal and vertical: lay the strip out again.
                self.vertical = vertical;
                self.painted_for = Default::default();
                self.last_place = Default::default();
            }
            if alive {
                return true;
            }
            let popup = has_sdtaskbar(self.taskbar);
            let Some(strip) = (if popup { create_copy(self.taskbar) } else { create_strip(self.taskbar) }) else {
                return false;
            };
            dbg_log(&format!("strip created {:?} parent {:?} popup {}", strip, self.taskbar, popup));
            self.strip = strip;
            self.strip_popup = popup;
            self.last_place = Default::default();
            // Shown only on another monitor: the new main strip stays hidden from the start (no
            // flash on the main taskbar before the first sync hides it).
            self.primary_hidden = self.chosen_bars().1;
        }
        if !self.fallback {
            self.paint();
            // While copies on other monitors still wait for their place, place() shows it with them.
            if !self.primary_hidden
                && self.mirrors_ready
                && !(self.strip_popup && unsafe { fullscreen_over(self.taskbar) || !IsWindowVisible(self.taskbar).as_bool() })
            {
                unsafe {
                    let _ = ShowWindow(self.strip, SW_SHOWNOACTIVATE);
                }
            }
        }
        true
    }

    /// Destroys any other strip window of this process on the taskbar (only `self.strip` may exist).
    fn remove_orphan_strips(&self) {
        unsafe {
            let mut after: Option<HWND> = None;
            while let Ok(h) = FindWindowExW(Some(self.taskbar), after, STRIP_CLASS, None) {
                if h.0.is_null() {
                    break;
                }
                after = Some(h);
                if h != self.strip && is_our_strip(h) {
                    dbg_log(&format!("removing orphaned strip {h:?}"));
                    let _ = DestroyWindow(h);
                }
            }
        }
    }

    /// Switches between the taskbar strip and notification-area icons.
    fn set_fallback(&mut self, on: bool) {
        if self.fallback == on {
            return;
        }
        dbg_log(&format!("fallback to notification-area icons: {on}"));
        self.fallback = on;
        // Recomputed by the next tick (sync_crowded) once the strip is back.
        self.crowded = false;
        self.crowded_ticks = 0;
        self.short_ticks.clear();
        if on {
            self.hide_tip();
            self.drop_mirrors();
            if !self.strip.0.is_null() {
                unsafe {
                    let _ = ShowWindow(self.strip, SW_HIDE);
                }
            }
            self.sync_tray();
        } else {
            self.remove_tray();
            self.last_place = Default::default();
            self.paint();
        }
    }

    /// One tick of the 1 s placement timer: keep the strip in place, or fall back.
    fn tick(&mut self) {
        if std::env::var_os("BH_FORCE_FALLBACK").is_some() {
            return;
        }
        if self.icons_mode {
            // Notification-area icons by choice: Windows owns their place and order.
            if !self.fallback {
                self.set_fallback(true);
            }
            return;
        }
        if self.ensure_strip() {
            self.remove_orphan_strips();
            self.fail_ticks = 0;
            if self.fallback {
                self.set_fallback(false);
            }
            // Re-render when the taskbar changed height, DPI or theme since the last render
            // (e.g. after leaving a full-screen game).
            if self.render_target() != self.painted_for {
                self.paint();
            } else {
                self.place();
            }
            self.sync_mirrors();
            self.sync_crowded();
            self.sync_app_icon();
        } else {
            self.fail_ticks += 1;
            if self.fail_ticks >= FALLBACK_AFTER_TICKS {
                self.set_fallback(true);
            }
        }
    }

    /// Drops the last room answer for `bar`. A placement that could not be decided (UI Automation
    /// still pending, taskbar hidden by a game, copy not rendered yet...) must not leave an old
    /// "no room" behind for this tick to announce.
    fn forget_room(&self, bar: HWND) {
        self.rooms.borrow_mut().remove(&(bar.0 as isize));
    }

    /// No taskbar the widget is shown on has room beside its task buttons: the levels move to
    /// notification-area icons (said once in a notification) until one has room again.
    fn sync_crowded(&mut self) {
        // A mode switch while the menu is up would hide the strip that owns it (#609, menu).
        if MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let mut bars: Vec<isize> = self.mirrors.iter().map(|m| m.0.0 as isize).collect();
        if !self.primary_hidden {
            bars.push(self.taskbar.0 as isize);
        }
        let crowded = {
            let rooms = self.rooms.borrow();
            !bars.is_empty() && bars.iter().all(|b| rooms.get(b) == Some(&false))
        };
        // One taskbar without room while others have it: said for that monitor.
        let short: HashSet<isize> = {
            let rooms = self.rooms.borrow();
            bars.iter().filter(|b| rooms.get(b) == Some(&false)).copied().collect()
        };
        // A tray-icon change lasting a single tick used to send a notification and switch to
        // notification-area icons, then switch straight back (#609). Require the answer to hold
        // for a few ticks before saying it or changing modes.
        self.short_ticks.retain(|b, _| short.contains(b));
        let new: Vec<isize> = short.iter().filter_map(|b| {
            let n = self.short_ticks.entry(*b).or_insert(0);
            *n += 1;
            (*n == CROWDED_CONFIRM_TICKS).then_some(*b)
        }).collect();
        if !crowded {
            for b in new {
                if self.short_said.get(&b).is_none_or(|t| t.elapsed().as_secs() >= 600) {
                    self.short_said.insert(b, Instant::now());
                    let label = monitor_label(HWND(b as *mut c_void));
                    dbg_log(&format!("not enough room on {label}"));
                    self.notify_info(&trf("Not enough room on the taskbar of {monitor}", &[("monitor", &label)]), &tr(SHORT_ROOM_TEXT));
                }
            }
        }
        if crowded == self.crowded {
            self.crowded_ticks = 0;
            return;
        }
        self.crowded_ticks += 1;
        if self.crowded_ticks < CROWDED_CONFIRM_TICKS {
            return;
        }
        self.crowded_ticks = 0;
        self.crowded = crowded;
        dbg_log(&format!("not enough room on the taskbar: {crowded}"));
        if crowded {
            self.sync_tray();
            if self.crowded_said.is_none_or(|t| t.elapsed().as_secs() >= 600) {
                self.crowded_said = Some(Instant::now());
                self.notify_info(&tr("Not enough room on the taskbar"), &tr("The battery levels are shown as notification-area icons until there is room again. You can also move the widget: right-click it → Widget position."));
            }
        } else if !self.fallback {
            self.remove_tray();
        }
    }

    // ---- other monitors ("Show widget on") -----------------------------------------------------

    /// Keeps one copy of the strip on the taskbar of each extra monitor the user chose (left to
    /// right), next to that taskbar's clock. Taskbars of other monitors exist only when Windows
    /// shows the taskbar on all displays.
    fn sync_mirrors(&mut self) {
        // A menu opened on a copy owns that strip too: keep the copies as they are until it closes.
        if MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if self.fallback || self.strip.0.is_null() {
            self.drop_mirrors();
            self.mirrors_ready = true;
            return;
        }
        let (bars, hide_primary) = self.chosen_bars();
        self.set_primary_hidden(hide_primary);
        // The notification bell or a longer date moves every taskbar's clock: check the Windows 11
        // secondary taskbars again when the main taskbar's notification area changes.
        if !bars.is_empty() {
            unsafe {
                if let Ok(tray) = FindWindowExW(Some(self.taskbar), None, w!("TrayNotifyWnd"), None) {
                    let mut tr = RECT::default();
                    let _ = GetWindowRect(tray, &mut tr);
                    if tr != self.main_tray {
                        self.main_tray = tr;
                        batteryhub::tray_probe::taskbars_changed();
                    }
                }
            }
        }
        // Drop copies whose taskbar is gone or no longer wanted.
        let all_bars = secondary_taskbars();
        let usable = |bar: HWND, strip: HWND| unsafe { IsWindow(Some(strip)).as_bool() && GetWindow(strip, GW_OWNER).is_ok_and(|p| p == bar) };
        let mut keep = Vec::new();
        for (bar, strip) in std::mem::take(&mut self.mirrors) {
            if bars.contains(&bar) && usable(bar, strip) {
                keep.push((bar, strip));
            } else if all_bars.contains(&bar) && usable(bar, strip) {
                // No longer wanted on that monitor: pops out, then waits hidden for its return.
                if unsafe { IsWindowVisible(strip).as_bool() } {
                    self.start_pop(strip, true);
                }
                self.parked.push((bar, strip));
            } else if is_our_strip(strip) {
                unsafe {
                    let _ = DestroyWindow(strip);
                }
            }
        }
        self.mirrors = keep;
        // Parked copies whose taskbar is gone go too.
        for (bar, strip) in std::mem::take(&mut self.parked) {
            if all_bars.contains(&bar) && usable(bar, strip) {
                self.parked.push((bar, strip));
            } else if is_our_strip(strip) {
                self.mirror_in.retain(|(h, _, _)| *h != strip);
                unsafe {
                    let _ = DestroyWindow(strip);
                }
            }
        }
        let mut added = false;
        for bar in bars {
            if self.mirrors.iter().any(|m| m.0 == bar) {
                continue;
            }
            // A parked copy comes back (it pops in when place_mirror_at shows it).
            let strip = match self.parked.iter().position(|m| m.0 == bar) {
                Some(i) => self.parked.remove(i).1,
                None => {
                    let Some(strip) = create_copy(bar) else { continue };
                    dbg_log(&format!("mirror strip {strip:?} on {bar:?}"));
                    strip
                }
            };
            self.mirrors.push((bar, strip));
            added = true;
        }
        // Every other taskbar gets a hidden copy ready, so the widget can appear there smoothly.
        for bar in all_bars {
            if self.mirrors.iter().any(|m| m.0 == bar) || self.parked.iter().any(|m| m.0 == bar) {
                continue;
            }
            if let Some(strip) = create_copy(bar) {
                dbg_log(&format!("parked strip {strip:?} on {bar:?}"));
                self.parked.push((bar, strip));
                added = true;
            }
        }
        if added {
            self.paint(); // their own render, for their own taskbars
        }
        // Stray copies (e.g. left by an Explorer restart): our popups not in either list.
        {
            unsafe {
                let mut after: Option<HWND> = None;
                while let Ok(h) = FindWindowExW(None, after, STRIP_CLASS, None) {
                    if h.0.is_null() {
                        break;
                    }
                    after = Some(h);
                    if h != self.strip
                        && !self.mirrors.iter().any(|m| m.1 == h)
                        && !self.parked.iter().any(|m| m.1 == h)
                        && !self.popping(h)
                        && is_our_strip(h)
                    {
                        let _ = DestroyWindow(h);
                    }
                }
            }
        }
        let mut pending = false;
        for i in 0..self.mirrors.len() {
            pending |= self.place_mirror_at(i);
        }
        // The main strip waits for the copies, so all taskbars show the widget at the same moment.
        let was_ready = self.mirrors_ready;
        self.mirrors_ready = !pending;
        if self.mirrors_ready && !was_ready {
            self.last_place = Default::default();
            self.place();
        }
    }

    /// Taskbars of other monitors that show a copy of the widget, and whether the main taskbar's
    /// strip is hidden (the widget is shown only on one other monitor that is connected).
    fn chosen_bars(&self) -> (Vec<HWND>, bool) {
        let others = secondary_taskbars();
        match self.choice.as_str() {
            "all" => (others, false),
            "main" => (Vec::new(), false),
            id => {
                let ids: Vec<String> = others.iter().map(|b| monitor_id(*b)).collect();
                match find_monitor(&ids, id) {
                    Some(i) => (vec![others[i]], true),
                    None => (Vec::new(), false), // that monitor is not connected: main taskbar meanwhile
                }
            }
        }
    }

    /// Hides the main taskbar's strip while the widget is shown only on another monitor.
    fn set_primary_hidden(&mut self, hide: bool) {
        if self.strip.0.is_null() {
            self.primary_hidden = hide;
            return;
        }
        unsafe {
            if hide {
                if IsWindowVisible(self.strip).as_bool() && !self.primary_hidden {
                    // Leaving the main taskbar for another monitor: pops out, then hides.
                    self.start_pop(self.strip, true);
                } else if IsWindowVisible(self.strip).as_bool() && !self.popping(self.strip) {
                    let _ = ShowWindow(self.strip, SW_HIDE);
                }
                if !self.primary_hidden {
                    self.hide_tip();
                    // Windows 10 layout: give the task buttons their full width back.
                    if let (Ok(rebar), Ok(tray)) =
                        (FindWindowExW(Some(self.taskbar), None, w!("ReBarWindow32"), None), FindWindowExW(Some(self.taskbar), None, w!("TrayNotifyWnd"), None))
                    {
                        let (mut rr, mut tr) = (RECT::default(), RECT::default());
                        let _ = GetWindowRect(rebar, &mut rr);
                        let _ = GetWindowRect(tray, &mut tr);
                        let mut tl = POINT { x: rr.left, y: rr.top };
                        let mut tp = POINT { x: tr.left, y: tr.top };
                        let _ = ScreenToClient(self.taskbar, &mut tl);
                        let _ = ScreenToClient(self.taskbar, &mut tp);
                        if tp.x > tl.x {
                            let _ = MoveWindow(rebar, tl.x, tl.y, tp.x - tl.x, rr.bottom - rr.top, true);
                        }
                    }
                }
            } else if self.primary_hidden {
                self.last_place = Default::default();
                self.primary_hidden = false;
                let was = IsWindowVisible(self.strip).as_bool() && !self.popping(self.strip);
                self.place();
                if !was && IsWindowVisible(self.strip).as_bool() {
                    self.start_pop(self.strip, false);
                }
            }
        }
        self.primary_hidden = hide;
    }

    fn drop_mirrors(&mut self) {
        self.set_primary_hidden(false);
        self.mirror_in.retain(|(h, _, _)| *h == self.strip);
        let mut all = std::mem::take(&mut self.mirrors);
        all.append(&mut self.parked);
        for (_, strip) in all {
            if is_our_strip(strip) {
                unsafe {
                    let _ = DestroyWindow(strip);
                }
            }
        }
    }

    /// Places copy `i`; when that shows it (new, or hidden until now) it pops in with fresh content.
    fn place_mirror_at(&mut self, i: usize) -> bool {
        let (bar, strip) = self.mirrors[i];
        let was = unsafe { IsWindowVisible(strip).as_bool() };
        if was && MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
            return true; // its menu is open: never pop it out now
        }
        let pending = self.place_mirror(bar, strip);
        let out = std::mem::take(&mut *self.pop_out.borrow_mut());
        if out.contains(&strip) {
            // No room on its taskbar any more: pops out, then hides.
            if !self.leaving(strip) {
                self.start_pop(strip, true);
            }
            return pending;
        }
        if self.leaving(strip) && unsafe { IsWindowVisible(strip).as_bool() } {
            // Room again while it was popping out: it turns around.
            self.start_pop(strip, false);
        } else if !was && unsafe { IsWindowVisible(strip).as_bool() } {
            self.start_pop(strip, false);
        } else if !self.popping(strip) && unsafe { IsWindowVisible(strip).as_bool() } {
            // What a copy shows right after it appears can get lost on the way to the screen (it
            // stayed blank until the mouse went over it), so it gets its image again every tick.
            self.push(strip);
        }
        pending
    }

    /// A strip is popping out.
    fn leaving(&self, strip: HWND) -> bool {
        self.mirror_in.iter().any(|(h, _, l)| *h == strip && *l)
    }

    /// A strip is popping in or out (its frames come from TIMER_MIRROR_IN).
    fn popping(&self, strip: HWND) -> bool {
        self.mirror_in.iter().any(|(h, _, _)| *h == strip)
    }

    /// Starts `strip` popping in, or out (`leaving`). A strip turning around part-way continues
    /// from where it is.
    fn start_pop(&mut self, strip: HWND, leaving: bool) {
        let now = Instant::now();
        let t0 = match self.mirror_in.iter().position(|(h, _, _)| *h == strip) {
            Some(i) => {
                let (_, t, was_leaving) = self.mirror_in.remove(i);
                let done = (t.elapsed().as_secs_f32() * 1000.0 / MIRROR_IN_MS).min(1.0);
                if was_leaving == leaving {
                    t
                } else {
                    now - std::time::Duration::from_secs_f32((1.0 - done) * MIRROR_IN_MS / 1000.0)
                }
            }
            None => now,
        };
        self.mirror_in.push((strip, t0, leaving));
        let p = (t0.elapsed().as_secs_f32() * 1000.0 / MIRROR_IN_MS).min(1.0);
        self.mirror_in_frame(strip, if leaving { 1.0 - p } else { p });
        unsafe {
            SetTimer(Some(self.main), TIMER_MIRROR_IN, 15, None);
        }
    }

    /// One frame of a copy popping in (`p` 0 to 1): it grows from 60 % around its centre with a
    /// small overshoot, fades in and is blurred while it moves, like the devices on the strip.
    fn mirror_in_frame(&self, strip: HWND, p: f32) {
        let Some(v) = self.view(strip) else { return };
        if p <= 0.0 {
            self.push_px_at(strip, &vec![0; v.px.len()], v.size, None);
            return;
        }
        if p >= 1.0 {
            self.push(strip);
            return;
        }
        let (c1, c3) = (1.70158f32, 2.70158f32);
        let back = 1.0 + c3 * (p - 1.0).powi(3) + c1 * (p - 1.0).powi(2);
        let k = (0.6 + 0.4 * back).max(0.05);
        let blur = ((1.0 - p) * (1.0 - p) * 4.0 * v.scale).round() as usize;
        let fade = p * p * (3.0 - 2.0 * p);
        let px = pop_pixels(&v.px, v.size.cx as usize, v.size.cy as usize, k, blur, fade);
        self.push_px_at(strip, &px, v.size, None);
    }

    /// TIMER_MIRROR_IN: advances the strips popping in or out; a copy that has popped out is
    /// destroyed, the main strip hidden. Stops when none is left.
    fn mirror_in_step(&mut self) {
        let list = std::mem::take(&mut self.mirror_in);
        for (strip, t0, leaving) in list {
            let p = (t0.elapsed().as_secs_f32() * 1000.0 / MIRROR_IN_MS).min(1.0);
            self.mirror_in_frame(strip, if leaving { 1.0 - p } else { p });
            if p < 1.0 {
                self.mirror_in.push((strip, t0, leaving));
            } else if leaving {
                unsafe {
                    // The main strip, a parked copy and a copy hidden for lack of room wait hidden.
                    if strip == self.strip || self.parked.iter().any(|m| m.1 == strip) || self.mirrors.iter().any(|m| m.1 == strip) {
                        let _ = ShowWindow(strip, SW_HIDE);
                        self.push(strip); // full content again for its next appearance
                    } else if is_our_strip(strip) && !self.mirrors.iter().any(|m| m.1 == strip) {
                        let _ = DestroyWindow(strip);
                    }
                }
            }
        }
        if self.mirror_in.is_empty() {
            unsafe {
                let _ = KillTimer(Some(self.main), TIMER_MIRROR_IN);
            }
        }
    }

    /// Places a copy just left of the secondary taskbar's clock (Windows 10 / ExplorerPatcher
    /// layout), or, when there is no clock window (Windows 11 draws it itself), just left of the
    /// first notification-area item that UI Automation reports (see tray_probe).
    /// Returns true while the copy is hidden because its place is not known yet.
    fn place_mirror(&self, bar: HWND, strip: HWND) -> bool {
        unsafe {
            // A copy is a topmost window of its own: it must not stay over a full-screen game or
            // video on that monitor (the taskbar itself goes behind those).
            if fullscreen_over(bar) {
                self.forget_room(bar);
                if IsWindowVisible(strip).as_bool() {
                    let _ = ShowWindow(strip, SW_HIDE);
                }
                return false;
            }
            let mut br = RECT::default();
            let _ = GetWindowRect(bar, &mut br);
            let dpi = GetDpiForWindow(bar);
            let s = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
            // Its own render: its own size, for its own taskbar's orientation.
            let Some(size) = self.view(strip).filter(|v| v.size.cx > 0).map(|v| v.size) else {
                self.forget_room(bar);
                return true;
            };
            if br.bottom - br.top > br.right - br.left {
                // A vertical taskbar (left / right): like the main one, just above its clock and
                // centred across its width.
                if !self.strip_vertical(strip) {
                    self.forget_room(bar);
                    return true; // rendered for a horizontal one: drawn again first
                }
                dbg_log(&format!("mirror: vertical bar {bar:?} rect {},{},{},{} main vertical {}", br.left, br.top, br.right, br.bottom, self.vertical));
                let bottom = match FindWindowExW(Some(bar), None, w!("ClockButton"), None) {
                    Ok(clock) if !clock.0.is_null() && IsWindowVisible(clock).as_bool() => {
                        let mut cr = RECT::default();
                        let _ = GetWindowRect(clock, &mut cr);
                        cr.top
                    }
                    // Windows 11 (with a vertical-taskbar mod) draws the clock itself: ask UI
                    // Automation where its notification-area items start, as for horizontal ones.
                    _ => match batteryhub::tray_probe::tray_left(bar, br, self.main, WM_TRAY_PROBED) {
                        batteryhub::tray_probe::TrayEdge::Known(y) => y,
                        batteryhub::tray_probe::TrayEdge::Pending => {
                            self.forget_room(bar);
                            return true;
                        }
                        batteryhub::tray_probe::TrayEdge::Unknown => br.bottom - (130.0 * s) as i32,
                    },
                };
                let mut p = POINT { x: br.left, y: bottom };
                let _ = ScreenToClient(bar, &mut p);
                let mut top = p.y - size.cy - (2.0 * s) as i32;
                let gap = self.layout_of(strip).offset;
                if gap > 0 {
                    top -= (gap as f32 * s) as i32;
                }
                let mut client = RECT::default();
                let _ = GetClientRect(bar, &mut client);
                let left = ((client.right - size.cx) / 2).max(0);
                self.forget_room(bar);
                self.show_mirror_at(bar, strip, left, top.max(0), size);
                return false;
            }
            // StartAllBack draws its own notification area: the clock may not be a separate
            // child window there, but TrayNotifyWnd is (#612). Its left edge is where the tray
            // starts, so the widget must stay to the left of it.
            let mut tray_rect = RECT::default();
            let has_tray = FindWindowExW(Some(bar), None, w!("TrayNotifyWnd"), None).is_ok_and(|tray| {
                IsWindowVisible(tray).as_bool()
                    && GetWindowRect(tray, &mut tray_rect).is_ok()
                    && tray_rect.left > br.left
                    && tray_rect.left < br.right
                    && tray_rect.right > tray_rect.left
                    && tray_rect.top < br.bottom
                    && tray_rect.bottom > br.top
            });
            let right = if has_tray {
                tray_rect.left
            } else {
                match FindWindowExW(Some(bar), None, w!("ClockButton"), None) {
                    Ok(clock) if !clock.0.is_null() && IsWindowVisible(clock).as_bool() => {
                        let mut cr = RECT::default();
                        let _ = GetWindowRect(clock, &mut cr);
                        cr.left
                    }
                    // Windows 11 draws this taskbar's clock itself: ask UI Automation where its
                    // notification-area items start (cached; an estimate until the first answer).
                    _ => match batteryhub::tray_probe::tray_left(bar, br, self.main, WM_TRAY_PROBED) {
                        batteryhub::tray_probe::TrayEdge::Known(x) => x,
                        // Not known yet (well under a second): stay hidden rather than show the copy in
                        // the wrong place; WM_TRAY_PROBED places it as soon as the answer is there.
                        batteryhub::tray_probe::TrayEdge::Pending => {
                            self.forget_room(bar);
                            return true;
                        }
                        batteryhub::tray_probe::TrayEdge::Unknown => br.right - (130.0 * s) as i32,
                    },
                }
            };
            let lay = self.layout_of(strip);
            let Some(left) = self.widget_left(bar, br, right, size.cx, s, &lay) else {
                self.forget_room(bar);
                return true;
            };
            let mut client = RECT::default();
            let _ = GetClientRect(bar, &mut client);
            let top = ((client.bottom - size.cy) / 2).max(0);
            // Narrow the task buttons so they never slide under the copy.
            if let Ok(worker) = FindWindowExW(Some(bar), None, w!("WorkerW"), None) {
                let mut wr = RECT::default();
                let _ = GetWindowRect(worker, &mut wr);
                let mut tl = POINT { x: wr.left, y: wr.top };
                let _ = ScreenToClient(bar, &mut tl);
                if tl.x + (wr.right - wr.left) > left && left > tl.x {
                    let _ = MoveWindow(worker, tl.x, tl.y, left - tl.x, wr.bottom - wr.top, true);
                }
            }
            // Not enough room beside this taskbar's task buttons: hidden there (see tick).
            let mut x0 = POINT { x: left, y: top };
            let _ = ClientToScreen(bar, &mut x0);
            let age = if lay.side != SIDE_RIGHT { ROOM_LEFT_AGE } else { ROOM_AGE };
            let room = lay.side == SIDE_FREE
                || batteryhub::tray_probe::room(bar, br, x0.x, x0.x + size.cx, self.main, WM_TRAY_PROBED, age, IsWindowVisible(strip).as_bool()).is_none_or(|r| r)
                    && x0.x + size.cx <= right;
            self.rooms.borrow_mut().insert(bar.0 as isize, room);
            if !room {
                self.mirror_glides.borrow_mut().retain(|g| g.0 != strip);
                // Pops out (place_mirror_at starts it), then hides.
                if IsWindowVisible(strip).as_bool() {
                    self.pop_out.borrow_mut().push(strip);
                }
                return false;
            }
            self.show_mirror_at(bar, strip, left, top, size);
        }
        false
    }

    /// Moves a copy to `left`, `top` (client coordinates of its taskbar) unless it is already there.
    /// Moving or resizing a layered child window can drop what it shows until its next update
    /// (it stayed blank until the mouse went over it), so it gets its image again after a move.
    fn show_mirror_at(&self, bar: HWND, strip: HWND, left: i32, top: i32, size: SIZE) {
        unsafe {
            let mut cur = RECT::default();
            let _ = GetWindowRect(strip, &mut cur);
            let mut cp = POINT { x: cur.left, y: cur.top };
            let _ = ScreenToClient(bar, &mut cp);
            let hidden = !IsWindowVisible(strip).as_bool();
            let same = cp.x == left && cp.y == top && cur.right - cur.left == size.cx && cur.bottom - cur.top == size.cy;
            if hidden || !same {
                // A popup: screen coordinates, above the (topmost) taskbar.
                let mut sp = POINT { x: left, y: top };
                let _ = ClientToScreen(bar, &mut sp);
                // Shown, same size, only a new place: it glides there (glide_step), like the main strip.
                let same_size = cur.right - cur.left == size.cx && cur.bottom - cur.top == size.cy;
                let mut glides = self.mirror_glides.borrow_mut();
                if !hidden && same_size && !self.popping(strip) && self.moving.is_none() {
                    let from = POINT { x: cur.left, y: cur.top };
                    match glides.iter_mut().find(|g| g.0 == strip) {
                        Some(g) if g.2 == sp => {}
                        Some(g) => *g = (strip, from, sp, Instant::now()),
                        None => glides.push((strip, from, sp, Instant::now())),
                    }
                    SetTimer(Some(self.main), TIMER_GLIDE, 15, None);
                    return;
                }
                glides.retain(|g| g.0 != strip);
                drop(glides);
                if self.popping(strip) {
                    let _ = SetWindowPos(strip, Some(HWND_TOPMOST), sp.x, sp.y, size.cx, size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                } else {
                    // New image, size and place in one step, then shown (on top) if it was hidden.
                    let Some(v) = self.view(strip) else { return };
                    let px = if strip == self.hovered && v.lit.len() == v.px.len() { &v.lit } else { &v.px };
                    self.push_px_at(strip, px, v.size, Some(sp));
                    if hidden {
                        let _ = SetWindowPos(strip, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    }
                }
            }
        }
    }

    // ---- notification-area fallback ------------------------------------------------------------

    fn tray_data(&self, uid: u32) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.main, uID: uid, ..Default::default() }
    }

    /// One icon per device with its level drawn in it; the tooltip is the same as on the strip.
    fn sync_tray(&mut self) {
        let light = self.light;
        let normal = if light { rgb(20, 20, 20) } else { rgb(255, 255, 255) };
        let size = unsafe { windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, windows::Win32::UI::HiDpi::GetDpiForSystem()) };
        let devices = self.visible();
        let mut wanted: Vec<TrayIcon> = devices
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let th = settings::threshold(SETTINGS_KEY, &d.key);
                let text = if d.percent >= 0 { d.percent.to_string() } else { "?".into() };
                (d.key.clone(), text, ui::level_color(d, light, normal, th), d.percent, self.tip_text(i))
            })
            .collect();
        if wanted.is_empty() {
            wanted.push(("\u{0}none".into(), "-".into(), normal, -1, tr("BatteryHub: no devices found")));
        }
        let keep: HashSet<String> = wanted.iter().map(|w| w.0.clone()).collect();
        let stale: Vec<String> = self.tray.keys().filter(|k| !keep.contains(*k)).cloned().collect();
        for k in stale {
            self.remove_tray_icon(&k);
        }
        for (key, text, color, level, tip) in wanted {
            let Some(icon) = ui::number_icon(&text, color, level, size, light) else {
                continue;
            };
            // The id comes from the device key, so a device keeps its icon id across restarts and
            // Windows remembers per device whether it is shown on the taskbar or hidden.
            let uid = match self.tray.get(&key) {
                Some((u, _)) => *u,
                None => tray_uid(&key, self.tray.values().map(|v| v.0).collect()),
            };
            let mut d = self.tray_data(uid);
            d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
            d.uCallbackMessage = WM_TRAY;
            d.hIcon = icon;
            ui::fill_wide(&mut d.szTip, &tip);
            unsafe {
                let known = self.tray.contains_key(&key);
                if !known || !Shell_NotifyIconW(NIM_MODIFY, &d).as_bool() {
                    let _ = Shell_NotifyIconW(NIM_ADD, &d);
                }
            }
            if let Some((_, old)) = self.tray.insert(key, (uid, icon)) {
                unsafe {
                    let _ = DestroyIcon(old);
                }
            }
        }
    }

    fn remove_tray_icon(&mut self, key: &str) {
        if let Some((uid, icon)) = self.tray.remove(key) {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data(uid));
                let _ = DestroyIcon(icon);
            }
        }
    }

    /// The app's own notification-area icon: shown while the strip has nothing to show because the
    /// empty-strip icon is turned off, so the menu stays a right-click away.
    fn sync_app_icon(&mut self) {
        let want = self.hide_empty && self.loaded && !self.fallback && self.visible().is_empty();
        unsafe {
            match (want, self.app_icon) {
                (true, None) => {
                    let size = windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, windows::Win32::UI::HiDpi::GetDpiForSystem());
                    let hinst = GetModuleHandleW(None).unwrap_or_default();
                    let Ok(h) = LoadImageW(Some(hinst.into()), windows::core::PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) else { return };
                    let icon = HICON(h.0);
                    let mut d = self.tray_data(APP_UID);
                    d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
                    d.uCallbackMessage = WM_TRAY;
                    d.hIcon = icon;
                    ui::fill_wide(&mut d.szTip, &tr("BatteryHub: no devices found"));
                    if !Shell_NotifyIconW(NIM_MODIFY, &d).as_bool() {
                        let _ = Shell_NotifyIconW(NIM_ADD, &d);
                    }
                    self.app_icon = Some(icon);
                }
                (false, Some(icon)) => {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data(APP_UID));
                    let _ = DestroyIcon(icon);
                    self.app_icon = None;
                }
                _ => {}
            }
        }
    }

    fn remove_tray(&mut self) {
        let keys: Vec<String> = self.tray.keys().cloned().collect();
        for k in keys {
            self.remove_tray_icon(&k);
        }
    }

    /// Keeps the strip just left of the notification area (checked every second, like the original).
    fn place(&mut self) {
        // The strip owns the open menu; hiding, popping or moving it now dismisses the menu and
        // leaves the widget apparently off until the next placement tick brings it back.
        if MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        // Waiting for the copies only before the widget first appears (so every taskbar shows it at
        // the same moment): a shown widget keeps following its place while a copy's is not known
        // yet (it moved only on the other monitor after a position change).
        if self.strip.0.is_null() || self.primary_hidden || (!self.mirrors_ready && unsafe { !IsWindowVisible(self.strip).as_bool() }) {
            self.forget_room(self.taskbar);
            return;
        }
        unsafe {
            if self.strip_popup && (fullscreen_over(self.taskbar) || !IsWindowVisible(self.taskbar).as_bool()) {
                self.forget_room(self.taskbar);
                if IsWindowVisible(self.strip).as_bool() {
                    let _ = ShowWindow(self.strip, SW_HIDE);
                }
                return;
            }
            let Ok(tray) = FindWindowExW(Some(self.taskbar), None, w!("TrayNotifyWnd"), None) else {
                self.forget_room(self.taskbar);
                return;
            };
            let (mut tr, mut bar) = (RECT::default(), RECT::default());
            let _ = GetWindowRect(tray, &mut tr);
            let _ = GetWindowRect(self.taskbar, &mut bar);
            if self.vertical {
                // Above the notification area, centred across the taskbar's width.
                let mut p = POINT { x: bar.left, y: tr.top };
                let _ = ScreenToClient(self.taskbar, &mut p);
                let mut client = RECT::default();
                let _ = GetClientRect(self.taskbar, &mut client);
                let mut top = p.y - self.mv.size.cy - (2.0 * self.scale()) as i32;
                if self.offset > 0 {
                    top -= (self.offset as f32 * self.scale()) as i32;
                }
                let top = top.max(0);
                let left = ((client.right - self.mv.size.cx) / 2).max(0);
                self.forget_room(self.taskbar);
                self.place_strip_at(left, top, (tr, bar, top));
                return;
            }
            // Placed as soon as UI Automation has told where the taskbar's items are.
            let lay = self.main_layout();
            let Some(left) = self.widget_left(self.taskbar, bar, tr.left, self.mv.size.cx, self.scale(), &lay) else {
                self.forget_room(self.taskbar);
                return;
            };
            let mut client = RECT::default();
            let _ = GetClientRect(self.taskbar, &mut client);
            let top = ((client.bottom - self.mv.size.cy) / 2).max(0);

            // Windows 10 / ExplorerPatcher taskbar: narrow the task buttons so they never slide under us.
            if let Ok(rebar) = FindWindowExW(Some(self.taskbar), None, w!("ReBarWindow32"), None).map_err(|_| ()).and_then(|r| if self.align_left { Err(()) } else { Ok(r) }) {
                let mut rr = RECT::default();
                let _ = GetWindowRect(rebar, &mut rr);
                let mut tl = POINT { x: rr.left, y: rr.top };
                let _ = ScreenToClient(self.taskbar, &mut tl);
                if tl.x + (rr.right - rr.left) > left && left > tl.x {
                    let _ = MoveWindow(rebar, tl.x, tl.y, left - tl.x, rr.bottom - rr.top, true);
                }
            }
            // Not enough room beside the task buttons: hidden on this taskbar (see tick).
            let mut x0 = POINT { x: left, y: top };
            let _ = ClientToScreen(self.taskbar, &mut x0);
            let age = if self.align_left { ROOM_LEFT_AGE } else { ROOM_AGE };
            // Put there with the mouse: stays there.
            let room = lay.side == SIDE_FREE
                || batteryhub::tray_probe::room(self.taskbar, bar, x0.x, x0.x + self.mv.size.cx, self.main, WM_TRAY_PROBED, age, IsWindowVisible(self.strip).as_bool())
                    .is_none_or(|r| r)
                    && x0.x + self.mv.size.cx <= tr.left;
            self.rooms.borrow_mut().insert(self.taskbar.0 as isize, room);
            if !room {
                self.glide = None;
                self.last_place = Default::default();
                // Pops out (the same animation as everywhere else), then hides.
                if IsWindowVisible(self.strip).as_bool() && !self.leaving(self.strip) {
                    self.hide_tip();
                    self.start_pop(self.strip, true);
                }
                return;
            }
            // Room again: one popping out turns around; a hidden one pops in once shown.
            let strip = self.strip;
            if self.leaving(strip) {
                self.start_pop(strip, false);
            }
            let was = IsWindowVisible(strip).as_bool();
            self.place_strip_at(left, top, (tr, bar, left));
            if !was && IsWindowVisible(strip).as_bool() && !self.popping(strip) {
                self.start_pop(strip, false);
            }
        }
    }

    fn place_strip_at(&mut self, left: i32, top: i32, key: (RECT, RECT, i32)) {
        unsafe {
            if self.strip_popup {
                let mut sp = POINT { x: left, y: top };
                let _ = ClientToScreen(self.taskbar, &mut sp);
                let hidden = !IsWindowVisible(self.strip).as_bool();
                let mut cur = RECT::default();
                let _ = GetWindowRect(self.strip, &mut cur);
                let moved = cur.left != sp.x || cur.top != sp.y || cur.right - cur.left != self.mv.size.cx || cur.bottom - cur.top != self.mv.size.cy;
                if !hidden && !moved && self.last_place == key {
                    return;
                }
                self.last_place = key;
                if self.popping(self.strip) {
                    let _ = SetWindowPos(self.strip, Some(HWND_TOPMOST), sp.x, sp.y, self.mv.size.cx, self.mv.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                } else {
                    let px = if self.strip == self.hovered && self.mv.lit.len() == self.mv.px.len() { &self.mv.lit } else { &self.mv.px };
                    self.push_px_at(self.strip, px, self.mv.size, Some(sp));
                    if hidden {
                        let _ = SetWindowPos(self.strip, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    }
                }
                return;
            }
            let covered = GetWindow(self.strip, GW_HWNDPREV).is_ok_and(|h| !h.0.is_null()) || !IsWindowVisible(self.strip).as_bool();
            if !covered && self.last_place == key {
                return;
            }
            self.last_place = key;
            self.move_strip(left, top);
        }
    }

    /// Puts the strip at `left`, `top` (taskbar client coordinates), on top of the taskbar's own
    /// windows. A visible strip of the same size glides there (from where it is, also part-way
    /// through a glide) instead of jumping; a new size or a hidden strip is placed right away.
    fn move_strip(&mut self, left: i32, top: i32) {
        let to = POINT { x: left, y: top };
        unsafe {
            let mut cur = RECT::default();
            let _ = GetWindowRect(self.strip, &mut cur);
            let mut from = POINT { x: cur.left, y: cur.top };
            let _ = ScreenToClient(self.taskbar, &mut from);
            let same_size = cur.right - cur.left == self.mv.size.cx && cur.bottom - cur.top == self.mv.size.cy;
            let visible = IsWindowVisible(self.strip).as_bool();
            let raise = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW;
            if visible && same_size && self.glide.is_some_and(|g| g.1 == to) {
                let _ = SetWindowPos(self.strip, Some(HWND_TOP), 0, 0, 0, 0, raise);
                return;
            }
            if visible && same_size && from != to && self.moving.is_none() {
                self.glide = Some((from, to, Instant::now()));
                let _ = SetWindowPos(self.strip, Some(HWND_TOP), 0, 0, 0, 0, raise);
                SetTimer(Some(self.main), TIMER_GLIDE, 15, None);
                return;
            }
            self.glide = None;
            let _ = SetWindowPos(self.strip, Some(HWND_TOP), left, top, self.mv.size.cx, self.mv.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    /// The height shift drawn now (px before scaling): offset_y, or on its way there.
    fn shown_offset_y(&self) -> f32 {
        match self.y_glide {
            Some((from, t0)) => from + (self.offset_y as f32 - from) * glide_ease(t0),
            None => self.offset_y as f32,
        }
    }

    /// TIMER_GLIDE: one frame of the strip moving and / or its height shift changing.
    fn glide_step(&mut self) {
        if let Some((from, to, t0)) = self.glide {
            let e = glide_ease(t0);
            let x = from.x + ((to.x - from.x) as f32 * e).round() as i32;
            let y = from.y + ((to.y - from.y) as f32 * e).round() as i32;
            unsafe {
                let _ = SetWindowPos(self.strip, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            if e >= 1.0 {
                self.glide = None;
            }
        }
        let glides = std::mem::take(&mut *self.mirror_glides.borrow_mut());
        let mut going = Vec::new();
        for (strip, from, to, t0) in glides {
            if !self.mirrors.iter().any(|m| m.1 == strip) || unsafe { !IsWindowVisible(strip).as_bool() } {
                continue;
            }
            let e = glide_ease(t0);
            let at = POINT { x: from.x + ((to.x - from.x) as f32 * e).round() as i32, y: from.y + ((to.y - from.y) as f32 * e).round() as i32 };
            if let Some(v) = self.view(strip) {
                let px = if strip == self.hovered && v.lit.len() == v.px.len() { &v.lit } else { &v.px };
                self.push_px_at(strip, px, v.size, Some(at));
            }
            if e < 1.0 {
                going.push((strip, from, to, t0));
            }
        }
        self.mirror_glides.borrow_mut().extend(going);
        if let Some((_, t0)) = self.y_glide {
            if glide_ease(t0) >= 1.0 {
                self.y_glide = None;
            }
            self.paint();
        }
        if self.glide.is_none() && self.y_glide.is_none() && self.mirror_glides.borrow().is_empty() {
            unsafe {
                let _ = KillTimer(Some(self.main), TIMER_GLIDE);
            }
        }
    }

    // ---- hover card ----------------------------------------------------------------------------

    /// Records the levels of this read in the 7-day history (one point per change of level or
    /// charging state, or every 10 minutes), drops what is older than 7 days, and saves it on a
    /// worker thread at most every 10 minutes (right away while a dashboard shows it).
    fn record_history(&mut self, fresh: &[DeviceStatus]) {
        let now = batteryhub::history::now_secs();
        let gap = batteryhub::history::gap_limit(refresh_minutes());
        for s in fresh {
            if s.connected && !s.asleep && s.percent >= 0 && !s.key.starts_with("demo:") && !settings::battery_only(SETTINGS_KEY, &s.key) {
                self.history.record(&s.key, now, s.percent, s.charging, s.is_full(), gap);
                if let Some(b) = s.buds {
                    for (side, level, charging) in [("L", b.left, b.left_charging), ("R", b.right, b.right_charging)] {
                        if level >= 0 {
                            self.history.record(&format!("{}#{side}", s.key), now, level, charging, false, gap);
                        }
                    }
                }
            }
        }
        self.history.prune(now);
        let live = dashboard_open();
        if live {
            // The dashboard reads the file: give it the latest reading too.
            self.history.flush();
        }
        let due = self.history_saved.is_none_or(|t| t.elapsed().as_secs() >= batteryhub::history::STEP_SECS);
        if self.history.is_dirty() && (due || live) {
            let main = self.main.0 as usize;
            let started = self.history.save_in_background(history_path(), move |ok| unsafe {
                let _ = PostMessageW(Some(HWND(main as *mut c_void)), WM_HISTORY_SAVED, WPARAM(ok as usize), LPARAM(0));
            });
            if started {
                self.history_saved = Some(Instant::now());
            }
        }
    }

    /// Stores what is kept in memory and writes the history now (exit, sign-out, sleep).
    fn save_history_now(&mut self) {
        self.history.flush();
        if self.history.is_dirty() {
            let _ = self.history.save_to(&history_path());
        }
    }

    /// Card colours: background, border.
    fn card_colors(&self) -> (COLORREF, COLORREF) {
        if self.light { (rgb(249, 249, 249), rgb(214, 214, 214)) } else { (rgb(44, 44, 44), rgb(70, 70, 70)) }
    }

    /// What the card of `d` shows: when this changes, the card is drawn again. The minute is part
    /// of it, so the chart's time axis moves on while the mouse rests on a device.
    fn card_signature(&self, d: &DeviceStatus) -> u64 {
        let h = self.history.points(&d.key);
        let last = self.history.latest(&d.key).map(|p| (p.t, p.level, p.flags));
        let text = format!(
            "{:?}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}",
            card_opts(&d.key),
            d.key,
            d.name,
            d.percent,
            d.level_text,
            d.charging,
            d.full,
            d.asleep,
            d.via,
            self.light,
            self.scale().to_bits(),
            last,
            h.len(),
            batteryhub::history::now_secs() / 60,
            self.card_scrub.as_ref().filter(|(k, _)| *k == d.key).map(|(_, t)| t),
            d.buds,
            d.dock_percent,
            // The buds' own charts.
            ["#L", "#R"].map(|side| self.history.latest(&format!("{}{side}", d.key)).map(|p| (p.t, p.level, p.flags)))
        );
        batteryhub::util::fnv1a(&text) as u64
    }

    /// The card's content for `d`: from the cache when nothing changed, else drawn again.
    fn card_content(&mut self, d: &DeviceStatus) -> Rc<CardContent> {
        let sig = self.card_signature(d);
        if let Some(c) = self.card_cache.iter().find(|c| c.key == d.key && c.sig == sig) {
            return c.clone();
        }
        let t0 = Instant::now();
        let (w, h, px) = self.render_card(d);
        prof::add(&prof::RENDERS, 1);
        prof::add(&prof::RENDER_US, t0.elapsed().as_micros() as u64);
        let c = Rc::new(CardContent { sig, key: d.key.clone(), w, h, px, half: std::cell::OnceCell::new() });
        self.card_cache.retain(|o| o.key != d.key);
        self.card_cache.insert(0, c.clone());
        self.card_cache.truncate(2);
        c
    }

    /// Draws the content of the card for one device (name, level, bar, connection, last 24
    /// hours) on a transparent canvas; the rounded background is drawn per frame, so it can
    /// glide and stretch from one device's card to the next.
    fn render_card(&mut self, d: &DeviceStatus) -> (i32, i32, Vec<u32>) {
        self.ensure_fonts();
        let s = self.scale();
        let px = |v: f32| (v * s).round() as i32;
        let light = self.light;
        let fg = if light { rgb(0, 0, 0) } else { rgb(255, 255, 255) };
        let dim = if light { rgb(96, 96, 96) } else { rgb(170, 170, 170) };
        let track = if light { rgb(214, 214, 214) } else { rgb(80, 80, 80) };
        let grid = if light { rgb(200, 200, 200) } else { rgb(84, 84, 84) };
        let th = settings::threshold(SETTINGS_KEY, &d.key);
        let low = d.percent >= 0 && d.percent <= th && !d.charging;
        let accent = if low {
            ui::low_color(light)
        } else if light {
            rgb(16, 124, 16)
        } else {
            rgb(108, 203, 95)
        };
        let charge_c = if light { rgb(0, 95, 184) } else { rgb(96, 205, 255) };
        // The chart: green above the red-warning level, red at or below it (charging stays blue).
        let (ok_c, low_c) = (if light { rgb(16, 124, 16) } else { rgb(108, 203, 95) }, ui::low_color(light));
        let level_c = move |level: f64, charging: bool| {
            if charging {
                charge_c
            } else if level <= th as f64 {
                low_c
            } else {
                ok_c
            }
        };
        let Some(f) = self.fonts.as_ref() else { return (1, 1, vec![0]) };
        let (icon_f, name_f, big_f, small_f) = (f.card_icon, f.card_name, f.card_big, f.card_small);

        // As wide as the name needs, 300 to 400 px; longer names end in "…". Earbuds show their
        // case's level after the name, when they report it (the buds' levels are the big text).
        let pad = px(16.0);
        let buds = d
            .buds
            .filter(|_| d.connected && !d.asleep)
            .and_then(|b| b.case)
            .map(|(level, chg)| mask(&format!("{}{}", trf("Case {level}", &[("level", &format!("{level}%"))]), if chg { "⚡" } else { "" }), small_f, light));
        let buds_w = buds.as_ref().map_or(0, |m| m.w + px(10.0));
        let room = px(400.0) - 2 * pad - px(26.0) - buds_w;
        let mut name_text = d.name.clone();
        let mut name = mask(&name_text, name_f, light);
        let mut chars: Vec<char> = d.name.chars().collect();
        while name.w > room && chars.len() > 4 {
            chars.pop();
            name_text = format!("{}…", chars.iter().collect::<String>().trim_end());
            name = mask(&name_text, name_f, light);
        }
        let w = (name.w + buds_w + 2 * pad + px(26.0)).clamp(px(300.0), px(400.0));
        let chart_h = px(80.0);
        // Drawn on a tall canvas, then cut to the content's height plus the bottom margin.
        let tall = px(420.0);
        let mut cv = Canvas { w, h: tall, px: vec![0; (w * tall) as usize] };

        // Header: device glyph and name.
        let mut y = pad;
        let glyph = device_icon(d, icon_f, light);
        cv.draw(&glyph, pad, y + px(2.0), fg, 255);
        cv.draw(&name, pad + px(26.0), y, fg, 255);
        if let Some(b) = &buds {
            cv.draw(b, pad + px(26.0) + name.w + px(10.0), y + (name.h - b.h) / 2 + px(1.0), dim, 255);
        }
        y += px(34.0);

        // Big level (or the device's own word for it), the time left under it, then the bar.
        let big_text = if let Some(b) = d.buds.filter(|b| d.percent >= 0 && b.left >= 0 && b.right >= 0) {
            // Earbuds: each bud, labelled.
            format!("{}: {}%  {}: {}%", tr("L"), b.left, tr("R"), b.right)
        } else if d.percent >= 0 {
            format!("{}%", d.percent)
        } else if !d.level_text.is_empty() {
            d.level_text.clone()
        } else {
            "?".into()
        };
        let big = mask(&big_text, big_f, light);
        // Too long for the card at the big size (both buds): the name's size, on the same line.
        let (big, dy) = if big.w > w - 2 * pad {
            let small = mask(&big_text, name_f, light);
            let dy = (big.h - small.h) / 2;
            (small, dy)
        } else {
            (big, 0)
        };
        cv.draw(&big, pad, y + dy, if low { accent } else { fg }, 255);
        y += px(40.0);
        let now = batteryhub::history::now_secs();
        let series = self.history.series(&d.key);
        let read_now = d.connected && !d.asleep && d.percent >= 0;
        let st = batteryhub::stats::compute(&series, read_now.then_some((d.percent, d.charging)));
        // Rate of the forecast: down on battery, up while charging (none when full or not read).
        let est = if !read_now || d.is_full() {
            None
        } else if d.charging {
            st.charge_estimate()
        } else {
            st.discharge_estimate()
        };
        let (show_chart, show_time) = card_opts(&d.key);
        let left_text = if !read_now || d.is_full() || !show_time {
            String::new()
        } else {
            match st.time_left(d.percent, d.charging) {
                Some((h, _)) if d.charging => trf("full in about {time}", &[("time", &fmt_hours(h))]),
                Some((h, _)) => trf("about {time} left", &[("time", &fmt_hours(h))]),
                None => tr("Time left: not enough data yet"),
            }
        };
        if !left_text.is_empty() {
            let left = mask(&left_text, small_f, light);
            cv.draw(&left, pad, y, if est.is_some() { fg } else { dim }, 255);
        }
        if show_time {
            y += px(22.0); // same layout for every device, so cards share one height
        } else {
            y -= px(4.0); // no time-left line: the bar moves up under the level
        }
        cv.fill_rrect(pad, y, w - pad, y + px(6.0), px(3.0), track, 255);
        if d.percent > 0 {
            let fill = pad + (w - 2 * pad) * d.percent.min(100) / 100;
            cv.fill_rrect(pad, y, fill.max(pad + px(6.0)), y + px(6.0), px(3.0), accent, 255);
        }
        y += px(18.0);

        // Connection and state.
        let mut line = tr(&d.via);
        if d.charging {
            line.push_str(&format!(" · {}", if d.full { tr("Full") } else { tr("Charging") }));
        } else if d.asleep {
            line.push_str(&format!(" · {}", tr("Asleep")));
        }
        let via = mask(&line, small_f, light);
        cv.draw(&via, pad, y, dim, 255);
        // The spare battery in the headset's dock, when there is one.
        if let Some(dock) = d.dock_text() {
            y += px(20.0);
            cv.draw(&mask(&trf("Dock battery: {dock}", &[("dock", &dock)]), small_f, light), pad, y, dim, 255);
        }
        if !show_chart {
            // No chart: the card ends under the connection line.
            self.card_chart_y = (i32::MIN / 2, i32::MIN / 2);
            let h = (y + px(18.0) + pad).min(tall);
            cv.px.truncate((w * h) as usize);
            return (w, h, cv.px);
        }
        y += px(30.0);

        // The last 12 hours, now in the middle, and the next 12 hours as a forecast at the current
        // rate. Earbuds get one chart per bud, the left one over the right one. With the mouse on
        // the chart, the hour under it (level and change) replaces the title.
        let (from, to) = (now.saturating_sub(12 * 3600), now + 12 * 3600);
        let title_y = y;
        y += px(22.0);
        // (history key, side label, level now, charging now)
        let lanes: Vec<(String, Option<String>, i32, bool)> = match d.buds.filter(|_| d.connected) {
            Some(b) => vec![
                (format!("{}#L", d.key), Some(tr("L")), if read_now { b.left } else { -1 }, b.left_charging),
                (format!("{}#R", d.key), Some(tr("R")), if read_now { b.right } else { -1 }, b.right_charging),
            ],
            None => vec![(d.key.clone(), None, if read_now { d.percent } else { -1 }, d.charging)],
        };
        let lane_h = if lanes.len() > 1 { px(56.0) } else { chart_h };
        let lane_gap = px(12.0);
        let (x0, x1) = (pad, w - pad);
        let lw = px(2.0).max(2);
        let to_x = |t: u64| x0 + ((x1 - x0) as f32 * (t.saturating_sub(from) as f32 / (to - from) as f32).min(1.0)) as i32;
        let xm = to_x(now);
        let scrub = self.card_scrub.as_ref().filter(|(k, _)| *k == d.key).map(|(_, t)| *t);
        let first_top = y;
        let mut scrub_texts: Vec<(Option<String>, Option<f64>, Option<f64>)> = Vec::new();
        for (li, (key, label, level, charging)) in lanes.iter().enumerate() {
            let (top, bottom) = (y, y + lane_h);
            if li > 0 {
                // Solid line between the two buds' charts.
                let mid = top - lane_gap / 2;
                for x in x0..x1 {
                    cv.blend(x, mid, grid, 200);
                }
            }
            for gy in [top, (top + bottom) / 2, bottom] {
                let mut x = x0;
                while x < x1 {
                    cv.blend(x, gy, grid, 255);
                    x += px(4.0).max(2);
                }
            }
            let to_yf = |l: f64| bottom - ((bottom - top) as f64 * l.clamp(0.0, 100.0) / 100.0).round() as i32;
            let to_y = |l: u8| to_yf(l as f64);
            let lane_now = *level >= 0;
            let series = if label.is_none() { series.clone() } else { self.history.series(key) };
            let lane_est = if label.is_none() {
                est
            } else {
                let st = batteryhub::stats::compute(&series, lane_now.then_some((*level, *charging)));
                if !lane_now || *level >= 100 {
                    None
                } else if *charging {
                    st.charge_estimate()
                } else {
                    st.discharge_estimate()
                }
            };
            let segs = batteryhub::history::segments(&series, from, now);
            for seg in &segs {
                for pair in seg.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    // Steps, not slopes: a level holds until the next reading. The step takes the
                    // colour of its level, the drop (or rise) the colour of the level it reaches.
                    cv.line(to_x(a.t), to_y(a.level), to_x(b.t), to_y(a.level), lw, level_c(a.level as f64, a.charging()));
                    cv.line(to_x(b.t), to_y(a.level), to_x(b.t), to_y(b.level), lw, level_c(b.level as f64, b.charging()));
                }
            }
            // Dotted "now" line in the middle.
            let mut yy = top;
            while yy < bottom {
                cv.blend(xm, yy, grid, 255);
                yy += px(4.0).max(2);
            }
            // Forecast: dashed, from the current level until empty (or full), then level.
            let level_now = (*level).clamp(0, 100) as f64;
            if let Some(e) = lane_est {
                let room = if *charging { 100.0 - level_now } else { level_now };
                let reach = now + ((room / e.rate) * 3600.0).min((to - now) as f64) as u64;
                let end_level = batteryhub::stats::project(level_now, e.rate, *charging, reach - now);
                // On battery the forecast turns red where it reaches the red-warning level.
                let th_f = th as f64;
                let cross = (!*charging && level_now > th_f && end_level < th_f)
                    .then(|| now + ((level_now - th_f) / e.rate * 3600.0) as u64)
                    .filter(|&t| t > now && t < reach);
                match cross {
                    Some(t) => {
                        cv.dashed(xm, to_yf(level_now), to_x(t), to_yf(th_f), lw, ok_c, px(5.0));
                        cv.dashed(to_x(t), to_yf(th_f), to_x(reach), to_yf(end_level), lw, low_c, px(5.0));
                    }
                    None => cv.dashed(xm, to_yf(level_now), to_x(reach), to_yf(end_level), lw, level_c(level_now, *charging), px(5.0)),
                }
                if reach < to {
                    cv.dashed(to_x(reach), to_yf(end_level), x1, to_yf(end_level), lw, level_c(end_level, *charging), px(5.0));
                }
            }
            if let Some(last) = segs.last().and_then(|s| s.last()) {
                cv.disc(to_x(last.t), to_y(last.level), px(3.5), level_c(last.level as f64, last.charging()));
            }
            if let Some(l) = label {
                let m = mask(l, small_f, light);
                cv.draw(&m, x0 + px(3.0), top + px(2.0), if lane_now { fg } else { dim }, 255);
            }
            // The hour under the mouse.
            if let Some(t) = scrub {
                let open_until = if lane_now { now } else { 0 };
                let value_at = |t: u64| -> Option<f64> {
                    if t <= now {
                        batteryhub::stats::level_at(&series, t, open_until).map(|p| p.level as f64)
                    } else {
                        lane_est.map(|e| batteryhub::stats::project(level_now, e.rate, *charging, t - now))
                    }
                };
                let sx = to_x(t);
                for yy in top..=bottom {
                    cv.blend(sx, yy, dim, 255);
                }
                let v = if t.abs_diff(now) < 90 && lane_now { Some(level_now) } else { value_at(t) };
                if let Some(v) = v {
                    cv.disc(sx, to_yf(v), px(3.5), level_c(v, *charging && t >= now));
                }
                scrub_texts.push((label.clone(), v, value_at(t.saturating_sub(3600))));
            }
            y = bottom + lane_gap;
        }
        let bottom = y - lane_gap;
        self.card_chart_y = (first_top, bottom);
        let title_text = match scrub {
            Some(t) => {
                let clock = if t.abs_diff(now) < 90 {
                    tr("Now")
                } else {
                    let l = batteryhub::dashboard::local(t);
                    format!("{:02}:{:02}", l.hour, l.minute)
                };
                let approx = if t > now { "≈ " } else { "" };
                let parts: Vec<String> = scrub_texts
                    .iter()
                    .map(|(label, v, before)| {
                        let side = label.as_ref().map(|l| format!("{l} ")).unwrap_or_default();
                        match v {
                            // One chart: the level and its change over the hour before.
                            Some(v) if label.is_none() => {
                                let change = before.map(|p| format!("   {} / 1 h", fmt_change(v - p))).unwrap_or_default();
                                format!("{approx}{}%{change}", v.round())
                            }
                            Some(v) => format!("{side}{approx}{}%", v.round()),
                            None if label.is_none() => tr("no reading"),
                            None => format!("{side}-"),
                        }
                    })
                    .collect();
                let forecast = if t > now && scrub_texts.iter().any(|p| p.1.is_some()) { format!("   ({})", tr("forecast")) } else { String::new() };
                format!("{clock}   {}{forecast}", parts.join("   "))
            }
            None => tr("Last 12 hours and the next 12 (forecast)"),
        };
        let title = mask(&title_text, small_f, light);
        cv.draw(&title, pad, title_y, if scrub.is_some() { fg } else { dim }, 255);
        y = bottom + px(10.0);
        let ago = mask(&tr("12h ago"), small_f, light);
        cv.draw(&ago, x0, y, dim, 255);
        let nowm = mask(&tr("Now"), small_f, light);
        cv.draw(&nowm, xm - nowm.w / 2, y, dim, 255);
        let ahead = mask(&tr("in 12h"), small_f, light);
        cv.draw(&ahead, x1 - ahead.w, y, dim, 255);
        // As much room under the bottom labels as above the header.
        let h = (y + px(14.0) + px(18.0)).min(tall);
        cv.px.truncate((w * h) as usize);
        (w, h, cv.px)
    }

    /// Shows the card for device `i` above (or below) its part of `strip`.
    fn show_card(&mut self, i: usize, strip: HWND, from: i32, to: i32) {
        let Some(d) = self.visible_at(i).cloned() else { return };
        if self.card.0.is_null() {
            self.card = create_card(self.main);
        }
        if self.card.0.is_null() {
            return;
        }
        let content = self.card_content(&d);
        let (w, h) = (content.w, content.h);
        unsafe {
            let mut rc = RECT::default();
            let _ = GetWindowRect(strip, &mut rc);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(MonitorFromWindow(strip, MONITOR_DEFAULTTONEAREST), &mut mi);
            let gap = (8.0 * self.scale()) as i32;
            let work = mi.rcWork;
            let vertical = self.strip_vertical(strip);
            let (x, y, slide) = if vertical {
                // Beside the taskbar, level with the hovered device (no slide: it fades in).
                let mid = (mi.rcMonitor.left + mi.rcMonitor.right) / 2;
                let x = if rc.left < mid { rc.right + gap } else { rc.left - w - gap };
                let y = (rc.top + (from + to) / 2 - h / 2).clamp(work.top + gap, (work.bottom - h - gap).max(work.top));
                // Slides 10 px sideways, away from the taskbar.
                (x, y, (10.0 * self.scale()) as i32 * if x > rc.left { -1 } else { 1 })
            } else {
                let x = (rc.left + (from + to) / 2 - w / 2).clamp(work.left + gap, (work.right - w - gap).max(work.left));
                let y = if rc.top - h - gap >= work.top { rc.top - h - gap } else { rc.bottom + gap };
                // Slides 10 px towards its place: up when above the taskbar, down when below it.
                (x, y, (10.0 * self.scale()) as i32 * if y < rc.top { 1 } else { -1 })
            };
            let was_shown = self.card_anim.visible && self.card_anim.showing;
            let old_key = self.card_anim.content.as_ref().map(|c| c.key.clone());
            let same = was_shown && old_key.as_deref() == Some(d.key.as_str());
            let old = std::mem::take(&mut self.card_anim);
            // Already open on another device: blend the old card into the new one. A switch while
            // another is still running starts from what is on screen now (no jump back), and the
            // card that is more visible at that moment is the one that fades out.
            let prev = match (&old.content, was_shown && !same) {
                (Some(c), true) => {
                    let outgoing = match (&old.prev, old.at) {
                        (Some((_, _, p)), Some(at)) if at.elapsed().as_secs_f32() * 1000.0 < CARD_CROSS_MS * 0.5 => p.clone(),
                        _ => c.clone(),
                    };
                    let (dx, dw) = if old.drawn.1 > 0 { old.drawn } else { (old.x, c.w) };
                    Some((dx, dw, outgoing))
                }
                _ => None,
            };
            let crossing = prev.is_some();
            let prev_y = if old.drawn.1 > 0 { old.drawn_y } else { old.y };
            let prev_h = if old.drawn_h > 0 { old.drawn_h } else { old.content.as_ref().map_or(h, |c| c.h) };
            let (bg, border) = self.card_colors();
            let radius = (8.0 * self.scale()).round() as i32;
            let scale = self.scale();
            self.card_anim =
                CardAnim {
                at: old.at,
                showing: true,
                visible: true,
                x,
                y,
                slide,
                content: Some(content),
                prev,
                drawn: old.drawn,
                vertical,
                prev_y,
                drawn_y: old.drawn_y,
                prev_h,
                drawn_h: old.drawn_h,
                bg,
                border,
                radius,
                scale,
            };
            if crossing {
                self.card_anim.at = Some(Instant::now());
                SetTimer(Some(self.main), TIMER_CARD, 15, None);
            } else if same && self.card_anim.at.is_none() {
                // Same device, new values: swap the content at once.
                self.card_frame(1.0);
            } else if was_shown && !same {
                // Another device whose card has another height (e.g. earbuds with two charts):
                // no cross-fade between those, so the new card takes over at once. Before, it
                // kept showing the old device, or picked up the old card's half-done animation.
                self.card_anim.at = None;
                self.card_frame(1.0);
            } else {
                if !was_shown {
                    self.card_anim.at = Some(Instant::now());
                    self.card_frame(0.0);
                }
                SetTimer(Some(self.main), TIMER_CARD, 15, None);
            }
            let _ = SetWindowPos(self.card, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    fn point_in_window(hwnd: HWND, pt: POINT) -> bool {
        if hwnd.0.is_null() {
            return false;
        }
        unsafe {
            let mut rc = RECT::default();
            IsWindowVisible(hwnd).as_bool() && GetWindowRect(hwnd, &mut rc).is_ok() && PtInRect(&rc, pt).as_bool()
        }
    }

    /// A low-level hook is only needed while a clicked card is waiting for an outside click. It is
    /// installed on this thread, which already has the message loop the hook call needs.
    fn watch_outside_click(&mut self) {
        if self.click_hook.is_some() {
            return;
        }
        unsafe {
            if let Ok(hook) = SetWindowsHookExW(WH_MOUSE_LL, Some(click_hook_proc), None, 0) {
                self.click_hook = Some(hook);
            }
        }
    }

    fn stop_watching_clicks(&mut self) {
        if let Some(hook) = self.click_hook.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
        }
    }

    /// A click outside the card (in another program, on the desktop, on another taskbar button)
    /// closes it, like Windows 11's Control Center. Clicks on the card (chart scrubbing, its menu)
    /// and on our strips (where click_device handles the toggle) are left alone.
    fn outside_click(&mut self, pt: POINT) {
        if self.card_pinned.is_none() || self.card_dismiss != settings::CardDismiss::OutsideClick {
            return;
        }
        let mut strips = std::iter::once(self.strip)
            .chain(self.mirrors.iter().map(|m| m.1))
            .chain(self.parked.iter().map(|m| m.1));
        if Self::point_in_window(self.card, pt) || strips.any(|s| Self::point_in_window(s, pt)) {
            return;
        }
        dbg_log("click outside the card");
        self.hide_tip();
    }

    /// The mouse left the strip. An open card stays while the mouse moves onto it (the gap
    /// between the two is crossed in a moment); TIMER_AWAY then watches where it is.
    fn leave_strip(&mut self) {
        self.tracking = false;
        // The mouse is off the strip: drop the highlight. A hover card (and a clicked card in the
        // "mouse leaves" mode) gets the away timer; the default clicked card waits for a click.
        self.set_hovered(HWND::default());
        if !self.card.0.is_null() && self.card_anim.visible && self.card_anim.showing {
            if closes_when_away(self.card_pinned.is_some(), self.card_dismiss) {
                self.card_away = Some(Instant::now());
                unsafe {
                    SetTimer(Some(self.main), TIMER_AWAY, 50, None);
                }
            }
        } else {
            self.hide_tip();
            trim_memory();
        }
    }

    /// TIMER_AWAY: keeps the card while the mouse is on it, and closes it once the mouse has been
    /// on neither the card nor the strip for a moment.
    fn away_step(&mut self) {
        // A clicked card in the default mode is not bound to the mouse at all.
        if !closes_when_away(self.card_pinned.is_some(), self.card_dismiss) {
            return;
        }
        let mut pt = POINT::default();
        let under = unsafe { GetCursorPos(&mut pt).is_ok().then(|| WindowFromPoint(pt)) };
        if under.is_some_and(|w| w == self.card) && self.card_anim.showing {
            self.card_away = None;
            return;
        }
        self.set_card_scrub(None);
        let since = *self.card_away.get_or_insert_with(Instant::now);
        if since.elapsed().as_millis() >= CARD_AWAY_MS {
            self.hide_tip();
            trim_memory();
        }
    }

    /// Mouse on the card: the hour under it on the chart (whole hours, or now) is shown.
    fn card_mouse(&mut self) {
        let Some(content) = self.card_anim.content.clone() else { return };
        if !self.card_anim.showing || self.card_anim.prev.is_some() {
            return;
        }
        self.card_away = None;
        let (mut pt, mut rc) = (POINT::default(), RECT::default());
        unsafe {
            if GetCursorPos(&mut pt).is_err() || GetWindowRect(self.card, &mut rc).is_err() {
                return;
            }
        }
        let (cx, cy) = (pt.x - rc.left, pt.y - rc.top);
        let pad = (16.0 * self.card_anim.scale).round() as i32;
        let (x0, x1) = (pad, content.w - pad);
        let (top, bottom) = self.card_chart_y;
        let on_chart = card_opts(&content.key).0 && cy >= top - pad && cy <= bottom + pad && cx >= x0 - pad / 2 && cx <= x1 + pad / 2;
        let pick = on_chart.then(|| {
            let now = batteryhub::history::now_secs();
            let f = ((cx - x0) as f64 / (x1 - x0).max(1) as f64).clamp(0.0, 1.0);
            (content.key.clone(), snap_hour(now.saturating_sub(12 * 3600) as f64 + f * 24.0 * 3600.0, now))
        });
        self.set_card_scrub(pick);
    }

    fn set_card_scrub(&mut self, pick: Option<(String, u64)>) {
        if self.card_scrub == pick {
            return;
        }
        self.card_scrub = pick;
        // Draws the card again with (or without) the hour, in place.
        let Some(key) = self.card_anim.content.as_ref().map(|c| c.key.clone()) else { return };
        let Some(d) = self.devices.get(&key).map(|t| t.status.clone()) else { return };
        let content = self.card_content(&d);
        if self.card_anim.content.as_ref().is_some_and(|c| c.w == content.w && c.h == content.h) {
            self.card_anim.content = Some(content);
            if self.card_anim.at.is_none() {
                self.card_frame(1.0);
            }
        }
    }

    /// Fades the card out (then hides it).
    fn hide_card(&mut self) {
        if self.card.0.is_null() || !self.card_anim.visible || !self.card_anim.showing {
            return;
        }
        // A switch that is still running ends where it is: the fade-out starts from that frame.
        if self.card_anim.prev.take().is_some() {
            self.card_frame(1.0);
        }
        self.card_anim.showing = false;
        self.card_anim.at = Some(Instant::now());
        unsafe {
            SetTimer(Some(self.main), TIMER_CARD, 15, None);
        }
    }

    /// One animation frame: `t` 0..1 of the running fade-in (with a short rise), fade-out, or
    /// switch between devices (background glides and stretches, contents cross-fade). Drawn
    /// straight into the card's bitmap: no allocation per frame.
    fn card_frame(&mut self, t: f32) {
        let Some(content) = self.card_anim.content.clone() else { return };
        if self.card.0.is_null() {
            return;
        }
        let t = t.clamp(0.0, 1.0);
        let a = &self.card_anim;
        let (bg, border, radius, scale) = (a.bg, a.border, a.radius, a.scale);
        let h = content.h;
        let (pos, size, bx, bw, layers, alpha): (POINT, SIZE, i32, i32, Vec<Layer>, u8);
        let (drawn_y, bh): (i32, i32);
        if let (true, Some((ox, ow, old))) = (a.showing, &a.prev) {
            // Ease in-out, like the taskbar's window previews moving between buttons.
            let e = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
            let lerp = |p: i32, q: i32| p + ((q - p) as f32 * e).round() as i32;
            let (cx, cw) = (lerp(*ox, a.x), lerp(*ow, content.w));
            // Old content blurs away while the new one comes into focus.
            let (ro, rn) = ((BLUR_MAX * e * scale).round() as i32, (BLUR_MAX * (1.0 - e) * scale).round() as i32);
            // Height and top glide together: above the taskbar the bottom edge stays put, beside
            // a vertical one the card stays centred on the device it moves to.
            let (cy, ch) = (lerp(a.prev_y, a.y), lerp(a.prev_h, h).max(1));
            if a.vertical {
                // Devices are above one another: the whole card glides to the new one.
                pos = POINT { x: cx, y: cy };
                size = SIZE { cx: cw, cy: ch };
                (bx, bw) = (0, cw);
            } else {
                let (ux0, ux1) = (a.x.min(*ox), (a.x + content.w).max(ox + ow));
                pos = POINT { x: ux0, y: cy };
                size = SIZE { cx: ux1 - ux0, cy: ch };
                (bx, bw) = (cx - ux0, cw);
            }
            (drawn_y, bh) = (cy, ch);
            layers = vec![(old.clone(), 1.0 - e, ro), (content.clone(), e, rn)];
            alpha = 255;
            self.card_anim.drawn = (cx, cw);
        } else {
            let ease = 1.0 - (1.0 - t).powi(3); // ease-out cubic, like Windows 11 flyouts
            let (fade, dy) = if a.showing { (ease, a.slide as f32 * (1.0 - ease)) } else { (1.0 - t, 0.0) };
            // Comes into focus while appearing, goes slightly out of focus while disappearing.
            let r = if a.showing { BLUR_MAX * (1.0 - ease) } else { BLUR_MAX * 0.6 * t };
            let d = dy.round() as i32;
            (drawn_y, bh) = (a.y, h);
            pos = if a.vertical { POINT { x: a.x + d, y: a.y } } else { POINT { x: a.x, y: a.y + d } };
            size = SIZE { cx: content.w, cy: h };
            (bx, bw) = (0, content.w);
            layers = vec![(content.clone(), 1.0, (r * scale).round() as i32)];
            alpha = (fade * 255.0).round() as u8;
            self.card_anim.drawn = (a.x, content.w);
        }
        self.card_anim.drawn_y = drawn_y;
        self.card_anim.drawn_h = bh;
        let surface = match self.card_surface.take() {
            Some(s) if s.w >= size.cx && s.h >= size.cy => Some(s),
            // A quarter more than needed now: switching between neighbouring devices (the usual
            // case) then reuses the bitmap.
            _ => Surface::new(size.cx + content.w / 4, size.cy),
        };
        let Some(mut surface) = surface else { return };
        let stride = surface.w as usize;
        let dst = surface.pixels();
        let bgc = CardBg { bx, bw, h: bh, radius, bg, border };
        let t0 = Instant::now();
        compose_card(dst, stride, size.cx, size.cy, &bgc, &layers, &mut self.blur_scratch);
        let t1 = Instant::now();
        surface.present(self.card, pos, size, alpha);
        prof::add(&prof::FRAMES, 1);
        prof::add(&prof::COMPOSE_US, (t1 - t0).as_micros() as u64);
        prof::add(&prof::PRESENT_US, t1.elapsed().as_micros() as u64);
        self.card_surface = Some(surface);
    }

    /// TIMER_CARD: advances the card's animation; stops the timer (and hides) when done.
    fn card_step(&mut self) {
        let Some(at) = self.card_anim.at else {
            unsafe {
                let _ = KillTimer(Some(self.main), TIMER_CARD);
            }
            return;
        };
        let total = match (self.card_anim.showing, self.card_anim.prev.is_some()) {
            (true, true) => CARD_CROSS_MS,
            (true, false) => CARD_SHOW_MS,
            _ => CARD_HIDE_MS,
        };
        let t = at.elapsed().as_secs_f32() * 1000.0 / total;
        self.card_frame(t);
        if t >= 1.0 {
            self.card_anim.at = None;
            if self.card_anim.prev.take().is_some() {
                self.card_frame(1.0); // end of a cross-fade: only the new card, at its own size
            }
            unsafe {
                let _ = KillTimer(Some(self.main), TIMER_CARD);
                if !self.card_anim.showing {
                    let _ = ShowWindow(self.card, SW_HIDE);
                    prof::report();
                    // Closed: the bitmap, blur buffers and cached contents go back to Windows.
                    self.card_anim = CardAnim::default();
                    self.card_surface = None;
                    self.blur_scratch = BlurScratch::default();
                    self.card_cache.clear();
                }
            }
        }
    }

    // ---- hover tooltip -------------------------------------------------------------------------

    fn create_tooltip(&mut self) {
        unsafe {
            let Ok(tip) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                TOOLTIPS_CLASSW,
                None,
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                0,
                0,
                0,
                0,
                Some(self.main),
                None,
                GetModuleHandleW(None).ok().map(|m| m.into()),
                None,
            ) else {
                return;
            };
            let mut ti = tool_info(self.main, &mut [0u16]);
            SendMessageW(tip, TTM_ADDTOOLW, None, Some(LPARAM(&mut ti as *mut _ as isize)));
            SendMessageW(tip, TTM_SETMAXTIPWIDTH, None, Some(LPARAM(480)));
            self.tip = tip;
        }
    }

    /// Text shown when hovering a device, short: "PRO 2 LIGHTSPEED: 96%" (+ charging / asleep).
    fn tip_text(&self, i: usize) -> String {
        let devices = self.visible();
        let Some(d) = devices.get(i) else {
            return tr("BatteryHub: no devices found");
        };
        let level = d.level();
        let numeric = level.ends_with('%');
        let state = if d.asleep {
            format!(" ({})", tr("asleep"))
        } else if d.is_full() && numeric {
            format!(" ({})", tr("full"))
        } else if d.charging && numeric {
            format!(" ({})", tr("charging"))
        } else {
            String::new()
        };
        format!("{}: {level}{state}", d.name)
    }

    /// Mouse at (x, y) on a strip: the device under it (along x, or along y on a vertical taskbar).
    fn hover(&mut self, on: HWND, x: i32, y: i32) {
        if MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let vertical = self.strip_vertical(on);
        if self.drag.is_some() {
            self.drag_to(if vertical { y } else { x });
            return;
        }
        let x = if vertical { y } else { x };
        let hit = self.view(on).and_then(|v| v.hits.iter().find(|h| x >= h.0 && x < h.1).map(|h| (h.2, h.0, h.1)));
        let mode = hit.and_then(|h| self.visible_at(h.0)).map_or(settings::HoverMode::Card, |d| settings::hover_mode(SETTINGS_KEY, &d.key));
        let mode = hover_card_mode(mode);
        // "Card on click": passing the mouse over the device must not open the card.
        let card_click = hit.and_then(|h| self.visible_at(h.0)).is_some_and(|d| settings::card_on_click(SETTINGS_KEY, &d.key));
        // Mouse on the strip: the card stays, whatever it was opened by. Cancel the away timer
        // even when away_step() had just cleared `card_away` while the mouse was on the card.
        self.card_away = None;
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_AWAY);
        }
        // A pinned card owns the hover state: no highlight while it is open.
        if self.card_pinned.is_some() {
            self.set_hovered(HWND::default());
            return;
        }
        // The hover highlight (like Windows' own taskbar buttons) in every mode, "Nothing" included.
        self.set_hovered(on);
        if self.tip.0.is_null() {
            return;
        }
        if on != self.tip_on {
            self.tip_on = on;
            self.tracking = false;
            self.tip_for = None;
        }
        unsafe {
            if !self.tracking {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: on, dwHoverTime: 0 };
                self.tracking = TrackMouseEvent(&mut tme).is_ok();
            }
            let key = hit.map(|h| h.0).unwrap_or(usize::MAX);
            if self.tip_for == Some(key) {
                return;
            }
            self.tip_for = Some(key);
            // The card (name, level, last 24 hours) above the hovered device, or the short
            // tooltip, or nothing (the device's "On hover" setting); nothing between devices.
            match (hit, mode) {
                (Some(_), _) if !hover_shows_popup(mode, card_click) => {
                    // Clicked cards are opened in click_device(), never by hovering.
                    self.tooltip_off();
                    self.hide_card();
                }
                (Some((i, from, to)), settings::HoverMode::Card) => {
                    self.tooltip_off();
                    self.show_card(i, on, from, to)
                }
                (Some((i, from, to)), settings::HoverMode::Tip) => {
                    self.hide_card();
                    self.tooltip_on(i, on, from, to);
                }
                _ => {
                    self.tooltip_off();
                    self.hide_card()
                }
            }
        }
    }

    /// A click on a device with "Card on click": open its card and keep it there (#439).
    fn click_device(&mut self, strip: HWND, x: i32, y: i32) {
        if MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed) || self.moving.is_some() {
            return;
        }
        let pos = if self.strip_vertical(strip) { y } else { x };
        let hit = self.view(strip).and_then(|v| v.hits.iter().find(|h| pos >= h.0 && pos < h.1).map(|h| (h.2, h.0, h.1)));
        let Some((i, from, to)) = hit else {
            if self.card_pinned.is_some() {
                self.hide_tip();
            }
            return;
        };
        let Some(key) = self.visible_at(i).map(|d| d.key.clone()) else { return };
        let on = settings::card_on_click(SETTINGS_KEY, &key);
        dbg_log(&format!("click: {key} card_on_click={on}"));
        if !on {
            // Clicking a device that does not use a click card is still an interaction elsewhere.
            if self.card_pinned.is_some() {
                self.hide_tip();
            }
            return;
        }
        // By key: the device's place changes when others appear or leave.
        if self.card_pinned.as_ref().is_some_and(|(k, s)| *k == key && *s == strip) {
            self.hide_tip();
            return;
        }
        self.tooltip_off();
        self.show_card(i, strip, from, to);
        self.card_pinned = Some((key.clone(), strip));
        self.card_dismiss = settings::card_dismiss(SETTINGS_KEY, &key);
        if self.card_dismiss == settings::CardDismiss::OutsideClick {
            self.watch_outside_click();
        } else {
            self.stop_watching_clicks();
        }
        // The click card is independent of hover: no highlighted strip under the pinned card.
        self.set_hovered(HWND::default());
    }

    /// The short tooltip ("Mouse: 80% (charging)") above (or beside) device `i` of `strip`.
    fn tooltip_on(&mut self, i: usize, strip: HWND, from: i32, to: i32) {
        unsafe {
            ui::set_tooltip_theme(self.tip, !self.light);
            let mut text: Vec<u16> = self.tip_text(i).encode_utf16().chain(std::iter::once(0)).collect();
            let mut ti = tool_info(self.main, &mut text);
            SendMessageW(self.tip, TTM_UPDATETIPTEXTW, None, Some(LPARAM(&mut ti as *mut _ as isize)));
            SendMessageW(self.tip, TTM_TRACKACTIVATE, Some(WPARAM(1)), Some(LPARAM(&mut ti as *mut _ as isize)));
            let size = SendMessageW(self.tip, TTM_GETBUBBLESIZE, None, Some(LPARAM(&mut ti as *mut _ as isize))).0;
            let (tw, th) = ((size & 0xFFFF) as i32, ((size >> 16) & 0xFFFF) as i32);
            let mut rc = RECT::default();
            let _ = GetWindowRect(strip, &mut rc);
            let gap = (4.0 * self.scale()) as i32;
            let (cx, cy) = if self.strip_vertical(strip) {
                let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(MonitorFromWindow(strip, MONITOR_DEFAULTTONEAREST), &mut mi);
                let left_side = rc.left < (mi.rcMonitor.left + mi.rcMonitor.right) / 2;
                (if left_side { rc.right + gap } else { rc.left - tw - gap }, rc.top + (from + to) / 2 - th / 2)
            } else {
                let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(MonitorFromWindow(strip, MONITOR_DEFAULTTONEAREST), &mut mi);
                let above = rc.top - th - gap >= mi.rcMonitor.top;
                (rc.left + (from + to) / 2 - tw / 2, if above { rc.top - th - gap } else { rc.bottom + gap })
            };
            let pos = ((cy as u32 & 0xFFFF) << 16) | (cx as u32 & 0xFFFF);
            SendMessageW(self.tip, TTM_TRACKPOSITION, None, Some(LPARAM(pos as isize)));
        }
    }

    fn tooltip_off(&mut self) {
        if !self.tip.0.is_null() {
            unsafe {
                let mut ti = tool_info(self.main, &mut [0u16]);
                SendMessageW(self.tip, TTM_TRACKACTIVATE, Some(WPARAM(0)), Some(LPARAM(&mut ti as *mut _ as isize)));
            }
        }
    }

    fn hide_tip(&mut self) {
        self.set_hovered(HWND::default());
        self.card_pinned = None;
        self.card_away = None;
        self.card_scrub = None;
        self.stop_watching_clicks();
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_AWAY);
        }
        self.hide_card();
        self.tracking = false;
        self.tip_for = None;
        if !self.tip.0.is_null() {
            unsafe {
                let mut ti = tool_info(self.main, &mut [0u16]);
                SendMessageW(self.tip, TTM_TRACKACTIVATE, Some(WPARAM(0)), Some(LPARAM(&mut ti as *mut _ as isize)));
            }
        }
    }

    /// After a menu closes, re-arm the strip under the cursor. `hide_tip` clears tracking, and if
    /// the mouse has not moved since the right-click no MOUSEMOVE comes to do it: the widget then
    /// looks dead until the pointer is nudged.
    fn after_menu(&mut self) {
        let mut pt = POINT::default();
        if !unsafe { GetCursorPos(&mut pt) }.is_ok() {
            return;
        }
        let strips: Vec<HWND> = std::iter::once(self.strip)
            .chain(self.mirrors.iter().map(|m| m.1))
            .chain(self.parked.iter().map(|m| m.1))
            .collect();
        for strip in strips {
            if strip.0.is_null() || !unsafe { IsWindow(Some(strip)) }.as_bool() {
                continue;
            }
            let mut rc = RECT::default();
            if unsafe { GetWindowRect(strip, &mut rc) }.is_ok() && unsafe { PtInRect(&rc, pt) }.as_bool() {
                let mut at = pt;
                let _ = unsafe { ScreenToClient(strip, &mut at) };
                self.hover(strip, at.x, at.y);
                break;
            }
        }
    }

    // ---- polling -------------------------------------------------------------------------------

    fn start_poll(&mut self) {
        if self.polling {
            // A read (full or quick) is running: if this one is for a device change, do it right
            // after, instead of losing it.
            if self.devchange || self.forget {
                self.pending_change = true;
            }
            return;
        }
        self.polling = true;
        // Enumerate the devices again only when something may have changed; a periodic read
        // reuses the device list (and still notices added / removed HID devices).
        let rescan = !self.scanned || self.devchange || self.forget;
        self.scanned = true;
        dbg_log(&format!("poll started (device change: {}, rescan: {rescan}, interval: {} min)", self.devchange, refresh_minutes()));
        ui::poll_async(self.main, WM_POLL_DONE, rescan);
    }

    /// Quick read of headset dongles and of receivers whose device is off or asleep (switching a
    /// headset on / off or waking
    /// a mouse is not a device change for Windows).
    fn start_quick(&mut self) {
        if self.polling || !refresh_on_change() {
            return;
        }
        let mut ids: HashSet<(u16, u16)> = self.offline_hw.clone();
        ids.extend(self.devices.values().filter(|t| t.status.asleep).filter_map(|t| t.status.hw));
        // Headsets on a 2.4 GHz dongle are switched off and on all the time, and the dongle stays
        // plugged in, so Windows reports nothing: check them too (one short status report each).
        ids.extend(self.devices.values().filter(|t| matches!(t.status.kind, DeviceKind::Headset | DeviceKind::Earbuds)).filter_map(|t| t.status.hw));
        // AirPods only broadcast their state: listen for a moment so a bud taken out of the case
        // (or put back) shows within seconds.
        let airpods = self.devices.keys().any(|k| k.starts_with("airpods:"));
        if ids.is_empty() && !airpods {
            return;
        }
        self.polling = true;
        let ids: Vec<(u16, u16)> = ids.into_iter().collect();
        dbg_log(&format!("quick read of {ids:04X?}{}", if airpods { " + AirPods" } else { "" }));
        let ids2 = ids.clone();
        QUICK_IDS.with(|q| *q.borrow_mut() = ids);
        ui::run_async(self.main, WM_QUICK_DONE, move || {
            let mut out = if ids2.is_empty() { Vec::new() } else { batteryhub::device::poll_hardware(&ids2) };
            if airpods {
                out.extend(batteryhub::device::poll_airpods());
            }
            out
        });
    }

    /// Reads of joined connections become one logical device under the canonical key (#644).
    /// One reading wins; levels are never averaged, and only one point per poll reaches the history.
    fn canonicalize(&self, fresh: &mut Vec<DeviceStatus>) {
        if self.merged.is_empty() {
            return;
        }
        let roots: HashSet<String> = self.merged.values().cloned().collect();
        for s in fresh.iter_mut() {
            s.key = settings::resolve_merged(&self.merged, &s.key);
        }
        // Prefer a live reading with a level, then power, over an asleep / unknown one.
        let rank = |s: &DeviceStatus| -> (u8, u8) {
            let live = if s.connected && !s.asleep && s.percent >= 0 { 2 } else if s.connected { 1 } else { 0 };
            (live, u8::from(s.charging || s.full))
        };
        let mut best: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (i, s) in fresh.iter().enumerate() {
            if !roots.contains(&s.key) {
                continue;
            }
            match best.get(&s.key) {
                Some(&j) if rank(&fresh[j]) >= rank(s) => {}
                _ => {
                    best.insert(s.key.clone(), i);
                }
            }
        }
        if best.is_empty() {
            return;
        }
        let keep: HashSet<usize> = best.into_values().collect();
        let mut out = Vec::with_capacity(keep.len());
        for (i, s) in std::mem::take(fresh).into_iter().enumerate() {
            if !roots.contains(&s.key) || keep.contains(&i) {
                out.push(s);
            }
        }
        *fresh = out;
    }

    /// A quick read sees only some connections of a joined device (#644): one that is off, asleep
    /// or without a level must not replace a connection it did not read. When the connection shown
    /// goes off itself, a full read looks for the others right after.
    fn keep_other_connections(&mut self, fresh: &mut Vec<DeviceStatus>) {
        let mut lost = false;
        fresh.retain(|s| {
            if !self.merged.values().any(|t| *t == s.key) {
                return true;
            }
            let Some(shown) = self.devices.get(&s.key).map(|t| &t.status) else { return true };
            if shown.hw == s.hw {
                lost |= !s.connected;
                return true;
            }
            s.connected && !s.asleep && s.percent >= 0
        });
        self.pending_change |= lost;
    }

    /// Merges a quick read: only devices behind the receivers in `ids` are updated or removed.
    fn on_quick_result(&mut self, mut fresh: Vec<DeviceStatus>, ids: &[(u16, u16)]) {
        self.polling = false;
        self.canonicalize(&mut fresh);
        self.keep_other_connections(&mut fresh);
        let keys: HashSet<String> = fresh.iter().map(|s| s.key.clone()).collect();
        self.devices.retain(|k, t| {
            if self.merged.contains_key(k) {
                return false; // a joined connection is now part of its target (#644)
            }
            let Some(hw) = t.status.hw else { return true };
            if !ids.contains(&hw) || keys.contains(k) {
                return true;
            }
            let present = batteryhub::device::hid_present(hw);
            match batteryhub::device::missing_device(Some(hw), present, t.status.grace_sec, t.seen.elapsed().as_secs(), MIN_GRACE_SEC) {
                Missing::Remove => false,
                Missing::Asleep => {
                    t.status.asleep = true;
                    true
                }
                Missing::Keep => true,
            }
        });
        for id in ids {
            self.offline_hw.remove(id);
        }
        self.merge(fresh);
        if std::mem::take(&mut self.pending_change) {
            self.devchange = true;
            self.start_poll();
        }
    }

    /// Adds / updates the devices of a read; devices reporting "off" are removed and their
    /// receiver is remembered for quick checks.
    fn merge(&mut self, mut fresh: Vec<DeviceStatus>) {
        self.doubt_zero(&mut fresh);
        self.record_history(&fresh);
        self.remember_devices(&fresh);
        let now = Instant::now();
        for mut s in fresh {
            if !self.order.contains(&s.key) {
                self.order.push(s.key.clone());
                settings::apply_order(&mut self.order, &self.saved_order);
                self.reordered = true;
            }
            if !s.connected {
                if let Some(hw) = s.hw {
                    self.offline_hw.insert(hw);
                }
                self.devices.remove(&s.key);
                continue;
            }
            let reported = std::mem::take(&mut s.name);
            s.name = self.names.get(&s.key).cloned().unwrap_or_else(|| reported.clone());
            // Ring style: a change of power state animates the bolt colour too — green -> blue at
            // 100%, and blue -> the normal icon colour on unplug (never a detour back through green).
            if self.ring {
                self.track_ring_power(&s, now);
            }
            self.check_critical(&s);
            self.devices.insert(s.key.clone(), Tracked { status: s, seen: now, reported });
        }
        self.order.retain(|k| self.devices.contains_key(k));
        self.paint();
        self.send_to_dashboards();
    }

    /// Every device keeps its place: one seen for the first time goes to the far left (saved).
    /// Devices are dated as seen (at most hourly), and those not seen for 7 days are forgotten.
    fn remember_devices(&mut self, fresh: &[DeviceStatus]) {
        let connected: Vec<String> = fresh.iter().filter(|s| s.connected && !s.key.starts_with("demo:")).map(|s| s.key.clone()).collect();
        let new: Vec<String> = connected.iter().filter(|k| !self.saved_order.contains(k)).cloned().collect();
        if !new.is_empty() {
            self.saved_order = settings::with_new_first(&self.saved_order, &new);
            settings::set_order(SETTINGS_KEY, &self.saved_order);
            dbg_log(&format!("new devices placed first: {new:?}"));
        }
        // The dashboard lists every device with saved history, also the ones not here now.
        for s in fresh.iter().filter(|s| s.connected && !s.key.starts_with("demo:")) {
            let reported = self.devices.get(&s.key).map(|t| t.reported.clone()).unwrap_or_else(|| s.name.clone());
            let line = batteryhub::dashboard::encode_status(&DeviceStatus { connected: false, percent: -1, ..s.clone() }, &reported);
            let sig = (s.name.clone(), s.kind as u8, s.via.clone());
            if self.known_saved.get(&s.key) != Some(&sig) && settings::set_known(SETTINGS_KEY, &s.key, &line) {
                self.known_saved.insert(s.key.clone(), sig);
            }
        }
        let now = batteryhub::history::now_secs();
        let due: Vec<String> = connected.into_iter().filter(|k| self.seen_saved.get(k).is_none_or(|t| now.saturating_sub(*t) >= 3600)).collect();
        if !due.is_empty() && settings::mark_seen(SETTINGS_KEY, &due, now) {
            for k in due {
                self.seen_saved.insert(k, now);
            }
        }
        if now.saturating_sub(self.forgot_at) >= 3600 {
            self.forgot_at = now;
            let gone = settings::forget_unseen(SETTINGS_KEY, now);
            if !gone.is_empty() {
                dbg_log(&format!("forgot devices not seen for 7 days: {gone:?}"));
                for k in &gone {
                    self.history.remove(k);
                }
                self.history.mark_dirty();
                self.saved_order = settings::order(SETTINGS_KEY);
                self.names = settings::names(SETTINGS_KEY);
                self.merged = settings::merged_targets(SETTINGS_KEY);
                self.hidden = settings::hidden(SETTINGS_KEY);
            }
        }
    }

    /// Two devices swap places on the strip (and in the menu); the order is saved.
    fn swap_devices(&mut self, a: &str, b: &str) {
        if a == b || self.lock_order {
            return;
        }
        if self.own_layout(self.menu_on) {
            let mut cur = self.order.clone();
            let lay = self.layout_of(self.menu_on);
            settings::apply_order(&mut cur, &lay.order);
            let order = settings::swapped(&cur, &lay.order, a, b);
            self.change_layout(self.menu_on, true, |l| l.order = order);
            return;
        }
        self.saved_order = settings::swapped(&self.order, &self.saved_order, a, b);
        settings::set_order(SETTINGS_KEY, &self.saved_order);
        settings::apply_order(&mut self.order, &self.saved_order);
        self.reordered = true;
        self.card_cache.clear();
        self.paint();
    }

    /// While dragging: `a` takes the place of `b`, the devices between shift by one (saved).
    fn move_device(&mut self, a: &str, b: &str) {
        if self.own_layout(self.drag_on) {
            let mut cur = self.order.clone();
            let lay = self.layout_of(self.drag_on);
            settings::apply_order(&mut cur, &lay.order);
            let order = settings::moved(&cur, &lay.order, a, b);
            self.change_layout(self.drag_on, true, |l| l.order = order);
            return;
        }
        self.saved_order = settings::moved(&self.order, &self.saved_order, a, b);
        settings::set_order(SETTINGS_KEY, &self.saved_order);
        settings::apply_order(&mut self.order, &self.saved_order);
        self.reordered = true;
        self.card_cache.clear();
        self.paint();
    }

    /// The items on the strip this frame, in order: the visible devices, the "no devices found"
    /// glyph once the first read found none (nothing before the first read), and the items that
    /// are leaving, at their old place. Starts the appear / leave animations.
    fn strip_items(&mut self, devices: &[DeviceStatus]) -> Vec<Item> {
        let mut want: Vec<(String, Option<DeviceStatus>)> = devices.iter().map(|d| (d.key.clone(), Some(d.clone()))).collect();
        if self.loaded && devices.is_empty() && !self.hide_empty {
            want.push((NONE_KEY.into(), None));
        }
        if self.manual && self.polling {
            want.push((REFRESH_KEY.into(), None));
        }
        let old = std::mem::take(&mut self.shown);
        // Gone since the last render: pops out.
        for (k, _) in &old {
            if !want.iter().any(|w| &w.0 == k) {
                self.presence.entry(k.clone()).or_insert(Presence { leaving: true, at: Instant::now() }).turn(true);
            }
        }
        // New: waits until what leaves has popped out, then appears, one after another.
        let wait = self
            .presence
            .values()
            .filter(|p| p.leaving)
            .map(|p| (POP_MS - p.at.elapsed().as_secs_f32() * 1000.0).max(0.0) as u64)
            .max()
            .unwrap_or(0);
        let mut n = 0;
        for (k, _) in &want {
            if let Some(p) = self.presence.get_mut(k) {
                p.turn(false);
            } else if !old.iter().any(|o| &o.0 == k) {
                let at = Instant::now() + std::time::Duration::from_millis(wait + n * STAGGER_MS);
                self.presence.insert(k.clone(), Presence { leaving: false, at });
                n += 1;
            }
        }
        // Leaving items stay where they were: after the item before them in the last render.
        let mut list: Vec<(String, Option<DeviceStatus>, bool)> = want.into_iter().map(|(k, d)| (k, d, true)).collect();
        for (j, (k, d)) in old.iter().enumerate() {
            if list.iter().any(|l| &l.0 == k) || !self.presence.get(k).is_some_and(|p| p.leaving) {
                continue;
            }
            let at = old[..j].iter().rev().find_map(|(before, _)| list.iter().position(|l| &l.0 == before)).map(|i| i + 1).unwrap_or(0);
            list.insert(at, (k.clone(), d.clone(), false));
        }
        let mut out = Vec::with_capacity(list.len());
        let mut vis = 0;
        for (key, dev, live) in list {
            let (f, p, done) = self.presence.get(&key).map(|p| p.frame()).unwrap_or((1.0, 1.0, false));
            if done {
                self.presence.remove(&key);
                if !live {
                    continue;
                }
            }
            let v = (live && dev.is_some()).then(|| {
                vis += 1;
                vis - 1
            });
            self.shown.push((key.clone(), dev.clone()));
            out.push(Item { key, dev, vis: v, f: if done { 1.0 } else { f }, p: if done { 1.0 } else { p } });
        }
        out
    }

    /// Where each device is drawn along the strip this frame: (offset from its place, blur radius,
    /// opacity 0-255). The dragged device follows the mouse; devices that changed places slide
    /// from where they were drawn, blurred while they move, like the card's contents.
    fn motion(&self, v: &mut View, keys: &[String], ranges: &[(i32, i32)], len: i32, s: f32, drag_here: bool) -> Vec<(i32, i32, u32)> {
        let now = Instant::now();
        let held = |k: &str| drag_here && self.drag.as_deref() == Some(k);
        if std::mem::take(&mut v.reordered) {
            for (k, r) in keys.iter().zip(ranges) {
                if held(k) {
                    continue;
                }
                if let Some(&was) = v.drawn_at.get(k) {
                    // Positions are kept from the end of the strip (it is anchored there).
                    let d = (len as f32 - was) - r.0 as f32;
                    if d.abs() >= 1.0 {
                        v.slides.insert(k.clone(), (d, now));
                    }
                }
            }
        }
        v.slides.retain(|_, (_, t0)| (t0.elapsed().as_secs_f32() * 1000.0) < SLIDE_MS);
        let mut out = Vec::with_capacity(keys.len());
        v.starts.clear();
        let before = std::mem::take(&mut v.drawn_at);
        v.strip_len = len;
        for (k, r) in keys.iter().zip(ranges) {
            let (off, blur, alpha) = if held(k) {
                // Held by the mouse, kept on the strip.
                let want = self.drag_at - self.grab;
                let at = want.clamp(0, (len - (r.1 - r.0)).max(0));
                (at - r.0, 0, 200)
            } else if let Some((d, t0)) = v.slides.get(k) {
                let p: f32 = (t0.elapsed().as_secs_f32() * 1000.0 / SLIDE_MS).clamp(0.0, 1.0);
                let left = (1.0f32 - p).powi(3); // ease-out: what is left of the way
                let off = (d * left).round() as i32;
                // Blur with the speed: strongest at the start, gone when it settles.
                let blur = ((1.0 - p) * (1.0 - p) * 3.0 * s).round() as i32;
                (off, blur.min((d.abs() / 4.0) as i32), 255)
            } else if !self.presence.is_empty()
                && let Some(&was) = before.get(k)
            {
                // Moved aside by a device appearing or leaving: blurred with its speed.
                let moved = (was - (len - r.0) as f32).abs();
                (0, ((moved / 2.0).round() as i32).min((3.0 * s).round() as i32), 255)
            } else {
                (0, 0, 255)
            };
            v.starts.insert(k.clone(), r.0);
            v.drawn_at.insert(k.clone(), (len - (r.0 + off)) as f32);
            out.push((off, blur, alpha));
        }
        out
    }

    /// Left button down on a strip: the device under the mouse can be dragged to another place.
    fn start_drag(&mut self, strip: HWND, x: i32, y: i32) {
        self.drag_moved = false;
        if self.lock_order {
            return;
        }
        let Some(v) = self.view(strip) else { return };
        let x = if v.vertical { y } else { x };
        let Some(i) = v.hits.iter().find(|h| x >= h.0 && x < h.1).map(|h| h.2) else { return };
        let Some(key) = self.visible_at(i).map(|d| d.key.clone()) else { return };
        let start = v.starts.get(&key).copied().unwrap_or(x);
        self.drag_at = x;
        self.grab = x - start;
        self.drag = Some(key);
        self.drag_on = strip;
        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(strip);
        }
    }

    /// While dragging: the device moves to the place under the mouse.
    fn drag_to(&mut self, x: i32) {
        let Some(key) = self.drag.clone() else { return };
        if x == self.drag_at {
            return;
        }
        self.drag_moved = true;
        self.drag_at = x;
        self.hide_tip();
        let hits = self.view(self.drag_on).map(|v| v.hits.clone()).unwrap_or_default();
        let hit = hits.iter().find(|h| x >= h.0 && x < h.1).copied();
        let mine = hits.iter().position(|h| self.visible_at(h.2).is_some_and(|d| d.key == key));
        if let (Some((from, to, i)), Some(j)) = (hit, mine)
            && let Some(other) = self.visible_at(i).map(|d| d.key.clone())
            && other != key
        {
            // Past the middle of the other device: it makes room (no flicker between two of
            // different widths).
            let mid = (from + to) / 2;
            let k = hits.iter().position(|h| h.2 == i).unwrap_or(j);
            if (k > j && x >= mid) || (k < j && x <= mid) {
                self.move_device(&key, &other);
                return;
            }
        }
        self.paint();
    }

    fn end_drag(&mut self) {
        if let Some(key) = self.drag.take() {
            unsafe {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
            }
            // Let go: the device slides into its place.
            let on = self.drag_on;
            if let Some(v) = self.view_mut(on)
                && let (Some(&from_end), Some(&place)) = (v.drawn_at.get(&key), v.starts.get(&key))
            {
                let d = (v.strip_len as f32 - from_end) - place as f32;
                if d.abs() >= 1.0 {
                    v.slides.insert(key, (d, Instant::now()));
                }
            }
            self.paint();
        }
    }

    /// Deletes every measurement of a device; it is measured again from the next reading.
    fn reset_history(&mut self, key: &str) {
        self.history.flush();
        self.history.remove(key);
        self.history.mark_dirty();
        self.save_history_now();
        self.card_cache.clear();
        batteryhub::dashboard::notify_history_changed();
        dbg_log(&format!("measurements of {key} deleted"));
    }

    /// A level that drops to 0 % at once from more than a few percent is a misread more often than
    /// an empty battery (e.g. a headset's status report while it wakes up: 64 %, 0 %, 64 % three
    /// seconds later): it is shown and recorded only when the next reading says 0 % too; until then
    /// the level before it stays.
    fn doubt_zero(&mut self, fresh: &mut [DeviceStatus]) {
        for s in fresh.iter_mut() {
            if !s.connected || s.percent != 0 {
                self.zero_once.remove(&s.key);
                continue;
            }
            let before = self.devices.get(&s.key).map(|t| t.status.percent).unwrap_or(-1);
            if before > 5 && self.zero_once.insert(s.key.clone()) {
                s.percent = before;
            } else {
                self.zero_once.remove(&s.key);
            }
        }
    }

    /// Phones with "Rising level means charging" (experimental): a level above the last one marks
    /// the phone as charging until a level below the last one (e.g. 15 % saved, then 21 %:
    /// charging, until 82 % is followed by 81 %). A level that stays the same keeps it charging
    /// (Windows may just not report a new level), but not for longer than RISE_FLAT. The first
    /// level compared is the last one saved.
    fn infer_charging(&mut self, fresh: &mut [DeviceStatus]) {
        for s in fresh.iter_mut() {
            if s.kind != DeviceKind::Phone || s.percent < 0 || !s.connected || !settings::rise_charging(SETTINGS_KEY, &s.key) {
                continue;
            }
            let saved = self.history.latest(&s.key).map(|p| p.level as i32);
            let st = self.rising.entry(s.key.clone()).or_insert((saved.unwrap_or(s.percent), false, Instant::now()));
            if s.percent > st.0 {
                st.1 = true;
                st.2 = Instant::now();
            } else if s.percent < st.0 {
                st.1 = false;
                st.2 = Instant::now();
            } else if st.2.elapsed() >= RISE_FLAT {
                st.1 = false;
            }
            st.0 = s.percent;
            if st.1 && !s.charging {
                s.charging = true;
                s.full = s.percent >= 100;
            }
        }
    }

    /// "Delete device" in the dashboard: forgets a disconnected device altogether (measurements,
    /// settings and its entry). A device that is connected right now is left alone.
    fn delete_device(&mut self, key: &str) {
        if self.devices.get(key).is_some_and(|t| t.status.connected) {
            return;
        }
        self.devices.remove(key);
        self.order.retain(|k| k != key);
        self.reset_history(key);
        settings::forget_device(SETTINGS_KEY, key);
        self.saved_order = settings::order(SETTINGS_KEY);
        self.names = settings::names(SETTINGS_KEY);
        self.merged = settings::merged_targets(SETTINGS_KEY);
        self.hidden = settings::hidden(SETTINGS_KEY);
        self.paint();
        self.send_to_dashboards();
        dbg_log(&format!("device {key} deleted"));
    }

    /// Fresh readings for open dashboards (they show the live level and state).
    fn send_to_dashboards(&self) {
        if batteryhub::dashboard::any_open() {
            batteryhub::dashboard::send_statuses(self.main, &self.dashboard_devices());
        }
    }

    /// The devices the dashboard lists: the ones read now (in the strip's order), then every other
    /// device that has saved history, shown as not connected with its last saved level.
    fn dashboard_devices(&self) -> Vec<(DeviceStatus, String)> {
        let mut list: Vec<(DeviceStatus, String)> =
            self.order.iter().filter_map(|k| self.devices.get(k)).map(|t| (t.status.clone(), t.reported.clone())).collect();
        for (key, line) in settings::known(SETTINGS_KEY) {
            if list.iter().any(|(s, _)| s.key == key) {
                continue;
            }
            let Some(last) = self.history.latest(&key) else { continue };
            let Some((mut s, reported)) = batteryhub::dashboard::decode_status(&line) else { continue };
            s.key = key;
            if let Some(n) = self.names.get(&s.key) {
                s.name = n.clone();
            }
            s.connected = false;
            s.percent = last.level as i32;
            s.charging = false;
            list.push((s, reported));
        }
        list
    }

    /// Names chosen with "Rename device" were changed: show them everywhere.
    fn reload_names(&mut self) {
        self.names = settings::names(SETTINGS_KEY);
        for (k, t) in self.devices.iter_mut() {
            t.status.name = self.names.get(k).cloned().unwrap_or_else(|| t.reported.clone());
        }
        self.card_cache.clear();
        self.paint();
        self.send_to_dashboards();
    }

    /// "Warn at 5%": one notification when a device on battery falls to 5% (see settings).
    fn check_critical(&mut self, s: &DeviceStatus) {
        if !s.connected || s.asleep || !settings::warn_critical(SETTINGS_KEY, &s.key) {
            return;
        }
        if self.critical.check(&s.key, s.percent, s.charging) {
            self.notify_low(&s.name, s.percent);
        }
    }

    /// Shows a Windows notification (a balloon of a temporary notification-area icon, which
    /// Windows 10 / 11 show as a toast). The icon is removed when the notification is gone.
    fn notify_low(&mut self, name: &str, level: i32) {
        let size = unsafe { windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, windows::Win32::UI::HiDpi::GetDpiForSystem()) };
        let Some(icon) = ui::number_icon(&level.to_string(), ui::low_color(self.light), level, size, self.light) else { return };
        let mut d = self.tray_data(NOTIFY_UID);
        d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_INFO;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = icon;
        d.dwInfoFlags = NIIF_WARNING;
        ui::fill_wide(&mut d.szTip, &format!("{name}: {level}%"));
        ui::fill_wide(&mut d.szInfoTitle, &trf("{name}: battery at {level}%", &[("name", name), ("level", &level.to_string())]));
        ui::fill_wide(&mut d.szInfo, &tr("Charge it soon. BatteryHub warns once per discharge (right-click the device to turn this off)."));
        unsafe {
            if !Shell_NotifyIconW(NIM_MODIFY, &d).as_bool() {
                let _ = Shell_NotifyIconW(NIM_ADD, &d);
            }
            if let Some(old) = self.notify_icon.replace(icon) {
                let _ = DestroyIcon(old);
            }
            SetTimer(Some(self.main), TIMER_NOTIFY, 60_000, None);
        }
        dbg_log(&format!("low-battery notification: {name} {level}%"));
    }

    /// Shows a Windows notification with the app's icon (see notify_low).
    fn notify_info(&mut self, title: &str, text: &str) {
        let size = unsafe { windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, windows::Win32::UI::HiDpi::GetDpiForSystem()) };
        let icon = unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let Ok(h) = LoadImageW(Some(hinst.into()), windows::core::PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) else { return };
            HICON(h.0)
        };
        let mut d = self.tray_data(NOTIFY_UID);
        d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_INFO;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = icon;
        d.dwInfoFlags = NIIF_INFO;
        ui::fill_wide(&mut d.szTip, "BatteryHub");
        ui::fill_wide(&mut d.szInfoTitle, title);
        ui::fill_wide(&mut d.szInfo, text);
        unsafe {
            if !Shell_NotifyIconW(NIM_MODIFY, &d).as_bool() {
                let _ = Shell_NotifyIconW(NIM_ADD, &d);
            }
            if let Some(old) = self.notify_icon.replace(icon) {
                let _ = DestroyIcon(old);
            }
            SetTimer(Some(self.main), TIMER_NOTIFY, 60_000, None);
        }
    }

    /// Removes the temporary notification icon.
    fn end_notify(&mut self) {
        if let Some(icon) = self.notify_icon.take() {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data(NOTIFY_UID));
                let _ = DestroyIcon(icon);
                let _ = KillTimer(Some(self.main), TIMER_NOTIFY);
            }
        }
    }

    /// Menu refresh: `devices` = full rescan that drops devices that are gone right away;
    /// otherwise only the battery levels are read again (devices keep their grace period).
    fn manual_refresh(&mut self, devices: bool) {
        self.forget |= devices;
        self.manual = true;
        self.start_poll();
        self.paint(); // shows the ↻ indicator
    }

    fn on_result(&mut self, mut fresh: Vec<DeviceStatus>) {
        fresh.extend(demo_devices());
        self.infer_charging(&mut fresh);
        self.canonicalize(&mut fresh);
        self.polling = false;
        self.loaded = true;
        self.manual = false;
        self.devchange = false;
        if std::mem::take(&mut self.forget) {
            self.devices.clear();
            self.order.clear();
        }
        let keys: HashSet<String> = fresh.iter().map(|s| s.key.clone()).collect();
        // A device missing from this read:
        //  * read through a receiver / cable that is gone (unplugged): removed at once;
        //  * read through hardware that is still there: the device itself sleeps or missed one
        //    answer - keep the last value (dimmed while it sleeps) for its grace period;
        //  * Bluetooth / BLE / XInput devices are only reported while connected, so missing
        //    means disconnected: removed at once.
        self.devices.retain(|k, t| {
            if self.merged.contains_key(k) {
                return false; // a joined connection is now part of its target (#644)
            }
            if keys.contains(k) {
                return true;
            }
            let present = t.status.hw.is_some_and(batteryhub::device::hid_present);
            match batteryhub::device::missing_device(t.status.hw, present, t.status.grace_sec, t.seen.elapsed().as_secs(), MIN_GRACE_SEC) {
                Missing::Remove => false,
                Missing::Asleep => {
                    t.status.asleep = true;
                    true
                }
                Missing::Keep => true,
            }
        });
        self.offline_hw.clear();
        self.merge(fresh);
        let live: HashSet<String> = self.devices.keys().cloned().collect();
        self.ring_charge.retain(|k, _| live.contains(k));
        if std::mem::take(&mut self.pending_change) {
            self.devchange = true;
            self.start_poll();
        }
        if let Some(pair) = self.find_duplicate_pair() {
            self.duplicate_prompt = Some(pair);
            let _ = unsafe { PostMessageW(Some(self.main), WM_SAME_NAME_PROMPT, WPARAM(0), LPARAM(0)) };
        }
    }

    /// Finds two live connections the user sees under the same name. One offer per pair is kept in
    /// the registry, so a declined (or accepted) offer never comes back after a restart.
    fn find_duplicate_pair(&self) -> Option<(String, String)> {
        if !settings::same_name_prompt(SETTINGS_KEY) {
            return None;
        }
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, t) in &self.devices {
            if !t.status.connected || t.status.asleep || key.starts_with("demo:") || settings::is_merged(&self.merged, key) {
                continue;
            }
            let name = t.status.name.trim();
            if !name.is_empty() {
                groups.entry(name.to_lowercase()).or_default().push(key.clone());
            }
        }
        groups.values().find_map(|keys| {
            first_unasked_pair(keys, |a, b| settings::duplicate_prompt_asked(SETTINGS_KEY, a, b))
        })
    }
}

/// First pair of same-named connections whose offer has not been made yet (oldest keys first).
fn first_unasked_pair(keys: &[String], asked: impl Fn(&str, &str) -> bool) -> Option<(String, String)> {
    for i in 0..keys.len() {
        for j in i + 1..keys.len() {
            let (a, b) = if keys[i] <= keys[j] { (&keys[i], &keys[j]) } else { (&keys[j], &keys[i]) };
            if !asked(a, b) {
                return Some((a.clone(), b.clone()));
            }
        }
    }
    None
}

impl App {
    /// Re-reads the taskbar theme and TaskbarOffset (start-up and WM_SETTINGCHANGE only).
    fn refresh_settings(&mut self) {
        settings::forget_obsolete_merges(SETTINGS_KEY);
        self.light = ui::taskbar_is_light();
        self.offset = batteryhub::util::reg_get_dword(SETTINGS_KEY, "TaskbarOffset").unwrap_or(0);
        let side = batteryhub::util::reg_get_dword(SETTINGS_KEY, "WidgetLeft").unwrap_or(0);
        self.side = side.min(SIDE_FREE);
        self.align_left = side != SIDE_RIGHT;
        self.after_icons = side == SIDE_ICONS;
        self.free_x = batteryhub::util::reg_get_dword(SETTINGS_KEY, "WidgetFreeX").unwrap_or(0) as i32;
        self.mirroring = batteryhub::util::reg_get_dword(SETTINGS_KEY, "MirrorWidgets").is_none_or(|v| v != 0);
        self.lock_order = batteryhub::util::reg_get_dword(SETTINGS_KEY, "LockDeviceOrder").unwrap_or(0) != 0;
        self.charge_anim = batteryhub::util::reg_get_dword(SETTINGS_KEY, "ChargingAnimation").unwrap_or(0) != 0;
        self.ring = batteryhub::util::reg_get_dword(SETTINGS_KEY, "WidgetStyle").unwrap_or(0) == 1;
        self.connection_icons = batteryhub::util::reg_get_dword(SETTINGS_KEY, "ConnectionIcons").unwrap_or(0) != 0;
        settings::ORDER_LOCKED.store(self.lock_order, std::sync::atomic::Ordering::Relaxed);
        self.hide_empty = batteryhub::util::reg_get_dword(SETTINGS_KEY, "HideWhenEmpty").unwrap_or(0) != 0;
        self.offset_y = batteryhub::util::reg_get_dword(SETTINGS_KEY, "TaskbarOffsetY").unwrap_or(0) as i32;
        self.background = batteryhub::util::reg_get_dword(SETTINGS_KEY, "WidgetBackground").unwrap_or(0) != 0;
        self.icons_mode = batteryhub::util::reg_get_dword(SETTINGS_KEY, "NotificationIcons").unwrap_or(0) != 0;
        self.compact = batteryhub::util::reg_get_dword(SETTINGS_KEY, "CompactWidget").unwrap_or(0) != 0;
        self.choice = widget_choice();
    }
}

// ---- check for updates ---------------------------------------------------------------------

/// A check or download is running (the menu item is greyed out meanwhile).
static UPDATE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const UPDATE_TITLE: &str = "BatteryHub - Check for updates";

/// Runs `work` on a worker thread and posts its boxed result to `hwnd` as `msg`.
fn post_from_thread<T: Send + 'static>(hwnd: HWND, msg: u32, work: impl FnOnce() -> T + Send + 'static) {
    let h = hwnd.0 as usize;
    std::thread::spawn(move || {
        let ptr = Box::into_raw(Box::new(work()));
        unsafe {
            if PostMessageW(Some(HWND(h as *mut c_void)), msg, WPARAM(0), LPARAM(ptr as isize)).is_err() {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

/// The running check was started automatically: only "a new update is available" is shown.
static UPDATE_SILENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The running download is a silent install: no message boxes, the setup runs without a window.
static UPDATE_QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn check_for_updates(main: HWND, silent: bool) {
    if UPDATE_BUSY.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    UPDATE_SILENT.store(silent, std::sync::atomic::Ordering::Relaxed);
    post_from_thread(main, WM_UPDATE_CHECKED, batteryhub::update::check);
}

fn message(owner: HWND, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        let _ = SetForegroundWindow(owner);
        MessageBoxW(Some(owner), &windows::core::HSTRING::from(text), &windows::core::HSTRING::from(tr(UPDATE_TITLE)), style | MB_SETFOREGROUND | MB_TOPMOST)
    }
}

fn on_update_checked(main: HWND, check: batteryhub::update::Check) {
    use batteryhub::install::{label_of, version_label};
    use batteryhub::update::Check;
    trim_memory();
    let silent = UPDATE_SILENT.swap(false, std::sync::atomic::Ordering::Relaxed);
    if silent && !matches!(check, Check::Available(_)) {
        UPDATE_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
        if matches!(check, Check::Failed(_)) && AUTO_UPDATE_TRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < AUTO_UPDATE_RETRIES {
            unsafe {
                SetTimer(Some(main), TIMER_AUTO_UPDATE, AUTO_UPDATE_RETRY_MS, None);
            }
        }
        return;
    }
    match check {
        Check::UpToDate { latest } => {
            UPDATE_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
            message(
                main,
                &trf(
                    "No new updates.\n\nYou have the latest version ({current}).\nLatest release on GitHub: {latest}.",
                    &[("current", &version_label()), ("latest", &label_of(&latest))],
                ),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Check::Failed(e) => {
            UPDATE_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
            message(
                main,
                &trf(
                    "Could not check for updates:\n{error}\n\nNew versions are also on GitHub:\n{url}",
                    &[("error", &e.to_string()), ("url", &format!("https://github.com/{}/releases", batteryhub::update::REPO))],
                ),
                MB_OK | MB_ICONWARNING,
            );
        }
        // Automatic check with "Install updates silently": download and install, show nothing.
        Check::Available(r) if silent && silent_install() => {
            UPDATE_QUIET.store(true, std::sync::atomic::Ordering::Relaxed);
            post_from_thread(main, WM_UPDATE_DOWNLOADED, move || batteryhub::update::download(&r));
        }
        Check::Available(r) => {
            let text = trf(
                "A new update is available:\n\n        {current}  ->  {new}\n\nDo you want to update now?\n\nThe setup is downloaded from GitHub, checked against the release's SHA-256 checksum and started; it keeps your settings and autostart.",
                &[("current", &version_label()), ("new", &label_of(&r.version))],
            );
            if message(main, &text, MB_YESNO | MB_ICONQUESTION) == IDYES {
                post_from_thread(main, WM_UPDATE_DOWNLOADED, move || batteryhub::update::download(&r));
            } else {
                UPDATE_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

fn on_update_downloaded(main: HWND, result: Result<batteryhub::update::Verified, String>) {
    UPDATE_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
    trim_memory();
    if UPDATE_QUIET.swap(false, std::sync::atomic::Ordering::Relaxed) {
        // No console window and no "Update finished" box afterwards. A failed download or start
        // is not shown either; the next start tries again.
        if let Ok(setup) = result {
            let _ = setup.start(&["/update", "/S", "/nonotify"], true);
        }
        return;
    }
    match result {
        // The setup closes this program (and starts the new version) itself.
        Ok(setup) => {
            let path = setup.path.clone();
            if let Err(e) = setup.start(&["/update"], false) {
                message(
                    main,
                    &trf("Could not start the setup:\n{error}\n\n{path}", &[("error", &e.to_string()), ("path", &path.display().to_string())]),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
        Err(e) => {
            message(
                main,
                &format!(
                    "The update could not be downloaded:
{e}"
                ),
                MB_OK | MB_ICONWARNING,
            );
        }
    }
}

/// "Install another version...": the setup kept next to the program opens on its version list
/// (any release, also an older one; verified like an update).
/// Opens a GitHub page in the default browser.
fn open_url(url: &str) {
    if !url.starts_with("https://github.com/") && url != DISCORD_URL && url != SUPPORT_URL {
        return;
    }
    unsafe {
        let _ = windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &windows::core::HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
}

/// The latest releases (version, page), newest first, for the Help menu. Fetched in the
/// background at most once an hour (when the menu opens); empty until the first answer.
fn recent_releases() -> Vec<(String, String)> {
    type Releases = Option<(Instant, Vec<(String, String)>)>;
    static CACHE: std::sync::Mutex<Releases> = std::sync::Mutex::new(None);
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let (list, stale) = match &*CACHE.lock().unwrap_or_else(|e| e.into_inner()) {
        Some((at, list)) => (list.clone(), at.elapsed() > std::time::Duration::from_secs(3600) || list.is_empty() && at.elapsed() > std::time::Duration::from_secs(60)),
        None => (Vec::new(), true),
    };
    if stale && !BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        std::thread::spawn(|| {
            let found = batteryhub::update::list_releases();
            if let Err(e) = &found {
                dbg_log(&format!("recent releases: {e}"));
            }
            let list: Vec<(String, String)> =
                found.map(|v| v.into_iter().filter(|r| !r.prerelease).take(HELP_RELEASES).map(|r| (r.version, r.page_url)).collect()).unwrap_or_default();
            dbg_log(&format!("recent releases: {} loaded", list.len()));
            *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list));
            BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
    list
}

fn open_version_list(main: HWND) {
    let setup = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("BatteryHub-Setup.exe")));
    match setup {
        Some(s) if s.exists() => {
            let _ = std::process::Command::new(s).arg("/versions").spawn();
        }
        _ => {
            message(
                main,
                &format!(
                    "The setup is not next to this program (it is kept there by an installation).

All versions are on GitHub:
https://github.com/{}/releases",
                    batteryhub::update::REPO
                ),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
}

/// Hands memory that was only needed for a moment (a read, a Bluetooth check, a render) back to
/// Windows, so between those moments the program keeps well under 1 MB resident. Pages it needs
/// again come back from the standby list (soft faults), which costs microseconds.
fn trim_memory() {
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Battery refresh interval in minutes (menu choice, default 1 minute).
/// The language chosen in the menu: a language code, or "" for Windows' language.
fn language_choice() -> String {
    batteryhub::util::reg_get_string(SETTINGS_KEY, "Language").unwrap_or_default()
}

/// Switches the language ("" = Windows' language), saves the choice and redraws.
fn set_language(code: &str) {
    if code.is_empty() {
        batteryhub::util::reg_delete_value(SETTINGS_KEY, "Language");
    } else {
        batteryhub::util::reg_set_string(SETTINGS_KEY, "Language", code);
    }
    batteryhub::i18n::set_language(code);
    with_app(|a| {
        a.hide_tip();
        a.card_cache.clear();
        a.paint();
    });
}

fn refresh_minutes() -> u32 {
    batteryhub::util::reg_get_dword(SETTINGS_KEY, "RefreshMinutes").filter(|m| INTERVALS.iter().any(|(v, _)| v == m)).unwrap_or(1)
}

/// Refresh when a device connects or disconnects (default on). Read from the registry once and
/// cached (it is checked every few seconds); the menu updates the cache when it is toggled.
static ON_CHANGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2); // 2 = not read yet

fn refresh_on_change() -> bool {
    use std::sync::atomic::Ordering;
    match ON_CHANGE.load(Ordering::Relaxed) {
        2 => {
            let on = batteryhub::util::reg_get_dword(SETTINGS_KEY, "RefreshOnDeviceChange").unwrap_or(1) != 0;
            ON_CHANGE.store(on as u8, Ordering::Relaxed);
            on
        }
        v => v != 0,
    }
}

/// Check for updates when BatteryHub starts (default off).
fn auto_update() -> bool {
    batteryhub::util::reg_get_dword(SETTINGS_KEY, "AutoCheckUpdates").unwrap_or(0) != 0
}

/// With the automatic check: install a new version in the background without asking (default off).
fn silent_install() -> bool {
    batteryhub::util::reg_get_dword(SETTINGS_KEY, "SilentUpdates").unwrap_or(0) != 0
}

fn autostart_name() -> &'static str {
    batteryhub::install::TASKBAR.run_value.unwrap_or("BatteryHub Taskbar")
}

fn save_dword(name: &str, value: u32) {
    if let Some(k) = batteryhub::install::create(batteryhub::install::HKCU, SETTINGS_KEY) {
        k.set_dword(name, value);
    }
}

/// A full-screen window (game, video) is in front on the monitor of taskbar `bar`. Explorer
/// itself decides that: it takes such a monitor's taskbar out of the topmost band (it goes behind
/// the game), so the copy follows the taskbar. Guessing from the window's size hid it wrongly for
/// borderless windows that cover a monitor without being full-screen apps (e.g. Wallpaper Engine).
fn fullscreen_over(bar: HWND) -> bool {
    unsafe { GetWindowLongW(bar, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 == 0 }
}

/// EVENT_SYSTEM_FOREGROUND: tells the main window, which places the copies again.
unsafe extern "system" fn foreground_changed(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    unsafe {
        if let Ok(main) = FindWindowW(MAIN_CLASS, None) {
            let _ = PostMessageW(Some(main), WM_FOREGROUND, WPARAM(0), LPARAM(0));
        }
    }
}

/// Creates a (hidden) copy for another monitor's taskbar: a topmost popup owned by that taskbar,
/// not a child of it. Windows 11 composes child windows of the secondary taskbars late (their
/// animations stuttered, and a new copy could stay blank), top-level windows on time.
fn create_copy(taskbar: HWND) -> Option<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let r = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            STRIP_CLASS,
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            Some(taskbar),
            None,
            Some(hinst.into()),
            None,
        );
        if r.is_err() {
            dbg_log(&format!("CreateWindowExW (copy) failed: {:?}", windows::Win32::Foundation::GetLastError()));
        }
        r.ok()
    }
}

/// Stardock Start11's enhanced taskbar sits inside `Shell_TrayWnd` as `SDTaskbar` and fights
/// child-window Z-order; a topmost popup owned by `Shell_TrayWnd` stays cleanly above it.
fn has_sdtaskbar(bar: HWND) -> bool {
    unsafe {
        let mut after: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(Some(bar), after, w!("#32770"), None) {
            if h.0.is_null() {
                break;
            }
            after = Some(h);
            let mut txt = [0u16; 32];
            let n = GetWindowTextW(h, &mut txt) as usize;
            if batteryhub::util::from_wide(&txt[..n]) == "SDTaskbar" {
                return true;
            }
        }
        false
    }
}

/// Creates a (hidden) strip window as a child of a taskbar.
fn create_strip(taskbar: HWND) -> Option<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let r = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_NOACTIVATE,
            STRIP_CLASS,
            w!(""),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            Some(taskbar),
            None,
            Some(hinst.into()),
            None,
        );
        if r.is_err() {
            dbg_log(&format!("CreateWindowExW failed: {:?}", windows::Win32::Foundation::GetLastError()));
        }
        r.ok()
    }
}

/// Taskbars of the other monitors, left to right.
fn secondary_taskbars() -> Vec<HWND> {
    let mut out: Vec<(i32, i32, HWND)> = Vec::new();
    unsafe {
        let mut after: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(None, after, w!("Shell_SecondaryTrayWnd"), None) {
            if h.0.is_null() {
                break;
            }
            after = Some(h);
            if !IsWindowVisible(h).as_bool() {
                continue;
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            out.push((r.left, r.top, h));
        }
    }
    out.sort_by_key(|o| (o.0, o.1));
    out.into_iter().map(|o| o.2).collect()
}

/// "Show widget on": "all", "main" (default) or the id of one other monitor. v1.2 stored a number
/// of monitors instead (1 = main only, more = all).
fn widget_choice() -> String {
    if let Some(v) = batteryhub::util::reg_get_string(SETTINGS_KEY, "WidgetMonitor").filter(|v| !v.is_empty()) {
        return v;
    }
    match batteryhub::util::reg_get_dword(SETTINGS_KEY, "WidgetMonitors") {
        Some(n) if n >= 2 => "all".into(),
        _ => "main".into(),
    }
}

fn monitor_info(bar: HWND) -> Option<MONITORINFOEXW> {
    unsafe {
        let mon = MonitorFromWindow(bar, MONITOR_DEFAULTTONULL);
        if mon.is_invalid() {
            return None;
        }
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        GetMonitorInfoW(mon, &mut mi.monitorInfo as *mut MONITORINFO).as_bool().then_some(mi)
    }
}

/// A monitor's lasting identity: its device interface name (survives reboots, Explorer restarts and
/// Windows renumbering the displays), else "\\.\DISPLAYn".
fn monitor_id(bar: HWND) -> String {
    let Some(mi) = monitor_info(bar) else { return String::new() };
    let device = batteryhub::util::from_wide(&mi.szDevice);
    unsafe {
        let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if EnumDisplayDevicesW(&windows::core::HSTRING::from(device.as_str()), 0, &mut dd, 1 /* EDD_GET_DEVICE_INTERFACE_NAME */).as_bool() {
            let id = batteryhub::util::from_wide(&dd.DeviceID);
            if !id.is_empty() {
                return id;
            }
        }
    }
    device
}

/// "Display 2 (left)": the number Windows gives the display and where it is next to the main one.
fn monitor_label(bar: HWND) -> String {
    let Some(mi) = monitor_info(bar) else { return tr("Other monitor") };
    let device = batteryhub::util::from_wide(&mi.szDevice);
    let mut label = match device.find("DISPLAY") {
        Some(i) => trf("Display {number}", &[("number", &device[i + 7..])]),
        None => tr("Other monitor"),
    };
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut pm = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut pm).as_bool() {
            let (m, p) = (mi.monitorInfo.rcMonitor, pm.rcMonitor);
            let dx = (m.left + m.right - p.left - p.right) / 2;
            let dy = (m.top + m.bottom - p.top - p.bottom) / 2;
            let side = if dx.abs() >= dy.abs() {
                if dx < 0 { n_("{display} (left)") } else { n_("{display} (right)") }
            } else if dy < 0 {
                n_("{display} (above)")
            } else {
                n_("{display} (below)")
            };
            label = trf(side, &[("display", &label)]);
        }
    }
    label
}

/// The model part of a monitor id ("AUO7EAD" in \\?\DISPLAY#AUO7EAD#4&...#{...}).
fn monitor_model(id: &str) -> &str {
    let Some(a) = id.find("DISPLAY#").map(|a| a + 8) else { return "" };
    id[a..].find('#').map(|b| &id[a..a + b]).unwrap_or("")
}

/// Index of the chosen monitor among `ids`, or None when it is not connected. A saved id that
/// matches none exactly still matches the only connected monitor of the same model (a laptop
/// panel's id changes when a MUX switch moves it to the other GPU).
fn find_monitor(ids: &[String], choice: &str) -> Option<usize> {
    if let Some(i) = ids.iter().position(|i| i == choice) {
        return Some(i);
    }
    let model = monitor_model(choice);
    if model.is_empty() {
        return None;
    }
    let same: Vec<usize> = (0..ids.len()).filter(|&i| monitor_model(&ids[i]) == model).collect();
    (same.len() == 1).then(|| same[0])
}

/// A strip window created by this process (a handle that outlived Explorer may have been reused by
/// someone else's window, which must never be destroyed).
/// Notification-area icon id for a device: stable (from its key), never 0, and different from the
/// ids already in use (a hash clash moves to the next free id).
fn tray_uid(key: &str, used: Vec<u32>) -> u32 {
    let mut uid = (batteryhub::util::fnv1a(key) & 0x7FFF_FFFF).max(1);
    while used.contains(&uid) {
        uid = uid.wrapping_add(1).max(1);
    }
    uid
}

fn is_our_strip(h: HWND) -> bool {
    unsafe {
        if h.0.is_null() || !IsWindow(Some(h)).as_bool() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut class = [0u16; 64];
        let n = GetClassNameW(h, &mut class) as usize;
        pid == std::process::id() && String::from_utf16_lossy(&class[..n]) == "BatteryHub.Taskbar.Strip"
    }
}

/// "BatteryHub is free": shown once after a new installation.
fn free_notice() {
    let text = tr(FREE_NOTICE);
    unsafe {
        MessageBoxW(
            None,
            &windows::core::HSTRING::from(text),
            &windows::core::HSTRING::from(tr("BatteryHub is free")),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

const FREE_NOTICE: &str = n_("BatteryHub is free and will always stay free.\n\nNobody should ever pay for it. If you paid someone for BatteryHub, you were scammed: ask them for a refund.\n\nThe only official place to get it is the GitHub page: github.com/ferloom/Wireless-Device-Battery-Monitor\n\nMade with love by ferloom.");

/// What the card of a device holds: (battery chart, time left). "Battery only" turns both off.
fn card_opts(key: &str) -> (bool, bool) {
    if settings::battery_only(SETTINGS_KEY, key) {
        return (false, false);
    }
    (settings::card_chart(SETTINGS_KEY, key), settings::card_time(SETTINGS_KEY, key))
}

fn tool_info(owner: HWND, text: &mut [u16]) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_TRACK | TTF_ABSOLUTE,
        hwnd: owner,
        uId: 1,
        lpszText: windows::core::PWSTR(text.as_mut_ptr()),
        ..Default::default()
    }
}

fn dbg_log(msg: &str) {
    if std::env::var_os("BH_DEBUG").is_some() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("bh2.log")) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

/// The right-click menu (strip and fallback icons): per-device warning levels, refresh options, exit.
/// The taskbar is set to hide automatically.
fn taskbar_autohides() -> bool {
    use windows::Win32::UI::Shell::{ABM_GETSTATE, APPBARDATA, SHAppBarMessage};
    let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, ..Default::default() };
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut abd) & 0x1 != 0 } // ABS_AUTOHIDE
}

/// Window that owns the context menu. A right-click on the widget inside a taskbar gives the menu to
/// that widget (a child of the taskbar): the taskbar then is the active window and stays shown while
/// the menu is open, also when it hides itself automatically (it hid and took the menu along when our
/// own window owned it). A copy on another monitor (a window of its own) owns its menu only while the
/// taskbars hide automatically; everything else (tray icon, card) uses our hidden main window.
/// `from` is the window that was right-clicked; the cursor position alone missed the widget where its
/// pixels are transparent.
unsafe fn menu_owner(main: HWND, from: Option<HWND>) -> HWND {
    unsafe {
        let hides = taskbar_autohides();
        let owner = match from {
            Some(h) if !h.0.is_null() && IsWindow(Some(h)).as_bool() => {
                let child = GetWindowLongW(h, GWL_STYLE) as u32 & WS_CHILD.0 != 0;
                if child || hides { h } else { main }
            }
            _ => main,
        };
        dbg_log(&format!("menu: from {from:?} autohide {hides} owner {owner:?} main {main:?}"));
        owner
    }
}

/// Windows' colour dialog, starting at `current`; None when cancelled.
unsafe fn choose_color(owner: HWND, current: Option<(u8, u8, u8)>) -> Option<(u8, u8, u8)> {
    use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
    static CUSTOM: std::sync::Mutex<[u32; 16]> = std::sync::Mutex::new([0x00FF_FFFF; 16]);
    let mut custom = CUSTOM.lock().unwrap_or_else(|e| e.into_inner()).map(COLORREF);
    let mut cc = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: current.map(|(r, g, b)| rgb(r, g, b)).unwrap_or(COLORREF(0x00FF_FFFF)),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    let ok = unsafe { ChooseColorW(&mut cc) }.as_bool();
    *CUSTOM.lock().unwrap_or_else(|e| e.into_inner()) = custom.map(|c| c.0);
    let c = cc.rgbResult.0;
    ok.then_some(((c & 0xFF) as u8, (c >> 8 & 0xFF) as u8, (c >> 16 & 0xFF) as u8))
}

/// Windows' file dialog for a device's own icon (#476); None when cancelled.
unsafe fn choose_icon(owner: HWND) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Controls::Dialogs::{
        OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW, GetOpenFileNameW,
    };
    let filter: Vec<u16> = "Icon files\0*.ico;*.png;*.bmp;*.gif;*.jpg;*.jpeg\0All files\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = tr("Custom icon...").encode_utf16().chain(Some(0)).collect();
    let mut file = vec![0u16; 32768];
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR::from_raw(filter.as_ptr()),
        lpstrFile: PWSTR::from_raw(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR::from_raw(title.as_ptr()),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_HIDEREADONLY | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let n = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    let path = String::from_utf16_lossy(&file[..n]);
    (!path.is_empty()).then_some(path)
}

unsafe fn show_menu(from: Option<HWND>) {
    unsafe {
        // One menu at a time. While it is up, `MENU_OPEN` also keeps the widget from popping out
        // (see `place`): a second right-click here would nest menus and leave the flag out of sync.
        let Some(guard) = MenuGuard::enter() else { return };
        let (main, devices, renamed, hidden, vertical) = with_app(|a| {
            a.hide_tip();
            a.menu_on = from.filter(|h| a.view(*h).is_some()).unwrap_or(a.strip);
            let renamed: HashSet<String> = a.names.keys().cloned().collect();
            (a.main, a.connected(), renamed, a.hidden.clone(), a.vertical)
        })
        .unwrap_or_default();
        let dark = !ui::taskbar_is_light();
        let mut icons = ui::MenuIcons::new(main, dark);
        if let Ok(menu) = CreatePopupMenu() {
            ui::MenuIcons::check_or_icon(menu);
            // The dashboard at once (it opens on the first device; the others are a click away).
            let has_any = !devices.is_empty() || with_app(|a| !a.dashboard_devices().is_empty()).unwrap_or(false);
            let _ = AppendMenuW(menu, MF_STRING | if has_any { MF_ENABLED } else { MF_GRAYED }, ID_DASHBOARD, &windows::core::HSTRING::from(tr("Dashboard...")));
            icons.set(menu, ID_DASHBOARD as u32, false, ui::glyph::DASHBOARD);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            // One submenu per device: dashboard, history, statistics, rename and its warnings.
            settings::append_device_menus(menu, &devices, SETTINGS_KEY, &renamed, &hidden, vertical, &mut icons);
            if !devices.is_empty() {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            }
            let _ = AppendMenuW(menu, MF_STRING, ID_REFRESH_DEVICES, &windows::core::HSTRING::from(tr("Refresh devices")));
            icons.set(menu, ID_REFRESH_DEVICES as u32, false, ui::glyph::SYNC);
            let _ = AppendMenuW(menu, MF_STRING, ID_REFRESH_LEVELS, &windows::core::HSTRING::from(tr("Refresh battery levels")));
            icons.set(menu, ID_REFRESH_LEVELS as u32, false, ui::glyph::REFRESH);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            // Look of the widget: plate behind it, position, and which monitors show it.
            let (choice, background, icons_mode, hide_empty, vertical_bar, compact) =
                with_app(|a| (a.choice.clone(), a.background, a.icons_mode, a.hide_empty, a.vertical, a.compact)).unwrap_or_default();
            // The settings of the widget the menu was opened on (its own with "Mirror other widgets" off).
            let (target, lay, mirroring, lock_order) =
                with_app(|a| (a.menu_on, a.layout_of(a.menu_on), a.mirroring, a.lock_order)).unwrap_or_default();
            let (offset, offset_y, align_left) = (lay.offset, lay.offset_y, lay.side != SIDE_RIGHT);
            let others: Vec<(String, String)> =
                secondary_taskbars().into_iter().take(ID_MONITOR_GONE - ID_MONITOR_FIRST).map(|b| (monitor_id(b), monitor_label(b))).collect();
            let languages = batteryhub::i18n::available();
            // Widget: how and where the strip looks (#520).
            if let Ok(widget) = CreatePopupMenu() {
                ui::MenuIcons::check_or_icon(widget);
                let _ = AppendMenuW(
                    widget,
                    MF_STRING | check(!hide_empty),
                    ID_SHOW_EMPTY,
                    &windows::core::HSTRING::from(tr("Show the icon when no device is connected")),
                );
                let _ = AppendMenuW(
                    widget,
                    MF_STRING | check(icons_mode),
                    ID_ICONS_MODE,
                    &windows::core::HSTRING::from(tr("Show as notification-area icons (Windows arranges them)")),
                );
                let _ = AppendMenuW(
                    widget,
                    MF_STRING | check(background),
                    ID_BACKGROUND,
                    &windows::core::HSTRING::from(tr("Background behind taskbar text (for translucent taskbars)")),
                );
                // Widget position: next to the clock or next to the taskbar icons (horizontal taskbars).
                if let Ok(sub) = CreatePopupMenu() {
                    let _ = AppendMenuW(sub, MF_STRING, ID_SIDE_RIGHT, &windows::core::HSTRING::from(tr("Right")));
                    let left_ok = if vertical_bar { MF_GRAYED } else { MF_ENABLED };
                    let _ = AppendMenuW(sub, MF_STRING | left_ok, ID_SIDE_ICONS, &windows::core::HSTRING::from(tr("Next to the icons")));
                    let _ = AppendMenuW(sub, MF_STRING | left_ok, ID_SIDE_LEFT, &windows::core::HSTRING::from(tr("Left")));
                    // Right before the taskbar icons: only when they are centred.
                    let before_ok = if vertical_bar || !taskbar_centred() { MF_GRAYED } else { MF_ENABLED };
                    let _ = AppendMenuW(sub, MF_STRING | before_ok, ID_SIDE_BEFORE, &windows::core::HSTRING::from(tr("Before the icons")));
                    let on = match lay.side {
                        _ if vertical_bar => Some(ID_SIDE_RIGHT),
                        SIDE_LEFT => Some(ID_SIDE_LEFT),
                        SIDE_ICONS => Some(ID_SIDE_ICONS),
                        SIDE_BEFORE => Some(ID_SIDE_BEFORE),
                        SIDE_FREE => None,
                        _ => Some(ID_SIDE_RIGHT),
                    };
                    match on {
                        Some(on) => {
                            let _ = CheckMenuRadioItem(sub, ID_SIDE_RIGHT as u32, ID_SIDE_BEFORE as u32, on as u32, MF_BYCOMMAND.0);
                        }
                        None => {
                            // Put somewhere with the mouse ("Position on the taskbar > Change position").
                            let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED | MF_CHECKED, 0, &windows::core::HSTRING::from(tr("Where you moved it")));
                        }
                    }
                    let n = GetMenuItemCount(Some(widget));
                    let _ = AppendMenuW(widget, MF_STRING | MF_POPUP, sub.0 as usize, &windows::core::HSTRING::from(tr("Widget position")));
                    icons.set(widget, n as u32, true, ui::glyph::SETTINGS);
                }
                let _ = AppendMenuW(
                    widget,
                    MF_STRING | check(compact),
                    ID_COMPACT,
                    &windows::core::HSTRING::from(tr("Icons above percentages (narrower)")),
                );
                if let Ok(sub) = CreatePopupMenu() {
                    let mut checked = ID_POSITION_CUSTOM;
                    for (i, px) in POSITION_OFFSETS.iter().enumerate() {
                        let label = if *px == 0 { tr("Next to the notification area") } else { trf("{px} px to the left", &[("px", &px.to_string())]) };
                        let grey = if align_left && !vertical_bar { MF_GRAYED } else { MF_ENABLED };
                        let _ = AppendMenuW(sub, MF_STRING | grey, ID_POSITION_FIRST + i, &windows::core::HSTRING::from(label));
                        if *px == offset {
                            checked = ID_POSITION_FIRST + i;
                        }
                    }
                    if checked == ID_POSITION_CUSTOM {
                        // Set with the 10 px steps (or by hand in the registry, TaskbarOffset).
                        let label = trf("Custom: {px} px to the left", &[("px", &offset.to_string())]);
                        let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, ID_POSITION_CUSTOM, &windows::core::HSTRING::from(label));
                    }
                    let _ = CheckMenuRadioItem(sub, ID_POSITION_FIRST as u32, ID_POSITION_CUSTOM as u32, checked as u32, MF_BYCOMMAND.0);
                    let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                    let step = |on: bool| if on && !(align_left && !vertical_bar) { MF_STRING } else { MF_STRING | MF_GRAYED };
                    let _ = AppendMenuW(sub, step(offset < POSITION_MAX), ID_POSITION_LEFT, &windows::core::HSTRING::from(tr("Move 10 px left")));
                    icons.set(sub, ID_POSITION_LEFT as u32, false, ui::glyph::LEFT);
                    let _ = AppendMenuW(sub, step(offset > 0), ID_POSITION_RIGHT, &windows::core::HSTRING::from(tr("Move 10 px right")));
                    icons.set(sub, ID_POSITION_RIGHT as u32, false, ui::glyph::RIGHT);
                    // Anywhere along the taskbar: the widget follows the mouse, a click puts it down.
                    let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING | if vertical_bar { MF_GRAYED } else { MF_ENABLED },
                        ID_POSITION_MOVE,
                        &windows::core::HSTRING::from(tr("Change position (move it, then click)")),
                    );
                    icons.set(sub, ID_POSITION_MOVE as u32, false, ui::glyph::SETTINGS);
                    let n = GetMenuItemCount(Some(widget));
                    let _ = AppendMenuW(widget, MF_STRING | MF_POPUP, sub.0 as usize, &windows::core::HSTRING::from(tr("Position on the taskbar")));
                    icons.set(widget, n as u32, true, ui::glyph::SETTINGS);
                }
                if let Ok(sub) = CreatePopupMenu() {
                    let mut checked = ID_HEIGHT_CUSTOM;
                    for (i, px) in HEIGHT_OFFSETS.iter().enumerate() {
                        let label = match *px {
                            0 => tr("Centred"),
                            p if p < 0 => trf("{px} px up", &[("px", &(-p).to_string())]),
                            p => trf("{px} px down", &[("px", &p.to_string())]),
                        };
                        let _ = AppendMenuW(sub, MF_STRING, ID_HEIGHT_FIRST + i, &windows::core::HSTRING::from(label));
                        if *px == offset_y {
                            checked = ID_HEIGHT_FIRST + i;
                        }
                    }
                    if checked == ID_HEIGHT_CUSTOM {
                        let label = if offset_y < 0 {
                            trf("Custom: {px} px up", &[("px", &(-offset_y).to_string())])
                        } else {
                            trf("Custom: {px} px down", &[("px", &offset_y.to_string())])
                        };
                        let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, ID_HEIGHT_CUSTOM, &windows::core::HSTRING::from(label));
                    }
                    let _ = CheckMenuRadioItem(sub, ID_HEIGHT_FIRST as u32, ID_HEIGHT_CUSTOM as u32, checked as u32, MF_BYCOMMAND.0);
                    let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                    let step = |on: bool| if on { MF_STRING } else { MF_STRING | MF_GRAYED };
                    let _ = AppendMenuW(sub, step(offset_y > -HEIGHT_MAX), ID_HEIGHT_UP, &windows::core::HSTRING::from(tr("Move 10 px up")));
                    icons.set(sub, ID_HEIGHT_UP as u32, false, ui::glyph::UP);
                    let _ = AppendMenuW(sub, step(offset_y < HEIGHT_MAX), ID_HEIGHT_DOWN, &windows::core::HSTRING::from(tr("Move 10 px down")));
                    icons.set(sub, ID_HEIGHT_DOWN as u32, false, ui::glyph::DOWN);
                    let n = GetMenuItemCount(Some(widget));
                    let _ = AppendMenuW(widget, MF_STRING | MF_POPUP, sub.0 as usize, &windows::core::HSTRING::from(tr("Height on the taskbar")));
                    icons.set(widget, n as u32, true, ui::glyph::SETTINGS);
                }
                // With more than one monitor: all of them, the main one only, or one other monitor only.
                if !others.is_empty()
                    && let Ok(sub) = CreatePopupMenu()
                {
                    let _ = AppendMenuW(sub, MF_STRING, ID_MONITOR_ALL, &windows::core::HSTRING::from(tr("All monitors")));
                    let _ = AppendMenuW(sub, MF_STRING, ID_MONITOR_MAIN, &windows::core::HSTRING::from(tr("Main monitor only")));
                    let ids: Vec<String> = others.iter().map(|o| o.0.clone()).collect();
                    let checked = match choice.as_str() {
                        "all" => ID_MONITOR_ALL,
                        "main" => ID_MONITOR_MAIN,
                        id => find_monitor(&ids, id).map(|i| ID_MONITOR_FIRST + i).unwrap_or(ID_MONITOR_GONE),
                    };
                    for (i, (_, label)) in others.iter().enumerate() {
                        let _ = AppendMenuW(sub, MF_STRING, ID_MONITOR_FIRST + i, &windows::core::HSTRING::from(trf("{monitor} only", &[("monitor", label)])));
                    }
                    if checked == ID_MONITOR_GONE {
                        let _ = AppendMenuW(
                            sub,
                            MF_STRING | MF_GRAYED,
                            ID_MONITOR_GONE,
                            &windows::core::HSTRING::from(tr("Chosen monitor (not connected, shown on main)")),
                        );
                    }
                    let _ = CheckMenuRadioItem(sub, ID_MONITOR_ALL as u32, ID_MONITOR_GONE as u32, checked as u32, MF_BYCOMMAND.0);
                    let n = GetMenuItemCount(Some(widget));
                    let _ = AppendMenuW(widget, MF_STRING | MF_POPUP, sub.0 as usize, &windows::core::HSTRING::from(tr("Show widget on")));
                    icons.set(widget, n as u32, true, ui::glyph::MONITOR);
                    let _ = AppendMenuW(widget, MF_STRING | check(mirroring), ID_MIRRORING, &windows::core::HSTRING::from(tr("Mirror other widgets")));
                }
                let _ = AppendMenuW(widget, MF_STRING | check(lock_order), ID_LOCK_ORDER, &windows::core::HSTRING::from(tr("Lock device positions")));
                let (charge_anim, ring, connection_icons) = with_app(|a| (a.charge_anim, a.ring, a.connection_icons)).unwrap_or_default();
                let _ = AppendMenuW(widget, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                let _ = AppendMenuW(widget, MF_STRING | check(ring), ID_RING, &windows::core::HSTRING::from(tr("Ring style (icon inside a ring filled to the level)")));
                let _ = AppendMenuW(widget, MF_STRING | check(charge_anim), ID_CHARGE_ANIM, &windows::core::HSTRING::from(tr("Charging animation")));
                let _ = AppendMenuW(widget, MF_STRING | check(connection_icons), ID_CONNECTION_ICONS, &windows::core::HSTRING::from(tr("Show connection icons")));
                let n = GetMenuItemCount(Some(menu));
                let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, widget.0 as usize, &windows::core::HSTRING::from(tr("Widget")));
                icons.set(menu, n as u32, true, ui::glyph::VIEW);
            }
            // Settings: reading, language, autostart and updates.
            if let Ok(settings_menu) = CreatePopupMenu() {
                ui::MenuIcons::check_or_icon(settings_menu);
                // How often the battery levels are read, and whether a device change triggers a read.
                if let Ok(sub) = CreatePopupMenu() {
                    let current = refresh_minutes();
                    for (j, (m, label)) in INTERVALS.iter().enumerate() {
                        let check = if *m == current { MF_CHECKED } else { MF_UNCHECKED };
                        let _ = AppendMenuW(sub, MF_STRING | check, ID_INTERVAL_BASE + j, &windows::core::HSTRING::from(tr(label)));
                    }
                    let n = GetMenuItemCount(Some(settings_menu));
                    let _ = AppendMenuW(settings_menu, MF_STRING | MF_POPUP, sub.0 as usize, &windows::core::HSTRING::from(tr("Refresh battery every")));
                    icons.set(settings_menu, n as u32, true, ui::glyph::HISTORY);
                }
                let on_change = if refresh_on_change() { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(settings_menu, MF_STRING | on_change, ID_ON_CHANGE, &windows::core::HSTRING::from(tr("Refresh when a device connects or disconnects")));
                let _ = AppendMenuW(settings_menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                // Language: Windows' language, every built-in and user language, and a new one.
                if let Ok(sub) = CreatePopupMenu() {
                    let chosen = language_choice();
                    // Names the language it resolves to: Windows' display language, not its region format.
                    let codes: Vec<String> = languages.iter().map(|(c, _)| c.clone()).collect();
                    let auto = batteryhub::i18n::pick(&batteryhub::i18n::windows_language(), &codes);
                    let auto_name = languages.iter().find(|(c, _)| c.eq_ignore_ascii_case(&auto)).map_or(auto.as_str(), |(_, n)| n.as_str());
                    let auto_label = format!("{} ({auto_name})", tr("Windows language"));
                    let _ = AppendMenuW(sub, MF_STRING | check(chosen.is_empty()), ID_LANG_AUTO, &windows::core::HSTRING::from(auto_label));
                    let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                    for (i, (code, name)) in languages.iter().enumerate().take(ID_LANG_LAST - ID_LANG_FIRST + 1) {
                        let on = chosen.eq_ignore_ascii_case(code);
                        let _ = AppendMenuW(sub, MF_STRING | check(on), ID_LANG_FIRST + i, &windows::core::HSTRING::from(name.as_str()));
                    }
                    let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                    let _ = AppendMenuW(sub, MF_STRING, ID_LANG_NEW, &windows::core::HSTRING::from(tr("Create your own language...")));
                    let _ = AppendMenuW(sub, MF_STRING, ID_LANG_FOLDER, &windows::core::HSTRING::from(tr("Open the languages folder")));
                    let n = GetMenuItemCount(Some(settings_menu));
                    let _ = AppendMenuW(settings_menu, MF_STRING | MF_POPUP, sub.0 as usize, &windows::core::HSTRING::from(tr("Language")));
                    icons.set(settings_menu, n as u32, true, ui::glyph::LANGUAGE);
                }
                let _ = AppendMenuW(settings_menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                let _ = AppendMenuW(
                    settings_menu,
                    MF_STRING | check(ui::autostart_enabled(autostart_name())),
                    ID_AUTOSTART,
                    &windows::core::HSTRING::from(tr("Automatically start with Windows")),
                );
                let _ = AppendMenuW(settings_menu, MF_STRING | check(auto_update()), ID_AUTO_UPDATE, &windows::core::HSTRING::from(tr("Automatically check for updates")));
                let _ = AppendMenuW(
                    settings_menu,
                    MF_STRING | check(silent_install()) | if auto_update() { MF_ENABLED } else { MF_GRAYED },
                    ID_SILENT_UPDATE,
                    &windows::core::HSTRING::from(tr("Install updates silently")),
                );
                let n = GetMenuItemCount(Some(menu));
                let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, settings_menu.0 as usize, &windows::core::HSTRING::from(tr("Settings")));
                icons.set(menu, n as u32, true, ui::glyph::SETTINGS);
            }
            // Help: updates, other versions, device requests, the project's pages and the latest
            // releases (their pages; loaded in the background).
            if let Ok(sub) = CreatePopupMenu() {
                ui::MenuIcons::check_or_icon(sub);
                let busy = UPDATE_BUSY.load(std::sync::atomic::Ordering::Relaxed);
                let label = if busy { tr("Checking for updates...") } else { tr("Check for updates...") };
                let _ = AppendMenuW(sub, MF_STRING | if busy { MF_GRAYED } else { MF_ENABLED }, ID_CHECK_UPDATES, &windows::core::HSTRING::from(label));
                icons.set(sub, ID_CHECK_UPDATES as u32, false, ui::glyph::DOWNLOAD);
                let _ = AppendMenuW(sub, MF_STRING, ID_VERSIONS, &windows::core::HSTRING::from(tr("Install another version...")));
                icons.set(sub, ID_VERSIONS as u32, false, ui::glyph::HISTORY);
                let _ = AppendMenuW(sub, MF_STRING, ID_REQUEST, &windows::core::HSTRING::from(tr("Request support for a device...")));
                icons.set(sub, ID_REQUEST as u32, false, ui::glyph::MAIL);
                let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                let _ = AppendMenuW(sub, MF_STRING, ID_HELP_GITHUB, &windows::core::HSTRING::from(tr("Go to the GitHub page")));
                icons.set(sub, ID_HELP_GITHUB as u32, false, ui::glyph::INFO);
                let _ = AppendMenuW(sub, MF_STRING, ID_HELP_DISCORD, &windows::core::HSTRING::from(tr("Join the Discord server")));
                let _ = AppendMenuW(sub, MF_STRING, ID_HELP_FREE, &windows::core::HSTRING::from(format!("{}  ♥", tr("BatteryHub is free and will stay free"))));
                let _ = AppendMenuW(sub, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
                let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, 0, &windows::core::HSTRING::from(tr("Recent updates")));
                let releases = recent_releases();
                if releases.is_empty() {
                    let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, 0, &windows::core::HSTRING::from(tr("Loading...")));
                }
                for (i, (version, _)) in releases.iter().enumerate().take(HELP_RELEASES) {
                    let label = if *version == batteryhub::install::VERSION { format!("v{version}  ✓") } else { format!("v{version}") };
                    let _ = AppendMenuW(sub, MF_STRING, ID_HELP_RELEASE_FIRST + i, &windows::core::HSTRING::from(label));
                }
                let n = GetMenuItemCount(Some(menu));
                let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, sub.0 as usize, &windows::core::HSTRING::from(tr("Help")));
                icons.set(menu, n as u32, true, ui::glyph::INFO);
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, &windows::core::HSTRING::from(tr("Exit")));
            icons.set(menu, ID_EXIT as u32, false, ui::glyph::EXIT);
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // With an auto-hiding taskbar the menu belongs to the strip under the cursor (a child of
            // the taskbar): the taskbar then stays the active window and stays shown while the menu
            // is open. Owned by our own window instead, the taskbar hid as soon as the mouse left it
            // for the menu, and took the menu with it.
            let mut owner = menu_owner(main, from);
            // The window to activate: the taskbar itself for the strip inside it (a child window can't
            // be the foreground window). With nothing active, StartAllBack's auto-hide hid the
            // taskbar as soon as the mouse left it for the menu and activated its own edge window,
            // which closed the menu.
            let root = GetAncestor(owner, GA_ROOT);
            let active = if root.0.is_null() { owner } else { root };
            let _ = SetForegroundWindow(active);
            if active != main && GetForegroundWindow() != active {
                // Not allowed to activate the taskbar: our own window then, so the menu stays open
                // even if the taskbar hides.
                owner = main;
                let _ = SetForegroundWindow(main);
            }
            dbg_log(&format!("menu: active {active:?} foreground {:?} owner {owner:?}", GetForegroundWindow()));
            // Menu colours follow the taskbar: dark on a dark taskbar, light on a light one.
            ui::set_menu_theme(owner, dark);
            // Drawn like Windows 11's own context menus (rows, icons, highlight, chevrons).
            let modern = ui::ModernMenu::apply(menu, &icons, owner, dark);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, owner, None);
            // Whatever the mouse crossed while the menu was open does not open the card.
            with_app(|a| a.hide_tip());
            drop(modern);
            let _ = PostMessageW(Some(main), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            drop(guard);
            // No MOUSEMOVE arrives if the pointer stayed still during the menu: re-arm the strip so
            // it reacts at once instead of looking switched off for a moment.
            with_app(|a| a.after_menu());
            match cmd.0 as usize {
                ID_EXIT => {
                    let _ = DestroyWindow(main);
                }
                ID_REFRESH_DEVICES => {
                    with_app(|a| a.manual_refresh(true));
                }
                ID_DASHBOARD => {
                    if let Some((first, _)) = with_app(|a| a.dashboard_devices().into_iter().next()).flatten() {
                        device_action(&first, settings::DeviceAction::Dashboard(0), &devices);
                    }
                }
                ID_REFRESH_LEVELS => {
                    with_app(|a| a.manual_refresh(false));
                }
                ID_CHECK_UPDATES => check_for_updates(main, false),
                ID_VERSIONS => open_version_list(main),
                ID_ICONS_MODE => {
                    let on = with_app(|a| {
                        a.icons_mode = !a.icons_mode;
                        save_dword("NotificationIcons", a.icons_mode as u32);
                        if a.icons_mode {
                            a.set_fallback(true);
                        } else {
                            a.set_fallback(false);
                            a.tick();
                        }
                        a.icons_mode
                    })
                    .unwrap_or(false);
                    if on {
                        message(
                            main,
                            &tr(
                                "BatteryHub now shows one notification-area icon per device, with its level drawn in the icon.\n\nWindows places these icons like any other: drag them to reorder, or move them in or out of the hidden-icons area. If you don't see them, open Settings > Personalization > Taskbar > Other system tray icons and turn BatteryHub on.",
                            ),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                ID_SIDE_RIGHT | ID_SIDE_LEFT | ID_SIDE_ICONS | ID_SIDE_BEFORE => {
                    let side = match cmd.0 as usize {
                        ID_SIDE_LEFT => SIDE_LEFT,
                        ID_SIDE_ICONS => SIDE_ICONS,
                        ID_SIDE_BEFORE => SIDE_BEFORE,
                        _ => SIDE_RIGHT,
                    };
                    with_app(|a| a.change_layout(target, true, |l| l.side = side));
                }
                ID_POSITION_MOVE => {
                    with_app(|a| a.begin_move(target));
                }
                ID_RING => {
                    with_app(|a| {
                        a.ring = !a.ring;
                        save_dword("WidgetStyle", a.ring as u32);
                        a.paint();
                        a.tick();
                    });
                }
                ID_CHARGE_ANIM => {
                    with_app(|a| {
                        a.charge_anim = !a.charge_anim;
                        save_dword("ChargingAnimation", a.charge_anim as u32);
                        a.paint();
                    });
                }
                ID_CONNECTION_ICONS => {
                    with_app(|a| {
                        a.connection_icons = !a.connection_icons;
                        save_dword("ConnectionIcons", a.connection_icons as u32);
                        // Same pop in both directions: appearing and disappearing.
                        a.badge_at = Some((Instant::now(), a.connection_icons));
                        a.paint();
                    });
                }
                ID_LOCK_ORDER => {
                    with_app(|a| {
                        a.lock_order = !a.lock_order;
                        save_dword("LockDeviceOrder", a.lock_order as u32);
                        settings::ORDER_LOCKED.store(a.lock_order, std::sync::atomic::Ordering::Relaxed);
                    });
                }
                ID_MIRRORING => {
                    let on = !mirroring;
                    message(
                        main,
                        &tr(if on { MIRROR_ON_TEXT } else { MIRROR_OFF_TEXT }),
                        MB_OK | MB_ICONINFORMATION,
                    );
                    with_app(|a| {
                        // Turning it off: every copy starts from the main widget's place and order.
                        if !on {
                            let lay = a.main_layout();
                            let bars: Vec<HWND> = a.mirrors.iter().chain(a.parked.iter()).map(|m| m.0).collect();
                            for bar in bars {
                                if load_layout(bar).is_none() {
                                    let key = widget_key(bar);
                                    save_layout(&key, &lay);
                                    settings::set_order(&key, &lay.order);
                                }
                            }
                        }
                        a.mirroring = on;
                        save_dword("MirrorWidgets", on as u32);
                        for v in a.views.values_mut() {
                            v.lay = None;
                            v.reordered = true;
                        }
                        a.last_place = Default::default();
                        a.paint();
                        a.sync_mirrors();
                    });
                }
                ID_SHOW_EMPTY => {
                    with_app(|a| {
                        a.hide_empty = !a.hide_empty;
                        save_dword("HideWhenEmpty", a.hide_empty as u32);
                        a.paint();
                        a.sync_app_icon();
                    });
                }
                ID_HELP_GITHUB => open_url(&format!("https://github.com/{}", batteryhub::update::REPO)),
                ID_HELP_DISCORD => open_url(DISCORD_URL),
                ID_HELP_FREE => open_url(SUPPORT_URL),
                id if (ID_HELP_RELEASE_FIRST..ID_HELP_RELEASE_FIRST + HELP_RELEASES).contains(&id) => {
                    if let Some((_, url)) = recent_releases().get(id - ID_HELP_RELEASE_FIRST) {
                        open_url(url);
                    }
                }
                ID_COMPACT => {
                    with_app(|a| {
                        a.compact = !a.compact;
                        save_dword("CompactWidget", a.compact as u32);
                        a.paint();
                        a.tick();
                        // The new layout pops in on every taskbar alike (the copies on other monitors
                        // too, which otherwise just switched).
                        let mut strips: Vec<HWND> = a.mirrors.iter().map(|m| m.1).collect();
                        strips.push(a.strip);
                        for st in strips {
                            if !st.0.is_null() && IsWindowVisible(st).as_bool() {
                                a.start_pop(st, false);
                            }
                        }
                    });
                }
                ID_BACKGROUND => {
                    with_app(|a| {
                        a.background = !a.background;
                        save_dword("WidgetBackground", a.background as u32);
                        a.paint();
                    });
                }
                id if (ID_POSITION_FIRST..ID_POSITION_FIRST + POSITION_OFFSETS.len()).contains(&id) => {
                    with_app(|a| a.change_layout(target, true, |l| l.offset = POSITION_OFFSETS[id - ID_POSITION_FIRST]));
                }
                ID_POSITION_LEFT => {
                    with_app(|a| a.change_layout(target, true, |l| l.offset = (offset + 10).min(POSITION_MAX)));
                }
                ID_POSITION_RIGHT => {
                    with_app(|a| a.change_layout(target, true, |l| l.offset = offset.saturating_sub(10)));
                }
                id if (ID_HEIGHT_FIRST..ID_HEIGHT_FIRST + HEIGHT_OFFSETS.len()).contains(&id) => {
                    with_app(|a| a.change_layout(target, true, |l| l.offset_y = HEIGHT_OFFSETS[id - ID_HEIGHT_FIRST]));
                }
                ID_HEIGHT_UP => {
                    with_app(|a| a.change_layout(target, true, |l| l.offset_y = (offset_y - 10).max(-HEIGHT_MAX)));
                }
                ID_HEIGHT_DOWN => {
                    with_app(|a| a.change_layout(target, true, |l| l.offset_y = (offset_y + 10).min(HEIGHT_MAX)));
                }
                id if id == ID_MONITOR_ALL || id == ID_MONITOR_MAIN || (ID_MONITOR_FIRST..ID_MONITOR_FIRST + others.len()).contains(&id) => {
                    let v = match id {
                        ID_MONITOR_ALL => "all".to_string(),
                        ID_MONITOR_MAIN => "main".to_string(),
                        i => others[i - ID_MONITOR_FIRST].0.clone(),
                    };
                    batteryhub::util::reg_set_string(SETTINGS_KEY, "WidgetMonitor", &v);
                    with_app(|a| {
                        a.choice = v;
                        a.sync_mirrors();
                    });
                }
                ID_AUTOSTART => {
                    let on = !ui::autostart_enabled(autostart_name());
                    ui::set_autostart(autostart_name(), on);
                    if on {
                        // A "disabled" state left in Task Manager would keep it from starting.
                        batteryhub::util::reg_delete_value(batteryhub::install::STARTUP_APPROVED_KEY, autostart_name());
                    }
                }
                ID_AUTO_UPDATE => {
                    let on = !auto_update();
                    save_dword("AutoCheckUpdates", on as u32);
                    if on {
                        message(
                            main,
                            &tr(
                                "Automatic update check is on.\n\nBatteryHub looks for a new version only when it starts (at sign-in or when you start it), not while it runs. When there is one, it asks you whether to update (or, with \"Install updates silently\", installs it in the background).",
                            ),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                ID_SILENT_UPDATE => {
                    let on = !silent_install();
                    save_dword("SilentUpdates", on as u32);
                    if on {
                        message(
                            main,
                            &tr(
                                "Silent updates are on.\n\nWhen BatteryHub starts and finds a new version, it downloads it from GitHub, checks its SHA-256 checksum and installs it in the background, without asking and without showing anything. Your settings and autostart are kept.",
                            ),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                ID_REQUEST => batteryhub::report_window::show(),
                ID_LANG_AUTO => set_language(""),
                id if (ID_LANG_FIRST..=ID_LANG_LAST).contains(&id) => {
                    if let Some((code, _)) = languages.get(id - ID_LANG_FIRST) {
                        set_language(code);
                    }
                }
                ID_LANG_NEW => match batteryhub::i18n::create_template() {
                    Ok(path) => {
                        let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
                        message(
                            main,
                            &trf(
                                "A new language file was created:\n{path}\n\nIt lists every text of BatteryHub. Write the translation after the equals sign on each line, set @code and @name at the top, save it, and then pick it in the Language menu. Texts you leave out stay in English.",
                                &[("path", &path.display().to_string())],
                            ),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                    Err(e) => {
                        message(main, &trf("Could not create the language file:\n{error}", &[("error", &e.to_string())]), MB_OK | MB_ICONWARNING);
                    }
                },
                ID_LANG_FOLDER => {
                    let dir = batteryhub::i18n::user_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
                }
                ID_ON_CHANGE => {
                    let on = !refresh_on_change();
                    save_dword("RefreshOnDeviceChange", on as u32);
                    ON_CHANGE.store(on as u8, std::sync::atomic::Ordering::Relaxed);
                }
                id if (ID_INTERVAL_BASE..ID_INTERVAL_BASE + INTERVALS.len()).contains(&id) => {
                    let minutes = INTERVALS[id - ID_INTERVAL_BASE].0;
                    save_dword("RefreshMinutes", minutes);
                    SetTimer(Some(main), TIMER_POLL, minutes * 60_000, None);
                }
                id if id >= settings::MERGE_ID_BASE => {
                    if let Some((dev, target)) = settings::decode_merge_id(id)
                        && let (Some(a), Some(b)) = (devices.get(dev), devices.get(target))
                    {
                        // The menu hides Join on joined devices; a stale command must not rejoin
                        // them either. Only Separate is offered until they are apart again.
                        let merged = settings::merged_targets(SETTINGS_KEY);
                        if !settings::is_merged(&merged, &a.key) && !settings::is_merged(&merged, &b.key) {
                            join_devices(a, b);
                        }
                    }
                }
                id => {
                    if let Some((dev, action)) = settings::decode_menu_id(id)
                        && let Some(d) = devices.get(dev)
                    {
                        device_action(d, action, &devices);
                    }
                }
            }
        }
    }
}


/// Progress of a glide that started at `t0`, eased out (0 to 1).
fn glide_ease(t0: Instant) -> f32 {
    let p = (t0.elapsed().as_secs_f32() * 1000.0 / GLIDE_MS).min(1.0);
    1.0 - (1.0 - p).powi(3)
}


/// "Reset measurements": asks first, then deletes the device's history.
const SHORT_ROOM_TEXT: &str = n_(
    "The widget is hidden on that taskbar until there is room again. You can also move it: right-click it → Widget position."
);
const MIRROR_ON_TEXT: &str = n_("Mirror other widgets is on.\n\nThe widgets on all your taskbars now look and behave the same: moving the widget, changing its position or height, or dragging a device to another place on one taskbar changes every taskbar alike. Each widget is still drawn for its own monitor (its own scaling and resolution).");
const MIRROR_OFF_TEXT: &str = n_("Mirror other widgets is off.\n\nEach taskbar's widget is now on its own: right-click the widget on a monitor to change its position and height there, and drag its devices into an order of its own. What you set on one taskbar no longer changes the others. Each widget starts where it is now.");

const BATTERY_ONLY_QUESTION: &str = n_(
    "Use Battery only for {name}?\n\nIts battery history and statistics are deleted, and from now on nothing is recorded or worked out for it: only its level is shown. This cannot be undone."
);

fn confirm_reset(main: HWND, d: &DeviceStatus) {
    let text = batteryhub::dashboard::reset_question(&d.name);
    let answer = unsafe {
        MessageBoxW(
            Some(main),
            &windows::core::HSTRING::from(text),
            &windows::core::HSTRING::from(tr("Reset measurements")),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
        )
    };
    if answer == IDYES {
        with_app(|a| a.reset_history(&d.key));
    }
}

/// An item of a device's submenu was chosen.
/// Drops a device's custom icon and redraws every widget (#476). Also used when the dashboard's
/// "Use the built-in icon" button asks for it over WM_COPYDATA.
fn reset_icon(key: &str) {
    if let Some(old) = settings::icon_file(SETTINGS_KEY, key) {
        batteryhub::custom_icon::forget(&old);
    }
    settings::clear_icon_file(SETTINGS_KEY, key);
    with_app(|a| {
        a.card_cache.clear();
        a.paint();
    });
}

fn device_action(d: &DeviceStatus, action: settings::DeviceAction, devices: &[DeviceStatus]) {
    use settings::DeviceAction;
    let reported = with_app(|a| a.devices.get(&d.key).map(|t| t.reported.clone())).flatten().unwrap_or_else(|| d.name.clone());
    match action {
        DeviceAction::Dashboard(tab) => {
            // The dashboard reads the history file: write the latest readings first.
            with_app(|a| a.save_history_now());
            batteryhub::dashboard::open(d, &reported, tab);
        }
        DeviceAction::Rename => batteryhub::rename::open(d, &reported),
        DeviceAction::ResetName => {
            settings::set_name(SETTINGS_KEY, &d.key, None);
            with_app(|a| a.reload_names());
        }
        DeviceAction::ToggleShown => {
            with_app(|a| {
                let hide = !a.hidden.contains(&d.key);
                settings::set_hidden(SETTINGS_KEY, &d.key, hide);
                a.hidden = settings::hidden(SETTINGS_KEY);
                a.hide_tip();
                a.card_cache.clear();
                a.paint();
            });
        }
        DeviceAction::Hover(m) => {
            settings::set_hover_mode(SETTINGS_KEY, &d.key, m);
            with_app(|a| a.hide_tip());
        }
        DeviceAction::ToggleCardOnClick => {
            let on = !settings::card_on_click(SETTINGS_KEY, &d.key);
            settings::set_card_on_click(SETTINGS_KEY, &d.key, on);
            dbg_log(&format!("menu: {} card_on_click={on}", d.key));
            with_app(|a| a.hide_tip());
        }
        DeviceAction::CardDismiss(mode) => {
            settings::set_card_dismiss(SETTINGS_KEY, &d.key, mode);
            dbg_log(&format!("menu: {} card_dismiss={mode:?}", d.key));
            with_app(|a| a.hide_tip());
        }
        DeviceAction::ToggleCardChart => {
            settings::set_card_chart(SETTINGS_KEY, &d.key, !settings::card_chart(SETTINGS_KEY, &d.key));
            with_app(|a| a.card_cache.clear());
        }
        DeviceAction::ToggleCardTime => {
            settings::set_card_time(SETTINGS_KEY, &d.key, !settings::card_time(SETTINGS_KEY, &d.key));
            with_app(|a| a.card_cache.clear());
        }
        DeviceAction::ToggleBatteryOnly => {
            let on = !settings::battery_only(SETTINGS_KEY, &d.key);
            // Turning it on deletes what was measured: asked first.
            if on {
                let Some(main) = with_app(|a| a.main) else { return };
                let answer = unsafe {
                    MessageBoxW(
                        Some(main),
                        &windows::core::HSTRING::from(trf(BATTERY_ONLY_QUESTION, &[("name", &d.name)])),
                        &windows::core::HSTRING::from(tr("Battery only")),
                        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
                    )
                };
                if answer != IDYES {
                    return;
                }
            }
            settings::set_battery_only(SETTINGS_KEY, &d.key, on);
            with_app(|a| {
                if on {
                    a.reset_history(&d.key);
                }
                a.card_cache.clear();
            });
        }
        DeviceAction::ToggleGlow => {
            settings::set_glow(SETTINGS_KEY, &d.key, !settings::glow(SETTINGS_KEY, &d.key));
            with_app(|a| a.paint());
        }
        DeviceAction::ToggleRiseCharging => {
            settings::set_rise_charging(SETTINGS_KEY, &d.key, !settings::rise_charging(SETTINGS_KEY, &d.key));
            with_app(|a| {
                a.rising.remove(&d.key);
                a.card_cache.clear();
            });
        }
        DeviceAction::ToggleWarn5 => {
            settings::set_warn_critical(SETTINGS_KEY, &d.key, !settings::warn_critical(SETTINGS_KEY, &d.key));
        }
        DeviceAction::IconColor(c) => {
            settings::set_icon_color(SETTINGS_KEY, &d.key, c);
            with_app(|a| {
                a.card_cache.clear();
                a.paint();
            });
        }
        DeviceAction::CustomColor => {
            let Some(main) = with_app(|a| a.main) else { return };
            let current = settings::icon_rgb(settings::icon_color(SETTINGS_KEY, &d.key), d.vid);
            if let Some((r, g, b)) = unsafe { choose_color(main, current) } {
                settings::set_icon_color(SETTINGS_KEY, &d.key, settings::IconColor::Rgb(r, g, b));
                with_app(|a| {
                    a.card_cache.clear();
                    a.paint();
                });
            }
        }
        DeviceAction::ToggleRedIcon => {
            settings::set_red_icon(SETTINGS_KEY, &d.key, !settings::red_icon(SETTINGS_KEY, &d.key));
            with_app(|a| {
                a.card_cache.clear();
                a.paint();
            });
        }
        DeviceAction::Threshold(level) => {
            settings::set_threshold(SETTINGS_KEY, &d.key, level);
            with_app(|a| {
                a.card_cache.clear();
                a.paint();
            });
        }
        DeviceAction::Move(on) => {
            let Some(i) = devices.iter().position(|x| x.key == d.key) else { return };
            let j = if on { i + 1 } else { i.wrapping_sub(1) };
            if let Some(other) = devices.get(j) {
                with_app(|a| a.swap_devices(&d.key, &other.key));
            }
        }
        DeviceAction::Unmerge => separate_device(&d.key),
        DeviceAction::ToggleSameNamePrompt => {
            settings::set_same_name_prompt(SETTINGS_KEY, !settings::same_name_prompt(SETTINGS_KEY));
            with_app(|a| a.paint());
        }
        DeviceAction::CustomIcon => {
            let Some(main) = with_app(|a| a.main) else { return };
            let Some(path) = (unsafe { choose_icon(main) }) else { return };
            batteryhub::custom_icon::forget(&path); // the file may have changed since it failed
            if batteryhub::custom_icon::load(&path, 64).is_none() {
                message(main, &tr("The file could not be read as an image."), MB_ICONWARNING);
                return;
            }
            if let Some(old) = settings::icon_file(SETTINGS_KEY, &d.key) {
                batteryhub::custom_icon::forget(&old);
            }
            settings::set_icon_file(SETTINGS_KEY, &d.key, &path);
            with_app(|a| {
                a.card_cache.clear();
                a.paint();
            });
        }
        DeviceAction::ResetIcon => reset_icon(&d.key),
        DeviceAction::ResetHistory => {
            if let Some(main) = with_app(|a| a.main) {
                confirm_reset(main, d);
            }
        }
    }
}

/// Joins two connections of one physical device into one logical device (#644). The second one
/// keeps its key, history and settings; the first one's readings and settings are moved under it.
fn join_devices(from: &DeviceStatus, to: &DeviceStatus) {
    let Some(main) = with_app(|a| a.main) else { return };
    let question = trf(
        "Join \"{a}\" and \"{b}\" as one device?\n\nTheir battery history and settings are combined under \"{b}\". Both connections then show one device. They can be separated again from the device menu, but separating deletes their battery history.",
        &[("a", &from.name), ("b", &to.name)],
    );
    let answer = unsafe {
        MessageBoxW(
            Some(main),
            &windows::core::HSTRING::from(question),
            &windows::core::HSTRING::from(tr("Join with another connection")),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
        )
    };
    if answer == IDYES {
        perform_join(from, to);
    }
}

/// The one-time offer found by a poll. It is marked as made before the question, so a "No", a
/// closed dialog or a crash afterwards cannot bring it back. `join_devices` then asks the same
/// question a user gets from the menu, without a second confirmation.
fn on_same_name_prompt() {
    let Some((a, b)) = with_app(|app| app.duplicate_prompt.take()).flatten() else {
        return;
    };
    settings::mark_duplicate_prompt_asked(SETTINGS_KEY, &a, &b);
    let Some(statuses) = with_app(|app| {
        (
            app.devices.get(&a).map(|t| t.status.clone()),
            app.devices.get(&b).map(|t| t.status.clone()),
        )
    }) else {
        return;
    };
    if let (Some(from), Some(to)) = statuses {
        join_devices(&from, &to);
    }
}

fn perform_join(from: &DeviceStatus, to: &DeviceStatus) {
    with_app(|a| {
        a.save_history_now();
        a.history.merge_series(&from.key, &to.key, batteryhub::history::gap_limit(refresh_minutes()));
        a.history.mark_dirty();
        a.save_history_now();
        batteryhub::dashboard::notify_history_changed();
    });
    settings::move_device_settings(SETTINGS_KEY, &from.key, &to.key);
    settings::set_merged(SETTINGS_KEY, &from.key, &to.key);
    settings::clear_separated(SETTINGS_KEY, &from.key);
    settings::clear_separated(SETTINGS_KEY, &to.key);
    reload_after_merge();
}

/// Undoes a join for future readings (#644). The menu entry is on the kept device, so clear every
/// connection joined into it. Both connections get an invisible "already separated" mark, and all
/// their measurements are dropped: a chart spanning the joined and separated readings is nonsense.
fn separate_device(key: &str) {
    let map = settings::merged_targets(SETTINGS_KEY);
    let mut keys = settings::merged_members(&map, key);
    if keys.is_empty() {
        return; // the hidden mark already stopped a repeated separation
    }
    if !keys.iter().any(|k| k == key) {
        keys.push(key.to_string());
    }
    for member in &keys {
        settings::clear_merged(SETTINGS_KEY, member);
    }
    settings::mark_separated(SETTINGS_KEY, &keys);
    with_app(|a| {
        for k in &keys {
            a.reset_history(k);
        }
    });
    reload_after_merge();
    // The joined connection was removed from the strip while merged. Read again right away so both
    // devices appear at once instead of waiting for the next interval.
    with_app(|a| a.manual_refresh(true));
}

fn reload_after_merge() {
    with_app(|a| {
        a.merged = settings::merged_targets(SETTINGS_KEY);
        a.names = settings::names(SETTINGS_KEY);
        a.hidden = settings::hidden(SETTINGS_KEY);
        a.saved_order = settings::order(SETTINGS_KEY);
        a.card_cache.clear();
        a.paint();
        a.send_to_dashboards();
    });
}

// ---- window procedures -------------------------------------------------------------------------

/// Windows of the taskbar itself (Windows 10 / 11 / ExplorerPatcher), never treated as widgets.
const TASKBAR_PARTS: [&str; 16] = [
    "Start",
    "TrayNotifyWnd",
    "ReBarWindow32",
    "MSTaskSwWClass",
    "MSTaskListWClass",
    "TrayDummySearchControl",
    "Windows.UI.Composition.DesktopWindowContentBridge",
    "Windows.UI.Core.CoreWindow",
    "Windows.UI.Input.InputSite.WindowClass",
    "WorkerW",
    "ClockButton",
    "TrayClockWClass",
    "TrayButton",
    "TrayShowDesktopButtonWClass",
    "SIBTrayButton",
    "BatteryHub.Taskbar.Strip",
];

/// x ranges (client coordinates of `bar`) of other programs' windows embedded in the taskbar,
/// such as network or CPU meters: direct child windows that are not part of the taskbar.
/// Spans in `bar`'s client x as screen x.
fn screen_spans(bar: HWND, spans: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut p = POINT::default();
    unsafe {
        let _ = ClientToScreen(bar, &mut p);
    }
    spans.iter().map(|&(a, b)| (a + p.x, b + p.x)).collect()
}

fn foreign_widgets(bar: HWND) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    unsafe {
        let mut br = RECT::default();
        let _ = GetClientRect(bar, &mut br);
        let mut after: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(Some(bar), after, None, None) {
            if h.0.is_null() {
                break;
            }
            after = Some(h);
            if !IsWindowVisible(h).as_bool() {
                continue;
            }
            let mut cls = [0u16; 128];
            let n = GetClassNameW(h, &mut cls);
            let name = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            if TASKBAR_PARTS.contains(&name.as_str()) {
                continue;
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            let mut tl = POINT { x: r.left, y: r.top };
            let _ = ScreenToClient(bar, &mut tl);
            let w = r.right - r.left;
            // Skip empty and taskbar-wide windows (hosts, not widgets).
            if w <= 0 || r.bottom <= r.top || w * 2 > br.right - br.left {
                continue;
            }
            out.push((tl.x, tl.x + w));
        }
    }
    out
}

/// Moves a strip of width `w` that would start at `left` further left until it overlaps none
/// of the other widgets, keeping `gap` px between them.
fn avoid_widgets(bar: HWND, left: i32, w: i32, gap: i32) -> i32 {
    let others = foreign_widgets(bar);
    shift_left_of(left, w, gap, &others)
}

/// Placed pieces (x, y, shown), total height, device rows (from, to, device).
type ColumnLayout = (Vec<(i32, i32, bool)>, i32, Vec<(i32, i32, usize)>);

/// Lays out strip pieces (device, width, height) as a column `w` wide: per device its first
/// piece (the icon) on one line and the rest (level, bolt) centred on the next; a piece that does
/// not fit beside the level is left out. Pieces of no device get a line of their own. Returns
/// (x, y, shown) per piece, the total height, and the y range of each device (from, to, device).
fn column_layout(pieces: &[(Option<usize>, i32, i32)], w: i32, pad: i32, line_gap: i32, dev_gap: i32, piece_gap: i32) -> ColumnLayout {
    let mut out = vec![(0, 0, false); pieces.len()];
    let mut hits: Vec<(i32, i32, usize)> = Vec::new();
    let mut y = pad;
    let mut i = 0;
    while i < pieces.len() {
        let dev = pieces[i].0;
        let mut j = i + 1;
        while dev.is_some() && j < pieces.len() && pieces[j].0 == dev {
            j += 1;
        }
        let start = y;
        // First line: the icon.
        let (_, iw, ih) = pieces[i];
        out[i] = ((w - iw) / 2, y, true);
        y += ih;
        // Second line: the level and what fits beside it.
        if j > i + 1 {
            y += line_gap;
            let mut line_w = 0;
            let mut last = i + 1;
            #[allow(clippy::needless_range_loop)]
            for k in i + 1..j {
                let add = pieces[k].1 + if k > i + 1 { piece_gap } else { 0 };
                if k > i + 1 && line_w + add > w - 2 {
                    break;
                }
                line_w += add;
                last = k;
            }
            let line_h = (i + 1..=last).map(|k| pieces[k].2).max().unwrap_or(0);
            let mut x = (w - line_w) / 2;
            for k in i + 1..=last {
                out[k] = (x, y + (line_h - pieces[k].2) / 2, true);
                x += pieces[k].1 + piece_gap;
            }
            y += line_h;
        }
        if let Some(d) = dev {
            hits.push((start - dev_gap / 2, y + dev_gap / 2, d));
        }
        i = j;
        if i < pieces.len() {
            y += dev_gap;
        }
    }
    let h = y + pad;
    if let Some(first) = hits.first_mut() {
        first.0 = 0;
    }
    if let Some(last) = hits.last_mut() {
        last.1 = h;
    }
    (out, h, hits)
}

fn shift_left_of(mut left: i32, w: i32, gap: i32, others: &[(i32, i32)]) -> i32 {
    for _ in 0..16 {
        match others.iter().filter(|(a, b)| *a < left + w && *b > left).map(|(a, _)| *a).min() {
            Some(a) => left = a - gap - w,
            None => break,
        }
    }
    left
}

/// The hover card's animation: what it shows, where it ends up, and how far it slides.
#[derive(Default)]
struct CardAnim {
    /// Start of the running fade, None when it is at rest.
    at: Option<Instant>,
    /// Fading in (true) or out.
    showing: bool,
    /// On screen (or fading out).
    visible: bool,
    x: i32,
    y: i32,
    slide: i32,
    content: Option<Rc<CardContent>>,
    /// While switching devices: where the background started (x, width) and the previous card's
    /// content, fading out as the background glides to the new card.
    prev: Option<(i32, i32, Rc<CardContent>)>,
    /// Background drawn in the last frame (screen x, width): the next switch starts there.
    drawn: (i32, i32),
    /// Vertical taskbar: the card sits beside it, switches glide up / down (from `prev_y`, the
    /// screen y drawn when the switch started) and it slides in sideways.
    vertical: bool,
    prev_y: i32,
    drawn_y: i32,
    /// Height drawn when the switch started, and in the last frame: cards of different height
    /// (earbuds have two charts) glide from one height to the other.
    prev_h: i32,
    drawn_h: i32,
    bg: COLORREF,
    border: COLORREF,
    radius: i32,
    /// DPI scale of the monitor the card is on.
    scale: f32,
}

/// The card's bitmap and memory DC, kept while the card is open.
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Surface {
    fn new(w: i32, h: i32) -> Option<Self> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bmp) if !bits.is_null() => {
                    let old = SelectObject(dc, HGDIOBJ(bmp.0));
                    Some(Surface { dc, bmp, old, bits: bits.cast(), w, h })
                }
                _ => {
                    let _ = DeleteDC(dc);
                    None
                }
            }
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
    }

    /// Shows the top-left `size` of the bitmap at `pt` (screen coordinates) with opacity `alpha`.
    fn present(&self, hwnd: HWND, pt: POINT, size: SIZE, alpha: u8) {
        unsafe {
            let _ = GdiFlush();
            let src = POINT::default();
            let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(hwnd, None, Some(&pt), Some(&size), Some(self.dc), Some(&src), COLORREF(0), Some(&bf), ULW_ALPHA);
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

const CARD_CLASS: windows::core::PCWSTR = w!("BatteryHub.Taskbar.Card");

/// The hover card: a layered, topmost popup owned by the main window. It takes the mouse (to pick
/// an hour on its chart) but is never activated.
fn create_card(owner: HWND) -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            CARD_CLASS,
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            Some(owner),
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default()
    }
}

extern "system" fn card_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                with_app(|a| a.card_mouse());
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_menu(None);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// WH_MOUSE_LL runs on the thread that installed it, from its message loop, so it can safely use
/// `with_app`. It is installed only while a card is waiting for an outside click.
extern "system" fn click_hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == HC_ACTION as i32 && (wp.0 as u32 == WM_LBUTTONDOWN || wp.0 as u32 == WM_RBUTTONDOWN) {
            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_ok() {
                with_app(|a| a.outside_click(pt));
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

extern "system" fn strip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // The context menu is owned by the strip with an auto-hiding taskbar (menu_owner).
            WM_MEASUREITEM if ui::measure_menu_item(lp) => LRESULT(1),
            WM_DRAWITEM if ui::draw_menu_item(lp) => LRESULT(1),
            WM_MOUSEMOVE => {
                with_app(|a| {
                    if a.moving.is_some() {
                        a.move_step();
                    } else {
                        a.hover(hwnd, (lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                    }
                });
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                // Re-ordering or redrawing the strip can report a leave while the mouse is still
                // on it: then keep the card and track again, otherwise close it.
                let mut pt = POINT::default();
                let mut rc = RECT::default();
                let still_over = GetCursorPos(&mut pt).is_ok() && GetWindowRect(hwnd, &mut rc).is_ok() && PtInRect(&rc, pt).as_bool();
                if still_over {
                    with_app(|a| {
                        a.tracking = false;
                        let mut c = pt;
                        let _ = ScreenToClient(hwnd, &mut c);
                        a.tip_for = None;
                        a.hover(hwnd, c.x, c.y);
                    });
                } else {
                    with_app(|a| a.leave_strip());
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                // Right-click while moving: back to where it was.
                if with_app(|a| a.moving.is_some()).unwrap_or(false) {
                    with_app(|a| a.end_move(false));
                    return LRESULT(0);
                }
                with_app(|a| a.end_drag());
                show_menu(Some(hwnd));
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                with_app(|a| {
                    if a.moving.is_some() {
                        a.end_move(true);
                        a.drag_moved = true; // placing the widget is not a click on a device
                    } else {
                        a.start_drag(hwnd, (lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                with_app(|a| {
                    let moved = a.drag_moved;
                    a.end_drag();
                    if !moved {
                        a.click_device(hwnd, x, y);
                    }
                });
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                with_app(|a| {
                    a.drag = None;
                    a.drag_moved = true;
                    a.end_move(true);
                });
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_POLL_DONE => {
                let devices = ui::take_result(lp);
                with_app(|a| a.on_result(devices));
                // Hand pages used only during the read (enumeration, Bluetooth, fonts) back to
                // Windows until the next read, instead of keeping them resident for a minute or more.
                trim_memory();
                LRESULT(0)
            }
            WM_SAME_NAME_PROMPT => {
                on_same_name_prompt();
                LRESULT(0)
            }
            WM_TIMER if wp.0 == TIMER_CARD => {
                // Animation frames: no memory trim here (it runs ~60 times a second for 0.15 s).
                with_app(|a| a.card_step());
                LRESULT(0)
            }
            WM_TIMER => {
                with_app(|a| match wp.0 {
                    TIMER_POLL => a.start_poll(),
                    TIMER_DEVCHANGE => {
                        let _ = KillTimer(Some(hwnd), TIMER_DEVCHANGE);
                        a.devchange = true;
                        a.start_poll();
                    }
                    TIMER_QUICK => a.start_quick(),
                    TIMER_NOTIFY => a.end_notify(),
                    TIMER_AWAY => a.away_step(),
                    TIMER_SLIDE | TIMER_CHARGE | TIMER_RING_CHARGE | TIMER_BADGE => a.paint(),
                    TIMER_MIRROR_IN => a.mirror_in_step(),
                    TIMER_GLIDE => a.glide_step(),
                    TIMER_LEFT_PROBE => {
                        a.left_probes = a.left_probes.saturating_sub(1);
                        if a.left_probes == 0 {
                            let _ = KillTimer(Some(hwnd), TIMER_LEFT_PROBE);
                        }
                        batteryhub::tray_probe::left_refresh_soon();
                        a.place();
                        for i in 0..a.mirrors.len() {
                            a.place_mirror_at(i);
                        }
                    }
                    TIMER_UPDATED => {
                        let _ = KillTimer(Some(hwnd), TIMER_UPDATED);
                        if let Some(old) = a.updated_from.take() {
                            let text = trf(
                                "Update finished.\n\nBatteryHub was updated:  {old}  ->  {new}\n\nYour settings and autostart were kept.",
                                &[("old", &batteryhub::install::label_of(&old)), ("new", &batteryhub::install::version_label())],
                            );
                            post_from_thread(hwnd, WM_SHOW_UPDATED, move || text);
                        }
                    }
                    TIMER_AUTO_UPDATE => {
                        let _ = KillTimer(Some(hwnd), TIMER_AUTO_UPDATE);
                        check_for_updates(hwnd, true);
                    }
                    TIMER_BTWATCH => {
                        // Bluetooth connect / disconnect does not add or remove device nodes, so
                        // Windows sends no device-change message for it: compare a signature.
                        if refresh_on_change() {
                            let sig = batteryhub::drivers::bluetooth::connection_signature();
                            if sig != a.bt_sig {
                                a.bt_sig = sig;
                                SetTimer(Some(hwnd), TIMER_DEVCHANGE, 1500, None);
                            }
                            trim_memory();
                        }
                    }
                    _ => a.tick(),
                });
                // Every tick touches data pages of user32 / GDI / UI Automation, which count as the
                // process's own memory: hand them back right away, so Task Manager shows ~0.2 MB
                // nearly all the time instead of ~0.75 MB between the slower trims.
                trim_memory();
                LRESULT(0)
            }
            // A device was plugged in / removed (USB receiver, cable, Bluetooth link) or the PC woke
            // up: read again once things settle, if the user left that option on.
            // A device was plugged in or removed (HID interface arrival / removal, or any device
            // node change): read again once things settle, if the user left that option on.
            WM_DEVICECHANGE_MSG if matches!(wp.0, DBT_DEVNODES_CHANGED | DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE) => {
                if refresh_on_change() {
                    SetTimer(Some(hwnd), TIMER_DEVCHANGE, 1500, None);
                }
                LRESULT(1)
            }
            WM_QUICK_DONE => {
                let devices = ui::take_result(lp);
                let ids = QUICK_IDS.with(|q| std::mem::take(&mut *q.borrow_mut()));
                with_app(|a| a.on_quick_result(devices, &ids));
                trim_memory();
                LRESULT(0)
            }
            WM_FOREGROUND => {
                with_app(|a| {
                    if a.strip_popup {
                        a.place();
                    }
                    if !a.mirrors.is_empty() {
                        a.sync_mirrors();
                    }
                });
                LRESULT(0)
            }
            WM_TRAY_PROBED => {
                with_app(|a| {
                    if a.align_left {
                        a.place();
                    }
                    a.sync_mirrors();
                });
                trim_memory();
                LRESULT(0)
            }
            WM_SHOW_UPDATED => {
                // Shown outside the timer handler, so the app is not borrowed while the box is open.
                let text = *Box::from_raw(lp.0 as *mut String);
                message(hwnd, &text, MB_OK | MB_ICONINFORMATION);
                LRESULT(0)
            }
            WM_UPDATE_CHECKED => {
                let check = *Box::from_raw(lp.0 as *mut batteryhub::update::Check);
                on_update_checked(hwnd, check);
                LRESULT(0)
            }
            WM_UPDATE_DOWNLOADED => {
                let result = *Box::from_raw(lp.0 as *mut Result<batteryhub::update::Verified, String>);
                on_update_downloaded(hwnd, result);
                LRESULT(0)
            }
            WM_TRAY if wp.0 as u32 == NOTIFY_UID => {
                // The low-battery notification closed or was clicked: its icon goes.
                const NIN_BALLOONHIDE: u32 = 0x403;
                const NIN_BALLOONTIMEOUT: u32 = 0x404;
                const NIN_BALLOONUSERCLICK: u32 = 0x405;
                if matches!((lp.0 & 0xFFFF) as u32, NIN_BALLOONHIDE | NIN_BALLOONTIMEOUT | NIN_BALLOONUSERCLICK) {
                    with_app(|a| a.end_notify());
                }
                LRESULT(0)
            }
            WM_TRAY => {
                if matches!((lp.0 & 0xFFFF) as u32, WM_RBUTTONUP | WM_CONTEXTMENU) {
                    show_menu(None);
                }
                LRESULT(0)
            }
            WM_MEASUREITEM if ui::measure_menu_item(lp) => LRESULT(1),
            WM_DRAWITEM if ui::draw_menu_item(lp) => LRESULT(1),
            WM_COPYDATA => {
                // From a dashboard: delete a device's measurements (it asked the user already).
                let cds = &*(lp.0 as *const windows::Win32::System::DataExchange::COPYDATASTRUCT);
                if cds.dwData == batteryhub::dashboard::COPY_RESET && !cds.lpData.is_null() {
                    let key = String::from_utf8_lossy(std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize)).to_string();
                    with_app(|a| a.reset_history(&key));
                } else if cds.dwData == batteryhub::dashboard::COPY_DELETE && !cds.lpData.is_null() {
                    let key = String::from_utf8_lossy(std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize)).to_string();
                    with_app(|a| a.delete_device(&key));
                } else if cds.dwData == batteryhub::dashboard::COPY_RESET_ICON && !cds.lpData.is_null() {
                    let key = String::from_utf8_lossy(std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize)).to_string();
                    reset_icon(&key);
                } else if cds.dwData == batteryhub::dashboard::COPY_HELLO {
                    // A dashboard just opened: it gets every device right away (its device bar).
                    with_app(|a| a.send_to_dashboards());
                }
                LRESULT(1)
            }
            WM_HISTORY_SAVED => {
                with_app(|a| {
                    if wp.0 == 0 {
                        a.history.mark_dirty(); // try again at the next read
                    }
                });
                // An open dashboard reads the new file.
                batteryhub::dashboard::notify_history_changed();
                LRESULT(0)
            }
            // Sign-out, restart or shutdown: write the history before the process is ended.
            WM_ENDSESSION if wp.0 != 0 => {
                with_app(|a| a.save_history_now());
                LRESULT(0)
            }
            WM_POWERBROADCAST if wp.0 == PBT_APMSUSPEND => {
                with_app(|a| a.save_history_now());
                LRESULT(1)
            }
            WM_POWERBROADCAST if wp.0 == PBT_APMRESUMEAUTOMATIC => {
                if refresh_on_change() {
                    SetTimer(Some(hwnd), TIMER_DEVCHANGE, 5000, None);
                }
                LRESULT(1)
            }
            WM_TIMECHANGE => {
                batteryhub::dashboard::time_zone_changed();
                with_app(|a| a.card_cache.clear());
                LRESULT(0)
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
                with_app(|a| {
                    a.refresh_settings(); // light/dark taskbar
                    a.paint(); // or DPI changed
                });
                LRESULT(0)
            }
            WM_DESTROY => {
                with_app(|a| {
                    a.save_history_now();
                    a.end_notify();
                    if !a.strip.0.is_null() {
                        let _ = DestroyWindow(a.strip);
                    }
                    a.remove_tray();
                });
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => {
                let (tc, nm, sh) = with_app(|a| (a.taskbar_created, a.names_msg, a.shell_msg)).unwrap_or((0, 0, 0));
                if sh != 0 && msg == sh {
                    // A window opened or closed: its task button comes or goes, so with the widget
                    // next to the taskbar icons, look again soon (the button animates in).
                    let code = (wp.0 & 0x7FFF) as u32;
                    if code == HSHELL_WINDOWCREATED || code == HSHELL_WINDOWDESTROYED {
                        with_app(|a| {
                            if !a.vertical && !a.fallback {
                                a.left_probes = LEFT_PROBES;
                                SetTimer(Some(hwnd), TIMER_LEFT_PROBE, 200, None);
                            }
                        });
                    }
                    return LRESULT(0);
                }
                if nm != 0 && msg == nm {
                    with_app(|a| a.reload_names());
                    return LRESULT(0);
                }
                if tc != 0 && msg == tc {
                    // Explorer restarted. The 1 s timer may already have put a new strip on the new
                    // taskbar, so the strip is not thrown away blindly (that left a second, orphaned
                    // strip behind): tick() keeps a live one and recreates a dead one. Fallback
                    // icons have to be added again.
                    with_app(|a| {
                        a.last_place = Default::default();
                        a.painted_for = Default::default();
                        a.tray.clear();
                        a.tick();
                        if a.fallback {
                            a.sync_tray();
                        }
                    });
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

fn main() {
    unsafe {
        batteryhub::i18n::set_language(&language_choice());
        // `--report`: only the "Request support for a device" window (also while BatteryHub runs).
        if std::env::args().any(|a| a == "--report") {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            batteryhub::report_window::run_standalone();
            return;
        }
        // `--listen VID:PID`: diagnostic recording of a device's HID reports (see listen.rs).
        let args: Vec<String> = std::env::args().collect();
        // `--dashboard <tab> <device>` / `--rename <device>`: windows opened from the device menu,
        // each in its own short-lived process.
        if let Some(i) = args.iter().position(|a| a == "--dashboard") {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            batteryhub::dashboard::run_standalone(&args[i + 1..]);
            return;
        }
        if let Some(i) = args.iter().position(|a| a == "--rename") {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            batteryhub::rename::run_standalone(&args[i + 1..]);
            return;
        }
        if let Some(i) = args.iter().position(|a| a == "--listen") {
            batteryhub::listen::run(args.get(i + 1).map(|s| s.as_str()));
            return;
        }
        // One instance only (diagnostic runs with BH_DEBUG may start a second one for testing).
        // GetLastError must be read right after CreateMutexW: reading an environment variable that
        // is not set overwrites it (ERROR_ENVVAR_NOT_FOUND), which used to let every copy start.
        let _mutex = CreateMutexW(None, true, w!("Local\\BatteryHub.Taskbar"));
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        let second_ok = std::env::var_os("BH_DEBUG").is_some() || std::env::var_os("BH_SECOND_INSTANCE").is_some();
        if already_running && !second_ok {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // No text input anywhere: skip loading the IME / text services framework.
        let _ = windows::Win32::UI::Input::Ime::ImmDisableIME(u32::MAX);

        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        for (class, proc_) in
            [(MAIN_CLASS, main_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), (STRIP_CLASS, strip_proc), (CARD_CLASS, card_proc)]
        {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: hinst.into(),
                hCursor: arrow,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
        let Ok(main) = CreateWindowExW(WS_EX_TOOLWINDOW, MAIN_CLASS, w!("BatteryHub Taskbar"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None)
        else {
            return;
        };
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        // Arrival / removal of any HID device (USB receivers, cables, Bluetooth HID links) is
        // reported to this window even though it is hidden.
        let mut filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
            dbcc_classguid: windows::Win32::Devices::HumanInterfaceDevice::HidD_GetHidGuid(),
            ..Default::default()
        };
        let _ = RegisterDeviceNotificationW(HANDLE(main.0), &mut filter as *mut _ as *const c_void, DEVICE_NOTIFY_WINDOW_HANDLE);
        with_app(|a| {
            a.main = main;
            a.taskbar_created = taskbar_created;
            if RegisterShellHookWindow(main).as_bool() {
                a.shell_msg = RegisterWindowMessageW(w!("SHELLHOOK"));
            }
            a.refresh_settings();
            a.create_tooltip();
            a.history = batteryhub::history::History::load(EDITION, SETTINGS_KEY);
            a.names = settings::names(SETTINGS_KEY);
            a.merged = settings::merged_targets(SETTINGS_KEY);
            a.hidden = settings::hidden(SETTINGS_KEY);
            a.saved_order = settings::order(SETTINGS_KEY);
            a.names_msg = batteryhub::rename::names_changed_message();
            // BH_FORCE_FALLBACK (diagnostics): start with notification-area icons. Otherwise the
            // fallback is decided by tick() after FALLBACK_AFTER_TICKS, so a taskbar that is not
            // ready yet at sign-in does not make icons flash up for a second.
            if std::env::var_os("BH_FORCE_FALLBACK").is_some() || a.icons_mode {
                a.set_fallback(true);
            } else if a.ensure_strip() {
                // Copies for other monitors right away (not at the first timer tick), so the
                // widget appears on all taskbars together as soon as their places are known.
                a.sync_mirrors();
            }
            a.bt_sig = batteryhub::drivers::bluetooth::connection_signature();
            a.start_poll();
        });
        SetTimer(Some(main), TIMER_POLL, refresh_minutes() * 60_000, None);
        SetTimer(Some(main), TIMER_PLACE, 1000, None);
        // Copies hide over full-screen windows at once, not only at the next placement tick.
        let _ = windows::Win32::UI::Accessibility::SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        SetTimer(Some(main), TIMER_BTWATCH, 3000, None);
        SetTimer(Some(main), TIMER_QUICK, QUICK_MS, None);
        if auto_update() {
            SetTimer(Some(main), TIMER_AUTO_UPDATE, AUTO_UPDATE_DELAY_MS, None);
        }
        // The Help menu's latest releases, loaded in the background now, so they are there the
        // first time the menu opens (not "Loading...").
        let _ = recent_releases();
        // Started by the setup after a new installation: say once that BatteryHub is free (people
        // were found selling it). Its own thread, so the strip keeps working while it is open.
        if batteryhub::util::reg_get_string(SETTINGS_KEY, "FreeNotice").is_some() {
            batteryhub::util::reg_delete_value(SETTINGS_KEY, "FreeNotice");
            std::thread::spawn(free_notice);
        }
        // Started by the setup after an update: say so once.
        if let Some(old) = batteryhub::util::reg_get_string(SETTINGS_KEY, "UpdatedFrom") {
            batteryhub::util::reg_delete_value(SETTINGS_KEY, "UpdatedFrom");
            if batteryhub::install::label_of(&old) != batteryhub::install::version_label() {
                with_app(|a| a.updated_from = Some(old));
                SetTimer(Some(main), TIMER_UPDATED, 2000, None);
            }
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Tab / Enter / Esc in the "Request support for a device" window.
            if batteryhub::report_window::pre_translate(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        with_app(|a| a.stop_watching_clicks());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_offer_is_made_once_per_pair() {
        let keys = vec!["b".to_string(), "a".to_string(), "c".to_string()];
        assert_eq!(first_unasked_pair(&keys, |_, _| false), Some(("a".into(), "b".into())));
        assert_eq!(
            first_unasked_pair(&keys, |a, b| a == "a" && b == "b"),
            Some(("b".into(), "c".into()))
        );
        assert_eq!(first_unasked_pair(&keys, |_, _| true), None);
        assert_eq!(first_unasked_pair(&keys[..1], |_, _| false), None);
    }

    #[test]
    fn stale_room_answer_is_forgotten_when_placement_is_skipped() {
        let app = App::default();
        app.rooms.borrow_mut().insert(42, false);
        app.forget_room(HWND(42 as *mut _));
        assert!(app.rooms.borrow().is_empty());
    }

    #[test]
    fn old_click_mode_is_a_normal_hover_card() {
        assert_eq!(hover_card_mode(settings::HoverMode::Click), settings::HoverMode::Card);
        assert_eq!(hover_card_mode(settings::HoverMode::Tip), settings::HoverMode::Tip);
        assert_eq!(hover_card_mode(settings::HoverMode::Off), settings::HoverMode::Off);
    }

    #[test]
    fn clicked_card_waits_for_a_click_by_default() {
        use settings::CardDismiss::{Away, OutsideClick};
        assert!(closes_when_away(false, OutsideClick)); // a hover card always follows the mouse
        assert!(closes_when_away(true, Away));
        assert!(!closes_when_away(true, OutsideClick));
    }

    #[test]
    fn card_on_click_keeps_hover_quiet() {
        use settings::HoverMode::{Card, Click, Off, Tip};
        // Checked: nothing opens while the mouse merely passes over the device.
        for mode in [Card, Tip, Off, Click] {
            assert!(!hover_shows_popup(mode, true));
        }
        assert!(hover_shows_popup(Card, false));
        assert!(hover_shows_popup(Tip, false));
        assert!(hover_shows_popup(Click, false));
        assert!(!hover_shows_popup(Off, false));
    }

    #[test]
    fn ring_and_bolt_turn_blue_once_full() {
        let dev = |percent, charging, full| DeviceStatus { percent, charging, full, ..Default::default() };
        for light in [false, true] {
            assert_eq!(App::ring_power_color(&dev(62, true, false), light), ui::charging_color(light));
            assert_eq!(App::ring_power_color(&dev(100, true, false), light), ui::full_color(light));
            assert_eq!(App::ring_power_color(&dev(100, true, true), light), ui::full_color(light));
        }
    }

    #[test]
    fn ring_colour_animates_charging_to_full_and_full_straight_to_normal() {
        let light = false;
        let base = rgb(255, 255, 255);
        let t0 = Instant::now();
        let half = t0 + std::time::Duration::from_millis(500);
        let mut app = App::default();
        let d = DeviceStatus { connected: true, percent: 100, charging: false, ..Default::default() };

        // Unplug from 100%: blue -> white, with no green stop on the way.
        app.ring_charge.insert("k".into(), RingAnim { at: t0, from: RingPower::Full, to: RingPower::Off });
        assert_eq!(app.ring_color("k", &d, light, base, t0, true), ui::full_color(light));
        let mid = app.ring_color("k", &d, light, base, half, true);
        let (r, _, b) = split(mid);
        assert!(b > 200 && r > 100, "expected a blue-to-white blend, got {mid:?}");
        assert_ne!(mid, ui::charging_color(light));
        let (open, _, animating) = app.ring_charge_state("k", false, 100, half);
        assert!(animating && open > 0.0 && open < 1.0);

        // Reaching 100% while still on power only fades green -> blue; the arc stays open.
        app.ring_charge.insert("k".into(), RingAnim { at: t0, from: RingPower::Charging, to: RingPower::Full });
        let (open, _, animating) = app.ring_charge_state("k", true, 100, half);
        assert!(animating);
        assert_eq!(open, 1.0);
        let mid = app.ring_color("k", &d, light, base, half, true);
        let (_, g, b) = split(mid);
        assert!(g > 150 && b > 150);
    }

    #[test]
    fn vertical_taskbar_column() {
        // Two devices (icon, level, bolt) and one (icon, level) in a 48 px wide column.
        let pieces = [(Some(0), 16, 16), (Some(0), 26, 14), (Some(0), 8, 11), (Some(1), 16, 16), (Some(1), 30, 14)];
        let (pos, h, hits) = column_layout(&pieces, 48, 8, 2, 12, 2);
        assert_eq!(pos[0], (16, 8, true)); // icon centred on its own line
        assert_eq!(pos[1].1, 8 + 16 + 2); // level on the next line
        assert!(pos[2].2 && pos[2].0 >= pos[1].0 + 26); // bolt beside the level (fits)
        assert!(pos.iter().all(|p| p.0 >= 0 && p.0 < 48));
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].0, hits[1].1), (0, h)); // the rows cover the whole column
        assert_eq!(hits[0].1, hits[1].0); // and touch each other
        assert!(pos[3].1 > pos[1].1 + 14); // second device below the first
        // Too narrow for the bolt: it is left out instead of overflowing.
        let (pos, _, _) = column_layout(&pieces[..3], 32, 8, 2, 12, 2);
        assert!(!pos[2].2);
        // The "no devices" glyph alone.
        let (pos, h, hits) = column_layout(&[(None, 16, 16)], 40, 8, 2, 12, 2);
        assert_eq!((pos[0], h, hits.len()), ((12, 8, true), 32, 0));
    }

    #[test]
    fn connection_badges_are_icons_only() {
        let mark = |via: &str, name: &str, vid: Option<u16>| connection_badge(via, name, vid);
        assert_eq!(mark("Bluetooth LE", "PRO 2 LIGHTSPEED", Some(0x046D)), Some((GLYPH_BLUETOOTH, false)));
        assert_eq!(mark("Wi-Fi (2.4 GHz)", "Headset", None), Some((GLYPH_WIFI, false)));
        assert_eq!(mark("2.4 GHz receiver", "Headset", None), Some((GLYPH_WIFI, false)));
        assert_eq!(mark("USB cable", "PRO 2 LIGHTSPEED", Some(0x046D)), Some((GLYPH_USB, false)));
        assert_eq!(mark("HID", "Controller", None), Some((GLYPH_HID, false)));
        assert_eq!(mark("2.4 GHz receiver", "PRO 2 LIGHTSPEED", Some(0x046D)), Some((GLYPH_WIFI, true)));
        assert_eq!(mark("2.4 GHz receiver", "My mouse", Some(0x046D)), Some((GLYPH_WIFI, true))); // renamed too
        assert_eq!(mark("Logitech receiver", "G Pro", None), Some((GLYPH_WIFI, true)));
        assert_eq!(mark("something else", "Headset", None), None);
        let icon_font = make_font(10, 400, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0);
        if !icon_font.0.is_null() {
            let wifi = mask(&GLYPH_WIFI.to_string(), icon_font, false);
            let bolt = bolt_mask((wifi.h as f32 * 0.9).round() as i32);
            let m = overlay_mask(&wifi, &bolt, (wifi.w - bolt.w) / 2, (wifi.h - bolt.h) / 2);
            unsafe {
                let _ = DeleteObject(HGDIOBJ(icon_font.0));
            }
            assert_eq!((m.w, m.h), (wifi.w, wifi.h)); // one icon, no extra line of text
            assert!(m.a.iter().any(|&a| a >= 200));
        }
    }

    #[test]
    fn ring_gap_opens_with_round_caps() {
        let icon = Mask { w: 1, h: 1, a: vec![255] };
        let (_, closed) = ring_masks(&icon, 28, 2.5, 100);
        let (_, open) = ring_masks_with(&icon, 28, 2.5, 1.0, RING_GAP, 1.0);
        assert_eq!(open.a.len(), 28 * 28);
        assert!(closed.a[2 * 28 + 14] > 128); // the top of the ring is there when closed
        assert_eq!(open.a[2 * 28 + 14], 0); // and gone when open
        assert!(open.a[14 * 28 + 26] > 128); // the rest stays fully charged
        assert!(open.a[5 * 28 + 21] > 0); // the end of the arc is rounded into the gap
    }

    #[test]
    fn ring_bolt_sits_in_the_top_gap() {
        let icon = Mask { w: 1, h: 1, a: vec![255] };
        let (base, open) = ring_masks_with(&icon, 28, 2.5, ring_fill_fraction(0.95, RING_GAP), RING_GAP, 1.0);
        assert!(base.a.iter().any(|&a| a > 0 && a < 128)); // the unfilled remainder stays faint but visible
        let bolt = ring_bolt_mask(&icon, 28, 2.5, RING_GAP, 1.0);
        let (mut x0, mut y0, mut x1, mut y1) = (28i32, 28i32, -1i32, -1i32);
        for y in 0..28 {
            for x in 0..28 {
                if bolt.a[y * 28 + x] > 8 {
                    (x0, y0, x1, y1) = (x0.min(x as i32), y0.min(y as i32), x1.max(x as i32), y1.max(y as i32));
                }
            }
        }
        assert!(x1 >= x0 && y1 >= y0);
        assert!(((x0 + x1 + 1) - 28).abs() <= 1); // centred on the ring's vertical axis
        assert!(y1 <= 6); // lifted 1 px into the top gap; the thinnest tip is clipped
        assert!(y1 < 10); // up in the top gap, not over the device icon in the middle
        assert_eq!(open.a[2 * 28 + 14], 0); // the top arc is gone
        assert!(open.a[14 * 28 + 25] > 128); // 95% still fills the rest of the ring
    }

    #[test]
    fn ring_at_30_percent_has_no_stray_cap_on_the_left() {
        let icon = Mask { w: 1, h: 1, a: vec![255] };
        let (_, arc) = ring_masks_with(&icon, 28, 2.5, ring_fill_fraction(0.30, RING_GAP), RING_GAP, 1.0);
        assert_eq!(arc.a[5 * 28 + 8], 0); // no bright pixels on the unfilled, left side
        assert!(arc.a[5 * 28 + 21] > 128); // the filled arc starts with a round cap
        assert!(arc.a[21 * 28 + 24] > 0); // and ends at 30% of the visible ring
    }

    #[test]
    fn ring_level_is_spread_over_the_visible_arc() {
        // With a 20% gap at the top, only 80% of the circle remains. 87% must fill 87% of that,
        // not 87% of the whole circle (which would cover 96% of what can be seen).
        let f = ring_fill_fraction(0.87, RING_GAP);
        assert!((f - (0.1 + 0.87 * 0.8)).abs() < 0.001);
        assert!((ring_fill_fraction(1.0, RING_GAP) - 0.9).abs() < 0.001);
        assert!((ring_fill_fraction(0.0, RING_GAP) - 0.1).abs() < 0.001);
        let icon = Mask { w: 1, h: 1, a: vec![255] };
        let (_, arc) = ring_masks_with(&icon, 28, 2.5, f, RING_GAP, 1.0);
        assert!(arc.a[14 * 28 + 25] > 128); // right-hand side is filled
        assert_eq!(arc.a[6 * 28 + 7], 0); // the left end is not: it reads as 87%, not 100%
    }

    /// A speaker on the strip at `percent`.
    fn speaker(percent: i32) -> DeviceStatus {
        DeviceStatus { key: "test:speaker".into(), name: "Speaker".into(), via: "Bluetooth".into(), kind: DeviceKind::Speaker, percent, ..Default::default() }
    }

    /// The strip's pieces for one device at 100 % scale.
    fn pieces_of(app: &mut App, d: DeviceStatus) -> Vec<Piece> {
        let item = Item { key: d.key.clone(), dev: Some(d), vis: Some(0), f: 1.0, p: 1.0 };
        app.pieces(&[item], 1.0, false)
    }

    #[test]
    fn ring_style_marks_an_unknown_level() {
        let mut app = App { ring: true, ..Default::default() };
        // On power too: still no bolt, arc or power colour.
        let on_power = DeviceStatus { charging: true, ..speaker(-1) };
        for d in [speaker(-1), on_power] {
            let p = pieces_of(&mut app, d);
            assert_eq!(p.len(), 2); // the ring and the "?", no bolt
            assert_eq!((p[0].m.w, p[0].m.h), (28, 28));
            let top = p[0].m.a[2 * 28 + 14];
            assert!(top > 0 && top < 128); // a closed, faint ring
            assert!(p[0].second.is_none()); // with nothing filled
            assert_eq!(p[0].color, rgb(255, 255, 255));
            let (_, _, text, _) = app.strip_fonts(1.0);
            let q = mask("?", text, false);
            assert!(q.a.iter().any(|&a| a > 0));
            assert_eq!(p[1].m.a, q.a);
        }
        // A coarse level is shown instead of the "?".
        let p = pieces_of(&mut app, DeviceStatus { level_text: "Low".into(), ..speaker(-1) });
        let (_, _, text, _) = app.strip_fonts(1.0);
        assert_eq!(p[1].m.a, mask("Low", text, false).a);
        // The connection mark follows, as it follows a known level's ring.
        app.connection_icons = true;
        let p = pieces_of(&mut app, speaker(-1));
        assert_eq!(p.len(), 3);
        assert_eq!((p[2].color, p[2].gap_after), (BADGE_GRAY, 0));
    }

    #[test]
    fn ring_style_fills_a_speaker_with_a_level() {
        let mut app = App { ring: true, ..Default::default() };
        for percent in [60, 100] {
            let p = pieces_of(&mut app, speaker(percent));
            assert_eq!(p.len(), 1);
            let (arc, _) = p[0].second.as_ref().unwrap();
            assert!(arc.a[14 * 28 + 25] > 128);
        }
    }

    #[test]
    fn unknown_level_without_ring_style_is_icon_and_question_mark() {
        let mut app = App::default();
        let p = pieces_of(&mut app, speaker(-1));
        assert_eq!(p.len(), 2);
        let (icons, _, text, _) = app.strip_fonts(1.0);
        assert_eq!(p[0].m.a, device_icon(&speaker(-1), icons, false).a);
        assert_eq!(p[1].m.a, mask("?", text, false).a);
    }

    #[test]
    fn a_quick_read_keeps_the_other_connection_of_a_joined_device() {
        let receiver = Some((1, 2));
        let reading = |hw, connected, percent| DeviceStatus { key: "bt".into(), hw, connected, percent, ..Default::default() };
        let mut app = App { merged: [("usb".to_string(), "bt".to_string())].into(), ..Default::default() };
        let show = |app: &mut App, s: DeviceStatus| {
            app.devices.insert("bt".into(), Tracked { status: s, seen: Instant::now(), reported: String::new() });
        };
        let quick = |app: &mut App, s: DeviceStatus| {
            let mut fresh = vec![s];
            app.keep_other_connections(&mut fresh);
            fresh.len()
        };
        // Shown over Bluetooth: the receiver that is off says nothing about it.
        show(&mut app, reading(None, true, 50));
        assert_eq!(quick(&mut app, reading(receiver, false, -1)), 0);
        assert_eq!(quick(&mut app, reading(receiver, true, -1)), 0);
        assert_eq!(quick(&mut app, reading(receiver, true, 60)), 1);
        assert!(!app.pending_change);
        // Shown through the receiver: it counts, and going off asks for a full read.
        show(&mut app, reading(receiver, true, 60));
        assert_eq!(quick(&mut app, reading(receiver, false, -1)), 1);
        assert!(app.pending_change);
    }

    #[test]
    fn a_custom_icon_keeps_its_shape_and_is_dimmed_once() {
        use batteryhub::custom_icon::Sprite;
        // One opaque pixel, one clear one.
        let sprite = Sprite { w: 2, h: 1, px: vec![0xFFFF_0000, 0] };
        let p = rgba_piece(None, sprite.clone(), rgb(255, 255, 255), 120, 0);
        assert_eq!(p.m.a, [255, 0]); // the glow follows the image, not its square
        assert_eq!((p.color, p.alpha), (rgb(255, 255, 255), 120));
        let icon = Mask { w: 2, h: 1, a: vec![255; 2] };
        let (base, arc) = ring_masks_with(&icon, 28, 2.5, 1.0, 0.0, 1.0);
        let px = compose_ring(&base, &arc, &sprite, rgb(255, 255, 255), rgb(0, 255, 0));
        assert_eq!(px[14 * 28 + 25], 0xFF00_FF00); // the filled ring, fully opaque
    }

    #[test]
    fn unknown_level_starts_no_ring_animation() {
        let mut app = App { ring: true, ..Default::default() };
        let charging = |percent| DeviceStatus { charging: true, ..speaker(percent) };
        app.track_ring_power(&charging(-1), Instant::now());
        assert!(app.ring_charge.is_empty());
        app.track_ring_power(&charging(50), Instant::now());
        assert_eq!(app.ring_charge.len(), 1);
        // The level gets lost while an animation runs: it is dropped.
        app.track_ring_power(&charging(-1), Instant::now());
        assert!(app.ring_charge.is_empty());
    }

    #[test]
    fn badge_toggle_animates_in_both_directions() {
        assert_eq!(badge_progress(0.0, true), 0.0);
        assert_eq!(badge_progress(BADGE_MS, true), 1.0);
        assert_eq!(badge_progress(0.0, false), 1.0);
        assert_eq!(badge_progress(BADGE_MS, false), 0.0);
        assert!((badge_progress(BADGE_MS / 2.0, true) - 0.5).abs() < 0.001);
        assert!((badge_progress(BADGE_MS / 2.0, false) - 0.5).abs() < 0.001);
        assert_eq!(badge_progress(BADGE_MS * 2.0, true), 1.0);
    }

    #[test]
    fn widgets_push_the_strip_left() {
        // Free spot: unchanged.
        assert_eq!(shift_left_of(1500, 100, 4, &[(1300, 1400)]), 1500);
        // Overlaps a meter at 1550..1650: moves left of it.
        assert_eq!(shift_left_of(1560, 100, 4, &[(1550, 1650)]), 1446);
        // Two widgets side by side: ends left of both.
        assert_eq!(shift_left_of(1560, 100, 4, &[(1550, 1650), (1400, 1500)]), 1296);
    }

    #[test]
    fn earbuds_icon_greys_the_bud_that_is_not_reported() {
        let both = buds_mask(32, true, true);
        let right_only = buds_mask(32, false, true);
        let sum = |m: &Mask, left: bool| -> u32 {
            (0..m.h).flat_map(|y| (0..m.w).map(move |x| (x, y))).filter(|(x, _)| (x * 2 < m.w) == left).map(|(x, y)| m.a[(y * m.w + x) as usize] as u32).sum()
        };
        assert!(sum(&both, true) > 1000 && sum(&both, false) > 1000);
        assert_eq!(sum(&right_only, false), sum(&both, false));
        assert!(sum(&right_only, true) * 2 < sum(&both, true));
        if let Some(path) = std::env::var_os("BH_DUMP_BUDS") {
            // Big preview for a look: both buds over the right one alone.
            let m = buds_mask(96, true, true);
            let r = buds_mask(96, false, true);
            let mut out = format!(
                "P2
{} {}
255
",
                m.w,
                m.h * 2
            );
            for a in m.a.iter().chain(r.a.iter()) {
                out.push_str(&format!("{} ", 255 - a));
            }
            std::fs::write(path, out).unwrap();
        }
    }
}
