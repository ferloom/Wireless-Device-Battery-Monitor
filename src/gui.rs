//! Small drawing toolkit for BatteryHub's own windows (dashboard, rename dialog, setup): a Windows
//! 11 colour palette, a back buffer with anti-aliased shapes and GDI text, a font cache, hit
//! areas and easing. Windows paint from a back buffer only when something changed, and animate
//! with a timer only while an animation runs.
use std::ffi::c_void;
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, RECT, SIZE};
use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::core::HSTRING;

use crate::ui::{rgb, split};

// ---- colours -----------------------------------------------------------------------------------

/// Windows 11 colours (dark or light, following the apps setting).
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    /// Window background (Mica-like solid).
    pub bg: COLORREF,
    /// Card background and its 1 px stroke.
    pub card: COLORREF,
    pub card_hover: COLORREF,
    pub stroke: COLORREF,
    pub text: COLORREF,
    pub dim: COLORREF,
    pub faint: COLORREF,
    pub accent: COLORREF,
    /// Text on an accent-coloured button.
    pub on_accent: COLORREF,
    pub green: COLORREF,
    pub red: COLORREF,
    pub orange: COLORREF,
    pub blue: COLORREF,
    pub track: COLORREF,
    pub button: COLORREF,
    pub button_hover: COLORREF,
}

impl Theme {
    pub fn current() -> Self {
        if crate::ui::apps_are_light() { Self::light() } else { Self::dark() }
    }

    pub fn dark() -> Self {
        Theme {
            dark: true,
            bg: rgb(32, 32, 32),
            card: rgb(43, 43, 43),
            card_hover: rgb(50, 50, 50),
            stroke: rgb(58, 58, 58),
            text: rgb(255, 255, 255),
            dim: rgb(200, 200, 200),
            faint: rgb(140, 140, 140),
            accent: rgb(76, 194, 255),
            on_accent: rgb(0, 0, 0),
            green: rgb(108, 203, 95),
            red: rgb(255, 99, 99),
            orange: rgb(252, 185, 0),
            blue: rgb(96, 205, 255),
            track: rgb(70, 70, 70),
            button: rgb(55, 55, 55),
            button_hover: rgb(62, 62, 62),
        }
    }

    pub fn light() -> Self {
        Theme {
            dark: false,
            bg: rgb(243, 243, 243),
            card: rgb(251, 251, 251),
            card_hover: rgb(246, 246, 246),
            stroke: rgb(229, 229, 229),
            text: rgb(26, 26, 26),
            dim: rgb(96, 96, 96),
            faint: rgb(140, 140, 140),
            accent: rgb(0, 95, 184),
            on_accent: rgb(255, 255, 255),
            green: rgb(15, 123, 15),
            red: rgb(196, 43, 28),
            orange: rgb(157, 93, 0),
            blue: rgb(0, 95, 184),
            track: rgb(214, 214, 214),
            button: rgb(253, 253, 253),
            button_hover: rgb(246, 246, 246),
        }
    }
}

/// `a` mixed towards `b` by `t` (0..1).
pub fn mix(a: COLORREF, b: COLORREF, t: f32) -> COLORREF {
    let (ar, ag, ab) = split(a);
    let (br, bg, bb) = split(b);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)).round() as u8;
    rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

// ---- easing ------------------------------------------------------------------------------------

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// A value moving from `from` to `to` over `ms` milliseconds.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    pub from: f32,
    pub to: f32,
    pub start: Instant,
    pub ms: f32,
}

impl Tween {
    pub fn new(v: f32) -> Self {
        Tween { from: v, to: v, start: Instant::now(), ms: 1.0 }
    }
    /// Starts moving from where it is now.
    pub fn go(&mut self, to: f32, ms: f32) {
        if (to - self.to).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value();
        self.to = to;
        self.start = Instant::now();
        self.ms = ms.max(1.0);
    }
    pub fn progress(&self) -> f32 {
        (self.start.elapsed().as_secs_f32() * 1000.0 / self.ms).min(1.0)
    }
    pub fn value(&self) -> f32 {
        self.from + (self.to - self.from) * ease_out(self.progress())
    }
    pub fn running(&self) -> bool {
        self.progress() < 1.0
    }
}

// ---- fonts -------------------------------------------------------------------------------------

/// The body font face (Segoe UI Variable on Windows 11, else Segoe UI).
pub fn text_face(display: bool) -> &'static str {
    static HAS_VARIABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let has = *HAS_VARIABLE.get_or_init(|| face_exists("Segoe UI Variable Text"));
    match (has, display) {
        (true, true) => "Segoe UI Variable Display",
        (true, false) => "Segoe UI Variable Text",
        _ => "Segoe UI",
    }
}

