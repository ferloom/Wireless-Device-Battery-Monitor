//! Shared pieces of the setup and of the program itself: names, folders and registry keys,
//! registry access, shortcuts and a verbose, timestamped console log. Everything lives in its own
//! folders, keys, shortcuts, window classes and autostart entry, so the original C++ BatteryHub
//! (Program Files\BatteryHub, HKCU\Software\WirelessBattery) is never touched.
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
};
use windows::Win32::System::Registry::*;
pub use windows::Win32::System::Registry::{HKEY_CURRENT_USER as HKCU, HKEY_LOCAL_MACHINE as HKLM};
use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

pub const PRODUCT: &str = "BatteryHub";
/// App version from the VERSION file (set by build.rs; may have four parts, e.g. 1.0.1.1).
pub const VERSION: &str = env!("BH_VERSION");
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_ONCE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
pub const STARTUP_APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// Parent of the settings key (removed when empty).
pub const ROOT_KEY: &str = r"Software\BatteryHub";

/// "v1.0" for this build's version "1.0.0".
pub fn version_label() -> String {
    label_of(VERSION)
}

/// "v1.5" for "1.5.0", "v1.0.1.1" for "1.0.1.1".
pub fn label_of(version: &str) -> String {
    let mut parts: Vec<&str> = version.trim_start_matches('v').split('.').collect();
    while parts.len() > 2 && parts.last() == Some(&"0") {
        parts.pop();
    }
    format!("v{}", parts.join("."))
}

/// The installable program (names, folders and keys in one place).
#[derive(Clone, Copy, Debug)]
pub struct Edition {
    /// "BatteryHub Taskbar"
    pub name: &'static str,
    /// "Taskbar"
    pub short: &'static str,
    pub exe: &'static str,
    /// HKCU key with InstallLocation / Version (and the program's own settings).
    pub settings_key: &'static str,
    /// Class of the program's main window (used to ask a running copy to exit).
    pub window_class: &'static str,
    /// Value name in HKCU\...\Run (taskbar edition only).
    pub run_value: Option<&'static str>,
    pub description: &'static str,
}

pub const TASKBAR: Edition = Edition {
    name: "BatteryHub Taskbar",
    short: "Taskbar",
    exe: "batteryhub-taskbar.exe",
    settings_key: r"Software\BatteryHub\Taskbar",
    window_class: "BatteryHub.Taskbar.Main",
    run_value: Some("BatteryHub Taskbar"),
    description: "Battery levels of your wireless devices, right on the taskbar",
};

impl Edition {
    /// %LOCALAPPDATA%\Programs\BatteryHub (shared parent, removed only when empty).
    pub fn root_dir() -> PathBuf {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
        PathBuf::from(base).join("Programs").join(PRODUCT)
    }
    /// %LOCALAPPDATA%\Programs\BatteryHub\Taskbar
    pub fn default_dir(&self) -> PathBuf {
        Self::root_dir().join(self.short)
    }
    pub fn start_menu_folder() -> PathBuf {
        known_folder(&FOLDERID_Programs).join(PRODUCT)
    }
    pub fn start_menu_shortcut(&self) -> PathBuf {
        Self::start_menu_folder().join(format!("{}.lnk", self.name))
    }
    pub fn desktop_shortcut(&self) -> PathBuf {
        known_folder(&FOLDERID_Desktop).join(format!("{}.lnk", self.name))
    }
}

/// A shell known folder (handles OneDrive-redirected Desktop etc.).
pub fn known_folder(id: &windows::core::GUID) -> PathBuf {
    unsafe {
        match SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                PathBuf::from(s)
            }
            Err(_) => PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()),
        }
    }
}

pub fn startup_dir() -> PathBuf {
    known_folder(&FOLDERID_Programs).join("Startup")
}

// ---- log ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Step,
    Info,
    Ok,
    Warn,
    Error,
    Detail,
}

/// Listener of the setup's log lines (the setup window).
pub type LogSink = Box<dyn Fn(Level, &str) + Send + Sync>;

/// The setup's log: every line goes to a log file (`%TEMP%\BatteryHub-Setup.log`, shown by
/// "Show details") and to an optional listener (the setup window). Each file and registry entry
/// that is created or removed is logged with its full path.
pub struct Log {
    start: Instant,
    file: std::sync::Mutex<Option<std::fs::File>>,
    sink: Option<LogSink>,
}

