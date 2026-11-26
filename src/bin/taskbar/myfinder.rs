use std::{ffi::c_void, os::windows::ffi::OsStrExt, sync::OnceLock};
use windows::{core::{s, PCWSTR}, Win32::{Foundation::HWND, System::LibraryLoader::{GetProcAddress, LoadLibraryW}}};

type Open = unsafe extern "C" fn(HWND, HWND) -> *mut c_void;
type Close = unsafe extern "C" fn(*mut c_void);
type Paint = unsafe extern "C" fn(*mut c_void, *const u32, u32, u32) -> i32;
type Place = unsafe extern "C" fn(*mut c_void, i32, i32, i32);
type Foreground = unsafe extern "C" fn(*mut c_void) -> i32;
struct Api { open: Open, close: Close, paint: Paint, place: Place, foreground: Foreground }
static API: OnceLock<Option<Api>> = OnceLock::new();

pub fn enabled() -> bool {
    !std::env::var_os("BH_MYFINDER").is_some_and(|value| value == "0") && api().is_some()
}

fn api() -> Option<&'static Api> {
    API.get_or_init(|| unsafe {
        let path = std::env::current_exe().ok()?.with_file_name("BatteryHubMyFinder.dll");
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Keep the module loaded until exit because its sampler can still be finishing a frame.
        let dll = LoadLibraryW(PCWSTR(wide.as_ptr())).ok()?;
        Some(Api {
            open: std::mem::transmute::<_, Open>(GetProcAddress(dll, s!("OpenFinderField"))?),
            close: std::mem::transmute::<_, Close>(GetProcAddress(dll, s!("CloseFinderField"))?),
            paint: std::mem::transmute::<_, Paint>(GetProcAddress(dll, s!("PaintFinderField"))?),
            place: std::mem::transmute::<_, Place>(GetProcAddress(dll, s!("PlaceFinderField"))?),
            foreground: std::mem::transmute::<_, Foreground>(GetProcAddress(dll, s!("FinderFieldForeground"))?),
        })
    }).as_ref()
}

pub struct FinderField { field: *mut c_void, api: &'static Api }
impl FinderField {
    pub fn open(window: HWND, host: HWND) -> Option<Self> {
        let api = api()?;
        let field = unsafe { (api.open)(window, host) };
        (!field.is_null()).then_some(Self { field, api })
    }
    pub fn paint(&self, pixels: &[u32], w: i32, h: i32) {
        if w > 0 && h > 0 && pixels.len() == w as usize * h as usize {
            unsafe { (self.api.paint)(self.field, pixels.as_ptr(), w as u32, h as u32) };
        }
    }
    pub fn place(&self, w: i32, h: i32, gap: i32) { unsafe { (self.api.place)(self.field, w, h, gap) } }
    pub fn light(&self, fallback: bool) -> bool {
        match unsafe { (self.api.foreground)(self.field) } { 1 => true, 2 => false, _ => fallback }
    }
}
impl Drop for FinderField {
    fn drop(&mut self) { if !self.field.is_null() { unsafe { (self.api.close)(self.field) } } }
}
