//! The setup window: a Windows 11 style dialog drawn with BatteryHub's own toolkit
//! (batteryhub::gui). Pages: home (what is installed, options, install / update / reinstall /
//! downgrade, uninstall, other versions), versions (every release on GitHub), uninstall (with the
//! choice to keep settings and history), progress (animated bar, current step, details) and the
//! result. The work runs on a worker thread and reports back with window messages; the window
//! repaints only when something changed and animates with a timer only while something moves.
use std::cell::RefCell;

use batteryhub::gui::{self, Buffer, Face, Fonts, Hits, Theme, Tween, rect};
use batteryhub::install::{self as inst, Edition, Level, Log};
use batteryhub::ui::rgb;
use batteryhub::update::Release;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::setup::{self, Choices, Opts, Plan, State};

const CLASS: PCWSTR = w!("BatteryHub.Setup");
const WM_WORK: u32 = WM_APP + 1;
const TIMER_ANIM: usize = 1;
const W: f32 = 640.0;
const H: f32 = 480.0;

// Hit ids.
const ID_PRIMARY: u32 = 1;
const ID_UNINSTALL: u32 = 2;
const ID_VERSIONS: u32 = 3;
const ID_CLOSE: u32 = 4;
const ID_BACK: u32 = 5;
const ID_DETAILS: u32 = 6;
const ID_OPEN: u32 = 7;
const ID_LOG: u32 = 8;
const ID_DESKTOP: u32 = 10;
const ID_AUTOSTART: u32 = 11;
const ID_LAUNCH: u32 = 12;
const ID_PURGE: u32 = 13;
const ID_CONFIRM_UNINSTALL: u32 = 14;
const ID_RELEASE: u32 = 100; // + index

#[derive(Clone, Copy, PartialEq, Debug)]
enum Page {
    Home,
    Versions,
    Uninstall,
    Progress,
    Done,
}

/// Messages from the worker thread.
enum Work {
    Progress(f32, String),
    Log(Level, String),
    Releases(Result<Vec<Release>, String>),
    Finished(Result<String, String>),
}

struct Ui {
    hwnd: HWND,
    ed: Edition,
    payload: &'static [u8],
    theme: Theme,
    s: f32,
    fonts: Fonts,
    buf: Option<Buffer>,
    /// Snapshot of the previous page while the new one fades in.
    prev: Option<Buffer>,
    page: Page,
    page_t: Tween,
    hits: Hits,
    hover: Option<u32>,
    tracking: bool,
    st: State,
    choices: Choices,
    purge: bool,
    /// Progress page.
    title: String,
    step: String,
    bar: Tween,
    details: bool,
    lines: Vec<(Level, String)>,
    busy: bool,
    /// Result page.
    ok: bool,
    message: String,
    check: Tween,
    /// What the finished operation was (an uninstall shows no "Open" button).
    uninstalled: bool,
    releases: Option<Result<Vec<Release>, String>>,
    /// First release row shown (the list scrolls with the mouse wheel).
    scroll: usize,
    started: std::time::Instant,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut u| u.as_mut().map(f)))
}

fn post(hwnd: usize, w: Work) {
    let p = Box::into_raw(Box::new(w));
    unsafe {
        if PostMessageW(Some(HWND(hwnd as *mut _)), WM_WORK, WPARAM(0), LPARAM(p as isize)).is_err() {
            drop(Box::from_raw(p));
        }
    }
}

