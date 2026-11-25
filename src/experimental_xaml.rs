//! Isolated bridge from the existing Rust taskbar renderer to the optional Explorer XAML host.
//! Device discovery and battery polling stay in the existing Rust monitor.

use std::ffi::c_void;
use windows::Win32::Foundation::HWND;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

unsafe extern "C" {
    fn bhx_publisher_new() -> *mut c_void;
    fn bhx_publisher_delete(value: *mut c_void);
    fn bhx_publisher_start(value: *mut c_void, owner: usize, taskbar: usize) -> i32;
    fn bhx_publisher_restart(value: *mut c_void, owner: usize, taskbar: usize) -> i32;
    fn bhx_publisher_stop(value: *mut c_void);
    fn bhx_publisher_request(value: *mut c_void, requested: i32) -> i32;
    fn bhx_publisher_publish(
        value: *mut c_void,
        pixels: *const u32,
        width: u32,
        height: u32,
        dpi: u32,
        rects: *const Rect,
        device_ids: *const u64,
        anchors: *const i32,
        count: u32,
        individual: i32,
        light: i32,
        hover: i32,
        locked: i32,
        desired_column: i32,
    ) -> i32;
    fn bhx_publisher_ready_slot(value: *mut c_void, rect: *mut Rect) -> i32;
}

pub struct Publisher(*mut c_void);

impl Default for Publisher {
    fn default() -> Self {
        Self(unsafe { bhx_publisher_new() })
    }
}

impl Publisher {
    pub fn start(&mut self, owner: HWND, taskbar: HWND) -> bool {
        !self.0.is_null() && unsafe { bhx_publisher_start(self.0, owner.0 as usize, taskbar.0 as usize) != 0 }
    }
    pub fn restart(&mut self, owner: HWND, taskbar: HWND) -> bool {
        !self.0.is_null() && unsafe { bhx_publisher_restart(self.0, owner.0 as usize, taskbar.0 as usize) != 0 }
    }
    pub fn set_requested(&self, value: bool) {
        if !self.0.is_null() {
            unsafe {
                bhx_publisher_request(self.0, i32::from(value));
            }
        }
    }
    pub fn publish(
        &self,
        pixels: &[u32],
        width: u32,
        height: u32,
        dpi: u32,
        rects: &[Rect],
        device_ids: &[u64],
        anchors: &[i32],
        individual: bool,
        light: bool,
        hover: bool,
        locked: bool,
        desired_column: i32,
    ) -> bool {
        if self.0.is_null()
            || width == 0
            || height == 0
            || pixels.len() != width as usize * height as usize
            || rects.len() != device_ids.len()
            || rects.len() != anchors.len()
        {
            return false;
        }
        unsafe {
            bhx_publisher_publish(
                self.0,
                pixels.as_ptr(),
                width,
                height,
                dpi,
                rects.as_ptr(),
                device_ids.as_ptr(),
                anchors.as_ptr(),
                rects.len() as u32,
                i32::from(individual),
                i32::from(light),
                i32::from(hover),
                i32::from(locked),
                desired_column,
            ) != 0
        }
    }
    pub fn ready_slot(&self) -> Option<Rect> {
        let mut rect = Rect::default();
        (!self.0.is_null() && unsafe { bhx_publisher_ready_slot(self.0, &mut rect) != 0 }).then_some(rect)
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                bhx_publisher_stop(self.0);
                bhx_publisher_delete(self.0);
            }
        }
    }
}
