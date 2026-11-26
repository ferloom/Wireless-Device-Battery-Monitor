//! Where the notification-area items (clock, bell...) start on a Windows 11 taskbar that has no
//! window for them. Windows 11 draws the secondary taskbars' clock in XAML, so there is no
//! `ClockButton` window to place the widget against; UI Automation still exposes those items
//! (automation id `SystemTrayIcon`) with their screen rectangles.
//!
//! UI Automation is asked from a worker thread (the widget's own window is a child of the taskbar,
//! so asking from the UI thread could wait on itself), and only when a taskbar is new, moved or
//! resized, when `taskbars_changed` says the main taskbar's notification area changed (the
//! notification bell appears on every taskbar at once and moves the clock), and every minute. The answer is
//! kept as a distance from the taskbar's right end, so it stays right while the taskbar is checked
//! again after a move or resize. When an answer arrives, `notify` gets `msg` so the widget can be
//! placed at once.
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, TreeScope_Descendants};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const REFRESH: Duration = Duration::from_secs(60);
/// A first answer normally takes well under a second; after this, give up waiting for it.
const FIRST_WAIT: Duration = Duration::from_secs(2);
const TRAY_ID: &str = "SystemTrayIcon";

/// Where a taskbar's notification area begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEdge {
    /// Screen x of the first notification-area item (screen y on a vertical taskbar).
    Known(i32),
    /// First check still running: keep the widget hidden for now.
    Pending,
    /// UI Automation did not tell: use an estimate.
    Unknown,
}

struct Entry {
    bar: (i32, i32, i32, i32),
    /// Distance of the first item from the taskbar's right end (bottom end when vertical).
    from_right: Option<i32>,
    at: Instant,
    /// Check again at the next call (the answer is kept until then).
    dirty: bool,
}

#[derive(Default)]
struct Cache {
    known: HashMap<isize, Entry>,
    /// First request for a taskbar not answered yet.
    asked: HashMap<isize, Instant>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

/// Where the notification area of `bar` (screen rectangle `bar_rect`) begins: its left edge, or its
/// top edge on a vertical taskbar. Starts a background check when the answer is missing, stale, or
/// the taskbar changed.
pub fn tray_left(bar: HWND, bar_rect: RECT, notify: HWND, msg: u32) -> TrayEdge {
    let key = bar.0 as isize;
    let r = (bar_rect.left, bar_rect.top, bar_rect.right, bar_rect.bottom);
    let vertical = r.3 - r.1 > r.2 - r.0;
    // The axis along the taskbar: (start, end) of the bar on it.
    let (lo, hi) = if vertical { (r.1, r.3) } else { (r.0, r.2) };
    let (edge, stale) = {
        let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.get_or_insert_with(Cache::default);
        match c.known.get(&key) {
            Some(e) => (e.from_right.map_or(TrayEdge::Unknown, |d| TrayEdge::Known(hi - d)), e.dirty || e.bar != r || e.at.elapsed() >= REFRESH),
            None => {
                let since = *c.asked.entry(key).or_insert_with(Instant::now);
                (if since.elapsed() < FIRST_WAIT { TrayEdge::Pending } else { TrayEdge::Unknown }, true)
            }
        }
    };
    if stale && !BUSY.swap(true, Ordering::SeqCst) {
        let notify = notify.0 as isize;
        std::thread::spawn(move || {
            let from_right = probe(key, vertical).filter(|x| *x > lo && *x < hi).map(|x| hi - x);
            {
                let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
                let c = g.get_or_insert_with(Cache::default);
                c.known.insert(key, Entry { bar: r, from_right, at: Instant::now(), dirty: false });
                c.known.retain(|_, e| e.at.elapsed() < REFRESH * 4);
                c.asked.remove(&key);
            }
            BUSY.store(false, Ordering::SeqCst);
            unsafe {
                let _ = PostMessageW(Some(HWND(notify as *mut _)), msg, WPARAM(0), LPARAM(0));
            }
        });
    }
    edge
}

/// The main taskbar's notification area changed size (e.g. the notification bell appeared or the
/// date got longer): check every taskbar again, keeping the current answers until then.
pub fn taskbars_changed() {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.as_mut() {
        for e in c.known.values_mut() {
            e.dirty = true;
        }
    }
}

/// Leftmost (topmost when `vertical`) `SystemTrayIcon` element under the taskbar window `bar`.
fn probe(bar: isize, vertical: bool) -> Option<i32> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let found = (|| -> windows::core::Result<Option<i32>> {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = uia.ElementFromHandle(HWND(bar as *mut _))?;
            let all = root.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition()?)?;
            let mut best: Option<i32> = None;
            for i in 0..all.Length()? {
                let e = all.GetElement(i)?;
                if e.CurrentAutomationId().map(|s| s.to_string()).unwrap_or_default() != TRAY_ID {
                    continue;
                }
                let b = e.CurrentBoundingRectangle()?;
                if b.right > b.left && b.bottom > b.top {
                    let edge = if vertical { b.top } else { b.left };
                    best = Some(best.map_or(edge, |x| x.min(edge)));
                }
            }
            Ok(best)
        })();
        if init {
            CoUninitialize();
        }
        found.ok().flatten()
    }
}

