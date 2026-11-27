//! System sensors shown on the taskbar like devices: CPU, GPU and RAM load, plus the
//! temperatures Windows reports.
//!
//! Everything is read without a kernel driver and without new dependencies:
//!  * CPU load: `GetSystemTimes` between two reads (kernel time includes idle);
//!  * RAM load: `GlobalMemoryStatusEx` (already a percentage);
//!  * GPU load and temperature: NVML (the NVIDIA driver's own library, loaded dynamically and
//!    used only when it is there), else the GPU Engine performance counters (load only);
//!  * CPU temperature: the Thermal Zone Information counters - many machines (mostly laptops)
//!    report one, desktop boards often do not, and then the temperature is simply not shown.
//!
//! A background thread reads the sensors every INTERVAL_MS and posts the readings to the
//! taskbar window as system devices (keys with SYS_PREFIX, see `DeviceStatus::is_system`).
#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER,
    PDH_HQUERY,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::device::{DeviceKind, DeviceStatus};

/// Keys of the system devices start with this: they are never battery devices.
pub const SYS_PREFIX: &str = "\u{0}sys:";
/// How often the sensors are read.
const INTERVAL_MS: u64 = 2000;
/// From this temperature (°C) on, the reading is drawn in the warning colour.
pub const HOT_C: i32 = 80;

/// The window the readings go to and the message they arrive as (set once at start-up).
static TARGET_HWND: AtomicUsize = AtomicUsize::new(0);
static TARGET_MSG: AtomicUsize = AtomicUsize::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Remembers where the readings go. Call once, before `set_enabled(true)`.
pub fn init(hwnd: HWND, msg: u32) {
    TARGET_HWND.store(hwnd.0 as usize, Ordering::Relaxed);
    TARGET_MSG.store(msg as usize, Ordering::Relaxed);
}

/// Starts or stops the sensor thread. Stopping keeps the last readings on the strip until the
/// taskbar removes them (see `remove_sys`).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if on && !RUNNING.swap(true, Ordering::SeqCst) {
        std::thread::spawn(read_loop);
    }
}

fn read_loop() {
    // (busy, total) 100 ns ticks of the last read, for the load between two reads.
    let mut cpu_prev: Option<(u64, u64)> = None;
    let mut cpu_temp = Pdh::open(CPU_TEMP_COUNTER);
    let mut gpu_fallback = Pdh::open(GPU_LOAD_COUNTER);
    let mut gpu_ready = false;
    let mut nvml = Nvml::load();
    loop {
        if !ENABLED.load(Ordering::SeqCst) {
            RUNNING.store(false, Ordering::SeqCst);
            // Turned on again while we were leaving: whoever claims RUNNING keeps going.
            if ENABLED.load(Ordering::SeqCst) && !RUNNING.swap(true, Ordering::SeqCst) {
                continue;
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(INTERVAL_MS));

        let mut items: Vec<DeviceStatus> = Vec::with_capacity(3);

        if let Some(percent) = cpu_load(&mut cpu_prev) {
            let temp = cpu_temp.as_mut().and_then(|q| q.refresh().map(|t| t.clamp(-10, 110)));
            items.push(sensor("cpu", "CPU", DeviceKind::Cpu, percent, temp));
        }
        if let Some(percent) = memory_load() {
            items.push(sensor("ram", "RAM", DeviceKind::Memory, percent, None));
        }

        let mut gpu: Option<DeviceStatus> = None;
        if let Some(n) = nvml.as_mut() {
            gpu = n.sample().map(|(percent, temp)| sensor("gpu", "GPU", DeviceKind::Gpu, percent, temp.map(|t| t.clamp(-10, 110))));
        }
        if gpu.is_none() {
            // The GPU Engine counters are rate counters: they need one collect to start from,
            // and are only read on the tick after it. The query is reopened every cycle, so
            // instances of processes that started or stopped are picked up again.
            match (gpu_fallback.is_some(), gpu_ready) {
                (false, _) => {
                    gpu_fallback = Pdh::open(GPU_LOAD_COUNTER);
                    gpu_ready = false;
                }
                (true, false) => {
                    if let Some(q) = gpu_fallback.as_mut() {
                        q.collect();
                    }
                    gpu_ready = true;
                }
                (true, true) => {
                    if let Some(q) = gpu_fallback.as_mut() {
                        q.collect();
                        if let Some(percent) = q.value(false) {
                            gpu = Some(sensor("gpu", "GPU", DeviceKind::Gpu, percent, None));
                        }
                    }
                    gpu_fallback = None;
                    gpu_ready = false;
                }
            }
        }
        if let Some(g) = gpu {
            items.push(g);
        }

        post(items);
    }
}

fn sensor(key: &str, name: &str, kind: DeviceKind, percent: i32, temp: Option<i32>) -> DeviceStatus {
    DeviceStatus {
        key: format!("{SYS_PREFIX}{key}"),
        name: name.into(),
        kind,
        percent: percent.clamp(0, 100),
        temp: temp.unwrap_or(-1),
        connected: true,
        ..Default::default()
    }
}

/// Hands the readings to the taskbar window (the same way the device poll does, see ui.rs).
fn post(devices: Vec<DeviceStatus>) {
    let (hwnd, msg) = (TARGET_HWND.load(Ordering::Relaxed), TARGET_MSG.load(Ordering::Relaxed));
    if hwnd == 0 {
        return;
    }
    let ptr = Box::into_raw(Box::new(devices));
    unsafe {
        if PostMessageW(Some(HWND(hwnd as *mut _)), msg as u32, WPARAM(0), LPARAM(ptr as isize)).is_err() {
            drop(Box::from_raw(ptr));
        }
    }
}

// ---- CPU and RAM -------------------------------------------------------------------------------

/// CPU load in percent since the previous read (None on the first one).
fn cpu_load(prev: &mut Option<(u64, u64)>) -> Option<i32> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_err() {
            return None;
        }
    }
    // Kernel time includes idle, so busy = kernel + user - idle.
    let i = filetime(&idle);
    let (k, u) = (filetime(&kernel), filetime(&user));
    let (busy, total) = (k + u - i, k + u);
    let Some((prev_busy, prev_total)) = *prev else {
        *prev = Some((busy, total));
        return None;
    };
    *prev = Some((busy, total));
    let (db, dt) = (busy.saturating_sub(prev_busy) as f64, total.saturating_sub(prev_total) as f64);
    (dt > 0.0).then(|| ((1.0 - db / dt) * 100.0).round().clamp(0.0, 100.0) as i32)
}

