//! BatteryHub setup engine: install, update, reinstall, downgrade (to the version in this setup,
//! or to any release from GitHub), and uninstall, per user (no administrator rights). The window
//! (window.rs) and silent mode (/S) both drive it; every file and registry entry it creates or
//! removes is logged with its full path (%TEMP%\BatteryHub-Setup.log).
//!
//! Command line: /install | /update | /uninstall, /S (silent), /autostart=yes|no,
//! /desktop=yes|no, /nolaunch, /nonotify (no "Update finished" message from the updated program),
//! /purge (uninstall: also remove settings and battery history), /versions (open the version list).
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use batteryhub::install::{self as inst, Edition, HKCU, Log};

// ---- options -----------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Install,
    Update,
    Uninstall,
}

#[derive(Clone, Debug, Default)]
pub struct Opts {
    pub action: Option<Action>,
    pub silent: bool,
    pub autostart: Option<bool>,
    pub desktop: Option<bool>,
    pub launch: bool,
    pub notify: bool,
    pub purge: bool,
    pub versions: bool,
}

pub fn parse_opts(args: impl Iterator<Item = String>) -> Opts {
    let mut o = Opts { launch: true, notify: true, ..Default::default() };
    for a in args {
        match a.to_lowercase().as_str() {
            "/install" | "--install" => o.action = Some(Action::Install),
            "/update" | "--update" => o.action = Some(Action::Update),
            "/uninstall" | "--uninstall" => o.action = Some(Action::Uninstall),
            "/s" | "--silent" => o.silent = true,
            "/autostart=yes" | "--autostart" => o.autostart = Some(true),
            "/autostart=no" | "--no-autostart" => o.autostart = Some(false),
            "/desktop=yes" => o.desktop = Some(true),
            "/desktop=no" => o.desktop = Some(false),
            "/nolaunch" | "--no-launch" => o.launch = false,
            "/nonotify" | "--no-notify" => o.notify = false,
            "/purge" => o.purge = true,
            "/versions" => o.versions = true,
            _ => {}
        }
    }
    o
}

// ---- what is installed -------------------------------------------------------------------------

/// What is on this PC for one edition.
#[derive(Clone, Debug)]
pub struct State {
    pub dir: PathBuf,
    pub exe: PathBuf,
    pub installed: bool,
    pub version: Option<String>,
    pub desktop: bool,
    pub start_menu: bool,
    pub autostart: bool,
    pub running: bool,
    /// The original C++ BatteryHub (reported, never touched).
    pub original: Option<PathBuf>,
}

pub fn read_state(ed: &Edition) -> State {
    let key = inst::open(HKCU, ed.settings_key, false);
    let dir = key.as_ref().and_then(|k| k.get("InstallLocation")).map(PathBuf::from).unwrap_or_else(|| ed.default_dir());
    let exe = dir.join(ed.exe);
    let installed = exe.exists();
    // The program file's own version resource is the truth (the registry can be stale, e.g. after
    // a file was replaced by hand); the version saved at install time is the fallback.
    let version = installed.then(|| inst::file_version(&exe).or_else(|| key.as_ref().and_then(|k| k.get("Version")))).flatten();
    let autostart = ed.run_value.is_some_and(|v| inst::open(HKCU, inst::RUN_KEY, false).and_then(|k| k.get(v)).is_some());
    let running = installed && !inst::processes_running(&exe).is_empty();
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    let original = [
        PathBuf::from(&pf).join(r"BatteryHub\BatteryHub.exe"),
        PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join(r"Programs\BatteryHub\BatteryHub.exe"),
    ]
    .into_iter()
    .find(|p| p.exists());
    State { dir, exe, installed, version, desktop: ed.desktop_shortcut().exists(), start_menu: ed.start_menu_shortcut().exists(), autostart, running, original }
}

/// What installing this setup's version means for what is installed now.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Install,
    /// Installed version (older than this setup).
    Update(String),
    Reinstall,
    /// Installed version (newer than this setup): installing goes back to an older version.
    Downgrade(String),
}

pub fn plan(st: &State) -> Plan {
    plan_for(st.installed, st.version.as_deref(), inst::VERSION)
}