// ---- the taskbar's left end ("Widget position: left") ------------------------------------------

/// How often the items at the taskbar's left end are checked again (task buttons come and go).
const LEFT_REFRESH: Duration = Duration::from_secs(2);

struct LeftEntry {
    bar: (i32, i32, i32, i32),
    /// The taskbar's items (left, right), screen x.
    items: Vec<(i32, i32)>,
    at: Instant,
    /// LEFT_GEN when it was checked.
    generation: u32,
}

static LEFT: Mutex<Option<HashMap<isize, LeftEntry>>> = Mutex::new(None);
/// Taskbars being checked right now (one worker per taskbar).
static LEFT_BUSY: Mutex<Option<std::collections::HashSet<isize>>> = Mutex::new(None);
/// Raised by left_refresh_soon: answers from an earlier generation are checked again at once.
static LEFT_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Makes the next left_content_end of every taskbar check it again (a window opened or closed).
pub fn left_refresh_soon() {
    LEFT_GEN.fetch_add(1, Ordering::SeqCst);
}

/// Screen x right after the items that start at the taskbar's left end: Start, search, task view
/// and the task buttons when they are aligned left, or the widgets (weather) button when the task
/// buttons are centred; the taskbar's own left edge when nothing starts there. None until the
/// first answer (then `notify` gets `msg`). Horizontal taskbars only.
pub fn left_content_end(bar: HWND, bar_rect: RECT, notify: HWND, msg: u32) -> Option<i32> {
    items(bar, bar_rect, notify, msg, LEFT_REFRESH).map(|v| left_chain_end(&v, bar_rect.left, bar_rect.bottom - bar_rect.top))
}

/// Screen x right after the last of the taskbar's items before `limit` (the notification area):
/// the end of the task buttons wherever they are (left or centred). Items overlapping `exclude`
/// (other programs' widgets, screen x) don't count. None until the first answer.
pub fn icons_end(bar: HWND, bar_rect: RECT, limit: i32, exclude: &[(i32, i32)], notify: HWND, msg: u32) -> Option<i32> {
    items(bar, bar_rect, notify, msg, LEFT_REFRESH).map(|v| last_item_end(&v, bar_rect.left, limit, exclude))
}

/// Screen x where the taskbar's icons start: the first item after the items at the taskbar's left
/// end (with centred icons, the first centred one), before `limit`; items over `exclude` don't
/// count. None until the first answer.
pub fn icons_start(bar: HWND, bar_rect: RECT, limit: i32, exclude: &[(i32, i32)], notify: HWND, msg: u32) -> Option<i32> {
    items(bar, bar_rect, notify, msg, LEFT_REFRESH).map(|v| first_item_start(&v, bar_rect.left, bar_rect.bottom - bar_rect.top, limit, exclude))
}

/// The left end of the first item after the run at the taskbar's left end (see left_chain_end)
/// and before `limit`, skipping those over `exclude`; the end of that run when there is none.
pub fn first_item_start(items: &[(i32, i32)], bar_left: i32, bar_h: i32, limit: i32, exclude: &[(i32, i32)]) -> i32 {
    let chain = left_chain_end(items, bar_left, bar_h);
    items
        .iter()
        .filter(|&&(l, r)| l >= chain && r <= limit && r > l && !exclude.iter().any(|&(a, b)| l < b && r > a))
        .map(|&(l, _)| l)
        .min()
        .unwrap_or(chain)
}