fn face_exists(face: &str) -> bool {
    unsafe {
        let f = crate::ui::font(12, 400, face);
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, HGDIOBJ(f.0));
        let mut got = [0u16; 64];
        GetTextFaceW(dc, Some(&mut got));
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(HGDIOBJ(f.0));
        crate::util::from_wide(&got).eq_ignore_ascii_case(face)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Text,
    Display,
    Icons,
}

/// Fonts by (size in px, weight, face), created once per window and DPI.
#[derive(Default)]
pub struct Fonts {
    list: Vec<((i32, i32, Face), HFONT)>,
}

impl Fonts {
    pub fn get(&mut self, px: i32, weight: i32, face: Face) -> HFONT {
        if let Some((_, f)) = self.list.iter().find(|(k, _)| *k == (px, weight, face)) {
            return *f;
        }
        let name = match face {
            Face::Text => text_face(false),
            Face::Display => text_face(true),
            Face::Icons => crate::ui::icon_font_face(),
        };
        let f = unsafe {
            CreateFontW(
                -px,
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
                if face == Face::Icons { ANTIALIASED_QUALITY } else { CLEARTYPE_QUALITY },
                0,
                &HSTRING::from(name),
            )
        };
        self.list.push(((px, weight, face), f));
        f
    }

    /// Frees every font (after a DPI change, or on close).
    pub fn clear(&mut self) {
        for (_, f) in self.list.drain(..) {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        self.clear();
    }
}

// ---- back buffer -------------------------------------------------------------------------------

/// A 32-bit bitmap selected into a memory DC: shapes are written straight into its pixels, text
/// is drawn with GDI. Opaque (the window shows it with BitBlt).
pub struct Buffer {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    pub w: i32,
    pub h: i32,
}

impl Buffer {
    pub fn new(w: i32, h: i32) -> Option<Self> {
        let (w, h) = (w.max(1), h.max(1));
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
                    SetBkMode(dc, TRANSPARENT);
                    Some(Buffer { dc, bmp, old, bits: bits.cast(), w, h })
                }
                _ => {
                    let _ = DeleteDC(dc);
                    None
                }
            }
        }
    }

    pub fn px(&mut self) -> &mut [u32] {
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize)
        }
    }

    /// Draws a user-chosen icon: premultiplied ARGB pixels (`custom_icon::Sprite`), source-over,
    /// with an optional extra fade. The dashboard uses it for the device bar and the header.
    pub fn rgba(&mut self, px: &[u32], w: i32, x: i32, y: i32, alpha: f32) {
        if w <= 0 || alpha <= 0.0 {
            return;
        }
        let (bw, bh) = (self.w, self.h);
        let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
        if a == 0 {
            return;
        }
        let h = (px.len() / w as usize) as i32;
        let dst = self.px();
        for yy in 0..h {
            for xx in 0..w {
                let (dx, dy) = (x + xx, y + yy);
                if dx < 0 || dy < 0 || dx >= bw || dy >= bh {
                    continue;
                }
                let s = px[(yy * w + xx) as usize];
                let sa = ((s >> 24) & 0xFF) * a / 255;
                if sa == 0 {
                    continue;
                }
                let d = &mut dst[(dy * bw + dx) as usize];
                let inv = 255 - sa;
                let ch = |shift: u32| (((s >> shift) & 0xFF) * a / 255 + ((*d >> shift) & 0xFF) * inv / 255).min(255);
                let na = (sa + ((*d >> 24) & 0xFF) * inv / 255).min(255);
                *d = na << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
            }
        }
    }

    /// Copies all pixels of `other` (same size) into this buffer.
    pub fn copy_from(&mut self, other: &mut Buffer) {
        if other.w == self.w && other.h == self.h {
            let src = other.px().to_vec();
            self.px().copy_from_slice(&src);
        }
    }

    /// Shows the buffer in `hdc` at (0, 0).
    pub fn blit(&self, hdc: HDC) {
        unsafe {
            let _ = BitBlt(hdc, 0, 0, self.w, self.h, Some(self.dc), 0, 0, SRCCOPY);
        }
    }

    // -- shapes (coordinates in px) --

    fn put(&mut self, x: i32, y: i32, c: u32, a: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let i = (y * self.w + x) as usize;
        let d = unsafe { &mut *self.bits.add(i) };
        if a >= 1.0 {
            *d = c;
            return;
        }
        let ai = (a * 256.0) as u32;
        let ch = |sh: u32| ((((c >> sh) & 0xFF) * ai + ((*d >> sh) & 0xFF) * (256 - ai)) >> 8) & 0xFF;
        *d = 0xFF00_0000 | ch(16) << 16 | ch(8) << 8 | ch(0);
    }

    pub fn clear(&mut self, c: COLORREF) {
        let v = pixel(c);
        self.px().fill(v);
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: COLORREF) {
        let v = pixel(c);
        let (x0, x1) = (x.max(0), (x + w).min(self.w));
        let (y0, y1) = (y.max(0), (y + h).min(self.h));
        if x0 >= x1 {
            return;
        }
        let stride = self.w as usize;
        let px = self.px();
        for yy in y0..y1 {
            px[yy as usize * stride + x0 as usize..yy as usize * stride + x1 as usize].fill(v);
        }
    }

    /// Filled rounded rectangle with anti-aliased corners, at opacity `alpha` (0..1).
    #[allow(clippy::too_many_arguments)]
    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: COLORREF, alpha: f32) {
        let v = pixel(c);
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let _ = self.px();
        let (x0, x1) = (x.floor() as i32, (x + w).ceil() as i32);
        let (y0, y1) = (y.floor() as i32, (y + h).ceil() as i32);
        for yy in y0.max(0)..y1.min(self.h) {
            let cy = yy as f32 + 0.5;
            for xx in x0.max(0)..x1.min(self.w) {
                let cx = xx as f32 + 0.5;
                // Distance outside the shape (negative inside), rounded box SDF.
                let qx = (cx - (x + w / 2.0)).abs() - (w / 2.0 - r);
                let qy = (cy - (y + h / 2.0)).abs() - (h / 2.0 - r);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
                let cov = (0.5 - outside).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.put(xx, yy, v, cov * alpha);
                }
            }
        }
    }

    /// Card: 1 px stroke and fill, like Windows 11 cards.
    #[allow(clippy::too_many_arguments)]
    pub fn card(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: COLORREF, stroke: COLORREF) {
        self.rrect(x, y, w, h, r, stroke, 1.0);
        self.rrect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, (r - 1.0).max(0.0), fill, 1.0);
    }

    /// Anti-aliased ring of thickness `t` around (cx, cy): the arc runs clockwise from the top over
    /// `frac` (0..1) of the circle, with round ends; `frac` >= 1 draws the whole ring.
    #[allow(clippy::too_many_arguments)]
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, t: f32, frac: f32, c: COLORREF, alpha: f32) {
        if frac <= 0.0 {
            return;
        }
        let v = pixel(c);
        let _ = self.px();
        let tau = std::f32::consts::TAU;
        let sweep = frac.min(1.0) * tau;
        // The two ends, for the round caps.
        let end = |a: f32| (cx + r * a.sin(), cy - r * a.cos());
        let (s0, s1) = (end(0.0), end(sweep));
        let reach = r + t;
        for yy in (cy - reach).floor() as i32..=(cy + reach).ceil() as i32 {
            for xx in (cx - reach).floor() as i32..=(cx + reach).ceil() as i32 {
                let (x, y) = (xx as f32 + 0.5, yy as f32 + 0.5);
                let (dx, dy) = (x - cx, y - cy);
                // Angle clockwise from the top, 0..TAU.
                let mut a = dx.atan2(-dy);
                if a < 0.0 {
                    a += tau;
                }
                let sd = if frac >= 1.0 || a <= sweep {
                    (dx.hypot(dy) - r).abs() - t / 2.0
                } else {
                    (x - s0.0).hypot(y - s0.1).min((x - s1.0).hypot(y - s1.1)) - t / 2.0
                };
                let cov = (0.5 - sd).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.put(xx, yy, v, cov * alpha);
                }
            }
        }
    }

    /// Draws a size x size coverage mask (0-255) at (x, y) in colour `c`.
    pub fn mask(&mut self, x: i32, y: i32, size: i32, a: &[u8], c: COLORREF) {
        let v = pixel(c);
        let _ = self.px();
        for yy in 0..size {
            for xx in 0..size {
                let cov = a[(yy * size + xx) as usize];
                if cov > 0 {
                    self.put(x + xx, y + yy, v, cov as f32 / 255.0);
                }
            }
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: COLORREF, alpha: f32) {
        self.rrect(cx - r, cy - r, 2.0 * r, 2.0 * r, r, c, alpha);
    }

    /// Anti-aliased line of width `t`.
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, t: f32, c: COLORREF, alpha: f32) {
        let v = pixel(c);
        let _ = self.px();
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let half = t / 2.0;
        let (bx0, bx1) = ((x0.min(x1) - half - 1.0).floor() as i32, (x0.max(x1) + half + 1.0).ceil() as i32);
        let (by0, by1) = ((y0.min(y1) - half - 1.0).floor() as i32, (y0.max(y1) + half + 1.0).ceil() as i32);
        for yy in by0.max(0)..by1.min(self.h) {
            for xx in bx0.max(0)..bx1.min(self.w) {
                let (px, py) = (xx as f32 + 0.5, yy as f32 + 0.5);
                let u = (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0);
                let (ex, ey) = (x0 + u * dx - px, y0 + u * dy - py);
                let d = (ex * ex + ey * ey).sqrt();
                let cov = (half + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.put(xx, yy, v, cov * alpha);
                }
            }
        }
    }

    /// Vertical gradient fill from the line down to `bottom` (the area under a chart line):
    /// column `x` gets `top_alpha` at `y` fading to 0 at `bottom`.
    pub fn column_fade(&mut self, x: i32, y: f32, bottom: f32, c: COLORREF, top_alpha: f32) {
        let v = pixel(c);
        let _ = self.px();
        let (y0, y1) = (y.ceil() as i32, bottom as i32);
        let span = (bottom - y).max(1.0);
        for yy in y0.max(0)..y1.min(self.h) {
            let a = top_alpha * (1.0 - (yy as f32 - y) / span);
            self.put(x, yy, v, a);
        }
    }

    // -- text --

    /// Draws `text` into `rc` (px) with GDI's DrawTextW and the given flags.
    pub fn text(&mut self, text: &str, font: HFONT, c: COLORREF, rc: RECT, flags: DRAW_TEXT_FORMAT) {
        let mut w: Vec<u16> = text.encode_utf16().collect();
        let mut rc = rc;
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(font.0));
            SetTextColor(self.dc, c);
            DrawTextW(self.dc, &mut w, &mut rc, flags | DT_NOPREFIX);
            SelectObject(self.dc, old);
        }
    }

    /// Single line at (x, y) top-left; returns its width.
    pub fn text_at(&mut self, text: &str, font: HFONT, c: COLORREF, x: i32, y: i32) -> i32 {
        let sz = self.measure(text, font);
        self.text(text, font, c, RECT { left: x, top: y, right: x + sz.cx + 2, bottom: y + sz.cy }, DT_LEFT | DT_SINGLELINE);
        sz.cx
    }

    pub fn measure(&mut self, text: &str, font: HFONT) -> SIZE {
        let w: Vec<u16> = text.encode_utf16().collect();
        let mut sz = SIZE::default();
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(font.0));
            let _ = GetTextExtentPoint32W(self.dc, &w, &mut sz);
            SelectObject(self.dc, old);
        }
        sz
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

