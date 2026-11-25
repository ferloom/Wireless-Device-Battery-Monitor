#![windows_subsystem = "windows"]

//! Small, event-driven launcher for the isolated XAML preview. It owns the app and native host
//! lifetimes, and restores a stable BatteryHub only if it had to close one before starting.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LPARAM, WAIT_OBJECT_0, WPARAM},
        System::Threading::{
            CREATE_NO_WINDOW, CreateMutexW, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ReleaseMutex,
            WaitForMultipleObjects, WaitForSingleObject,
        },
        UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE},
    },
    core::{HSTRING, w},
};

struct KernelHandle(HANDLE);

impl Drop for KernelHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn app_paths() -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let run = exe.parent().ok_or("supervisor has no parent directory")?;
    let candidate = run.parent().ok_or("supervisor is not in candidate\\run")?;
    let app = run.join("BatteryHub-Experimental.exe");
    let host = candidate.join("host").join("BatteryHubTaskbarHostTest.exe");
    let profile = candidate.join("profile");
    if !app.is_file() || !host.is_file() {
        return Err("experimental app or native host is missing".into());
    }
    Ok((app, host, profile))
}

fn log(profile: &Path, message: &str) {
    let path = profile.join("logs").join("supervisor.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {message}", chrono_lite_timestamp());
    }
}

fn chrono_lite_timestamp() -> String {
    // Keep the helper dependency-free; the native host writes precise UTC timestamps.
    format!("unix_ms={}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis())
}

fn stable_exe() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?;
    Ok(PathBuf::from(local).join("Programs\\BatteryHub\\Taskbar\\batteryhub-taskbar.exe"))
}

fn owned_windows(class: &str) -> Vec<(u32, windows::Win32::Foundation::HWND)> {
    let class = HSTRING::from(class);
    let mut result = Vec::new();
    let mut after = None;
    loop {
        let Ok(hwnd) = (unsafe { FindWindowExW(None, after, &class, None) }) else { break };
        if hwnd.0.is_null() {
            break;
        }
        after = Some(hwnd);
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if pid != 0 {
            result.push((pid, hwnd));
        }
    }
    result
}

fn ask_stable_to_close(exe: &Path, profile: &Path) -> Result<bool, String> {
    let processes = batteryhub::install::processes_running(exe);
    if processes.len() > 1 {
        return Err("multiple stable BatteryHub processes are running".into());
    }
    let Some(&pid) = processes.first() else { return Ok(false) };
    let hwnd = owned_windows("BatteryHub.Taskbar.Main")
        .into_iter()
        .find_map(|(window_pid, hwnd)| (window_pid == pid).then_some(hwnd))
        .ok_or("stable BatteryHub is running but its main window was not found; refusing HID overlap")?;
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) }
        .map_err(|e| format!("cannot wait for stable BatteryHub: {e}"))?;
    let process = KernelHandle(process);
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.map_err(|e| format!("cannot request stable BatteryHub to close: {e}"))?;
    if unsafe { WaitForSingleObject(process.0, 20_000) } != WAIT_OBJECT_0 {
        return Err("stable BatteryHub did not close within 20 seconds; preview was not started".into());
    }
    log(profile, "stable instance closed before preview startup");
    Ok(true)
}

fn send_close(class: &str) {
    let class = HSTRING::from(class);
    if let Ok(hwnd) = unsafe { FindWindowExW(None, None, &class, None) } {
        if !hwnd.0.is_null() {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn stop_native_host(host: &Path, child: &mut Child) -> bool {
    let _ = Command::new(host).arg("--stop").creation_flags(CREATE_NO_WINDOW.0).status();
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, child.id()) }) else {
        return child.try_wait().ok().flatten().is_some();
    };
    let handle = KernelHandle(handle);
    let stopped = unsafe { WaitForSingleObject(handle.0, 12_000) } == WAIT_OBJECT_0;
    if stopped {
        let _ = child.wait();
    }
    stopped
}