impl Log {
    pub fn new(title: &str, sink: Option<LogSink>) -> Self {
        let file = std::fs::File::create(Self::path()).ok();
        let log = Self { start: Instant::now(), file: std::sync::Mutex::new(file), sink };
        log.line(Level::Info, &format!("{title} - {}", std::env::args().collect::<Vec<_>>().join(" ")));
        log
    }

    /// `%TEMP%\BatteryHub-Setup.log`
    pub fn path() -> PathBuf {
        std::env::temp_dir().join("BatteryHub-Setup.log")
    }

    fn line(&self, level: Level, msg: &str) {
        let tag = match level {
            Level::Step => "STEP",
            Level::Info => "INFO",
            Level::Ok => "OK",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Detail => "",
        };
        if let Ok(mut f) = self.file.lock()
            && let Some(f) = f.as_mut()
        {
            let _ = writeln!(f, "[{:8.3}] {tag:<5} {msg}", self.start.elapsed().as_secs_f64());
        }
        if let Some(s) = &self.sink {
            s(level, msg);
        }
    }

    pub fn info(&self, msg: impl AsRef<str>) {
        self.line(Level::Info, msg.as_ref());
    }
    pub fn step(&self, msg: impl AsRef<str>) {
        self.line(Level::Step, msg.as_ref());
    }
    pub fn ok(&self, msg: impl AsRef<str>) {
        self.line(Level::Ok, msg.as_ref());
    }
    pub fn warn(&self, msg: impl AsRef<str>) {
        self.line(Level::Warn, msg.as_ref());
    }
    pub fn error(&self, msg: impl AsRef<str>) {
        self.line(Level::Error, msg.as_ref());
    }
    pub fn detail(&self, msg: impl AsRef<str>) {
        self.line(Level::Detail, &format!("    {}", msg.as_ref()));
    }
}

// ---- registry ----------------------------------------------------------------------------------

pub struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

pub fn hive_name(h: HKEY) -> &'static str {
    if h == HKEY_LOCAL_MACHINE { "HKLM" } else { "HKCU" }
}

pub fn open(hive: HKEY, path: &str, write: bool) -> Option<Key> {
    let mut k = HKEY::default();
    let sam = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    let r = unsafe { RegOpenKeyExW(hive, &HSTRING::from(path), None, sam, &mut k) };
    (r == ERROR_SUCCESS).then_some(Key(k))
}

pub fn create(hive: HKEY, path: &str) -> Option<Key> {
    let mut k = HKEY::default();
    let r = unsafe { RegCreateKeyExW(hive, &HSTRING::from(path), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE, None, &mut k, None) };
    (r == ERROR_SUCCESS).then_some(Key(k))
}