pub fn plan_for(installed: bool, version: Option<&str>, this: &str) -> Plan {
    match (installed, version) {
        (false, _) => Plan::Install,
        (true, None) => Plan::Reinstall,
        (true, Some(v)) => match inst::version_cmp(v, this) {
            std::cmp::Ordering::Less => Plan::Update(v.to_string()),
            std::cmp::Ordering::Equal => Plan::Reinstall,
            std::cmp::Ordering::Greater => Plan::Downgrade(v.to_string()),
        },
    }
}

/// What the user chose (or the defaults for silent mode and updates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Choices {
    pub desktop: bool,
    pub start_menu: bool,
    pub autostart: bool,
    pub launch: bool,
    pub notify: bool,
}

/// Defaults: an existing installation keeps its shortcuts and autostart as they are; a new one
/// gets both shortcuts and autostart. Command-line options override.
pub fn default_choices(st: &State, o: &Opts) -> Choices {
    Choices {
        desktop: o.desktop.unwrap_or(if st.installed { st.desktop } else { true }),
        start_menu: true,
        autostart: o.autostart.unwrap_or(if st.installed { st.autostart } else { true }),
        launch: o.launch,
        notify: o.notify,
    }
}

// ---- helpers -----------------------------------------------------------------------------------

/// "2026-11-21 19:42 UTC" from a file time (no time-zone database needed).
pub fn fmt_time(t: SystemTime) -> String {
    let (y, m, d, hh, mm) = civil(t);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02} UTC")
}

fn civil(t: SystemTime) -> (i64, i64, i64, i64, i64) {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, rem / 3600, rem % 3600 / 60)
}

/// Progress of a running operation: share done (0..1) and what happens now.
pub type Progress<'a> = &'a dyn Fn(f32, &str);

/// Copy of the setup kept in the install folder: "Uninstall" in Windows Settings runs it.
pub const SETUP_COPY: &str = "BatteryHub-Setup.exe";

/// Entry in Settings > Apps > Installed apps (also what winget reads for the installed version).
pub fn apps_key(ed: &Edition) -> String {
    format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}", ed.name)
}

fn same_file(a: &Path, b: &Path) -> bool {
    inst::same_path(a, b) && a.exists()
}

/// Values written to the Settings > Apps entry. One entry per edition, under a fixed key name,
/// so updates, reinstalls and downgrades rewrite it instead of adding another one.
pub fn apps_values(ed: &Edition, dir: &Path, exe: &Path, setup: &Path, version: &str, today: &str) -> Vec<(&'static str, String)> {
    let quoted = format!("\"{}\"", setup.display());
    vec![
        ("DisplayName", ed.name.to_string()),
        ("DisplayVersion", version.to_string()),
        ("Publisher", "ferloom".to_string()),
        ("DisplayIcon", format!("\"{}\",0", exe.display())),
        ("InstallLocation", dir.display().to_string()),
        ("InstallDate", today.to_string()),
        ("UninstallString", format!("{quoted} /uninstall")),
        ("QuietUninstallString", format!("{quoted} /uninstall /S")),
        ("URLInfoAbout", format!("https://github.com/{}", batteryhub::update::REPO)),
        ("HelpLink", format!("https://github.com/{}/issues", batteryhub::update::REPO)),
        ("Comments", ed.description.to_string()),
    ]
}

/// Keeps a verified copy of this setup next to the program and registers the app in Settings >
/// Apps (name, version, publisher, icon, folder, uninstall commands, size).
fn register_app(ed: &Edition, log: &Log, dir: &Path, exe: &Path, created: &mut Vec<String>) -> Result<(), String> {
    let setup = dir.join(SETUP_COPY);
    let me = std::env::current_exe().map_err(|e| format!("cannot find this setup's own file: {e}"))?;
    if same_file(&me, &setup) {
        log.detail(format!("running from {}", setup.display()));
    } else {
        let bytes = std::fs::read(&me).map_err(|e| format!("cannot read this setup: {e}"))?;
        inst::install_file(&setup, &bytes, log, &|_| {})?;
        created.push(format!("setup     {}", setup.display()));
    }
    let key = apps_key(ed);
    log.info(format!("Registering in Settings > Apps: HKCU\\{key}"));
    let k = inst::create(HKCU, &key).ok_or("cannot create the Settings > Apps entry")?;
    let (y, m, d, _, _) = civil(SystemTime::now());
    for (n, v) in apps_values(ed, dir, exe, &setup, inst::VERSION, &format!("{y:04}{m:02}{d:02}")) {
        if k.set_string(n, &v) {
            log.detail(format!("set {n:<20} = {v}"));
        }
    }
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let size_kb = ((size(exe) + size(&setup)) / 1024) as u32;
    for (n, v) in [("NoModify", 1u32), ("NoRepair", 1), ("EstimatedSize", size_kb)] {
        k.set_dword(n, v);
        log.detail(format!("set {n:<20} = {v}"));
    }
    created.push(format!("registry  HKCU\\{key}"));
    Ok(())
}

