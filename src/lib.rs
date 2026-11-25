//! BatteryHub (Rust): a recode of github.com/krisgfx9/Wireless-Device-Battery-Monitor (C++, GPL-3.0), rewritten in Rust.
//! Shared core used by both editions: device model, Win32 HID layer and the drivers.
#![cfg(windows)]

pub mod ble;
pub mod cardfx;
pub mod dashboard;
pub mod device;
pub mod drivers;
#[cfg(feature = "experimental-xaml")]
pub mod experimental_xaml;
pub mod gui;
pub mod hid;
pub mod history;
pub mod i18n;
pub mod install;
pub mod listen;
pub mod rename;
pub mod report;
pub mod report_window;
pub mod settings;
pub mod stats;
pub mod tray_probe;
pub mod ui;
pub mod update;
pub mod util;

pub use device::{DeviceKind, DeviceStatus, poll_all_devices};