impl Key {
    /// Every value as (name, type, text form of the data). Buffers are sized from the key's
    /// longest name and value, so a large value (e.g. binary data) does not end the listing early.
    pub fn values(&self) -> Vec<(String, u32, String)> {
        let mut out = Vec::new();
        let (mut max_name, mut max_data) = (0u32, 0u32);
        unsafe {
            let _ = RegQueryInfoKeyW(self.0, None, None, None, None, None, None, None, Some(&mut max_name), Some(&mut max_data), None, None);
        }
        let mut name = vec![0u16; max_name as usize + 2];
        let mut data = vec![0u8; max_data as usize + 4];
        let mut i = 0;
        loop {
            let mut name_len = name.len() as u32;
            let mut ty = 0u32;
            let mut data_len = data.len() as u32;
            let r = unsafe {
                RegEnumValueW(self.0, i, Some(PWSTR(name.as_mut_ptr())), &mut name_len, None, Some(&mut ty), Some(data.as_mut_ptr()), Some(&mut data_len))
            };
            if r == ERROR_MORE_DATA && name.len() < 1 << 16 {
                // Changed since the size query: grow and read this index again.
                name.resize(name.len() * 2 + 64, 0);
                data.resize(data.len() * 2 + 256, 0);
                continue;
            }
            if r != ERROR_SUCCESS {
                break;
            }
            i += 1;
            let n = String::from_utf16_lossy(&name[..name_len as usize]);
            let text = match REG_VALUE_TYPE(ty) {
                REG_SZ | REG_EXPAND_SZ => {
                    let w: Vec<u16> = data[..data_len as usize].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    crate::util::from_wide(&w)
                }
                REG_DWORD if data_len >= 4 => format!("0x{:08X}", u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
                _ => data[..data_len as usize].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "),
            };
            out.push((n, ty, text));
        }
        out
    }

    pub fn set_string(&self, name: &str, value: &str) -> bool {
        let w = crate::util::wide(value);
        let bytes = unsafe { std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2) };
        unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, REG_SZ, Some(bytes)) == ERROR_SUCCESS }
    }

    pub fn set_dword(&self, name: &str, value: u32) -> bool {
        unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes())) == ERROR_SUCCESS }
    }

    pub fn delete_value(&self, name: &str) -> bool {
        unsafe { RegDeleteValueW(self.0, &HSTRING::from(name)) == ERROR_SUCCESS }
    }

    /// Copies a value from `src` to `name` here, keeping its registry type and raw bytes.
    pub fn copy_value(&self, src: &Key, from: &str, name: &str) -> bool {
        unsafe {
            let mut ty = REG_VALUE_TYPE(0);
            let mut len = 0u32;
            let q = RegQueryValueExW(src.0, &HSTRING::from(from), None, Some(&mut ty), None, Some(&mut len));
            if q != ERROR_SUCCESS {
                return false;
            }
            let mut data = vec![0u8; len as usize];
            let q = RegQueryValueExW(src.0, &HSTRING::from(from), None, Some(&mut ty), Some(data.as_mut_ptr()), Some(&mut len));
            q == ERROR_SUCCESS && RegSetValueExW(self.0, &HSTRING::from(name), None, ty, Some(&data[..len as usize])) == ERROR_SUCCESS
        }
    }
}

pub fn delete_tree(hive: HKEY, path: &str) -> bool {
    unsafe { RegDeleteTreeW(hive, &HSTRING::from(path)) == ERROR_SUCCESS }
}

// ---- shortcut ----------------------------------------------------------------------------------

/// Creates a .lnk file pointing at `target`.
pub fn create_shortcut(lnk: &std::path::Path, target: &std::path::Path, description: &str) -> windows::core::Result<()> {
    // Balanced COM initialisation: uninitialised again only if this call initialised it.
    struct Com(bool);
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    unsafe {
        let _com = Com(CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok());
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        if let Some(dir) = target.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetDescription(&HSTRING::from(description))?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

/// CRC-32 (IEEE), shown in the logs to verify extracted files.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub fn fmt_bytes(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out + " bytes"
}

/// Full path of a process's program file, if it can be read.
pub fn process_image(pid: u32) -> Option<PathBuf> {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Same file (case-insensitive, after resolving `..`, links and short names where possible).
pub fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    norm(a) == norm(b)
}

/// Ids of the running processes whose program file is exactly `exe` (never this process).
pub fn processes_running(exe: &std::path::Path) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let name = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        while more {
            // Cheap filter by file name first; the full path decides.
            let pid = e.th32ProcessID;
            if pid != std::process::id()
                && crate::util::from_wide(&e.szExeFile).to_lowercase() == name
                && process_image(pid).is_some_and(|p| same_path(&p, exe))
            {
                out.push(pid);
            }
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    out
}

fn terminate(pid: u32) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) else { return false };
        let ok = TerminateProcess(h, 1).is_ok();
        let _ = WaitForSingleObject(h, 3000);
        let _ = windows::Win32::Foundation::CloseHandle(h);
        ok
    }
}

/// Window classes of BatteryHub's windows (the program and the windows it opens in their own
/// processes: dashboard, rename dialog), asked to close before files are replaced.
const WINDOW_CLASSES: [&str; 3] = ["BatteryHub.Dashboard", "BatteryHub.Rename", "BatteryHub.Taskbar.Main"];