/// The right end of the last item ending at or before `limit`, skipping those over `exclude`;
/// `bar_left` when there is none.
pub fn last_item_end(items: &[(i32, i32)], bar_left: i32, limit: i32, exclude: &[(i32, i32)]) -> i32 {
    items
        .iter()
        .filter(|&&(l, r)| r <= limit && r > l && !exclude.iter().any(|&(a, b)| l < b && r > a))
        .map(|&(_, r)| r)
        .max()
        .unwrap_or(bar_left)
}

/// Whether the span `x0..x1` (screen x) of the taskbar `bar` is free of its items (task buttons,
/// Start, search, widgets...): Some(false) when the widget would cover one of them. None until the
/// first answer. Checked again after `max_age`, and after `left_refresh_soon` (a window opened or
/// closed). Horizontal taskbars only.
///
/// `shown`: the widget is on this taskbar now. It is then hidden only on a fresh answer: an
/// older one is checked again first (the widget got wider, or an item was there only for a
/// moment), and the widget stays meanwhile, instead of being hidden until `max_age` is up.
#[allow(clippy::too_many_arguments)]
pub fn room(bar: HWND, bar_rect: RECT, x0: i32, x1: i32, notify: HWND, msg: u32, max_age: Duration, shown: bool) -> Option<bool> {
    let v = items(bar, bar_rect, notify, msg, max_age)?;
    if has_room(&v, x0, x1) {
        return Some(true);
    }
    let blockers: Vec<_> = v.iter().filter(|&&(l, r)| l < x1 && r > x0).collect();
    if shown {
        let key = bar.0 as isize;
        let generation = LEFT_GEN.load(Ordering::SeqCst);
        let checked = LEFT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key)).map(|e| (e.at.elapsed(), e.generation));
        if let Some((age, g)) = checked
            && (age > ROOM_CONFIRM || g != generation)
        {
            if g == generation {
                crate::util::debug_log(|| format!("room {x0}..{x1}: covers {blockers:?} ({}s old), checking again", age.as_secs()));
                left_refresh_soon();
                let _ = items(bar, bar_rect, notify, msg, max_age);
            }
            return Some(true);
        }
    }
    if shown {
        crate::util::debug_log(|| format!("room {x0}..{x1}: no room, covers {blockers:?}"));
    }
    Some(false)
}

/// How recent a "no room" answer must be to hide a widget that is shown (see `room`).
const ROOM_CONFIRM: Duration = Duration::from_secs(3);

/// No item overlaps `x0..x1`.
pub fn has_room(items: &[(i32, i32)], x0: i32, x1: i32) -> bool {
    !items.iter().any(|&(l, r)| l < x1 && r > x0)
}

/// The taskbar's items (left, right) in screen x, from UI Automation on a worker thread (one per
/// taskbar); None until the first answer, then `notify` gets `msg` whenever they change.
fn items(bar: HWND, bar_rect: RECT, notify: HWND, msg: u32, max_age: Duration) -> Option<Vec<(i32, i32)>> {
    let key = bar.0 as isize;
    let r = (bar_rect.left, bar_rect.top, bar_rect.right, bar_rect.bottom);
    let generation = LEFT_GEN.load(Ordering::SeqCst);
    let (known, stale) = {
        let mut g = LEFT.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        match m.get(&key) {
            Some(e) if e.bar == r => (Some(e.items.clone()), e.at.elapsed() >= max_age || e.generation != generation),
            _ => (None, true),
        }
    };
    let start = stale && LEFT_BUSY.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(key);
    if start {
        let notify = notify.0 as isize;
        std::thread::spawn(move || {
            let mut items = probe_items(key).unwrap_or_default();
            items.sort();
            let changed = {
                let mut g = LEFT.lock().unwrap_or_else(|e| e.into_inner());
                let m = g.get_or_insert_with(HashMap::new);
                let old = m.insert(key, LeftEntry { bar: r, items: items.clone(), at: Instant::now(), generation });
                m.retain(|_, e| e.at.elapsed() < Duration::from_secs(600));
                old.is_none_or(|o| o.items != items || o.bar != r)
            };
            if let Some(b) = LEFT_BUSY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                b.remove(&key);
            }
            if changed {
                unsafe {
                    let _ = PostMessageW(Some(HWND(notify as *mut _)), msg, WPARAM(0), LPARAM(0));
                }
            }
        });
    }
    known
}

