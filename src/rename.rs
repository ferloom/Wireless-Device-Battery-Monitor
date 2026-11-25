//! "Rename device": a small dialog that stores a name of the user's for one device (settings
//! `Names`). Only BatteryHub shows it; the device and Windows keep the device's own name. The name
//! belongs to the device key, so it stays after a reconnect or a restart.
//!
//! Runs as its own process (`batteryhub-taskbar.exe --rename <device>`), like the dashboard, and
//! tells the taskbar program (registered message "BatteryHub.NamesChanged") when it saved.
use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::SetWindowTheme;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::device::DeviceStatus;
use crate::gui::{self, Buffer, Face, Fonts, Hits, Theme, rect};
use crate::i18n::{tr, trf};
use crate::settings::{self, NAME_MAX};

#[cfg(feature = "experimental-xaml")]
const CLASS: PCWSTR = w!("BatteryHub.Experimental.Rename");
#[cfg(not(feature = "experimental-xaml"))]
const CLASS: PCWSTR = w!("BatteryHub.Rename");
const SETTINGS_KEY: &str = crate::install::TASKBAR.settings_key;
const ID_EDIT: i32 = 100;
const ID_SAVE: u32 = 1;
const ID_CANCEL: u32 = 2;
const ID_RESET: u32 = 3;
const EM_SETSEL: u32 = 0xB1;
const EM_LIMITTEXT: u32 = 0xC5;

/// Message the taskbar program registers to hear about renamed devices.
pub fn names_changed_message() -> u32 {
    #[cfg(feature = "experimental-xaml")]
    let name = w!("BatteryHub.Experimental.NamesChanged");
    #[cfg(not(feature = "experimental-xaml"))]
    let name = w!("BatteryHub.NamesChanged");
    unsafe { RegisterWindowMessageW(name) }
}

/// Starts the dialog for `s` (one at a time: an open one comes to the front).
pub fn open(s: &DeviceStatus, reported: &str) {
    unsafe {
        if let Ok(h) = FindWindowW(CLASS, None)
            && !h.0.is_null()
        {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let _ = AllowSetForegroundWindow(pid);
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--rename").arg(crate::dashboard::encode_status(s, reported)).spawn();
    }
}

struct Dlg {
    hwnd: HWND,
    edit: HWND,
    theme: Theme,
    s: f32,
    fonts: Fonts,
    buf: Option<Buffer>,
    hits: Hits,
    hover: Option<u32>,
    status: DeviceStatus,
    reported: String,
    renamed: bool,
    edit_brush: HBRUSH,
}

thread_local! {
    static DLG: RefCell<Option<Dlg>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Dlg) -> R) -> Option<R> {
    DLG.with(|d| d.try_borrow_mut().ok().and_then(|mut d| d.as_mut().map(f)))
}

impl Dlg {
    fn px(&self, v: f32) -> i32 {
        (v * self.s).round() as i32
    }