fn filetime(t: &FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

/// RAM load in percent (Windows counts it for us).
fn memory_load() -> Option<i32> {
    let mut ms = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut ms).ok().map(|_| ms.dwMemoryLoad.clamp(0, 100) as i32) }
}

// ---- performance counters ----------------------------------------------------------------------

const GPU_LOAD_COUNTER: PCWSTR = w!("\\GPU Engine(*)\\Utilization Percentage");
const CPU_TEMP_COUNTER: PCWSTR = w!("\\Thermal Zone Information(*)\\Temperature");

/// One open performance counter query: a GPU Engine utilization or a thermal zone temperature.
/// The formatted value is the average since the previous collect, so the query is collected on
/// every tick and read one tick later.
struct Pdh {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl Pdh {
    fn open(path: PCWSTR) -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY(std::ptr::null_mut());
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = PDH_HCOUNTER(std::ptr::null_mut());
            if PdhAddEnglishCounterW(query, path, 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            Some(Self { query, counter })
        }
    }

    fn collect(&mut self) {
        unsafe {
            PdhCollectQueryData(self.query);
        }
    }

    /// The counter's value since the previous collect. Several instances (every GPU engine,
    /// every thermal zone) are combined: load adds up, temperatures take the highest.
    fn value(&self, temperature: bool) -> Option<i32> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        unsafe {
            if PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, None, &mut v) != 0 {
                return None;
            }
        }
        let x = unsafe { v.Anonymous.doubleValue };
        if !x.is_finite() {
            return None;
        }
        if temperature {
            // Kelvin in tenths -> Celsius.
            Some(((x / 10.0 - 273.15).round() as i32).clamp(-10, 110))
        } else {
            Some(x.round().clamp(0.0, 100.0) as i32)
        }
    }

    /// Collects and reads at once (the thermal zone temperature is instantaneous).
    fn refresh(&mut self) -> Option<i32> {
        self.collect();
        self.value(true)
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

// ---- NVIDIA (NVML) -----------------------------------------------------------------------------

/// The few NVML functions that are needed, resolved from the driver's own nvml.dll. The library
/// stays loaded for the process's lifetime (nothing calls FreeLibrary).
struct Nvml {
    device: *mut std::ffi::c_void,
    utilization: unsafe extern "C" fn(*mut std::ffi::c_void, *mut NvmlUtilization) -> u32,
    temperature: unsafe extern "C" fn(*mut std::ffi::c_void, u32, *mut u32) -> u32,
}

#[repr(C)]
struct NvmlUtilization {
    gpu: u32,
    memory: u32,
}

const NVML_TEMPERATURE_GPU: u32 = 0;

impl Nvml {
    fn load() -> Option<Self> {
        let lib = unsafe { LoadLibraryW(w!("nvml.dll")) }.ok()?.0 as isize;
        unsafe {
            let sym = |name: &[u8]| {
                GetProcAddress(windows::Win32::Foundation::HMODULE(lib as *mut _), windows::core::PCSTR(name.as_ptr())).map(|f| f as *const std::ffi::c_void)
            };
            let init: unsafe extern "C" fn() -> u32 = std::mem::transmute(sym(b"nvmlInit_v2\0")?);
            let handle: unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> u32 = std::mem::transmute(sym(b"nvmlDeviceGetHandleByIndex_v2\0")?);
            let utilization: unsafe extern "C" fn(*mut std::ffi::c_void, *mut NvmlUtilization) -> u32 =
                std::mem::transmute(sym(b"nvmlDeviceGetUtilizationRates\0")?);
            let temperature: unsafe extern "C" fn(*mut std::ffi::c_void, u32, *mut u32) -> u32 = std::mem::transmute(sym(b"nvmlDeviceGetTemperature\0")?);
            if init() != 0 {
                return None;
            }
            let mut device = std::ptr::null_mut();
            if handle(0, &mut device) != 0 || device.is_null() {
                return None;
            }
            Some(Self { device, utilization, temperature })
        }
    }

    /// (load in percent, temperature in °C).
    fn sample(&mut self) -> Option<(i32, Option<i32>)> {
        unsafe {
            let mut u = NvmlUtilization { gpu: 0, memory: 0 };
            if (self.utilization)(self.device, &mut u) != 0 {
                return None;
            }
            let mut t = 0u32;
            let temp = (self.temperature)(self.device, NVML_TEMPERATURE_GPU, &mut t);
            Some((u.gpu.clamp(0, 100) as i32, (temp == 0).then(|| t.clamp(0, 150) as i32)))
        }
    }
}