/// From the taskbar's items (left, right), sorted or not: the end of the run of items that starts
/// at the taskbar's left edge (within one taskbar height of it), gaps up to a quarter of the height
/// allowed; `bar_left` when no item starts there.
pub fn left_chain_end(items: &[(i32, i32)], bar_left: i32, bar_h: i32) -> i32 {
    let mut v: Vec<(i32, i32)> = items.to_vec();
    v.sort();
    let Some(&(first, mut end)) = v.first() else { return bar_left };
    if first - bar_left > bar_h {
        return bar_left;
    }
    for &(l, r) in &v[1..] {
        if l > end + bar_h / 4 {
            break;
        }
        end = end.max(r);
    }
    end
}

/// The taskbar's own items (buttons), as (left, right) in screen x: not the notification area,
/// not containers spanning a large part of the taskbar, and not BatteryHub's own windows.
fn probe_items(bar: isize) -> Option<Vec<(i32, i32)>> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let found = (|| -> windows::core::Result<Vec<(i32, i32)>> {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = uia.ElementFromHandle(HWND(bar as *mut _))?;
            let rb = root.CurrentBoundingRectangle()?;
            let width = rb.right - rb.left;
            let all = root.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition()?)?;
            let mut out = Vec::new();
            for i in 0..all.Length()? {
                let e = all.GetElement(i)?;
                let b = e.CurrentBoundingRectangle()?;
                if b.right <= b.left || b.bottom <= b.top || (b.right - b.left) * 3 > width {
                    continue;
                }
                let id = e.CurrentAutomationId().map(|s| s.to_string()).unwrap_or_default();
                let class = e.CurrentClassName().map(|s| s.to_string()).unwrap_or_default();
                if id == TRAY_ID || class.starts_with("BatteryHub") || matches!(class.as_str(), "TrayNotifyWnd" | "ReBarWindow32" | "MSTaskSwWClass" | "MSTaskListWClass") {
                    continue;
                }
                out.push((b.left, b.right));
            }
            Ok(out)
        })();
        if init {
            CoUninitialize();
        }
        found.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_of_the_icons() {
        // Centred task buttons, the tray from 1700: the widget goes after the last button.
        let items = [(12, 160), (700, 744), (744, 788), (1700, 1740)];
        assert_eq!(last_item_end(&items, 0, 1700, &[]), 788);
        // Another program's widget at 1500..1600 doesn't count.
        assert_eq!(last_item_end(&[(700, 788), (1500, 1600)], 0, 1700, &[(1500, 1600)]), 788);
        assert_eq!(last_item_end(&[], 0, 1700, &[]), 0);
    }

    #[test]
    fn start_of_the_icons() {
        // Widgets button at the left end, centred icons from 700, the tray from 1700.
        let items = [(0, 160), (700, 744), (744, 788), (1700, 1740)];
        assert_eq!(first_item_start(&items, 0, 48, 1700, &[]), 700);
        // Nothing centred: the end of the left run.
        assert_eq!(first_item_start(&[(0, 55), (55, 99)], 0, 48, 1700, &[]), 99);
    }

    #[test]
    fn room_beside_items() {
        let items = [(0, 55), (700, 744), (744, 788)];
        assert!(has_room(&items, 1500, 1600));
        assert!(!has_room(&items, 760, 860));
        assert!(has_room(&items, 788, 900));
    }

    #[test]
    fn left_chain() {
        // Icons aligned left: Start, task view, task buttons; the tray far right is not reached.
        let left = [(0, 55), (55, 99), (99, 143), (143, 187), (1699, 1731)];
        assert_eq!(left_chain_end(&left, 0, 48), 187);
        // Centred icons, nothing at the left end.
        assert_eq!(left_chain_end(&[(700, 744), (744, 788)], 0, 48), 0);
        // Centred icons with the widgets (weather) button at the left end.
        assert_eq!(left_chain_end(&[(700, 744), (12, 160)], 0, 48), 160);
        // A second monitor's taskbar to the left of the main one.
        assert_eq!(left_chain_end(&[(-1920, -1876), (-1876, -1832)], -1920, 48), -1832);
        assert_eq!(left_chain_end(&[], 5, 48), 5);
    }
}