/// COLORREF (0x00BBGGRR) as an opaque DIB pixel (0xFFRRGGBB).
pub fn pixel(c: COLORREF) -> u32 {
    let (r, g, b) = split(c);
    0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

// ---- hit areas ---------------------------------------------------------------------------------

/// Clickable areas registered while painting.
#[derive(Default)]
pub struct Hits {
    list: Vec<(RECT, u32)>,
}

impl Hits {
    pub fn clear(&mut self) {
        self.list.clear();
    }
    pub fn add(&mut self, r: RECT, id: u32) {
        self.list.push((r, id));
    }
    pub fn at(&self, x: i32, y: i32) -> Option<u32> {
        self.list.iter().rev().find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(_, id)| *id)
    }
    pub fn rect_of(&self, id: u32) -> Option<RECT> {
        self.list.iter().find(|(_, i)| *i == id).map(|(r, _)| *r)
    }
}

// ---- window frame ------------------------------------------------------------------------------

/// Dark or light title bar in the window's own background colour, rounded corners (Windows 11).
pub fn style_frame(hwnd: HWND, t: &Theme) {
    unsafe {
        let dark: i32 = t.dark as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &dark as *const i32 as *const c_void, 4);
        let caption = t.bg.0;
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(35), &caption as *const u32 as *const c_void, 4);
        let round: i32 = 2; // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(33), &round as *const i32 as *const c_void, 4);
    }
}