fn save_info(ed: &Edition, log: &Log, dir: &Path, created: &mut Vec<String>) {
    log.info(format!("Saving install information to HKCU\\{}", ed.settings_key));
    match inst::create(HKCU, ed.settings_key) {
        Some(k) => {
            for (n, v) in [("InstallLocation", dir.display().to_string()), ("Version", inst::VERSION.to_string()), ("InstalledOn", fmt_time(SystemTime::now()))]
            {
                if k.set_string(n, &v) {
                    log.detail(format!("set {n:<16} = {v}"));
                }
            }
            created.push(format!("registry  HKCU\\{}", ed.settings_key));
        }
        None => log.warn("Could not save install information."),
    }
}

fn shortcut(log: &Log, lnk: &Path, exe: &Path, want: bool, description: &str, created: &mut Vec<String>) {
    if want {
        if let Some(dir) = lnk.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match inst::create_shortcut(lnk, exe, description) {
            Ok(_) => {
                log.ok(format!("{} -> {}", lnk.display(), exe.display()));
                created.push(format!("shortcut  {}", lnk.display()));
            }
            Err(e) => log.warn(format!("Could not create {}: {e}", lnk.display())),
        }
    } else if lnk.exists() && std::fs::remove_file(lnk).is_ok() {
        log.ok(format!("Removed {} (not wanted)", lnk.display()));
    }
}

fn set_autostart(ed: &Edition, log: &Log, exe: &Path, on: bool, created: &mut Vec<String>) {
    let Some(name) = ed.run_value else { return };
    let Some(run) = inst::open(HKCU, inst::RUN_KEY, true).or_else(|| inst::create(HKCU, inst::RUN_KEY)) else {
        log.error("Cannot open the autostart registry key.");
        return;
    };
    if on {
        let cmd = format!("\"{}\"", exe.display());
        if run.set_string(name, &cmd) {
            log.ok(format!("Autostart on: HKCU\\{}\\{name} = {cmd}", inst::RUN_KEY));
            created.push(format!("autostart HKCU\\{}\\{name}", inst::RUN_KEY));
        }
        // A "disabled" state left in Task Manager would keep it from starting.
        if let Some(sa) = inst::open(HKCU, inst::STARTUP_APPROVED_KEY, true)
            && sa.delete_value(name)
        {
            log.detail("cleared an old Task Manager 'disabled' state");
        }
    } else if run.delete_value(name) {
        log.ok("Autostart off: removed the autostart entry.");
    } else {
        log.ok("Autostart off.");
    }
}

pub fn launch(ed: &Edition, log: &Log, exe: &Path) {
    use std::process::Stdio;
    match std::process::Command::new(exe)
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => log.ok(format!("{} started (process id {}).", ed.name, c.id())),
        Err(e) => log.error(format!("Could not start {}: {e}", ed.name)),
    }
}

// ---- install / update / reinstall / downgrade --------------------------------------------------