/// Stops every process that runs `exe` (the installed program file, full path): first asks its
/// windows to close (WM_CLOSE), then, after ~3 s, ends what is still running - but only processes
/// whose program file is exactly `exe`. Another copy of BatteryHub elsewhere (a portable copy, the
/// original C++ BatteryHub) is never touched. Returns whether the main program was running.
pub fn stop_running(ed: &Edition, exe: &std::path::Path, log: &Log) -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};
    log.info(format!("Looking for running copies of {}", exe.display()));
    let ours = |h: windows::Win32::Foundation::HWND| {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        process_image(pid).is_some_and(|p| same_path(&p, exe))
    };
    let mut was_running = false;
    let mut asked = false;
    for class in WINDOW_CLASSES {
        let class_w = HSTRING::from(class);
        let mut after = None;
        unsafe {
            while let Ok(h) = FindWindowExW(None, after, &class_w, None) {
                if h.0.is_null() {
                    break;
                }
                after = Some(h);
                if ours(h) {
                    was_running |= class == ed.window_class;
                    asked = true;
                    log.detail(format!("asking {class} (window {:?}) to close", h.0));
                    let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                } else {
                    log.detail(format!("skipping a {class} window of another program file"));
                }
            }
        }
    }
    for i in 0..30 {
        if processes_running(exe).is_empty() {
            if asked {
                log.ok(format!("{} exited after {} ms.", ed.name, i * 100));
                std::thread::sleep(std::time::Duration::from_millis(300)); // let Windows release the file
            } else {
                log.detail("not running");
            }
            return was_running;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for pid in processes_running(exe) {
        log.warn(format!("Process {pid} ({}) did not exit in time, ending it...", exe.display()));
        if terminate(pid) {
            log.ok(format!("Process {pid} ended."));
        } else {
            log.error(format!("Could not end process {pid}."));
        }
        was_running = true;
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    was_running
}

/// Writes `data` to `path` without ever leaving a broken file there: it goes to a temporary file
/// in the same folder first, is flushed to disk, read back and compared (size and SHA-256), and
/// only then moved over `path` in one step (retried for a few seconds while the old file is still
/// in use). On any failure `path` keeps its old content and the temporary file is removed.
/// `progress` gets the share written so far (0..1).
pub fn install_file(path: &std::path::Path, data: &[u8], log: &Log, progress: &dyn Fn(f32)) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder")?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.new", std::process::id()));
    let expected = crate::update::sha256_hex(data)?;
    log.info(format!("Writing {} ({}) via {}", path.display(), fmt_bytes(data.len() as u64), tmp.display()));
    log.detail(format!("SHA-256 {expected}"));
    let result = (|| -> Result<(), String> {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        const CHUNK: usize = 64 * 1024;
        for (i, chunk) in data.chunks(CHUNK).enumerate() {
            f.write_all(chunk).map_err(|e| format!("write error: {e}"))?;
            progress(((i * CHUNK + chunk.len()) as f32 / data.len().max(1) as f32).min(1.0));
        }
        f.sync_all().map_err(|e| format!("cannot flush to disk: {e}"))?;
        drop(f);
        let back = std::fs::read(&tmp).map_err(|e| format!("cannot read back: {e}"))?;
        let got = crate::update::sha256_hex(&back)?;
        if back.len() != data.len() || got != expected {
            return Err(format!("the written file is not complete or not correct (SHA-256 {got})"));
        }
        log.ok("Written and verified.");
        let mut last = String::new();
        for attempt in 1..=20 {
            match crate::util::replace_file(&tmp, path) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last = e.to_string();
                    log.warn(format!("{} is in use ({e}), retrying ({attempt}/20)...", path.display()));
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        }
        Err(format!("cannot replace {}: {last}", path.display()))
    })();
    match &result {
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            log.error(format!("{e} - {} was left as it was.", path.display()));
        }
        Ok(()) => log.ok(format!("Installed {}", path.display())),
    }
    result
}

/// Compares dotted versions ("1.0.0" < "1.0.1" < "1.1").
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |v: &str| -> Vec<u32> { v.trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect() };
    let (mut x, mut y) = (parse(a), parse(b));
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    x.cmp(&y)
}

/// Deletes a key only when it has no subkeys left (e.g. HKCU\Software\BatteryHub after both
/// editions were removed). Returns whether it was deleted.
pub fn delete_key_if_empty(hive: HKEY, path: &str) -> bool {
    unsafe { RegDeleteKeyW(hive, &HSTRING::from(path)) == ERROR_SUCCESS }
}

