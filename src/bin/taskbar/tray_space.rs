//! Reserve contiguous Explorer buttons for the original interactive strip. Never place it across
//! someone else's button, an overflow popup, or an incomplete reservation.
use super::*;
use windows::Win32::UI::Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect};

const COUNT: usize = 4;
const FIRST_ID: u32 = 0xBC00_0010;

#[derive(Default)]
pub(super) struct Reservation {
    added: [bool; COUNT],
}

impl Reservation {
    fn data(owner: HWND, slot: usize) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: owner, uID: FIRST_ID + slot as u32, ..Default::default() }
    }

    pub fn clear(&mut self, owner: HWND) {
        for (slot, added) in self.added.iter_mut().enumerate() {
            if *added {
                unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &Self::data(owner, slot)); }
                *added = false;
            }
        }
    }

    pub fn left(&mut self, owner: HWND, taskbar: HWND, tray: RECT, width: i32) -> Option<i32> {
        if width <= 0 { return None; }
        unsafe {
            if self.added.contains(&false) {
                // Monochrome AND=1, XOR=0 leaves the taskbar background untouched.
                let icon = CreateIcon(None, 16, 16, 1, 1, [0xff; 32].as_ptr(), [0; 32].as_ptr()).ok()?;
                for (slot, added) in self.added.iter_mut().enumerate() {
                    if *added { continue; }
                    let mut d = Self::data(owner, slot);
                    d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
                    d.uCallbackMessage = WM_TRAY;
                    d.hIcon = icon;
                    ui::fill_wide(&mut d.szTip, &format!("BatteryHub widget {}/{}", slot + 1, COUNT));
                    *added = Shell_NotifyIconW(NIM_ADD, &d).as_bool();
                }
                let _ = DestroyIcon(icon);
            }
            let mut slots = [RECT::default(); COUNT];
            for (i, r) in slots.iter_mut().enumerate() {
                let id = NOTIFYICONIDENTIFIER { cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32, hWnd: owner, uID: FIRST_ID + i as u32, ..Default::default() };
                *r = Shell_NotifyIconGetRect(&id).ok()?;
            }
            slots.sort_by_key(|r| r.left);
            let first = slots[0];
            let end = slots[COUNT - 1].right;
            if first.left < tray.left - 1 || end > tray.right || first.top < tray.top || first.bottom > tray.bottom || end - first.left < width
                || slots.iter().any(|r| r.right <= r.left || r.top != first.top || r.bottom != first.bottom)
                || slots.windows(2).any(|pair| (pair[1].left - pair[0].right).abs() > 1) {
                return None;
            }
            let mut p = POINT { x: first.left + (end - first.left - width) / 2, y: first.top };
            ScreenToClient(taskbar, &mut p).as_bool().then_some(p.x)
        }
    }
}