/// Installs the program in this setup. Settings, names, warning levels and the battery history
/// are never touched (they live in the settings key and the data folder, not the program folder).
pub fn install(ed: &Edition, payload: &[u8], log: &Log, st: &State, c: &Choices, progress: Progress) -> Result<Vec<String>, String> {
    let mut created = Vec::new();
    let what = match plan(st) {
        Plan::Install => "Installing".to_string(),
        Plan::Update(v) => format!("Updating from v{v}"),
        Plan::Reinstall => "Reinstalling".to_string(),
        Plan::Downgrade(v) => format!("Going back from v{v}"),
    };
    log.step(format!("{what}: {} v{}", ed.name, inst::VERSION));
    progress(0.02, "Closing BatteryHub if it is running");
    let dir = if st.installed { st.dir.clone() } else { ed.default_dir() };
    let exe = dir.join(ed.exe);
    let was_running = inst::stop_running(ed, &exe, log);

    progress(0.12, "Preparing the install folder");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        log.ok(format!("Created {}", dir.display()));
    }

    progress(0.15, "Installing the program file");
    log.detail(format!("start menu shortcut before: {}, desktop shortcut before: {}", st.start_menu, st.desktop));
    if st.installed {
        log.detail(format!("old: {} (v{})", exe.display(), st.version.as_deref().unwrap_or("?")));
    }
    inst::install_file(&exe, payload, log, &|f| progress(0.15 + 0.5 * f, "Installing the program file"))?;
    created.push(format!("program   {}", exe.display()));

    progress(0.68, "Registering BatteryHub in Windows Settings");
    save_info(ed, log, &dir, &mut created);
    register_app(ed, log, &dir, &exe, &mut created)?;

    progress(0.85, "Creating shortcuts");
    shortcut(log, &ed.start_menu_shortcut(), &exe, c.start_menu, ed.description, &mut created);
    shortcut(log, &ed.desktop_shortcut(), &exe, c.desktop, ed.description, &mut created);

    progress(0.93, "Setting up autostart");
    set_autostart(ed, log, &exe, c.autostart, &mut created);

    // A new installation (not an update): the program says once that it is free.
    if !st.installed
        && let Some(k) = inst::create(HKCU, ed.settings_key)
    {
        k.set_string("FreeNotice", "1");
    }
    if c.launch || was_running {
        progress(0.97, "Starting BatteryHub");
        // The updated program says "Update finished: vOLD -> vNEW" once when it starts.
        if let (true, Some(old), Some(k)) = (c.notify, &st.version, inst::create(HKCU, ed.settings_key))
            && old != inst::VERSION
        {
            k.set_string("UpdatedFrom", old);
        }
        launch(ed, log, &exe);
    }
    progress(1.0, "Done");
    Ok(created)
}

/// Installs another release from GitHub (an older or a newer one): its setup is downloaded and
/// verified like an update (SHA256SUMS.txt, signature when this build has a key) and run silently
/// with the current choices. Its own setup then writes the program, the Settings > Apps entry
/// (same key, so no duplicate) and the shortcuts; settings and history are kept.
pub fn install_release(log: &Log, r: &batteryhub::update::Release, c: &Choices, progress: Progress) -> Result<(), String> {
    log.step(format!("Installing v{} from GitHub", r.version));
    progress(0.05, &format!("Downloading v{}", r.version));
    let setup = batteryhub::update::download(r)?;
    log.ok(format!("Downloaded and verified {}", setup.path.display()));
    progress(0.45, &format!("Running the setup of v{}", r.version));
    let yes = |b: bool| if b { "yes" } else { "no" };
    let mut args =
        vec!["/install".to_string(), "/S".into(), format!("/autostart={}", yes(c.autostart)), format!("/desktop={}", yes(c.desktop)), "/nonotify".into()];
    if !c.launch {
        args.push("/nolaunch".into());
    }
    log.info(format!("{} {}", setup.path.display(), args.join(" ")));
    let path = setup.path.clone();
    let mut child = {
        use std::os::windows::process::CommandExt;
        // The file stays locked against changes until the process has started.
        let child = std::process::Command::new(&path).args(&args).creation_flags(0x0800_0000).spawn().map_err(|e| format!("Could not start the setup: {e}"))?;
        drop(setup);
        child
    };
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
    if !status.success() {
        return Err(format!("The setup of v{} reported an error (exit code {:?}).", r.version, status.code()));
    }
    progress(1.0, "Done");
    Ok(())
}

// ---- uninstall ---------------------------------------------------------------------------------

/// Setup and the taskbar write one named HKCU Run value containing the quoted executable.
fn owned_run_command(command: &str, exe: &Path) -> bool {
    let command = command.trim();
    let target = command.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(command);
    inst::same_path(Path::new(target), exe)
}

fn remove_autostart(ed: &Edition, exe: &Path, log: &Log, removed: &mut Vec<String>) -> Result<(), String> {
    let Some(name) = ed.run_value else { return Ok(()) };
    let Some(command) = inst::open(HKCU, inst::RUN_KEY, false).and_then(|k| k.get(name)) else { return Ok(()) };
    if !owned_run_command(&command, exe) {
        return Ok(());
    }
    if !inst::open(HKCU, inst::RUN_KEY, true).is_some_and(|k| k.delete_value(name)) {
        return Err(format!("Could not remove HKCU\\{}\\{name}", inst::RUN_KEY));
    }
    log.ok(format!("Removed autostart entry {name}"));
    removed.push(format!("autostart HKCU\\{}\\{name}", inst::RUN_KEY));
    if inst::open(HKCU, inst::STARTUP_APPROVED_KEY, true).is_some_and(|k| k.delete_value(name)) {
        removed.push(format!("autostart HKCU\\{}\\{name}", inst::STARTUP_APPROVED_KEY));
    }
    Ok(())
}