fn start_preview(app_path: &Path, host_path: &Path, profile: &Path) -> Result<(Child, Option<Child>), String> {
    fs::create_dir_all(profile.join("logs")).map_err(|e| e.to_string())?;
    let trace = profile.join("logs").join("publisher-xaml.jsonl");
    let app = Command::new(app_path)
        .arg("--xaml-preview")
        .current_dir(app_path.parent().ok_or("app path has no parent directory")?)
        .env("BH_EXPERIMENTAL_PROFILE", profile)
        .env("BATTERYHUB_XAML_TRACE_PATH", trace)
        .env_remove("BH_EXPERIMENTAL_DEMO")
        .env_remove("BH_DEBUG")
        .env_remove("BH_SECOND_INSTANCE")
        .spawn()
        .map_err(|e| format!("cannot start experimental BatteryHub: {e}"))?;
    let app_pid = app.id().to_string();
    let host = Command::new(host_path)
        .args(["--serve", app_pid.as_str()])
        .current_dir(host_path.parent().ok_or("host path has no parent directory")?)
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .ok();
    Ok((app, host))
}

fn wait_for_host_start(host_path: &Path, profile: &Path, max: Duration) -> bool {
    // Bounded startup check only. Once attached, the supervisor waits on process handles.
    let log_path = host_path.parent().unwrap_or(Path::new(".")).join("BatteryHubExperimentalXamlService.log");
    let start = Instant::now();
    while start.elapsed() < max {
        if std::fs::read_to_string(&log_path).is_ok_and(|s| s.contains("event=service-attached")) {
            log(profile, "XAML host attached");
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    log(profile, "XAML host did not attach within startup deadline; app fallback remains available");
    false
}

fn wait_app_and_host(app: &mut Child, host: Option<&mut Child>) -> Result<(), String> {
    let app_handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, app.id()) }.map_err(|e| format!("cannot wait for preview app: {e}"))?;
    let app_handle = KernelHandle(app_handle);
    if let Some(host) = host {
        let host_handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, host.id()) }.map_err(|e| format!("cannot wait for native host: {e}"))?;
        let host_handle = KernelHandle(host_handle);
        let handles = [app_handle.0, host_handle.0];
        let result = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if result == WAIT_OBJECT_0 {
            return Ok(());
        }
        if result.0 == WAIT_OBJECT_0.0 + 1 {
            // Host failure leaves BatteryHub's traditional strip as fallback; wait for app close.
            return if unsafe { WaitForSingleObject(app_handle.0, INFINITE) } == WAIT_OBJECT_0 {
                Ok(())
            } else {
                Err("waiting for preview app failed after host exit".into())
            };
        }
        return Err(format!("waiting for preview app/host failed: {result:?}"));
    }
    if unsafe { WaitForSingleObject(app_handle.0, INFINITE) } == WAIT_OBJECT_0 { Ok(()) } else { Err("waiting for preview app failed".into()) }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--stop") {
        send_close("BatteryHub.Experimental.Taskbar.Main");
        return Ok(());
    }
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\BatteryHub.Experimental.PreviewSupervisor.v1")) }
        .map_err(|e| format!("cannot create preview supervisor mutex: {e}"))?;
    let mutex = KernelHandle(mutex);
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err("an experimental preview supervisor is already running".into());
    }

    let (app_path, host_path, profile) = app_paths()?;
    fs::create_dir_all(profile.join("logs")).map_err(|e| e.to_string())?;
    let stable = stable_exe()?;
    let restore_stable = ask_stable_to_close(&stable, &profile)?;
    let (mut app, mut host) = match start_preview(&app_path, &host_path, &profile) {
        Ok(children) => children,
        Err(error) => {
            if restore_stable {
                let _ = Command::new(stable).spawn();
            }
            return Err(error);
        }
    };
    if host.is_some() {
        let _ = wait_for_host_start(&host_path, &profile, Duration::from_secs(90));
    }
    let wait_result = wait_app_and_host(&mut app, host.as_mut());
    let _ = app.wait();
    if let Some(host_child) = host.as_mut() {
        if !stop_native_host(&host_path, host_child) {
            log(&profile, "native host did not stop; stable restart withheld to avoid taskbar state conflict");
            return Err("native host did not detach; stable app was not restarted".into());
        }
    }
    if restore_stable {
        Command::new(&stable).spawn().map_err(|e| format!("cannot restore stable BatteryHub: {e}"))?;
        log(&profile, "stable BatteryHub restored after preview exit");
    }
    unsafe {
        let _ = ReleaseMutex(mutex.0);
    }
    wait_result
}

fn main() {
    let profile = app_paths().map(|(_, _, p)| p).unwrap_or_default();
    if let Err(error) = run() {
        if !profile.as_os_str().is_empty() {
            log(&profile, &format!("error: {error}"));
        }
    }
}