impl Ui {
    fn px(&self, v: f32) -> i32 {
        (v * self.s).round() as i32
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn animate(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_ANIM, 15, None);
        }
        self.invalidate();
    }

    fn animating(&self) -> bool {
        self.page_t.running() || self.bar.running() || self.check.running() || (self.page == Page::Progress && self.busy)
    }

    fn go(&mut self, page: Page) {
        if page == self.page {
            return;
        }
        // Keep what is on screen now; the new page fades and slides in over it.
        self.prev = self.buf.take();
        self.page = page;
        self.page_t = Tween::new(0.0);
        self.page_t.go(1.0, 240.0);
        self.hover = None;
        self.animate();
    }

    fn version_label(&self) -> String {
        inst::version_label()
    }

    // ---- actions -------------------------------------------------------------------------------

    fn log_sink(&self) -> inst::LogSink {
        let h = self.hwnd.0 as usize;
        Box::new(move |level, msg| post(h, Work::Log(level, msg.to_string())))
    }

    fn start_install(&mut self, release: Option<Release>) {
        let (ed, payload, st, c, h) = (self.ed, self.payload, self.st.clone(), self.choices, self.hwnd.0 as usize);
        self.title = match (&release, setup::plan(&st)) {
            (Some(r), _) => format!("Installing BatteryHub v{}", inst::label_of(&r.version).trim_start_matches('v')),
            (None, Plan::Install) => format!("Installing BatteryHub {}", self.version_label()),
            (None, Plan::Update(_)) => format!("Updating to {}", self.version_label()),
            (None, Plan::Reinstall) => format!("Reinstalling {}", self.version_label()),
            (None, Plan::Downgrade(_)) => format!("Going back to {}", self.version_label()),
        };
        self.begin_work();
        let sink = self.log_sink();
        std::thread::spawn(move || {
            let log = Log::new(&format!("{} {} - Setup", ed.name, inst::version_label()), Some(sink));
            let progress = |f: f32, t: &str| post(h, Work::Progress(f, t.to_string()));
            let r = match &release {
                Some(r) => setup::install_release(&log, r, &c, &progress).map(|_| format!("BatteryHub v{} is installed.", r.version)),
                None => setup::install(&ed, payload, &log, &st, &c, &progress).map(|_| format!("BatteryHub {} is installed.", inst::version_label())),
            };
            post(h, Work::Finished(r));
        });
    }

    fn start_uninstall(&mut self) {
        let (ed, st, purge, h) = (self.ed, self.st.clone(), self.purge, self.hwnd.0 as usize);
        self.title = "Uninstalling BatteryHub".into();
        self.uninstalled = true;
        self.begin_work();
        let sink = self.log_sink();
        std::thread::spawn(move || {
            let log = Log::new(&format!("{} {} - Uninstall", ed.name, inst::version_label()), Some(sink));
            let progress = |f: f32, t: &str| post(h, Work::Progress(f, t.to_string()));
            let r = setup::uninstall(&ed, &log, &st, purge, &progress).map(|_| {
                if purge {
                    "BatteryHub was removed, with its settings and battery history.".to_string()
                } else {
                    "BatteryHub was removed. Your settings and battery history were kept for a later install.".to_string()
                }
            });
            post(h, Work::Finished(r));
        });
    }

    fn begin_work(&mut self) {
        self.busy = true;
        self.step = "Starting".into();
        self.lines.clear();
        self.bar = Tween::new(0.0);
        self.started = std::time::Instant::now();
        self.go(Page::Progress);
    }

    fn load_releases(&mut self) {
        self.releases = None;
        let h = self.hwnd.0 as usize;
        std::thread::spawn(move || post(h, Work::Releases(batteryhub::update::list_releases())));
        self.go(Page::Versions);
    }

    fn on_work(&mut self, w: Work) {
        match w {
            Work::Progress(f, t) => {
                self.bar.go(f, 300.0);
                self.step = t;
            }
            Work::Log(level, msg) => {
                if !matches!(level, Level::Detail) {
                    self.lines.push((level, msg));
                    let n = self.lines.len();
                    if n > 200 {
                        self.lines.drain(..n - 200);
                    }
                }
            }
            Work::Releases(r) => self.releases = Some(r),
            Work::Finished(r) => {
                self.busy = false;
                self.bar.go(1.0, 200.0);
                self.st = setup::read_state(&self.ed);
                match r {
                    Ok(m) => {
                        self.ok = true;
                        self.message = m;
                    }
                    Err(e) => {
                        self.ok = false;
                        self.message = e;
                    }
                }
                self.check = Tween::new(0.0);
                self.check.go(1.0, 500.0);
                self.go(Page::Done);
            }
        }
        self.animate();
    }

    fn click(&mut self, id: u32) {
        match (self.page, id) {
            (_, ID_CLOSE) => unsafe {
                if !self.busy {
                    let _ = DestroyWindow(self.hwnd);
                }
            },
            (Page::Home, ID_PRIMARY) => self.start_install(None),
            (Page::Home, ID_UNINSTALL) => self.go(Page::Uninstall),
            (Page::Home, ID_VERSIONS) => self.load_releases(),
            (Page::Home, ID_DESKTOP) => self.choices.desktop = !self.choices.desktop,
            (Page::Home, ID_AUTOSTART) => self.choices.autostart = !self.choices.autostart,
            (Page::Home, ID_LAUNCH) => self.choices.launch = !self.choices.launch,
            (Page::Versions | Page::Uninstall, ID_BACK) => self.go(Page::Home),
            (Page::Versions, id) if id >= ID_RELEASE => {
                if let Some(Ok(list)) = &self.releases
                    && let Some(r) = list.get((id - ID_RELEASE) as usize).cloned()
                {
                    self.start_install(Some(r));
                }
            }
            (Page::Uninstall, ID_PURGE) => self.purge = !self.purge,
            (Page::Uninstall, ID_CONFIRM_UNINSTALL) => self.start_uninstall(),
            (Page::Progress | Page::Done, ID_DETAILS) => self.details = !self.details,
            (Page::Done, ID_OPEN) => {
                let exe = self.st.exe.clone();
                if exe.exists() {
                    let _ = std::process::Command::new(&exe).current_dir(exe.parent().unwrap_or(std::path::Path::new("."))).spawn();
                }
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            (Page::Done, ID_LOG) => unsafe {
                ShellExecuteW(None, w!("open"), &HSTRING::from(Log::path().as_os_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            },
            _ => {}
        }
        self.invalidate();
    }

    // ---- painting ------------------------------------------------------------------------------

    fn paint(&mut self, hdc: HDC) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let (w, h) = (rc.right, rc.bottom);
        if self.buf.as_ref().is_none_or(|b| b.w != w || b.h != h) {
            self.buf = Buffer::new(w, h);
        }
        let Some(mut b) = self.buf.take() else { return };
        self.hits.clear();
        let t = self.theme;
        b.clear(t.bg);
        let e = gui::ease_out(self.page_t.value());
        // The page slides up 12 px while it fades in.
        let dy = ((1.0 - e) * 12.0 * self.s) as i32;
        match self.page {
            Page::Home => self.home(&mut b, w, h, dy),
            Page::Versions => self.versions(&mut b, w, h, dy),
            Page::Uninstall => self.uninstall_page(&mut b, w, h, dy),
            Page::Progress => self.progress(&mut b, w, h, dy),
            Page::Done => self.done(&mut b, w, h, dy),
        }
        // Cross-fade from the previous page.
        if let Some(prev) = self.prev.as_mut() {
            if self.page_t.running() && prev.w == w && prev.h == h {
                let a = ((1.0 - e) * 255.0) as u32;
                for (d, &o) in b.px().iter_mut().zip(prev.px().iter()) {
                    let ch = |sh: u32| ((((o >> sh) & 0xFF) * a + ((*d >> sh) & 0xFF) * (255 - a)) / 255) << sh;
                    *d = 0xFF00_0000 | ch(16) | ch(8) | ch(0);
                }
            } else {
                self.prev = None;
            }
        }
        b.blit(hdc);
        self.buf = Some(b);
    }

    /// Big round icon with a glyph, the title and a line under it.
    #[allow(clippy::too_many_arguments)]
    fn header(&mut self, b: &mut Buffer, w: i32, top: i32, glyph: char, color: windows::Win32::Foundation::COLORREF, title: &str, sub: &str) -> i32 {
        let t = self.theme;
        let p = self.px(32.0);
        let r = self.px(24.0) as f32;
        b.circle(p as f32 + r, top as f32 + r, r, gui::mix(t.bg, color, 0.2), 1.0);
        let gf = self.fonts.get(self.px(22.0), 400, Face::Icons);
        b.text(&glyph.to_string(), gf, color, rect(p, top, 2 * r as i32, 2 * r as i32), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        let x = p + 2 * r as i32 + self.px(16.0);
        let tf = self.fonts.get(self.px(24.0), 600, Face::Display);
        b.text(title, tf, t.text, rect(x, top - self.px(1.0), w - x - p, self.px(32.0)), DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
        let sf = self.fonts.get(self.px(13.0), 400, Face::Text);
        b.text(sub, sf, t.dim, rect(x, top + self.px(32.0), w - x - p, self.px(20.0)), DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
        top + 2 * r as i32 + self.px(20.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn checkbox(&mut self, b: &mut Buffer, x: i32, y: i32, w: i32, label: &str, on: bool, id: u32) {
        let t = self.theme;
        let sz = self.px(20.0) as f32;
        let hover = self.hover == Some(id);
        if on {
            b.rrect(x as f32, y as f32, sz, sz, 4.0 * self.s, if hover { gui::mix(t.accent, rgb(255, 255, 255), 0.1) } else { t.accent }, 1.0);
            let gf = self.fonts.get(self.px(12.0), 400, Face::Icons);
            b.text("\u{E73E}", gf, t.on_accent, rect(x, y, sz as i32, sz as i32), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        } else {
            b.card(x as f32, y as f32, sz, sz, 4.0 * self.s, if hover { t.card_hover } else { t.card }, if hover { t.dim } else { t.faint });
        }
        let f = self.fonts.get(self.px(14.0), 400, Face::Text);
        b.text(label, f, t.text, rect(x + sz as i32 + self.px(10.0), y, w, sz as i32), DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        self.hits.add(rect(x - self.px(4.0), y - self.px(4.0), w + sz as i32 + self.px(14.0), sz as i32 + self.px(8.0)), id);
    }

    fn btn(&mut self, b: &mut Buffer, r: RECT, label: &str, primary: bool, id: u32, enabled: bool) {
        let hover = (self.hover == Some(id)) as u8 as f32;
        gui::button(b, &mut self.fonts, &self.theme, self.s, r, label, primary, hover, enabled);
        if enabled {
            self.hits.add(r, id);
        }
    }

    fn link(&mut self, b: &mut Buffer, x: i32, y: i32, label: &str, id: u32) -> i32 {
        let t = self.theme;
        let f = self.fonts.get(self.px(13.0), 400, Face::Text);
        let lw = b.measure(label, f).cx + self.px(16.0);
        let r = rect(x, y, lw, self.px(32.0));
        if self.hover == Some(id) {
            b.rrect(r.left as f32, r.top as f32, lw as f32, self.px(32.0) as f32, 4.0 * self.s, t.button_hover, 1.0);
        }
        b.text(label, f, t.accent, r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        self.hits.add(r, id);
        lw
    }

    fn footer_y(&self, h: i32) -> i32 {
        h - self.px(32.0) - self.px(24.0)
    }

    fn home(&mut self, b: &mut Buffer, w: i32, h: i32, dy: i32) {
        let t = self.theme;
        let p = self.px(32.0);
        let st = self.st.clone();
        let plan = setup::plan(&st);
        let mut y = self.header(b, w, self.px(28.0) + dy, '\u{E83F}', t.accent, &format!("BatteryHub {}", self.version_label()), self.ed.description);
        // What is installed now.
        let card_h = self.px(96.0);
        b.card(p as f32, y as f32, (w - 2 * p) as f32, card_h as f32, 8.0 * self.s, t.card, t.stroke);
        let lf = self.fonts.get(self.px(12.0), 400, Face::Text);
        let vf = self.fonts.get(self.px(14.0), 600, Face::Text);
        let nf = self.fonts.get(self.px(13.0), 400, Face::Text);
        let ip = self.px(16.0);
        let (status, status_c) = match &plan {
            Plan::Install => ("Not installed yet".to_string(), t.dim),
            Plan::Update(v) => (format!("v{v} installed - an update to {} is ready", self.version_label()), t.green),
            Plan::Reinstall => (format!("{} installed - this is the same version", self.version_label()), t.text),
            Plan::Downgrade(v) => (format!("v{v} installed - newer than this setup ({})", self.version_label()), t.orange),
        };
        b.text("On this PC", lf, t.faint, rect(p + ip, y + ip - self.px(2.0), w, self.px(18.0)), DT_LEFT | DT_SINGLELINE);
        b.text(&status, vf, status_c, rect(p + ip, y + ip + self.px(16.0), w - 2 * p - 2 * ip, self.px(22.0)), DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
        let mut where_ = if st.installed { st.dir.display().to_string() } else { format!("Installs to {}", self.ed.default_dir().display()) };
        if st.running {
            where_ += "  ·  running now (it is restarted)";
        }
        b.text(&where_, nf, t.dim, rect(p + ip, y + ip + self.px(40.0), w - 2 * p - 2 * ip, self.px(20.0)), DT_LEFT | DT_SINGLELINE | DT_PATH_ELLIPSIS);
        if let Some(o) = &st.original {
            let note = format!("The original BatteryHub ({}) is separate and stays as it is.", o.display());
            b.text(&note, lf, t.faint, rect(p + ip, y + ip + self.px(60.0), w - 2 * p - 2 * ip, self.px(18.0)), DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
        }
        y += card_h + self.px(20.0);
        // Options.
        let cw = w - 2 * p - self.px(40.0);
        let c = self.choices;
        self.checkbox(b, p + self.px(4.0), y, cw, "Desktop shortcut (a Start menu shortcut is always made)", c.desktop, ID_DESKTOP);
        y += self.px(32.0);
        self.checkbox(b, p + self.px(4.0), y, cw, "Start automatically when I sign in to Windows", c.autostart, ID_AUTOSTART);
        y += self.px(32.0);
        self.checkbox(b, p + self.px(4.0), y, cw, "Start BatteryHub when setup is done", c.launch, ID_LAUNCH);
        let _ = y;
        let _ = h;
        // Footer: other versions and uninstall on the left, the main action on the right.
        let fy = self.footer_y(h) + dy;
        let mut x = p - self.px(8.0);
        x += self.link(b, x, fy, "Other versions...", ID_VERSIONS);
        if st.installed {
            self.link(b, x, fy, "Uninstall...", ID_UNINSTALL);
        }
        let label = match &plan {
            Plan::Install => "Install".to_string(),
            Plan::Update(_) => format!("Update to {}", self.version_label()),
            Plan::Reinstall => "Reinstall".to_string(),
            Plan::Downgrade(_) => format!("Go back to {}", self.version_label()),
        };
        let bw = self.px(150.0).max(b.measure(&label, self.fonts.get(self.px(14.0), 400, Face::Text)).cx + self.px(32.0));
        let close_w = self.px(96.0);
        self.btn(b, rect(w - p - bw, fy, bw, self.px(32.0)), &label, true, ID_PRIMARY, true);
        self.btn(b, rect(w - p - bw - self.px(8.0) - close_w, fy, close_w, self.px(32.0)), "Close", false, ID_CLOSE, true);
    }

    fn versions(&mut self, b: &mut Buffer, w: i32, h: i32, dy: i32) {
        let t = self.theme;
        let p = self.px(32.0);
        let installed = self.st.version.clone();
        let y = self.header(
            b,
            w,
            self.px(28.0) + dy,
            '\u{E81C}',
            t.accent,
            "Other versions",
            "Install any release, also an older one. Settings and battery history are kept.",
        );
        let list_h = self.footer_y(h) - y - self.px(16.0);
        b.card(p as f32, y as f32, (w - 2 * p) as f32, list_h as f32, 8.0 * self.s, t.card, t.stroke);
        let f = self.fonts.get(self.px(14.0), 600, Face::Text);
        let nf = self.fonts.get(self.px(13.0), 400, Face::Text);
        match &self.releases.clone() {
            None => {
                // Loading: three dots pulsing.
                let ph = self.started.elapsed().as_secs_f32();
                for i in 0..3 {
                    let a = 0.35 + 0.65 * (((ph * 3.0 - i as f32 * 0.6).sin() + 1.0) / 2.0);
                    b.circle((w / 2 + (i - 1) * self.px(14.0)) as f32, (y + list_h / 2) as f32, 3.5 * self.s, t.accent, a);
                }
                self.started_anim();
            }
            Some(Err(e)) => {
                b.text(
                    &format!("Could not load the list from GitHub:\n{e}"),
                    nf,
                    t.red,
                    rect(p + self.px(16.0), y + self.px(16.0), w - 2 * p - self.px(32.0), list_h - self.px(32.0)),
                    DT_LEFT | DT_WORDBREAK,
                );
            }
            Some(Ok(list)) => {
                let row_h = self.px(44.0);
                let rows = ((list_h - self.px(12.0)) / row_h).max(1) as usize;
                self.scroll = self.scroll.min(list.len().saturating_sub(rows));
                let mut yy = y + self.px(6.0);
                for (i, r) in list.iter().enumerate().skip(self.scroll) {
                    if yy + row_h > y + list_h - self.px(4.0) {
                        break;
                    }
                    let id = ID_RELEASE + i as u32;
                    let rr = rect(p + self.px(6.0), yy, w - 2 * p - self.px(12.0), row_h);
                    if self.hover == Some(id) {
                        b.rrect(rr.left as f32, rr.top as f32, (rr.right - rr.left) as f32, row_h as f32, 4.0 * self.s, t.card_hover, 1.0);
                    }
                    let label = inst::label_of(&r.version);
                    b.text(&label, f, t.text, rect(rr.left + self.px(12.0), yy, self.px(90.0), row_h), DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                    let cmp = installed.as_deref().map(|v| inst::version_cmp(&r.version, v));
                    let (tag, c) = match (i, cmp) {
                        (_, Some(std::cmp::Ordering::Equal)) => ("installed now", t.green),
                        (0, _) => ("latest", t.accent),
                        (_, Some(std::cmp::Ordering::Less)) => ("older - goes back to this version", t.dim),
                        (_, Some(std::cmp::Ordering::Greater)) => ("newer", t.dim),
                        _ => ("", t.dim),
                    };
                    let tag = if r.prerelease { format!("{tag} (pre-release)") } else { tag.to_string() };
                    b.text(&tag, nf, c, rect(rr.left + self.px(100.0), yy, self.px(300.0), row_h), DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                    b.text("Install", nf, t.accent, rect(rr.right - self.px(90.0), yy, self.px(78.0), row_h), DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
                    self.hits.add(rr, id);
                    yy += row_h;
                }
                if list.len() > rows {
                    // Scroll bar.
                    let track_h = (list_h - self.px(16.0)) as f32;
                    let thumb = (track_h * rows as f32 / list.len() as f32).max(self.px(24.0) as f32);
                    let top = y as f32 + self.px(8.0) as f32 + (track_h - thumb) * self.scroll as f32 / (list.len() - rows) as f32;
                    b.rrect((w - p - self.px(8.0)) as f32, top, 3.0 * self.s, thumb, 1.5 * self.s, t.faint, 0.8);
                }
                if list.is_empty() {
                    b.text("No releases found.", nf, t.dim, rect(p + self.px(16.0), y + self.px(16.0), w, self.px(20.0)), DT_LEFT | DT_SINGLELINE);
                }
            }
        }
        let fy = self.footer_y(h) + dy;
        self.btn(b, rect(w - p - self.px(96.0), fy, self.px(96.0), self.px(32.0)), "Back", false, ID_BACK, true);
    }

    fn started_anim(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_ANIM, 30, None);
        }
    }

    fn uninstall_page(&mut self, b: &mut Buffer, w: i32, h: i32, dy: i32) {
        let t = self.theme;
        let p = self.px(32.0);
        let mut y = self.header(
            b,
            w,
            self.px(28.0) + dy,
            '\u{E74D}',
            t.red,
            "Uninstall BatteryHub?",
            &format!("{} {}", self.ed.name, self.st.version.as_deref().map(inst::label_of).unwrap_or_default()),
        );
        let nf = self.fonts.get(self.px(14.0), 400, Face::Text);
        let text =
            "The program, its shortcuts, its autostart entry and its entry in Settings > Apps are removed. If BatteryHub is running, it is closed first.";
        b.text(text, nf, t.dim, rect(p, y, w - 2 * p, self.px(44.0)), DT_LEFT | DT_WORDBREAK);
        y += self.px(60.0);
        let purge = self.purge;
        self.checkbox(b, p + self.px(4.0), y, w - 2 * p - self.px(40.0), "Also remove my settings and battery history", purge, ID_PURGE);
        y += self.px(30.0);
        let lf = self.fonts.get(self.px(12.0), 400, Face::Text);
        let note = if purge {
            "Warning levels, device names, options and the 7-day history are deleted for good."
        } else {
            "They stay, so a later install picks them up again (a few kilobytes)."
        };
        b.text(note, lf, if purge { t.orange } else { t.faint }, rect(p + self.px(34.0), y, w - 2 * p - self.px(40.0), self.px(20.0)), DT_LEFT | DT_SINGLELINE);
        let fy = self.footer_y(h) + dy;
        let bw = self.px(120.0);
        self.btn(b, rect(w - p - bw, fy, bw, self.px(32.0)), "Uninstall", true, ID_CONFIRM_UNINSTALL, true);
        self.btn(b, rect(w - p - bw - self.px(8.0) - self.px(96.0), fy, self.px(96.0), self.px(32.0)), "Cancel", false, ID_BACK, true);
    }

    fn progress(&mut self, b: &mut Buffer, w: i32, h: i32, dy: i32) {
        let t = self.theme;
        let p = self.px(32.0);
        let title = self.title.clone();
        let mut y = self.header(b, w, self.px(28.0) + dy, '\u{E896}', t.accent, &title, "Please wait - this takes a few seconds.");
        y += self.px(16.0);
        // The bar: track, the done part, and a soft light moving along it while work runs.
        let (bx, bw, bh) = (p as f32, (w - 2 * p) as f32, 4.0 * self.s);
        b.rrect(bx, y as f32, bw, bh, bh / 2.0, t.track, 1.0);
        let v = self.bar.value().clamp(0.0, 1.0);
        if v > 0.0 {
            b.rrect(bx, y as f32, (bw * v).max(bh), bh, bh / 2.0, t.accent, 1.0);
        }
        if self.busy {
            let ph = (self.started.elapsed().as_secs_f32() * 0.8).fract();
            let gx = bx + bw * v * ph;
            b.rrect(gx, y as f32, (bw * 0.08).min(bw * v), bh, bh / 2.0, rgb(255, 255, 255), 0.35);
        }
        y += self.px(16.0);
        let sf = self.fonts.get(self.px(14.0), 400, Face::Text);
        let pct = format!("{}%", (v * 100.0).round() as i32);
        let step = self.step.clone();
        b.text(&step, sf, t.text, rect(p, y, w - 2 * p - self.px(60.0), self.px(22.0)), DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
        b.text(&pct, sf, t.dim, rect(w - p - self.px(60.0), y, self.px(60.0), self.px(22.0)), DT_RIGHT | DT_SINGLELINE);
        y += self.px(36.0);
        self.details_box(b, p, y, w, self.footer_y(h) - self.px(12.0));
        let fy = self.footer_y(h) + dy;
        let label = if self.details { "Hide details" } else { "Show details" };
        self.link(b, p - self.px(8.0), fy, label, ID_DETAILS);
    }

    fn details_box(&mut self, b: &mut Buffer, p: i32, y: i32, w: i32, bottom: i32) {
        if !self.details || bottom - y < self.px(40.0) {
            return;
        }
        let t = self.theme;
        b.card(p as f32, y as f32, (w - 2 * p) as f32, (bottom - y) as f32, 6.0 * self.s, t.card, t.stroke);
        let f = self.fonts.get(self.px(12.0), 400, Face::Text);
        let lh = self.px(18.0);
        let n = ((bottom - y - self.px(16.0)) / lh).max(1) as usize;
        let start = self.lines.len().saturating_sub(n);
        let mut yy = y + self.px(8.0);
        for (level, l) in self.lines[start..].iter() {
            let c = match level {
                Level::Error => t.red,
                Level::Warn => t.orange,
                Level::Ok => t.green,
                Level::Step => t.text,
                _ => t.dim,
            };
            b.text(l, f, c, rect(p + self.px(12.0), yy, w - 2 * p - self.px(24.0), lh), DT_LEFT | DT_SINGLELINE | DT_PATH_ELLIPSIS);
            yy += lh;
        }
    }

    fn done(&mut self, b: &mut Buffer, w: i32, h: i32, dy: i32) {
        let t = self.theme;
        let p = self.px(32.0);
        let (glyph, c, title) = if self.ok { ('\u{E73E}', t.green, "All done") } else { ('\u{E711}', t.red, "Something went wrong") };
        // The result icon grows in with a slight overshoot.
        let k = self.check.value();
        let pop = if k < 1.0 { 1.0 + 0.12 * (k * std::f32::consts::PI).sin() } else { 1.0 };
        let r = 34.0 * self.s * k.min(1.0) * pop;
        let (cx, cy) = (w as f32 / 2.0, (self.px(96.0) + dy) as f32);
        if r > 1.0 {
            b.circle(cx, cy, r, c, 1.0);
            let gf = self.fonts.get((26.0 * self.s * k.min(1.0)).max(1.0) as i32, 400, Face::Icons);
            b.text(&glyph.to_string(), gf, t.bg, rect(cx as i32 - 40, cy as i32 - 40, 80, 80), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        let tf = self.fonts.get(self.px(24.0), 600, Face::Display);
        b.text(title, tf, t.text, rect(p, self.px(146.0) + dy, w - 2 * p, self.px(34.0)), DT_CENTER | DT_SINGLELINE);
        let nf = self.fonts.get(self.px(14.0), 400, Face::Text);
        let msg = self.message.clone();
        b.text(
            &msg,
            nf,
            if self.ok { t.dim } else { t.text },
            rect(p + self.px(20.0), self.px(186.0) + dy, w - 2 * p - self.px(40.0), self.px(80.0)),
            DT_CENTER | DT_WORDBREAK,
        );
        self.details_box(b, p, self.px(270.0), w, self.footer_y(h) - self.px(12.0));
        let fy = self.footer_y(h) + dy;
        let mut x = p - self.px(8.0);
        x += self.link(b, x, fy, if self.details { "Hide details" } else { "Show details" }, ID_DETAILS);
        self.link(b, x, fy, "Open log file", ID_LOG);
        let bw = self.px(150.0);
        if self.ok && !self.uninstalled && self.st.installed {
            self.btn(b, rect(w - p - bw, fy, bw, self.px(32.0)), "Open BatteryHub", true, ID_OPEN, true);
            self.btn(b, rect(w - p - bw - self.px(8.0) - self.px(96.0), fy, self.px(96.0), self.px(32.0)), "Close", false, ID_CLOSE, true);
        } else {
            self.btn(b, rect(w - p - self.px(96.0), fy, self.px(96.0), self.px(32.0)), "Close", !self.ok || self.uninstalled, ID_CLOSE, true);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                with(|u| u.paint(hdc));
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER if wp.0 == TIMER_ANIM => {
                let more = with(|u| u.animating() || (u.page == Page::Versions && u.releases.is_none())).unwrap_or(false);
                let _ = InvalidateRect(Some(hwnd), None, false);
                if !more {
                    let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                }
                LRESULT(0)
            }
            WM_WORK => {
                let w = *Box::from_raw(lp.0 as *mut Work);
                with(|u| u.on_work(w));
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                with(|u| {
                    if !u.tracking {
                        let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        u.tracking = TrackMouseEvent(&mut tme).is_ok();
                    }
                    let h = u.hits.at(x, y);
                    if h != u.hover {
                        u.hover = h;
                        u.invalidate();
                    }
                    SetCursor(LoadCursorW(None, if h.is_some() { IDC_HAND } else { IDC_ARROW }).ok());
                });
                LRESULT(0)
            }
            0x02A3 /* WM_MOUSELEAVE */ => {
                with(|u| {
                    u.tracking = false;
                    if u.hover.take().is_some() {
                        u.invalidate();
                    }
                });
                LRESULT(0)
            }
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => LRESULT(1),
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                with(|u| {
                    if u.page == Page::Versions {
                        u.scroll = if delta > 0 { u.scroll.saturating_sub(1) } else { u.scroll + 1 };
                        u.invalidate();
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                if let Some(id) = with(|u| u.hits.at(x, y)).flatten() {
                    with(|u| u.click(id));
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if wp.0 == VK_ESCAPE.0 as usize {
                    with(|u| match u.page {
                        Page::Versions | Page::Uninstall => u.go(Page::Home),
                        _ => u.click(ID_CLOSE),
                    });
                } else if wp.0 == VK_RETURN.0 as usize {
                    with(|u| match u.page {
                        Page::Home => u.click(ID_PRIMARY),
                        Page::Done => u.click(ID_CLOSE),
                        _ => {}
                    });
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // Not while files are being written.
                if !with(|u| u.busy).unwrap_or(false) {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                with(|u| {
                    u.s = (wp.0 & 0xFFFF) as f32 / 96.0;
                    u.fonts.clear();
                    u.prev = None;
                });
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                with(|u| {
                    u.theme = Theme::current();
                    gui::style_frame(hwnd, &u.theme);
                    u.invalidate();
                });
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Runs the setup window. Returns the process exit code (0 = done or closed, 1 = failed).
pub fn run(ed: Edition, payload: &'static [u8], o: &Opts) -> i32 {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(std::ptr::dangling::<u16>())).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(mon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut wr = RECT { left: 0, top: 0, right: (W * s) as i32, bottom: (H * s) as i32 };
        let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut wr, style, false, WINDOW_EX_STYLE::default(), dx);
        let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
        let work = mi.rcWork;
        let (x, y) = (work.left + (work.right - work.left - w) / 2, work.top + (work.bottom - work.top - h) / 2);
        let title = HSTRING::from(format!("{} {} - Setup", ed.name, inst::version_label()));
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE::default(), CLASS, &title, style, x, y, w, h, None, None, Some(hinst.into()), None) else {
            return 1;
        };
        let theme = Theme::current();
        gui::style_frame(hwnd, &theme);
        let st = setup::read_state(&ed);
        let choices = setup::default_choices(&st, o);
        let ui = Ui {
            hwnd,
            ed,
            payload,
            theme,
            s: GetDpiForWindow(hwnd).max(96) as f32 / 96.0,
            fonts: Fonts::default(),
            buf: None,
            prev: None,
            page: Page::Home,
            page_t: Tween::new(1.0),
            hits: Hits::default(),
            hover: None,
            tracking: false,
            st,
            choices,
            purge: o.purge,
            title: String::new(),
            step: String::new(),
            bar: Tween::new(0.0),
            details: false,
            lines: Vec::new(),
            busy: false,
            ok: false,
            message: String::new(),
            check: Tween::new(0.0),
            uninstalled: false,
            releases: None,
            scroll: 0,
            started: std::time::Instant::now(),
        };
        UI.with(|c| *c.borrow_mut() = Some(ui));
        // Started with an action (from Settings > Apps, or by the program's own updater): go
        // straight to it.
        with(|u| match o.action {
            Some(setup::Action::Uninstall) if u.st.installed => {
                u.page = Page::Uninstall;
            }
            Some(setup::Action::Install | setup::Action::Update) => u.start_install(None),
            _ if o.versions => u.load_releases(),
            _ => {}
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        // The page fades in once at the start too.
        with(|u| {
            u.page_t = Tween::new(0.0);
            u.page_t.go(1.0, 300.0);
            u.animate();
        });
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let failed = UI.with(|c| c.borrow().as_ref().is_some_and(|u| u.page == Page::Done && !u.ok));
        if failed { 1 } else { 0 }
    }
}