    fn layout(&mut self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
            let p = self.px(24.0);
            let _ = MoveWindow(self.edit, p + self.px(10.0), self.px(104.0) + self.px(7.0), rc.right - 2 * p - self.px(20.0), self.px(20.0), true);
            let f = self.fonts.get(self.px(14.0), 400, Face::Text);
            SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
        }
    }

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
        let t = self.theme;
        self.hits.clear();
        b.clear(t.bg);
        let p = self.px(24.0);
        let tf = self.fonts.get(self.px(20.0), 600, Face::Display);
        b.text(&tr("Rename device"), tf, t.text, rect(p, self.px(18.0), w - 2 * p, self.px(30.0)), DT_LEFT | DT_SINGLELINE);
        let nf = self.fonts.get(self.px(13.0), 400, Face::Text);
        let own = if self.reported.is_empty() { self.status.name.clone() } else { self.reported.clone() };
        b.text(
            &trf("Shown on the taskbar and in BatteryHub. The device itself keeps its name (\"{name}\").", &[("name", &own)]),
            nf,
            t.dim,
            rect(p, self.px(54.0), w - 2 * p, self.px(40.0)),
            DT_LEFT | DT_WORDBREAK,
        );
        // Text box frame (the native edit control sits inside it), accent line at the bottom.
        let (bx, by, bw, bh) = (p as f32, self.px(104.0) as f32, (w - 2 * p) as f32, self.px(34.0) as f32);
        b.card(bx, by, bw, bh, 4.0 * self.s, t.card, t.stroke);
        b.rrect(bx + 1.0, by + bh - 2.0 * self.s, bw - 2.0, 2.0 * self.s, 1.0 * self.s, t.accent, 1.0);
        // Buttons.
        let bw2 = self.px(104.0);
        let bh2 = self.px(32.0);
        let y = h - p - bh2;
        let save = rect(w - p - 2 * bw2 - self.px(8.0), y, bw2, bh2);
        let cancel = rect(w - p - bw2, y, bw2, bh2);
        gui::button(&mut b, &mut self.fonts, &t, self.s, save, &tr("Save"), true, (self.hover == Some(ID_SAVE)) as u8 as f32, true);
        gui::button(&mut b, &mut self.fonts, &t, self.s, cancel, &tr("Cancel"), false, (self.hover == Some(ID_CANCEL)) as u8 as f32, true);
        self.hits.add(save, ID_SAVE);
        self.hits.add(cancel, ID_CANCEL);
        if self.renamed {
            let lf = self.fonts.get(self.px(13.0), 400, Face::Text);
            let label = tr("Use the device's own name");
            let lw = b.measure(&label, lf).cx + self.px(12.0);
            let r = rect(p - self.px(6.0), y, lw, bh2);
            if self.hover == Some(ID_RESET) {
                b.rrect(r.left as f32, r.top as f32, lw as f32, bh2 as f32, 4.0 * self.s, t.button_hover, 1.0);
            }
            b.text(&label, lf, t.accent, r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            self.hits.add(r, ID_RESET);
        }
        b.blit(hdc);
        self.buf = Some(b);
    }

    fn text(&self) -> String {
        unsafe {
            let n = GetWindowTextLengthW(self.edit).max(0) as usize;
            let mut buf = vec![0u16; n + 1];
            let got = GetWindowTextW(self.edit, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..got])
        }
    }

    fn save(&self, name: Option<&str>) {
        // Typing the device's own name back is the same as resetting.
        let name = name.and_then(settings::clean_name).filter(|n| *n != self.reported);
        settings::set_name(SETTINGS_KEY, &self.status.key, name.as_deref());
        unsafe {
            if let Ok(main) = FindWindowW(w!("BatteryHub.Taskbar.Main"), None) {
                let _ = PostMessageW(Some(main), names_changed_message(), WPARAM(0), LPARAM(0));
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                with(|d| d.paint(hdc));
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wp.0 as *mut _);
                with(|d| {
                    SetTextColor(hdc, d.theme.text);
                    SetBkColor(hdc, d.theme.card);
                    LRESULT(d.edit_brush.0 as isize)
                })
                .unwrap_or(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                with(|d| {
                    let h = d.hits.at(x, y);
                    if h != d.hover {
                        d.hover = h;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    SetCursor(LoadCursorW(None, if h.is_some() { IDC_HAND } else { IDC_ARROW }).ok());
                });
                LRESULT(0)
            }
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT && HWND(wp.0 as *mut _) == hwnd => LRESULT(1),
            WM_LBUTTONUP => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                match with(|d| d.hits.at(x, y)).flatten() {
                    Some(ID_SAVE) => {
                        with(|d| d.save(Some(&d.text())));
                    }
                    Some(ID_RESET) => {
                        with(|d| d.save(None));
                    }
                    Some(ID_CANCEL) => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                with(|d| {
                    d.s = (wp.0 & 0xFFFF) as f32 / 96.0;
                    d.fonts.clear();
                });
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                with(|d| d.layout());
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

/// `--rename <device>`: runs the dialog until it is closed.
pub fn run_standalone(args: &[String]) {
    let Some((status, reported)) = args.first().and_then(|s| crate::dashboard::decode_status(s)) else { return };
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(std::ptr::without_provenance(1)) /* MAKEINTRESOURCE(1) */).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(mon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let style = WS_CAPTION | WS_SYSMENU;
        let mut wr = RECT { left: 0, top: 0, right: (460.0 * s) as i32, bottom: (220.0 * s) as i32 };
        let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut wr, style, false, WINDOW_EX_STYLE::default(), dx);
        let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
        let work = mi.rcWork;
        let (x, y) = (work.left + (work.right - work.left - w) / 2, work.top + (work.bottom - work.top - h) / 2);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            &windows::core::HSTRING::from(tr("Rename device - BatteryHub")),
            style,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };
        let theme = Theme::current();
        gui::style_frame(hwnd, &theme);
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            &HSTRING::from(status.name.as_str()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(0x80 /* ES_AUTOHSCROLL */),
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(ID_EDIT as usize as *mut _)),
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default();
        if theme.dark {
            let _ = SetWindowTheme(edit, w!("DarkMode_CFD"), PCWSTR::null());
        }
        SendMessageW(edit, EM_LIMITTEXT, Some(WPARAM(NAME_MAX)), None);
        let renamed = !reported.is_empty() && reported != status.name;
        let d = Dlg {
            hwnd,
            edit,
            theme,
            s: GetDpiForWindow(hwnd).max(96) as f32 / 96.0,
            fonts: Fonts::default(),
            buf: None,
            hits: Hits::default(),
            hover: None,
            status,
            reported,
            renamed,
            edit_brush: CreateSolidBrush(COLORREF(theme.card.0)),
        };
        DLG.with(|c| *c.borrow_mut() = Some(d));
        with(|d| d.layout());
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(edit));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_RETURN.0 as usize {
                with(|d| d.save(Some(&d.text())));
                continue;
            }
            if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_ESCAPE.0 as usize {
                let _ = DestroyWindow(hwnd);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Some(b) = with(|d| d.edit_brush) {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
    }
}