/// Drawn button: rounded, accent (primary) or neutral, with a hover tint.
#[allow(clippy::too_many_arguments)]
pub fn button(b: &mut Buffer, fonts: &mut Fonts, t: &Theme, s: f32, r: RECT, label: &str, primary: bool, hover: f32, enabled: bool) {
    let (x, y, w, h) = (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let (fill, fg) = if primary {
        (if t.dark { mix(t.accent, rgb(255, 255, 255), 0.12 * hover) } else { mix(t.accent, rgb(0, 0, 0), 0.12 * hover) }, t.on_accent)
    } else {
        (mix(t.button, t.button_hover, hover), t.text)
    };
    let (fill, fg) = if enabled { (fill, fg) } else { (mix(t.button, t.bg, 0.5), t.faint) };
    if primary && enabled {
        b.rrect(x, y, w, h, 4.0 * s, fill, 1.0);
    } else {
        b.card(x, y, w, h, 4.0 * s, fill, t.stroke);
    }
    let f = fonts.get((14.0 * s).round() as i32, 400, Face::Text);
    b.text(label, f, fg, r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

// ---- the earbuds icon ----------------------------------------------------------------------------

/// Top and bottom of the earbuds' outline, as parts of the icon's size (for lining it up with glyphs).
pub const EARBUDS_INK: (f32, f32) = (0.13, 0.955);

/// Opacity of a bud that is not reporting (asleep or in the closed case).
const BUD_OFF: u32 = 110;

/// Coverage (0-255, size x size) of the earbuds icon in the outline style of the Fluent glyphs: two
/// buds with stems, the ear tips facing each other. A side that is off is drawn faint.
pub fn earbuds_icon(size: i32, left_on: bool, right_on: bool) -> Vec<u8> {
    let n = size.max(8);
    let s = n as f32;
    // Signed distances in units of the icon size (the right bud; the left one is its mirror).
    fn ellipse(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> f32 {
        (((x - cx) / rx).hypot((y - cy) / ry) - 1.0) * rx.min(ry)
    }
    fn capsule(x: f32, y: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32), r: f32) -> f32 {
        let (px, py, dx, dy) = (x - ax, y - ay, bx - ax, by - ay);
        let h = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        (px - dx * h).hypot(py - dy * h) - r
    }
    fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
        let h = (k - (a - b).abs()).max(0.0) / k;
        a.min(b) - h * h * k / 4.0
    }
    let body = |x: f32, y: f32| {
        let head = ellipse(x, y, 0.80, 0.31, 0.15, 0.145);
        smooth_min(head, capsule(x, y, (0.845, 0.38), (0.89, 0.86), 0.06), 0.06)
    };
    let tip = |x: f32, y: f32| ellipse(x, y, 0.635, 0.37, 0.05, 0.06);
    let stroke = (s / 14.0).max(1.0) / s;
    let ss = 4;
    let mut out = vec![0u8; (n * n) as usize];
    for py in 0..n {
        for px in 0..n {
            let left = px * 2 < n;
            let mut hits = 0;
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (px as f32 + (sx as f32 + 0.5) / ss as f32) / s;
                    let y = (py as f32 + (sy as f32 + 0.5) / ss as f32) / s;
                    let x = if left { 1.0 - x } else { x };
                    if body(x, y).abs() <= stroke / 2.0 || tip(x, y) <= stroke * 0.2 {
                        hits += 1;
                    }
                }
            }
            let cover = hits * 255 / (ss * ss) as u32;
            let on = if left { left_on } else { right_on };
            out[(py * n + px) as usize] = if on { cover } else { cover * BUD_OFF / 255 } as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tween_reaches_target() {
        let mut t = Tween::new(0.0);
        assert_eq!(t.value(), 0.0);
        t.go(10.0, 1.0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(!t.running());
        assert!((t.value() - 10.0).abs() < 1e-4);
    }

    #[test]
    fn buffer_shapes() {
        let mut b = Buffer::new(40, 20).unwrap();
        b.clear(rgb(0, 0, 0));
        b.rrect(0.0, 0.0, 40.0, 20.0, 6.0, rgb(255, 255, 255), 1.0);
        let px = b.px();
        assert_eq!(px[10 * 40 + 20] & 0xFFFFFF, 0xFFFFFF); // centre filled
        assert_eq!(px[0] & 0xFFFFFF, 0); // rounded corner left out
        assert!(px[2] & 0xFF > 0 && px[2] & 0xFF < 255 || px[40 + 1] & 0xFF > 0); // anti-aliased edge
        let mut hits = Hits::default();
        hits.add(rect(0, 0, 10, 10), 1);
        hits.add(rect(5, 5, 10, 10), 2);
        assert_eq!(hits.at(6, 6), Some(2));
        assert_eq!(hits.at(1, 1), Some(1));
        assert_eq!(hits.at(30, 30), None);
    }

    #[test]
    fn colours_mix() {
        assert_eq!(mix(rgb(0, 0, 0), rgb(200, 100, 50), 0.5), rgb(100, 50, 25));
        assert_eq!(pixel(rgb(1, 2, 3)), 0xFF01_0203);
    }
}