impl Key {
    pub fn get(&self, name: &str) -> Option<String> {
        self.values().into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)).map(|v| v.2)
    }
}

/// Product version from an exe's version resource ("1.0.1" or "1.0.1.1"), if it has one.
pub fn file_version(path: &std::path::Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW};
    let name = HSTRING::from(path.as_os_str());
    unsafe {
        let size = GetFileVersionInfoSizeW(&name, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&name, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(data.as_ptr().cast(), windows::core::w!("\\"), &mut info, &mut len).as_bool() || info.is_null() {
            return None;
        }
        let f = &*(info as *const VS_FIXEDFILEINFO);
        let v = format!("{}.{}.{}.{}", f.dwProductVersionMS >> 16, f.dwProductVersionMS & 0xFFFF, f.dwProductVersionLS >> 16, f.dwProductVersionLS & 0xFFFF);
        // "1.0.1.0" -> "1.0.1"; a fourth part is kept when set ("1.0.1.1").
        Some(v.strip_suffix(".0").map(str::to_string).unwrap_or(v))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_version_resource_of_an_exe() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let v = super::file_version(&std::path::Path::new(&windir).join(r"System32\notepad.exe"));
        assert!(v.is_some_and(|v| v.split('.').count() >= 3));
        assert!(super::file_version(std::path::Path::new(r"C:\does\not\exist.exe")).is_none());
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn create_write_read_delete() {
        let path = r"Software\BatteryHub-Test-Registry";
        let k = create(HKCU, path).expect("create");
        assert!(k.set_dword("D", 7), "set_dword");
        assert!(k.set_string("S", "abc"), "set_string");
        drop(k);
        let r = open(HKCU, path, false).expect("open");
        assert_eq!(r.get("S").as_deref(), Some("abc"));
        assert_eq!(r.get("D").as_deref(), Some("0x00000007"));
        drop(r);
        assert!(delete_tree(HKCU, path));
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    fn log() -> Log {
        Log { start: Instant::now(), file: std::sync::Mutex::new(None), sink: None }
    }

    #[test]
    fn file_is_replaced_whole_and_no_temp_file_stays() {
        let dir = std::env::temp_dir().join(format!("bh-install-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("program.exe");
        std::fs::write(&target, b"old").unwrap();
        let data = vec![7u8; 300_000];
        let steps = std::sync::Mutex::new(Vec::new());
        install_file(&target, &data, &log(), &|f| steps.lock().unwrap().push(f)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), data);
        assert_eq!(*steps.lock().unwrap().last().unwrap(), 1.0);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_replaced_keeps_its_old_content() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("bh-install-locked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("program.exe");
        std::fs::write(&target, b"old program").unwrap();
        // Held open without delete sharing, like a running program file: the move must fail.
        let hold = std::fs::OpenOptions::new().read(true).share_mode(0x1).open(&target).unwrap();
        let r = install_file(&target, b"new program", &log(), &|_| {});
        assert!(r.is_err());
        drop(hold);
        assert_eq!(std::fs::read(&target).unwrap(), b"old program");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the temporary file was removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn processes_are_matched_by_full_path_only() {
        let me = std::env::current_exe().unwrap();
        assert!(process_image(std::process::id()).is_some_and(|p| same_path(&p, &me)));
        // This process is never in the list, and a copy elsewhere with the same name never matches.
        assert!(!processes_running(&me).contains(&std::process::id()));
        let elsewhere = std::env::temp_dir().join("nowhere").join(me.file_name().unwrap());
        assert!(processes_running(&elsewhere).is_empty());
        assert!(processes_running(std::path::Path::new(r"C:\Windows\explorer.exe")).len() <= 64);
    }

    #[test]
    fn values_lists_past_large_values() {
        let path = r"Software\BatteryHub-Test-LargeValue";
        let k = create(HKCU, path).unwrap();
        // A large binary value first (like the old 24-hour history), then a short one.
        assert!(crate::util::reg_set_binary(path, "Big", &vec![1u8; 20_000]));
        assert!(k.set_string("Version", "1.5"));
        assert_eq!(k.get("Version").as_deref(), Some("1.5"));
        assert_eq!(k.values().len(), 2);
        drop(k);
        assert!(delete_tree(HKCU, path));
    }
}
