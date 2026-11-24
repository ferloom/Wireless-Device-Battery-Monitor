//! BatteryHub Taskbar. Runs in the background (the installer can add it to autostart)
//! and draws "🎧 25%  🖱 32%" straight onto the taskbar, left of the notification area, so no
//! hovering is needed. Levels reload every minute. Left click does nothing; right click opens the
//! menu (warning levels, refresh options, exit). When the taskbar cannot host the strip (vertical
//! taskbar, unknown Explorer layout) it falls back to one notification-area icon per device.
#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Instant;

use batteryhub::cardfx::{BlurScratch, CardBg, CardContent, Layer, compose_card};
use batteryhub::device::Missing;
use batteryhub::settings;
use batteryhub::ui::{self, rgb, split};
use batteryhub::{DeviceKind, DeviceStatus};
use windows::Win32::Foundation::{COLORREF, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::Controls::{
    TOOLTIPS_CLASSW, TTF_ABSOLUTE, TTF_TRACK, TTM_ADDTOOLW, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
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
/// Notification-area id of the low-battery notification (device icons use hashes of their keys).
const NOTIFY_UID: u32 = 0x5BA7;
const CARD_SHOW_MS: f32 = 160.0;
const CARD_HIDE_MS: f32 = 110.0;
/// Old and new card blending when the mouse moves to another device (like the taskbar's previews).
const CARD_CROSS_MS: f32 = 220.0;
/// Strongest blur of the card's content during its animations (px at 100 % scaling).
const BLUR_MAX: f32 = 6.0;
/// Automatic update check this long after start-up; when the network is not up yet (e.g. right
/// at sign-in) it is tried again every AUTO_UPDATE_RETRY_MS, at most AUTO_UPDATE_RETRIES times.
const AUTO_UPDATE_DELAY_MS: u32 = 2_000;
const AUTO_UPDATE_RETRY_MS: u32 = 15_000;
const AUTO_UPDATE_RETRIES: u32 = 8;
static AUTO_UPDATE_TRIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// How often receivers whose device is off or asleep are checked again.
const QUICK_MS: u32 = 10_000;
const WM_DEVICECHANGE_MSG: u32 = 0x0219;
const DBT_DEVNODES_CHANGED: usize = 0x0007;
const DBT_DEVICEARRIVAL: usize = 0x8000;
const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;
/// Seconds the strip may be impossible to place before falling back to notification-area icons.
const FALLBACK_AFTER_TICKS: u32 = 3;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
const PBT_APMSUSPEND: usize = 0x0004;
/// Battery refresh intervals offered in the menu, in minutes.
const INTERVALS: [(u32, &str); 5] = [(1, "1 minute"), (5, "5 minutes"), (10, "10 minutes"), (30, "30 minutes"), (60, "1 hour")];
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
const _: () = assert!(ID_INTERVAL_BASE > ID_ON_CHANGE && ID_INTERVAL_BASE + INTERVALS.len() <= ID_MONITOR_ALL, "menu ids overlap");
/// "Show widget on": all monitors, main monitor only, or ID_MONITOR_FIRST + i for one other monitor.
const ID_MONITOR_ALL: usize = 40;
const ID_MONITOR_MAIN: usize = 41;
const ID_MONITOR_FIRST: usize = 42;
const ID_MONITOR_GONE: usize = 73;
/// "Position on the taskbar": ID_POSITION_FIRST + i for POSITION_OFFSETS[i].
const ID_POSITION_FIRST: usize = 80;
const ID_POSITION_CUSTOM: usize = 95;
const POSITION_OFFSETS: [u32; 11] = [0, 20, 40, 60, 80, 100, 120, 140, 160, 180, 200];
/// "Height on the taskbar": ID_HEIGHT_FIRST + i for HEIGHT_OFFSETS[i] (px, + = down).
const ID_HEIGHT_FIRST: usize = 100;
const ID_HEIGHT_CUSTOM: usize = 115;
const HEIGHT_OFFSETS: [i32; 7] = [-15, -10, -5, 0, 5, 10, 15];

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
    /// Whether the devices were enumerated at least once (the first read always enumerates).
    scanned: bool,
    /// The next poll was triggered by a device connecting / disconnecting: devices that are gone
    /// and have no sleep grace period disappear at once instead of after MIN_GRACE_SEC.
    devchange: bool,
    size: SIZE,
    last_place: (RECT, RECT, i32),
    /// Main taskbar's notification area, to notice when it changes (see tray_probe).
    main_tray: RECT,
    /// Every copy on another monitor knows its place: the strips appear on all taskbars together.
    mirrors_ready: bool,
    /// Strip x ranges of the devices drawn in the last paint: (from, to, device index).
    hits: Vec<(i32, i32, usize)>,
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
    /// Copies of the strip on the taskbars of other monitors ("Show widget on"): (taskbar, strip).
    mirrors: Vec<(HWND, HWND)>,
    /// Pixels of the last render, pushed to every mirror as well.
    pixels: Vec<u32>,
    /// The same render on the hover plate (made once per render, not on every hover).
    lit: Vec<u32>,
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
    /// Version the setup just updated from (shown once in "Update finished").
    updated_from: Option<String>,
    /// Names chosen with "Rename device" (device key -> name).
    names: std::collections::HashMap<String, String>,
    /// "Warn at 5%" state per device.
    critical: settings::CriticalWarnings,
    /// Icon of the low-battery notification while it is shown.
    notify_icon: Option<HICON>,
    /// Registered message: a device was renamed.
    names_msg: u32,
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
    /// Receivers being read by the running quick read.
    static QUICK_IDS: RefCell<Vec<(u16, u16)>> = const { RefCell::new(Vec::new()) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().map(|mut a| f(&mut a)))
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

struct Piece {
    /// Index of the device this piece belongs to (for hover hit-testing), None for others.
    dev: Option<usize>,
    m: Mask,
    color: COLORREF,
    alpha: u32,
    gap_after: i32,
}

/// Every font the strip and the card draw with, for one DPI scale (creating a font and checking
/// its face costs a device context each time, so they are made once, not per render).
struct Fonts {
    scale: f32,
    icons: HFONT,
    bolt: HFONT,
    text: HFONT,
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
            card_icon: make_font(px(16.0), 400, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0),
            card_name: make_font(px(15.0), 600, "Segoe UI Variable Text", "Segoe UI", 0),
            card_big: make_font(px(30.0), 600, "Segoe UI Variable Display", "Segoe UI", 0),
            card_small: make_font(px(12.0), 400, "Segoe UI Variable Text", "Segoe UI", 0),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for f in [self.icons, self.bolt, self.text, self.card_icon, self.card_name, self.card_big, self.card_small] {
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
        self.order.iter().filter_map(|k| self.devices.get(k)).map(|t| &t.status).filter(|s| s.connected).nth(i)
    }

    fn scale(&self) -> f32 {
        let dpi = unsafe { GetDpiForWindow(self.taskbar) };
        if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
    }

    fn visible(&self) -> Vec<DeviceStatus> {
        self.order.iter().filter_map(|k| self.devices.get(k)).map(|t| t.status.clone()).filter(|s| s.connected).collect()
    }

    /// Renders the strip and hands it to the layered window.
    fn paint(&mut self) {
        if self.fallback {
            self.sync_tray();
            return;
        }
        if self.strip.0.is_null() {
            return;
        }
        self.painted_for = self.render_target();
        let s = self.scale();
        let px = |v: f32| (v * s).round() as i32;
        let light = self.light;
        // Same colours, fonts and spacing as the original C++ widget.
        let fg = if light { rgb(0, 0, 0) } else { rgb(255, 255, 255) };
        let red = ui::low_color(light);
        self.ensure_fonts();
        let (icons, bolt, text) = self.fonts.as_ref().map(|f| (f.icons, f.bolt, f.text)).unwrap_or_default();

        let mut pieces: Vec<Piece> = Vec::new();
        let devices = self.visible();
        if devices.is_empty() {
            // Battery-unknown glyph, so there is always something to right-click.
            pieces.push(Piece { dev: None, m: mask("\u{E996}", icons, light), color: fg, alpha: 110, gap_after: 0 });
        }
        for (i, d) in devices.iter().enumerate() {
            let alpha = if d.asleep { 120 } else { 255 };
            // Red at or below this device's own warning level (right-click menu, default 15%).
            let low = settings::is_low(d, settings::threshold(SETTINGS_KEY, &d.key));
            // Lightning bolt on power: green while charging, blue once it reached 100%.
            let bolt_color = ui::bolt_color(d, light);
            let mark = bolt_color.is_some();
            pieces.push(Piece { dev: Some(i), m: mask(&d.kind.glyph().to_string(), icons, light), color: fg, alpha, gap_after: px(5.0) });
            let t = if !d.level_text.is_empty() && d.percent < 0 {
                d.level_text.clone()
            } else if d.percent >= 0 {
                format!("{}%", d.percent)
            } else {
                "?".into()
            };
            pieces.push(Piece { dev: Some(i), m: mask(&t, text, light), color: if low { red } else { fg }, alpha, gap_after: if mark { px(2.0) } else { 0 } });
            if let Some(c) = bolt_color {
                pieces.push(Piece { dev: Some(i), m: mask("\u{E945}", bolt, light), color: c, alpha, gap_after: 0 });
            }
            if i + 1 < devices.len() {
                pieces.last_mut().unwrap().gap_after = px(14.0);
            }
        }
        if self.manual && self.polling {
            // Refresh started from the menu: a small refresh glyph until it finishes.
            pieces.last_mut().unwrap().gap_after = px(10.0);
            pieces.push(Piece { dev: None, m: mask("\u{E72C}", bolt, light), color: fg, alpha: 170, gap_after: 0 });
        }
        let mut tb = RECT::default();
        unsafe {
            let _ = GetClientRect(self.taskbar, &mut tb);
        }
        if self.vertical {
            self.paint_column(&pieces, (tb.right - tb.left).clamp(px(32.0), px(160.0)), s, light);
            return;
        }
        let h = (tb.bottom - tb.top).clamp(px(24.0), px(64.0));
        let pad = px(8.0);
        let w = pad * 2 + pieces.iter().map(|p| p.m.w + p.gap_after).sum::<i32>();
        // Alpha 1 everywhere: fully transparent pixels of a layered window let clicks fall through.
        let mut cv = Canvas { w, h, px: vec![0x0100_0000; (w * h) as usize] };
        if self.background {
            // Rounded plate in the taskbar's own colour: keeps the text readable on a transparent
            // or restyled taskbar (e.g. TranslucentTB, Windhawk's Taskbar Styler).
            let plate = if light { rgb(243, 243, 243) } else { rgb(32, 32, 32) };
            cv.round_rect(px(4.0), px(6.0), plate, 235);
        }
        let mut x = pad;
        self.hits.clear();
        for p in &pieces {
            if let Some(dev) = p.dev {
                // Each device owns its pieces plus half of the gap around them.
                match self.hits.last_mut() {
                    Some(h) if h.2 == dev => h.1 = x + p.m.w + p.gap_after / 2,
                    _ => {
                        let from = self.hits.last().map(|h| h.1).unwrap_or(0);
                        self.hits.push((from, x + p.m.w + p.gap_after / 2, dev));
                    }
                }
            }
            let y0 = ((h - p.m.h) / 2 + px(self.offset_y as f32)).clamp(0, (h - p.m.h).max(0));
            for yy in 0..p.m.h {
                for xx in 0..p.m.w {
                    let a = p.m.a[(yy * p.m.w + xx) as usize] as u32 * p.alpha / 255;
                    cv.blend(x + xx, y0 + yy, p.color, a);
                }
            }
            x += p.m.w + p.gap_after;
        }
        if let Some(last) = self.hits.last_mut() {
            last.1 = w;
        }
        self.size = SIZE { cx: w, cy: h };
        self.pixels = cv.px;
        self.lit = self.hover_pixels();
        self.push(self.strip);
        for i in 0..self.mirrors.len() {
            self.push(self.mirrors[i].1);
        }
        self.last_place.2 = -1; // force a reposition with the new width
        self.place();
        // The copies keep their left edge when their width changes: move them too, so their right
        // edge stays next to the clock (not only at the next timer tick).
        for i in 0..self.mirrors.len() {
            let (bar, strip) = self.mirrors[i];
            self.place_mirror(bar, strip);
        }
    }

    /// Vertical taskbar: each device as its icon over its level (the bolt beside the level when it
    /// fits), devices stacked, as wide as the taskbar. Hit ranges are rows (y).
    fn paint_column(&mut self, pieces: &[Piece], w: i32, s: f32, light: bool) {
        let px = |v: f32| (v * s).round() as i32;
        let sizes: Vec<(Option<usize>, i32, i32)> = pieces.iter().map(|p| (p.dev, p.m.w, p.m.h)).collect();
        let (placed, h, hits) = column_layout(&sizes, w, px(8.0), px(2.0), px(12.0), px(2.0));
        let mut cv = Canvas { w, h, px: vec![0x0100_0000; (w * h) as usize] };
        if self.background {
            let plate = if light { rgb(243, 243, 243) } else { rgb(32, 32, 32) };
            cv.round_rect(0, px(6.0), plate, 235);
        }
        for (p, &(x, y, show)) in pieces.iter().zip(&placed) {
            if show {
                cv.draw(&p.m, x, y, p.color, p.alpha);
            }
        }
        self.hits = hits;
        self.size = SIZE { cx: w, cy: h };
        self.pixels = cv.px;
        self.lit = self.hover_pixels();
        self.push(self.strip);
        self.last_place.2 = -1;
        self.place();
    }

    /// The last render on the taskbar's hover plate: a rounded, slightly lighter (dark taskbar) or
    /// darker (light taskbar) rectangle behind the content, like the taskbar's own buttons.
    fn hover_pixels(&self) -> Vec<u32> {
        let (w, h) = (self.size.cx, self.size.cy);
        let s = self.scale();
        let mut plate = Canvas { w, h, px: vec![0; (w * h) as usize] };
        let (color, alpha) = if self.light { (rgb(0, 0, 0), 14) } else { (rgb(255, 255, 255), 21) };
        plate.round_rect((4.0 * s).round() as i32, (4.0 * s).round() as i32, color, alpha);
        self.pixels
            .iter()
            .zip(&plate.px)
            .map(|(&c, &p)| {
                // Content over the plate, both premultiplied.
                let inv = 255 - (c >> 24);
                let ch = |sh: u32| (((c >> sh) & 0xFF) + ((p >> sh) & 0xFF) * inv / 255).min(255);
                ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0)
            })
            .collect()
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
        let (w, h) = (self.size.cx, self.size.cy);
        if strip.0.is_null() || self.pixels.len() != (w * h) as usize {
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
                if strip == self.hovered && self.lit.len() == self.pixels.len() {
                    std::ptr::copy_nonoverlapping(self.lit.as_ptr(), bits as *mut u32, self.lit.len());
                } else {
                    std::ptr::copy_nonoverlapping(self.pixels.as_ptr(), bits as *mut u32, self.pixels.len());
                }
                let ob = SelectObject(mem, HGDIOBJ(bmp.0));
                let src = POINT::default();
                let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                let _ = UpdateLayeredWindow(strip, Some(screen), None, Some(&self.size), Some(mem), Some(&src), COLORREF(0), Some(&bf), ULW_ALPHA);
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
                && GetParent(self.strip).is_ok_and(|p| p == self.taskbar);
            if !alive {
                if is_our_strip(self.strip) {
                    let _ = DestroyWindow(self.strip);
                }
                self.strip = HWND::default();
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
            let Some(strip) = create_strip(self.taskbar) else {
                return false;
            };
            dbg_log(&format!("strip created {:?} parent {:?}", strip, self.taskbar));
            self.strip = strip;
            self.last_place = Default::default();
            // Shown only on another monitor: the new main strip stays hidden from the start (no
            // flash on the main taskbar before the first sync hides it).
            self.primary_hidden = self.chosen_bars().1;
        }
        if !self.fallback {
            self.paint();
            // While copies on other monitors still wait for their place, place() shows it with them.
            if !self.primary_hidden && self.mirrors_ready {
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
        } else {
            self.fail_ticks += 1;
            if self.fail_ticks >= FALLBACK_AFTER_TICKS {
                self.set_fallback(true);
            }
        }
    }

    // ---- other monitors ("Show widget on") -----------------------------------------------------

    /// Keeps one copy of the strip on the taskbar of each extra monitor the user chose (left to
    /// right), next to that taskbar's clock. Taskbars of other monitors exist only when Windows
    /// shows the taskbar on all displays.
    fn sync_mirrors(&mut self) {
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
        let mut keep = Vec::new();
        for (bar, strip) in std::mem::take(&mut self.mirrors) {
            let alive = unsafe { bars.contains(&bar) && IsWindow(Some(strip)).as_bool() && GetParent(strip).is_ok_and(|p| p == bar) };
            if alive {
                keep.push((bar, strip));
            } else if is_our_strip(strip) {
                unsafe {
                    let _ = DestroyWindow(strip);
                }
            }
        }
        self.mirrors = keep;
        for bar in bars {
            if self.mirrors.iter().any(|m| m.0 == bar) {
                continue;
            }
            let Some(strip) = create_strip(bar) else { continue };
            dbg_log(&format!("mirror strip {strip:?} on {bar:?}"));
            self.mirrors.push((bar, strip));
            self.push(strip);
        }
        // Stray copies (e.g. left by an Explorer restart) on any secondary taskbar.
        for bar in secondary_taskbars() {
            unsafe {
                let mut after: Option<HWND> = None;
                while let Ok(h) = FindWindowExW(Some(bar), after, STRIP_CLASS, None) {
                    if h.0.is_null() {
                        break;
                    }
                    after = Some(h);
                    if !self.mirrors.iter().any(|m| m.1 == h) && is_our_strip(h) {
                        let _ = DestroyWindow(h);
                    }
                }
            }
        }
        let mut pending = false;
        for i in 0..self.mirrors.len() {
            let (bar, strip) = self.mirrors[i];
            pending |= self.place_mirror(bar, strip);
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
                if IsWindowVisible(self.strip).as_bool() {
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
                self.place();
            }
        }
        self.primary_hidden = hide;
    }

    fn drop_mirrors(&mut self) {
        self.set_primary_hidden(false);
        for (_, strip) in std::mem::take(&mut self.mirrors) {
            if is_our_strip(strip) {
                unsafe {
                    let _ = DestroyWindow(strip);
                }
            }
        }
    }

    /// Places a copy just left of the secondary taskbar's clock (Windows 10 / ExplorerPatcher
    /// layout), or, when there is no clock window (Windows 11 draws it itself), just left of the
    /// first notification-area item that UI Automation reports (see tray_probe).
    /// Returns true while the copy is hidden because its place is not known yet.
    fn place_mirror(&self, bar: HWND, strip: HWND) -> bool {
        unsafe {
            let mut br = RECT::default();
            let _ = GetWindowRect(bar, &mut br);
            if br.bottom - br.top > br.right - br.left {
                let _ = ShowWindow(strip, SW_HIDE);
                return false; // vertical
            }
            let dpi = GetDpiForWindow(bar);
            let s = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
            let right = match FindWindowExW(Some(bar), None, w!("ClockButton"), None) {
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
                    batteryhub::tray_probe::TrayEdge::Pending => return true,
                    batteryhub::tray_probe::TrayEdge::Unknown => br.right - (130.0 * s) as i32,
                },
            };
            let mut p = POINT { x: right, y: br.top };
            let _ = ScreenToClient(bar, &mut p);
            let mut left = p.x - self.size.cx - (2.0 * s) as i32;
            if self.offset > 0 {
                left -= (self.offset as f32 * s) as i32;
            }
            left = avoid_widgets(bar, left, self.size.cx, (4.0 * s) as i32);
            let mut client = RECT::default();
            let _ = GetClientRect(bar, &mut client);
            let top = ((client.bottom - self.size.cy) / 2).max(0);
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
            let mut cur = RECT::default();
            let _ = GetWindowRect(strip, &mut cur);
            let mut cp = POINT { x: cur.left, y: cur.top };
            let _ = ScreenToClient(bar, &mut cp);
            let covered = GetWindow(strip, GW_HWNDPREV).is_ok_and(|h| !h.0.is_null()) || !IsWindowVisible(strip).as_bool();
            let same = cp.x == left && cp.y == top && cur.right - cur.left == self.size.cx && cur.bottom - cur.top == self.size.cy;
            if covered || !same {
                let _ = SetWindowPos(strip, Some(HWND_TOP), left, top, self.size.cx, self.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
        false
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
            wanted.push(("\u{0}none".into(), "-".into(), normal, -1, "BatteryHub: no devices found".into()));
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

    fn remove_tray(&mut self) {
        let keys: Vec<String> = self.tray.keys().cloned().collect();
        for k in keys {
            self.remove_tray_icon(&k);
        }
    }

    /// Keeps the strip just left of the notification area (checked every second, like the original).
    fn place(&mut self) {
        if self.strip.0.is_null() || self.primary_hidden || !self.mirrors_ready {
            return;
        }
        unsafe {
            let Ok(tray) = FindWindowExW(Some(self.taskbar), None, w!("TrayNotifyWnd"), None) else {
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
                let mut top = p.y - self.size.cy - (2.0 * self.scale()) as i32;
                if self.offset > 0 {
                    top -= (self.offset as f32 * self.scale()) as i32;
                }
                let top = top.max(0);
                let left = ((client.right - self.size.cx) / 2).max(0);
                let covered = GetWindow(self.strip, GW_HWNDPREV).is_ok_and(|h| !h.0.is_null()) || !IsWindowVisible(self.strip).as_bool();
                let key = (tr, bar, top);
                if !covered && self.last_place == key {
                    return;
                }
                self.last_place = key;
                let _ = SetWindowPos(self.strip, Some(HWND_TOP), left, top, self.size.cx, self.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                return;
            }
            let mut p = POINT { x: tr.left, y: bar.top };
            let _ = ScreenToClient(self.taskbar, &mut p);
            let mut left = p.x - self.size.cx - (2.0 * self.scale()) as i32;
            // Optional extra gap (DPI-independent px), e.g. to sit next to another taskbar widget.
            if self.offset > 0 {
                left -= (self.offset as f32 * self.scale()) as i32;
            }
            // Other programs' widgets in the taskbar: sit left of them instead of on top.
            left = avoid_widgets(self.taskbar, left, self.size.cx, (4.0 * self.scale()) as i32);
            let mut client = RECT::default();
            let _ = GetClientRect(self.taskbar, &mut client);
            let top = ((client.bottom - self.size.cy) / 2).max(0);

            // Windows 10 / ExplorerPatcher taskbar: narrow the task buttons so they never slide under us.
            if let Ok(rebar) = FindWindowExW(Some(self.taskbar), None, w!("ReBarWindow32"), None) {
                let mut rr = RECT::default();
                let _ = GetWindowRect(rebar, &mut rr);
                let mut tl = POINT { x: rr.left, y: rr.top };
                let _ = ScreenToClient(self.taskbar, &mut tl);
                if tl.x + (rr.right - rr.left) > left && left > tl.x {
                    let _ = MoveWindow(rebar, tl.x, tl.y, left - tl.x, rr.bottom - rr.top, true);
                }
            }
            let covered = GetWindow(self.strip, GW_HWNDPREV).is_ok_and(|h| !h.0.is_null()) || !IsWindowVisible(self.strip).as_bool();
            let key = (tr, bar, left);
            if !covered && self.last_place == key {
                return;
            }
            self.last_place = key;
            let _ = SetWindowPos(self.strip, Some(HWND_TOP), left, top, self.size.cx, self.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
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
            if s.connected && !s.asleep && s.percent >= 0 && !s.key.starts_with("demo:") {
                self.history.record(&s.key, now, s.percent, s.charging, s.is_full(), gap);
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
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{:?}|{}",
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
            batteryhub::history::now_secs() / 60
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
        let Some(f) = self.fonts.as_ref() else { return (1, 1, vec![0]) };
        let (icon_f, name_f, big_f, small_f) = (f.card_icon, f.card_name, f.card_big, f.card_small);

        // As wide as the name needs, 300 to 400 px; longer names end in "…".
        let pad = px(16.0);
        let room = px(400.0) - 2 * pad - px(26.0);
        let mut name_text = d.name.clone();
        let mut name = mask(&name_text, name_f, light);
        let mut chars: Vec<char> = d.name.chars().collect();
        while name.w > room && chars.len() > 4 {
            chars.pop();
            name_text = format!("{}…", chars.iter().collect::<String>().trim_end());
            name = mask(&name_text, name_f, light);
        }
        let w = (name.w + 2 * pad + px(26.0)).clamp(px(300.0), px(400.0));
        let chart_h = px(80.0);
        // Drawn on a tall canvas, then cut to the content's height plus the bottom margin.
        let tall = px(360.0);
        let mut cv = Canvas { w, h: tall, px: vec![0; (w * tall) as usize] };

        // Header: device glyph and name.
        let mut y = pad;
        let glyph = mask(&d.kind.glyph().to_string(), icon_f, light);
        cv.draw(&glyph, pad, y + px(2.0), fg, 255);
        cv.draw(&name, pad + px(26.0), y, fg, 255);
        y += px(34.0);

        // Big level (or the device's own word for it), then the bar.
        let big_text = if d.percent >= 0 {
            format!("{}%", d.percent)
        } else if !d.level_text.is_empty() {
            d.level_text.clone()
        } else {
            "?".into()
        };
        let big = mask(&big_text, big_f, light);
        cv.draw(&big, pad, y, if low { accent } else { fg }, 255);
        y += px(42.0); // same layout for every device, so cards share one height
        cv.fill_rrect(pad, y, w - pad, y + px(6.0), px(3.0), track, 255);
        if d.percent > 0 {
            let fill = pad + (w - 2 * pad) * d.percent.min(100) / 100;
            cv.fill_rrect(pad, y, fill.max(pad + px(6.0)), y + px(6.0), px(3.0), accent, 255);
        }
        y += px(18.0);

        // Connection and state.
        let mut line = d.via.clone();
        if d.charging {
            line.push_str(if d.full { " · Full" } else { " · Charging" });
        } else if d.asleep {
            line.push_str(" · Asleep");
        }
        let via = mask(&line, small_f, light);
        cv.draw(&via, pad, y, dim, 255);
        y += px(30.0);

        // Last 24 hours (the history keeps 7 days; the dashboard shows all of it).
        let title = mask("Last 24 hours", small_f, light);
        cv.draw(&title, pad, y, dim, 255);
        y += px(22.0);
        let (x0, x1, top, bottom) = (pad, w - pad, y, y + chart_h);
        for gy in [top, (top + bottom) / 2, bottom] {
            let mut x = x0;
            while x < x1 {
                cv.blend(x, gy, grid, 255);
                x += px(4.0).max(2);
            }
        }
        let now = batteryhub::history::now_secs();
        let from = now.saturating_sub(24 * 3600);
        let lw = px(2.0).max(2);
        let segs = batteryhub::history::segments(&self.history.series(&d.key), from, now);
        let to_x = |t: u64| x0 + ((x1 - x0) as f32 * (t.saturating_sub(from) as f32 / (now - from).max(1) as f32).min(1.0)) as i32;
        let to_y = |l: u8| bottom - (chart_h as f32 * l as f32 / 100.0) as i32;
        for seg in &segs {
            for pair in seg.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let c = if a.charging() { charge_c } else { accent };
                // Steps, not slopes: a level holds until the next reading.
                cv.line(to_x(a.t), to_y(a.level), to_x(b.t), to_y(a.level), lw, c);
                cv.line(to_x(b.t), to_y(a.level), to_x(b.t), to_y(b.level), lw, c);
            }
        }
        if let Some(last) = segs.last().and_then(|s| s.last()) {
            // Dotted "now" line and the latest reading.
            let mut yy = top;
            while yy < bottom {
                cv.blend(x1, yy, grid, 255);
                yy += px(4.0).max(2);
            }
            cv.disc(to_x(last.t), to_y(last.level), px(3.5), if last.charging() { charge_c } else { accent });
        }
        y = bottom + px(10.0);
        let ago = mask("24h ago", small_f, light);
        cv.draw(&ago, x0, y, dim, 255);
        let nowm = mask("Now", small_f, light);
        cv.draw(&nowm, x1 - nowm.w, y, dim, 255);
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
            let (x, y, slide) = if self.vertical {
                // Beside the taskbar, level with the hovered device (no slide: it fades in).
                let mid = (mi.rcMonitor.left + mi.rcMonitor.right) / 2;
                let x = if rc.left < mid { rc.right + gap } else { rc.left - w - gap };
                let y = (rc.top + (from + to) / 2 - h / 2).clamp(work.top + gap, (work.bottom - h - gap).max(work.top));
                (x, y, 0)
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
                (Some(c), true) if c.h == h => {
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
            let (bg, border) = self.card_colors();
            let radius = (8.0 * self.scale()).round() as i32;
            let scale = self.scale();
            self.card_anim =
                CardAnim { at: old.at, showing: true, visible: true, x, y, slide, content: Some(content), prev, drawn: old.drawn, bg, border, radius, scale };
            if crossing {
                self.card_anim.at = Some(Instant::now());
                SetTimer(Some(self.main), TIMER_CARD, 15, None);
            } else if same && self.card_anim.at.is_none() {
                // Same device, new values: swap the content at once.
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
        if let (true, Some((ox, ow, old))) = (a.showing, &a.prev) {
            // Ease in-out, like the taskbar's window previews moving between buttons.
            let e = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
            let lerp = |p: i32, q: i32| p + ((q - p) as f32 * e).round() as i32;
            let (cx, cw) = (lerp(*ox, a.x), lerp(*ow, content.w));
            let (ux0, ux1) = (a.x.min(*ox), (a.x + content.w).max(ox + ow));
            // Old content blurs away while the new one comes into focus.
            let (ro, rn) = ((BLUR_MAX * e * scale).round() as i32, (BLUR_MAX * (1.0 - e) * scale).round() as i32);
            pos = POINT { x: ux0, y: a.y };
            size = SIZE { cx: ux1 - ux0, cy: h };
            (bx, bw) = (cx - ux0, cw);
            layers = vec![(old.clone(), 1.0 - e, ro), (content.clone(), e, rn)];
            alpha = 255;
            self.card_anim.drawn = (cx, cw);
        } else {
            let ease = 1.0 - (1.0 - t).powi(3); // ease-out cubic, like Windows 11 flyouts
            let (fade, dy) = if a.showing { (ease, a.slide as f32 * (1.0 - ease)) } else { (1.0 - t, 0.0) };
            // Comes into focus while appearing, goes slightly out of focus while disappearing.
            let r = if a.showing { BLUR_MAX * (1.0 - ease) } else { BLUR_MAX * 0.6 * t };
            pos = POINT { x: a.x, y: a.y + dy.round() as i32 };
            size = SIZE { cx: content.w, cy: h };
            (bx, bw) = (0, content.w);
            layers = vec![(content.clone(), 1.0, (r * scale).round() as i32)];
            alpha = (fade * 255.0).round() as u8;
            self.card_anim.drawn = (a.x, content.w);
        }
        let surface = match self.card_surface.take() {
            Some(s) if s.w >= size.cx && s.h >= size.cy => Some(s),
            // A quarter more than needed now: switching between neighbouring devices (the usual
            // case) then reuses the bitmap.
            _ => Surface::new(size.cx + content.w / 4, size.cy),
        };
        let Some(mut surface) = surface else { return };
        let stride = surface.w as usize;
        let dst = surface.pixels();
        let bgc = CardBg { bx, bw, h, radius, bg, border };
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
            return "BatteryHub: no devices found".into();
        };
        let level = d.level();
        let numeric = level.ends_with('%');
        let state = if d.asleep {
            " (asleep)"
        } else if d.is_full() && numeric {
            " (full)"
        } else if d.charging && numeric {
            " (charging)"
        } else {
            ""
        };
        format!("{}: {level}{state}", d.name)
    }

    /// Mouse at (x, y) on a strip: the device under it (along x, or along y on a vertical taskbar).
    fn hover(&mut self, on: HWND, x: i32, y: i32) {
        let x = if self.vertical { y } else { x };
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
            let hit = self.hits.iter().find(|h| x >= h.0 && x < h.1).map(|h| (h.2, h.0, h.1));
            let key = hit.map(|h| h.0).unwrap_or(usize::MAX);
            if self.tip_for == Some(key) {
                return;
            }
            self.tip_for = Some(key);
            // The card (name, level, last 24 hours) above the hovered device; nothing between devices.
            match hit {
                Some((i, from, to)) => self.show_card(i, on, from, to),
                None => self.hide_card(),
            }
        }
    }

    fn hide_tip(&mut self) {
        self.set_hovered(HWND::default());
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
        ids.extend(self.devices.values().filter(|t| t.status.kind == DeviceKind::Headset).filter_map(|t| t.status.hw));
        if ids.is_empty() {
            return;
        }
        self.polling = true;
        let ids: Vec<(u16, u16)> = ids.into_iter().collect();
        dbg_log(&format!("quick read of {ids:04X?}"));
        let ids2 = ids.clone();
        QUICK_IDS.with(|q| *q.borrow_mut() = ids);
        ui::run_async(self.main, WM_QUICK_DONE, move || batteryhub::device::poll_hardware(&ids2));
    }

    /// Merges a quick read: only devices behind the receivers in `ids` are updated or removed.
    fn on_quick_result(&mut self, fresh: Vec<DeviceStatus>, ids: &[(u16, u16)]) {
        self.polling = false;
        let keys: HashSet<String> = fresh.iter().map(|s| s.key.clone()).collect();
        self.devices.retain(|k, t| {
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
    fn merge(&mut self, fresh: Vec<DeviceStatus>) {
        self.record_history(&fresh);
        let now = Instant::now();
        for mut s in fresh {
            if !self.order.contains(&s.key) {
                self.order.push(s.key.clone());
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
            self.check_critical(&s);
            self.devices.insert(s.key.clone(), Tracked { status: s, seen: now, reported });
        }
        self.order.retain(|k| self.devices.contains_key(k));
        self.paint();
        self.send_to_dashboards();
    }

    /// Fresh readings for open dashboards (they show the live level and state).
    fn send_to_dashboards(&self) {
        if batteryhub::dashboard::any_open() {
            let list: Vec<(DeviceStatus, String)> = self.devices.values().map(|t| (t.status.clone(), t.reported.clone())).collect();
            batteryhub::dashboard::send_statuses(self.main, &list);
        }
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
        ui::fill_wide(&mut d.szInfoTitle, &format!("{name}: battery at {level}%"));
        ui::fill_wide(&mut d.szInfo, "Charge it soon. BatteryHub warns once per discharge (right-click the device to turn this off).");
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
        if std::mem::take(&mut self.pending_change) {
            self.devchange = true;
            self.start_poll();
        }
    }
}

impl App {
    /// Re-reads the taskbar theme and TaskbarOffset (start-up and WM_SETTINGCHANGE only).
    fn refresh_settings(&mut self) {
        self.light = ui::taskbar_is_light();
        self.offset = batteryhub::util::reg_get_dword(SETTINGS_KEY, "TaskbarOffset").unwrap_or(0);
        self.offset_y = batteryhub::util::reg_get_dword(SETTINGS_KEY, "TaskbarOffsetY").unwrap_or(0) as i32;
        self.background = batteryhub::util::reg_get_dword(SETTINGS_KEY, "WidgetBackground").unwrap_or(0) != 0;
        self.icons_mode = batteryhub::util::reg_get_dword(SETTINGS_KEY, "NotificationIcons").unwrap_or(0) != 0;
        self.choice = widget_choice();
    }
}

// ---- check for updates ---------------------------------------------------------------------

/// A check or download is running (the menu item is greyed out meanwhile).
static UPDATE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const UPDATE_TITLE: windows::core::PCWSTR = w!("BatteryHub - Check for updates");

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
        MessageBoxW(Some(owner), &windows::core::HSTRING::from(text), UPDATE_TITLE, style | MB_SETFOREGROUND | MB_TOPMOST)
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
                &format!("No new updates.\n\nYou have the latest version ({}).\nLatest release on GitHub: {}.", version_label(), label_of(&latest)),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Check::Failed(e) => {
            UPDATE_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
            message(
                main,
                &format!("Could not check for updates:\n{e}\n\nNew versions are also on GitHub:\nhttps://github.com/{}/releases", batteryhub::update::REPO),
                MB_OK | MB_ICONWARNING,
            );
        }
        // Automatic check with "Install updates silently": download and install, show nothing.
        Check::Available(r) if silent && silent_install() => {
            UPDATE_QUIET.store(true, std::sync::atomic::Ordering::Relaxed);
            post_from_thread(main, WM_UPDATE_DOWNLOADED, move || batteryhub::update::download(&r));
        }
        Check::Available(r) => {
            let text = format!(
                "A new update is available:\n\n        {}  ->  {}\n\nDo you want to update now?\n\nThe setup is downloaded from GitHub, checked against the release's SHA-256 checksum and started; it keeps your settings and autostart.",
                version_label(),
                label_of(&r.version)
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
                    &format!(
                        "Could not start the setup:
{e}

{}",
                        path.display()
                    ),
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
    let Some(mi) = monitor_info(bar) else { return "Other monitor".into() };
    let device = batteryhub::util::from_wide(&mi.szDevice);
    let mut label = match device.find("DISPLAY") {
        Some(i) => format!("Display {}", &device[i + 7..]),
        None => "Other monitor".into(),
    };
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut pm = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut pm).as_bool() {
            let (m, p) = (mi.monitorInfo.rcMonitor, pm.rcMonitor);
            let dx = (m.left + m.right - p.left - p.right) / 2;
            let dy = (m.top + m.bottom - p.top - p.bottom) / 2;
            let side = if dx.abs() >= dy.abs() {
                if dx < 0 { "left" } else { "right" }
            } else if dy < 0 {
                "above"
            } else {
                "below"
            };
            label += &format!(" ({side})");
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
unsafe fn show_menu() {
    unsafe {
        let (main, devices, renamed) = with_app(|a| {
            a.hide_tip();
            let renamed: HashSet<String> = a.names.keys().cloned().collect();
            (a.main, a.visible(), renamed)
        })
        .unwrap_or_default();
        let dark = !ui::taskbar_is_light();
        let mut icons = ui::MenuIcons::new(main, dark);
        if let Ok(menu) = CreatePopupMenu() {
            ui::MenuIcons::check_or_icon(menu);
            // One submenu per device: dashboard, history, statistics, rename and its warnings.
            settings::append_device_menus(menu, &devices, SETTINGS_KEY, &renamed, &mut icons);
            if !devices.is_empty() {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            }
            let _ = AppendMenuW(menu, MF_STRING, ID_REFRESH_DEVICES, w!("Refresh devices"));
            icons.set(menu, ID_REFRESH_DEVICES as u32, false, ui::glyph::SYNC);
            let _ = AppendMenuW(menu, MF_STRING, ID_REFRESH_LEVELS, w!("Refresh battery levels"));
            icons.set(menu, ID_REFRESH_LEVELS as u32, false, ui::glyph::REFRESH);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            // How often the battery levels are read, and whether a device change triggers a read.
            if let Ok(sub) = CreatePopupMenu() {
                let current = refresh_minutes();
                for (j, (m, label)) in INTERVALS.iter().enumerate() {
                    let check = if *m == current { MF_CHECKED } else { MF_UNCHECKED };
                    let _ = AppendMenuW(sub, MF_STRING | check, ID_INTERVAL_BASE + j, &windows::core::HSTRING::from(*label));
                }
                let n = GetMenuItemCount(Some(menu));
                let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, sub.0 as usize, w!("Refresh battery every"));
                icons.set(menu, n as u32, true, ui::glyph::HISTORY);
            }
            let on_change = if refresh_on_change() { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | on_change, ID_ON_CHANGE, w!("Refresh when a device connects or disconnects"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            // Look of the widget: plate behind it, position, and which monitors show it.
            let (choice, offset, background, offset_y, icons_mode) =
                with_app(|a| (a.choice.clone(), a.offset, a.background, a.offset_y, a.icons_mode)).unwrap_or_default();
            let _ = AppendMenuW(menu, MF_STRING | check(icons_mode), ID_ICONS_MODE, w!("Show as notification-area icons (Windows arranges them)"));
            let _ = AppendMenuW(menu, MF_STRING | check(background), ID_BACKGROUND, w!("Background behind taskbar text (for translucent taskbars)"));
            if let Ok(sub) = CreatePopupMenu() {
                let mut checked = ID_POSITION_CUSTOM;
                for (i, px) in POSITION_OFFSETS.iter().enumerate() {
                    let label = if *px == 0 { "Next to the notification area".to_string() } else { format!("{px} px to the left") };
                    let _ = AppendMenuW(sub, MF_STRING, ID_POSITION_FIRST + i, &windows::core::HSTRING::from(label));
                    if *px == offset {
                        checked = ID_POSITION_FIRST + i;
                    }
                }
                if checked == ID_POSITION_CUSTOM {
                    // A value set by hand in the registry (TaskbarOffset).
                    let label = format!("Custom ({offset} px, TaskbarOffset in the registry)");
                    let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, ID_POSITION_CUSTOM, &windows::core::HSTRING::from(label));
                }
                let _ = CheckMenuRadioItem(sub, ID_POSITION_FIRST as u32, ID_POSITION_CUSTOM as u32, checked as u32, MF_BYCOMMAND.0);
                let n = GetMenuItemCount(Some(menu));
                let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, sub.0 as usize, w!("Position on the taskbar"));
                icons.set(menu, n as u32, true, ui::glyph::SETTINGS);
            }
            if let Ok(sub) = CreatePopupMenu() {
                let mut checked = ID_HEIGHT_CUSTOM;
                for (i, px) in HEIGHT_OFFSETS.iter().enumerate() {
                    let label = match *px {
                        0 => "Centred".to_string(),
                        p if p < 0 => format!("{} px up", -p),
                        p => format!("{p} px down"),
                    };
                    let _ = AppendMenuW(sub, MF_STRING, ID_HEIGHT_FIRST + i, &windows::core::HSTRING::from(label));
                    if *px == offset_y {
                        checked = ID_HEIGHT_FIRST + i;
                    }
                }
                if checked == ID_HEIGHT_CUSTOM {
                    let label = format!("Custom ({offset_y} px, TaskbarOffsetY in the registry)");
                    let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, ID_HEIGHT_CUSTOM, &windows::core::HSTRING::from(label));
                }
                let _ = CheckMenuRadioItem(sub, ID_HEIGHT_FIRST as u32, ID_HEIGHT_CUSTOM as u32, checked as u32, MF_BYCOMMAND.0);
                let n = GetMenuItemCount(Some(menu));
                let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, sub.0 as usize, w!("Height on the taskbar"));
                icons.set(menu, n as u32, true, ui::glyph::SETTINGS);
            }
            // With more than one monitor: all of them, the main one only, or one other monitor only.
            let others: Vec<(String, String)> =
                secondary_taskbars().into_iter().take(ID_MONITOR_GONE - ID_MONITOR_FIRST).map(|b| (monitor_id(b), monitor_label(b))).collect();
            if !others.is_empty()
                && let Ok(sub) = CreatePopupMenu()
            {
                let _ = AppendMenuW(sub, MF_STRING, ID_MONITOR_ALL, w!("All monitors"));
                let _ = AppendMenuW(sub, MF_STRING, ID_MONITOR_MAIN, w!("Main monitor only"));
                let ids: Vec<String> = others.iter().map(|o| o.0.clone()).collect();
                let checked = match choice.as_str() {
                    "all" => ID_MONITOR_ALL,
                    "main" => ID_MONITOR_MAIN,
                    id => find_monitor(&ids, id).map(|i| ID_MONITOR_FIRST + i).unwrap_or(ID_MONITOR_GONE),
                };
                for (i, (_, label)) in others.iter().enumerate() {
                    let _ = AppendMenuW(sub, MF_STRING, ID_MONITOR_FIRST + i, &windows::core::HSTRING::from(format!("{label} only")));
                }
                if checked == ID_MONITOR_GONE {
                    let _ = AppendMenuW(sub, MF_STRING | MF_GRAYED, ID_MONITOR_GONE, w!("Chosen monitor (not connected, shown on main)"));
                }
                let _ = CheckMenuRadioItem(sub, ID_MONITOR_ALL as u32, ID_MONITOR_GONE as u32, checked as u32, MF_BYCOMMAND.0);
                let n = GetMenuItemCount(Some(menu));
                let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, sub.0 as usize, w!("Show widget on"));
                icons.set(menu, n as u32, true, ui::glyph::MONITOR);
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING | check(ui::autostart_enabled(autostart_name())), ID_AUTOSTART, w!("Automatically start with Windows"));
            let _ = AppendMenuW(menu, MF_STRING | check(auto_update()), ID_AUTO_UPDATE, w!("Automatically check for updates"));
            let _ = AppendMenuW(
                menu,
                MF_STRING | check(silent_install()) | if auto_update() { MF_ENABLED } else { MF_GRAYED },
                ID_SILENT_UPDATE,
                w!("Install updates silently"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, ID_REQUEST, w!("Request support for a device..."));
            icons.set(menu, ID_REQUEST as u32, false, ui::glyph::MAIL);
            let busy = UPDATE_BUSY.load(std::sync::atomic::Ordering::Relaxed);
            let label = if busy { "Checking for updates..." } else { "Check for updates..." };
            let _ = AppendMenuW(menu, MF_STRING | if busy { MF_GRAYED } else { MF_ENABLED }, ID_CHECK_UPDATES, &windows::core::HSTRING::from(label));
            icons.set(menu, ID_CHECK_UPDATES as u32, false, ui::glyph::DOWNLOAD);
            let _ = AppendMenuW(menu, MF_STRING, ID_VERSIONS, w!("Install another version..."));
            icons.set(menu, ID_VERSIONS as u32, false, ui::glyph::HISTORY);
            let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit"));
            icons.set(menu, ID_EXIT as u32, false, ui::glyph::EXIT);
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Menu colours follow the taskbar: dark on a dark taskbar, light on a light one.
            ui::set_menu_theme(main, dark);
            let _ = SetForegroundWindow(main);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, main, None);
            let _ = PostMessageW(Some(main), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match cmd.0 as usize {
                ID_EXIT => {
                    let _ = DestroyWindow(main);
                }
                ID_REFRESH_DEVICES => {
                    with_app(|a| a.manual_refresh(true));
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
                            "BatteryHub now shows one notification-area icon per device, with its level drawn in the icon.

Windows places these icons like any other: drag them to reorder, or move them in or out of the hidden-icons area. If you don't see them, open Settings > Personalization > Taskbar > Other system tray icons and turn BatteryHub on.",
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                ID_BACKGROUND => {
                    with_app(|a| {
                        a.background = !a.background;
                        save_dword("WidgetBackground", a.background as u32);
                        a.paint();
                    });
                }
                id if (ID_POSITION_FIRST..ID_POSITION_FIRST + POSITION_OFFSETS.len()).contains(&id) => {
                    let px = POSITION_OFFSETS[id - ID_POSITION_FIRST];
                    if px == 0 {
                        batteryhub::util::reg_delete_value(SETTINGS_KEY, "TaskbarOffset");
                    } else {
                        save_dword("TaskbarOffset", px);
                    }
                    with_app(|a| {
                        a.offset = px;
                        a.last_place = Default::default();
                        a.place();
                        a.sync_mirrors();
                    });
                }
                id if (ID_HEIGHT_FIRST..ID_HEIGHT_FIRST + HEIGHT_OFFSETS.len()).contains(&id) => {
                    let px = HEIGHT_OFFSETS[id - ID_HEIGHT_FIRST];
                    if px == 0 {
                        batteryhub::util::reg_delete_value(SETTINGS_KEY, "TaskbarOffsetY");
                    } else {
                        save_dword("TaskbarOffsetY", px as u32);
                    }
                    with_app(|a| {
                        a.offset_y = px;
                        a.paint();
                        a.sync_mirrors();
                    });
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
                            "Automatic update check is on.\n\nBatteryHub looks for a new version only when it starts (at sign-in or when you start it), not while it runs. When there is one, it asks you whether to update (or, with \"Install updates silently\", installs it in the background).",
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
                            "Silent updates are on.\n\nWhen BatteryHub starts and finds a new version, it downloads it from GitHub, checks its SHA-256 checksum and installs it in the background, without asking and without showing anything. Your settings and autostart are kept.",
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                ID_REQUEST => batteryhub::report_window::show(),
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
                id => {
                    if let Some((dev, action)) = settings::decode_menu_id(id)
                        && let Some(d) = devices.get(dev)
                    {
                        device_action(d, action);
                    }
                }
            }
        }
    }
}

/// An item of a device's submenu was chosen.
fn device_action(d: &DeviceStatus, action: settings::DeviceAction) {
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
        DeviceAction::ToggleWarn5 => {
            settings::set_warn_critical(SETTINGS_KEY, &d.key, !settings::warn_critical(SETTINGS_KEY, &d.key));
        }
        DeviceAction::Threshold(level) => {
            settings::set_threshold(SETTINGS_KEY, &d.key, level);
            with_app(|a| {
                a.card_cache.clear();
                a.paint();
            });
        }
    }
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

/// The hover card: a layered, click-through, topmost popup owned by the main window.
fn create_card(owner: HWND) -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
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
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

extern "system" fn strip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                with_app(|a| a.hover(hwnd, (lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32));
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
                    with_app(|a| a.hide_tip());
                    trim_memory();
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_menu();
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
                    TIMER_UPDATED => {
                        let _ = KillTimer(Some(hwnd), TIMER_UPDATED);
                        if let Some(old) = a.updated_from.take() {
                            let text = format!(
                                "Update finished.\n\nBatteryHub was updated:  {}  ->  {}\n\nYour settings and autostart were kept.",
                                batteryhub::install::label_of(&old),
                                batteryhub::install::version_label()
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
            WM_TRAY_PROBED => {
                with_app(|a| a.sync_mirrors());
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
                    show_menu();
                }
                LRESULT(0)
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
                let (tc, nm) = with_app(|a| (a.taskbar_created, a.names_msg)).unwrap_or((0, 0));
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
            a.refresh_settings();
            a.create_tooltip();
            a.history = batteryhub::history::History::load(EDITION, SETTINGS_KEY);
            a.names = settings::names(SETTINGS_KEY);
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
        SetTimer(Some(main), TIMER_BTWATCH, 3000, None);
        SetTimer(Some(main), TIMER_QUICK, QUICK_MS, None);
        if auto_update() {
            SetTimer(Some(main), TIMER_AUTO_UPDATE, AUTO_UPDATE_DELAY_MS, None);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn widgets_push_the_strip_left() {
        // Free spot: unchanged.
        assert_eq!(shift_left_of(1500, 100, 4, &[(1300, 1400)]), 1500);
        // Overlaps a meter at 1550..1650: moves left of it.
        assert_eq!(shift_left_of(1560, 100, 4, &[(1550, 1650)]), 1446);
        // Two widgets side by side: ends left of both.
        assert_eq!(shift_left_of(1560, 100, 4, &[(1550, 1650), (1400, 1500)]), 1296);
    }
}