fn remove_file(log: &Log, p: &Path, removed: &mut Vec<String>, label: &str) -> Result<(), String> {
    if !p.exists() {
        return Ok(());
    }
    for _ in 0..20 {
        if std::fs::remove_file(p).is_ok() {
            log.ok(format!("Deleted {}", p.display()));
            removed.push(format!("{label} {}", p.display()));
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let e = format!("Could not delete {}", p.display());
    log.error(&e);
    Err(e)
}

fn remove_dir_if_empty(log: &Log, p: &Path, removed: &mut Vec<String>) {
    if p.exists() && std::fs::remove_dir(p).is_ok() {
        log.ok(format!("Removed empty folder {}", p.display()));
        removed.push(format!("folder    {}", p.display()));
    }
}

/// Removes the Settings > Apps entry and the setup copy. When this setup *is* that copy (started
/// from Settings), it cannot delete itself: a hidden command deletes it a few seconds after exit.
fn unregister_app(ed: &Edition, log: &Log, dir: &Path, removed: &mut Vec<String>) {
    let key = apps_key(ed);
    if inst::open(HKCU, &key, false).is_some() && inst::delete_tree(HKCU, &key) {
        log.ok(format!("Deleted HKCU\\{key}"));
        removed.push(format!("registry  HKCU\\{key}"));
    }
    let setup = dir.join(SETUP_COPY);
    if !setup.exists() {
        return;
    }
    let me = std::env::current_exe().ok();
    if me.as_deref().is_some_and(|m| same_file(m, &setup)) {
        use std::os::windows::process::CommandExt;
        let cmd =
            format!("ping -n 6 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\" & rmdir \"{}\"", setup.display(), dir.display(), Edition::root_dir().display());
        // CREATE_NO_WINDOW | DETACHED_PROCESS
        match std::process::Command::new("cmd").arg("/c").raw_arg(&cmd).creation_flags(0x0800_0000 | 0x0000_0008).spawn() {
            Ok(_) => {
                log.ok(format!("{} will be deleted a few seconds after this window closes", setup.display()));
                removed.push(format!("setup     {}", setup.display()));
            }
            Err(e) => log.warn(format!("Could not schedule deleting {}: {e}", setup.display())),
        }
    } else {
        let _ = remove_file(log, &setup, removed, "setup    ");
    }
}

/// Install information in the settings key: removed on uninstall even when the settings stay.
const INSTALL_VALUES: [&str; 4] = ["InstallLocation", "Version", "InstalledOn", "UpdatedFrom"];

fn owns_legacy_installation(ed: &Edition) -> bool {
    ed.settings_key == inst::TASKBAR.settings_key
}

/// Leftovers of the pre-release "BatteryHub 2" test builds (taskbar edition only).
fn remove_legacy(log: &Log, removed: &mut Vec<String>) {
    if inst::open(HKCU, inst::RUN_KEY, true).is_some_and(|r| r.delete_value("BatteryHub2")) {
        removed.push(format!("autostart HKCU\\{}\\BatteryHub2", inst::RUN_KEY));
    }
    for key in [r"Software\BatteryHub2", r"Software\Microsoft\Windows\CurrentVersion\Uninstall\BatteryHub2"] {
        if inst::open(HKCU, key, false).is_some() && inst::delete_tree(HKCU, key) {
            removed.push(format!("registry  HKCU\\{key}"));
        }
    }
    let old_dir = Edition::root_dir().with_file_name("BatteryHub2");
    for f in ["batteryhub-tray.exe", "uninstall.exe"] {
        let p = old_dir.join(f);
        if p.exists() && std::fs::remove_file(&p).is_ok() {
            removed.push(format!("file      {}", p.display()));
        }
    }
    if old_dir.exists() && std::fs::remove_dir(&old_dir).is_ok() {
        removed.push(format!("folder    {}", old_dir.display()));
    }
    let old_lnk = inst::known_folder(&windows::Win32::UI::Shell::FOLDERID_Programs).join("BatteryHub 2.lnk");
    if old_lnk.exists() && std::fs::remove_file(&old_lnk).is_ok() {
        removed.push(format!("shortcut  {}", old_lnk.display()));
    }
    log.detail("checked old test-build leftovers");
}

/// Uninstalls: stops the program, removes its files, shortcuts, autostart and the Settings > Apps
/// entry. Settings (warning levels, names, options) and the battery history stay unless `purge`:
/// the user decides that in the uninstall window (or with /purge). Errors are collected and
/// reported; whatever can be removed still is.
pub fn uninstall(ed: &Edition, log: &Log, st: &State, purge: bool, progress: Progress) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    log.step(format!("Uninstalling {}", ed.name));
    progress(0.05, "Closing BatteryHub");
    inst::stop_running(ed, &st.exe, log);
    if !inst::processes_running(&st.exe).is_empty() {
        errors.push("BatteryHub is still running and could not be closed.".into());
    }

    progress(0.25, "Removing autostart entries");
    if let Err(e) = remove_autostart(ed, &st.exe, log, &mut removed) {
        errors.push(e);
    }

    progress(0.4, "Removing shortcuts");
    for lnk in [ed.desktop_shortcut(), ed.start_menu_shortcut()] {
        if let Err(e) = remove_file(log, &lnk, &mut removed, "shortcut ") {
            errors.push(e);
        }
    }
    remove_dir_if_empty(log, &Edition::start_menu_folder(), &mut removed);

    progress(0.55, "Removing program files");
    if let Err(e) = remove_file(log, &st.exe, &mut removed, "program  ") {
        errors.push(e);
    }
    unregister_app(ed, log, &st.dir, &mut removed);
    remove_dir_if_empty(log, &st.dir, &mut removed);
    remove_dir_if_empty(log, &Edition::root_dir(), &mut removed);

    progress(0.75, if purge { "Removing settings and battery history" } else { "Keeping your settings and battery history" });
    if purge {
        if inst::open(HKCU, ed.settings_key, false).is_some() && inst::delete_tree(HKCU, ed.settings_key) {
            log.ok(format!("Deleted HKCU\\{}", ed.settings_key));
            removed.push(format!("registry  HKCU\\{}", ed.settings_key));
        }
        let data = batteryhub::util::data_dir(ed.short);
        if data.exists() {
            match std::fs::remove_dir_all(&data) {
                Ok(()) => {
                    log.ok(format!("Deleted {}", data.display()));
                    removed.push(format!("data      {}", data.display()));
                }
                Err(e) => errors.push(format!("Could not delete {}: {e}", data.display())),
            }
        }
        if let Some(parent) = data.parent() {
            remove_dir_if_empty(log, parent, &mut removed);
        }
        inst::delete_key_if_empty(HKCU, inst::ROOT_KEY);
    } else if let Some(k) = inst::open(HKCU, ed.settings_key, true) {
        for v in INSTALL_VALUES {
            k.delete_value(v);
        }
        log.info(format!("Kept HKCU\\{} and {} (settings and battery history)", ed.settings_key, batteryhub::util::data_dir(ed.short).display()));
        drop(k);
        // Nothing of the user's left: no empty key behind.
        if inst::open(HKCU, ed.settings_key, false).is_some_and(|k| k.values().is_empty()) {
            let _ = inst::delete_key_if_empty(HKCU, ed.settings_key);
            inst::delete_key_if_empty(HKCU, inst::ROOT_KEY);
        }
    }

    progress(0.9, "Cleaning up");
    if owns_legacy_installation(ed) {
        remove_legacy(log, &mut removed);
    }
    progress(1.0, "Done");
    if errors.is_empty() { Ok(removed) } else { Err(errors.join("\n")) }
}

// ---- silent mode -------------------------------------------------------------------------------

/// /S: no window; the exit code says whether it worked (details in the log file).
pub fn run_silent(ed: &Edition, payload: &[u8], o: &Opts) -> i32 {
    let log = Log::new(&format!("{} {} - Setup (silent)", ed.name, inst::version_label()), None);
    let st = read_state(ed);
    let c = default_choices(&st, o);
    let none = |_: f32, _: &str| {};
    let r = match o.action {
        Some(Action::Uninstall) if !st.installed => Ok(Vec::new()),
        Some(Action::Uninstall) => uninstall(ed, &log, &st, o.purge, &none),
        Some(Action::Update) if !st.installed => Err("Nothing to update: not installed.".into()),
        _ => install(ed, payload, &log, &st, &c, &none),
    };
    match r {
        Ok(list) => {
            for l in list {
                log.detail(l);
            }
            0
        }
        Err(e) => {
            log.error(e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_matching_is_strict() {
        let exe = Path::new(r"C:\Apps\BatteryHub Taskbar\batteryhub-taskbar.exe");
        assert!(owned_run_command(&format!("\"{}\"", exe.display()), exe));
        assert!(owned_run_command(&exe.display().to_string(), exe));
        assert!(!owned_run_command(&format!("backup.exe --source \"{}\"", exe.display()), exe));
        assert!(!owned_run_command(r"D:\Other\batteryhub-taskbar.exe", exe));
        assert!(!owned_run_command(r"C:\Apps\BatteryHub Taskbar\backup.exe", exe));
    }

    #[test]
    fn only_taskbar_owns_the_legacy_installation() {
        let other = Edition { settings_key: r"Software\BatteryHub\SetupTest", ..inst::TASKBAR };
        assert!(owns_legacy_installation(&inst::TASKBAR));
        assert!(!owns_legacy_installation(&other));
    }

    #[test]
    fn plans_tell_update_from_downgrade() {
        assert_eq!(plan_for(false, None, "1.6"), Plan::Install);
        assert_eq!(plan_for(true, Some("1.5"), "1.6"), Plan::Update("1.5".into()));
        assert_eq!(plan_for(true, Some("1.6"), "1.6"), Plan::Reinstall);
        assert_eq!(plan_for(true, Some("1.6.0"), "1.6"), Plan::Reinstall);
        assert_eq!(plan_for(true, Some("1.7"), "1.6"), Plan::Downgrade("1.7".into()));
        assert_eq!(plan_for(true, Some("1.10"), "1.9"), Plan::Downgrade("1.10".into()));
        assert_eq!(plan_for(true, None, "1.6"), Plan::Reinstall);
    }

    #[test]
    fn options_and_defaults() {
        let o = parse_opts(["/uninstall", "/S", "/purge"].iter().map(|s| s.to_string()));
        assert_eq!((o.action, o.silent, o.purge), (Some(Action::Uninstall), true, true));
        let o = parse_opts(["/install", "/autostart=no", "/desktop=no", "/nolaunch"].iter().map(|s| s.to_string()));
        let fresh = State {
            dir: PathBuf::new(),
            exe: PathBuf::new(),
            installed: false,
            version: None,
            desktop: false,
            start_menu: false,
            autostart: false,
            running: false,
            original: None,
        };
        let c = default_choices(&fresh, &o);
        assert!(!c.autostart && !c.desktop && !c.launch && c.start_menu);
        // An update keeps what the user had.
        let had = State { installed: true, desktop: false, autostart: true, ..fresh.clone() };
        let c = default_choices(&had, &parse_opts(std::iter::empty()));
        assert!(c.autostart && !c.desktop);
        // Uninstall keeps settings and history unless asked.
        assert!(!parse_opts(["/uninstall", "/S"].iter().map(|s| s.to_string())).purge);
    }

    #[test]
    fn settings_apps_entry_values() {
        let t = inst::TASKBAR;
        let dir = PathBuf::from(r"C:\Users\u\AppData\Local\Programs\BatteryHub\Taskbar");
        let v = apps_values(&t, &dir, &dir.join(t.exe), &dir.join(SETUP_COPY), "1.6", "20261124");
        let get = |n: &str| v.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get("DisplayName"), "BatteryHub Taskbar");
        assert_eq!(get("DisplayVersion"), "1.6");
        assert_eq!(get("Publisher"), "ferloom");
        assert_eq!(get("InstallLocation"), dir.display().to_string());
        assert_eq!(get("UninstallString"), format!("\"{}\" /uninstall", dir.join(SETUP_COPY).display()));
        assert_eq!(get("QuietUninstallString"), format!("\"{}\" /uninstall /S", dir.join(SETUP_COPY).display()));
        assert!(get("DisplayIcon").ends_with("batteryhub-taskbar.exe\",0"));
        // One fixed key per edition: an update or a downgrade rewrites it, never adds another.
        assert_eq!(apps_key(&t), r"Software\Microsoft\Windows\CurrentVersion\Uninstall\BatteryHub Taskbar");
    }

    #[test]
    fn times() {
        assert_eq!(fmt_time(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_795_283_000)), "2026-11-21 17:43 UTC");
    }

    /// A whole life cycle with a separate test edition (own folder, keys, names - the real
    /// BatteryHub is never touched): install, user data appears, reinstall, uninstall keeping the
    /// data, install again, uninstall with purge.
    #[test]
    fn install_update_uninstall_cycle() {
        let ed = Edition {
            name: "BatteryHub Setup Test",
            short: "SetupTest",
            exe: "bh-setup-test.exe",
            settings_key: r"Software\BatteryHub\SetupTest",
            window_class: "BatteryHub.SetupTest.Main",
            run_value: Some("BatteryHub Setup Test"),
            description: "test",
        };
        let log = Log::new("test", None);
        let none = |_: f32, _: &str| {};
        let c = Choices { desktop: false, start_menu: false, autostart: true, launch: false, notify: false };
        let payload_a = b"MZ first build".repeat(1000);
        let st = read_state(&ed);
        assert_eq!(plan(&st), Plan::Install);
        install(&ed, &payload_a, &log, &st, &c, &none).unwrap();
        let st = read_state(&ed);
        assert!(st.installed && st.autostart);
        assert_eq!(std::fs::read(&st.exe).unwrap(), payload_a);
        assert_eq!(st.version.as_deref(), Some(inst::VERSION));
        assert_eq!(plan(&st), Plan::Reinstall);
        let apps = inst::open(HKCU, &apps_key(&ed), false).unwrap();
        assert_eq!(apps.get("DisplayVersion").as_deref(), Some(inst::VERSION));
        assert!(apps.get("UninstallString").unwrap().contains(SETUP_COPY));
        drop(apps);
        assert!(st.dir.join(SETUP_COPY).exists());
        // The user's own data appears: a warning level and the history file.
        inst::create(HKCU, &format!(r"{}\Thresholds", ed.settings_key)).unwrap().set_dword("dev", 20);
        let data = batteryhub::util::data_dir(ed.short);
        batteryhub::history::write_atomic(&data.join("History.bin"), b"BHH2").unwrap();
        // Reinstall with a new build: replaced in place, still one Settings > Apps entry.
        let payload_b = b"MZ second build".repeat(1000);
        install(&ed, &payload_b, &log, &st, &c, &none).unwrap();
        assert_eq!(std::fs::read(&st.exe).unwrap(), payload_b);
        let entries = inst::open(HKCU, r"Software\Microsoft\Windows\CurrentVersion\Uninstall", false)
            .map(|k| {
                let mut n = 0;
                for name in ["BatteryHub Setup Test", "BatteryHub Setup Test (2)"] {
                    if inst::open(HKCU, &format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{name}"), false).is_some() {
                        n += 1;
                    }
                }
                drop(k);
                n
            })
            .unwrap_or(0);
        assert_eq!(entries, 1);
        // No temporary files left in the program folder.
        let leftovers = std::fs::read_dir(&st.dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".new")).count();
        assert_eq!(leftovers, 0);
        // Uninstall, keeping settings and history.
        let st = read_state(&ed);
        uninstall(&ed, &log, &st, false, &none).unwrap();
        assert!(!st.exe.exists() && !st.dir.join(SETUP_COPY).exists());
        assert!(inst::open(HKCU, &apps_key(&ed), false).is_none());
        assert!(inst::open(HKCU, inst::RUN_KEY, false).and_then(|k| k.get("BatteryHub Setup Test")).is_none());
        let k = inst::open(HKCU, ed.settings_key, false).expect("settings kept");
        assert!(k.get("Version").is_none() && k.get("InstallLocation").is_none());
        drop(k);
        assert!(inst::open(HKCU, &format!(r"{}\Thresholds", ed.settings_key), false).and_then(|k| k.get("dev")).is_some());
        assert!(data.join("History.bin").exists());
        assert!(!read_state(&ed).installed);
        // Install again: the kept settings are there; then remove everything.
        install(&ed, &payload_a, &log, &read_state(&ed), &c, &none).unwrap();
        uninstall(&ed, &log, &read_state(&ed), true, &none).unwrap();
        assert!(inst::open(HKCU, ed.settings_key, false).is_none());
        assert!(!data.exists());
        assert!(!ed.default_dir().exists());
    }
}
