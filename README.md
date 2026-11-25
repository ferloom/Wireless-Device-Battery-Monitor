<p align="center"><img src="res/batteryhub.png" width="112" alt="BatteryHub icon"></p>

<h1 align="center">BatteryHub v1.7.4</h1>

<p align="center">
<b>An ultra-lightweight Rust implementation of BatteryHub for Windows.</b><br>
Battery levels of your wireless mouse, headset, keyboard and controller, right on the taskbar,<br>
in about <b>0.2 MB of RAM</b>.
</p>

<p align="center">
<a href="../../releases/latest"><img src="https://img.shields.io/github/v/release/ferloom/Wireless-Device-Battery-Monitor?label=download&logo=windows" alt="Latest release"></a>
<a href="../../actions/workflows/ci.yml"><img src="../../actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<img src="https://img.shields.io/badge/written%20in-Rust-dea584?logo=rust" alt="Written in Rust">
<img src="https://img.shields.io/badge/license-GPL--3.0-blue" alt="GPL-3.0">
<a href="https://revolut.me/p2ps"><img src="https://img.shields.io/badge/support-Revolut-191C1F?logo=revolut&logoColor=white" alt="Support on Revolut"></a>
</p>

<p align="center">
<img src="res/screenshot-card.png" width="300" alt="Hover card: Logitech PRO 2 LIGHTSPEED, 71%, about 3 d 13 h left, level bar, Logitech receiver, chart of the last 12 hours and a 12-hour forecast"><br>
<img src="res/screenshot-taskbar.png" width="402" alt="BatteryHub on the Windows 11 taskbar next to the notification area: headset 72%, mouse 71%">
</p>

> [!IMPORTANT]
> **This is a recode of krisgfx9/Wireless-Device-Battery-Monitor** (the C++ BatteryHub by krisgfx9), not a fork and not the original.
> It rewrites the original in Rust. The idea, the look of the taskbar strip and the device protocols all come from the original project, and the credit for them goes to its authors.
>
> **The original is currently unavailable on GitHub.** On 22 November 2026 the krisgfx9 account was suspended by GitHub, which took the original repository offline with it. According to the original author, no reason was given; they have appealed, so the original may come back. Until then, the original author shares downloads through the project's Discord.
>
> This recode is still **not the original**, but it **is supported and will stay supported**: it keeps being developed and updated, with new devices added from [device requests](../../issues).

Developed by [@ferloom](https://github.com/ferloom) with the help of Claude (AI).

---

## Contents

[Download](#download) · [Support the project](#support-the-project) · [Problem with a new version?](#problem-with-a-new-version-downgrade-and-report-it) · [Features](#features) · [Using it](#using-it) · [Your own language](#translations-create-your-own-language) · [Setup](#setup-install--update--uninstall) · [Resource usage vs. the original](#resource-usage-vs-the-original) · [Taskbar compatibility](#taskbar-compatibility) · [Supported devices](#supported-devices) · [Unsupported devices](#unsupported-devices) · [Testing](#testing) · [Building](#building-from-source) · [Contributors](#contributors) · [License](#license)

## Download

Get **`BatteryHub-Taskbar-Setup-v1.7.4.exe`** from the [**latest release**](../../releases/latest), run it and click **Install**.

- No administrator rights needed (per-user install).
- Already installed? Use *Check for updates...* in the right-click menu, or run the new setup and click **Update**.
- It does not interfere with the original C++ BatteryHub if you have that installed too.
- Windows may show a SmartScreen warning because the file is not code-signed: *More info* → *Run anyway*.

## Support the project

BatteryHub is free and stays free. It is made by one person in their spare time, and every new device, fix and update takes hours of reading protocols and testing.

If BatteryHub is useful to you, a small tip helps a lot: it **lets me spend more time on it, so new devices and updates come faster**, and it supports me directly. Any amount is appreciated, thank you!

<p align="center">
<a href="https://revolut.me/p2ps"><img src="https://img.shields.io/badge/Revolut-Support%20BatteryHub-191C1F?style=for-the-badge&logo=revolut&logoColor=white" alt="Support BatteryHub on Revolut" height="40"></a><br>
<sub>Sends a tip through Revolut: <a href="https://revolut.me/p2ps">revolut.me/p2ps</a></sub>
</p>

Not able to tip? Reporting a device, testing a new version or sharing BatteryHub helps just as much.

## Problem with a new version? Downgrade and report it

If something breaks after an update, you can always go back to an older version:

- **From v1.6 on:** right-click BatteryHub → *Install another version...* (or *Other versions...* in the setup window) lists every release from GitHub; pick one and click **Go back to ...**. It is downloaded and checked against its SHA-256 checksum like an update.
- **By hand:** open [**Releases**](../../releases), download the setup of an earlier version (e.g. `BatteryHub-Taskbar-Setup-v1.5.exe`) and run it. It finds the newer version you have installed and offers to reinstall with its own (older) version.

Your warning levels, device names, options and battery history are kept either way.

*Check for updates* never offers an older version, so the downgrade stays in place until you update again. Versions before v1.1 do not have *Check for updates*; to come back later, run the newer setup.

**Please also report the problem** in [**Issues**](../../issues/new) so it can be fixed in the next version. It helps to include:

- the version that has the problem and the one that works (both are shown at the top of the setup window);
- Windows version and taskbar (Windows 11, Windows 10, ExplorerPatcher: normal or small taskbar);
- your devices (e.g. "Logitech PRO 2 LIGHTSPEED on its receiver", "AirPods over Bluetooth");
- what happened and what you expected, ideally with a screenshot of the taskbar.

## Features

- **On the taskbar, next to the clock:** one icon and percentage per device (`🎧 27%  🖱 96%`), in the original's look (Segoe Fluent Icons, Segoe UI Variable, ClearType), following light / dark mode and DPI.
- **Hover a device** for a card with its name, level, **estimated time left** (or time to full while charging), connection and a chart of the **last 12 hours with now in the middle and a forecast of the next 12** (dashed; charging in blue), fading in and cross-fading between devices like the taskbar's window previews; the widget lights up like the taskbar's own buttons.
- **Move the mouse onto the card** and it stays open: along the chart it shows each hour's level and its change from the hour before, measured in the past and forecast ahead. The estimate is made from what has been measured so far, from the first 1% drop on, and gets more exact over time; once there is an hour of data with a 5% drop, the average of all measured stretches on battery is used.
- **Earbuds** (AirPods, SteelSeries GameBuds): their own icon, with the bud that does not report (in its case, or off) greyed out; the taskbar shows the weaker bud; the card shows each bud's level next to the name (`L 80%  R 75%`) and a chart per bud (left over right), and so does the dashboard. AirPods are re-read every 10 s, so a bud taken out of the case shows within seconds.
- **Device dashboard** (right-click a device → *Dashboard...*): a bar of all devices at the top to switch between them, level and state, a 24 h / 3-day / 7-day chart with charging marked, the charge and discharge sessions, and statistics (typical battery life, time left, charge and discharge rates per hour). Where there is too little data it says so instead of guessing.
- **7-day battery history:** one point per change (at least every 10 minutes), kept 7 days, in `%LOCALAPPDATA%\BatteryHub\Taskbar\History.bin`. Times when BatteryHub was not running stay empty: nothing is filled in. Times are stored in UTC and shown in the time zone Windows is set to, so moving to another country or a daylight-saving switch does not shift the history.
- **Rename a device** (shown everywhere in BatteryHub only; the device itself is not changed) and **Warn at 5%**: one notification when a device on battery drops to 5% or less, again only after it was charged.
- **Charging at a glance:** green ⚡ while charging, blue ⚡ at 100%, red percentage at or below the device's warning level.
- **Per-device warning level:** 50 / 40 / 30 / 20 / 15 / 10 / 5%.
- **Reacts at once** when a receiver, cable or Bluetooth device connects or disconnects, and within ~10 s when a headset on a 2.4 GHz dongle is switched on or off.
- **Refresh interval** of 1 / 5 / 10 / 30 minutes or 1 hour.
- **More monitors:** show the widget on all monitors, the main one only, or only one other monitor; on every taskbar it sits right next to that taskbar's clock and appears at the same moment.
- **Notification-area icons instead** (optional): one icon per device that Windows itself places, so it can be dragged between other tray icons and never overlaps other taskbar widgets.
- **Next to other taskbar widgets:** when another program places its own widget in the taskbar (network or CPU meters...), BatteryHub sits left of it instead of on top of it.
- **Position and background:** move the widget further from the clock or up / down (presets, or in 10 px steps), and an optional plate behind it for transparent taskbars.
- **Start with Windows** and **automatic update check** switched on or off right from the menu.
- **Check for updates** from the menu, with a SHA-256-verified download.
- **21 languages** (English, Polski, Deutsch, Français, Español, Português (Brasil), Italiano, Nederlands, Čeština, Magyar, Svenska, Русский, Українська, Türkçe, 日本語, 한국어, 简体中文, 繁體中文, Tiếng Việt, Bahasa Indonesia, ไทย), following the Windows display language or chosen in the menu, and **your own**: *Language → Create your own language...* writes a file with every text to translate (see [Your own language](#translations-create-your-own-language)).
- **Hide a device from the taskbar** (per device, from its menu): it is still read, listed in the menu, and kept in the history and the 5% warning.
- **Your order:** drag a device along the widget to another place (the others slide aside), or use *Move left / Move right* in its menu; every device keeps its place, and a device seen for the first time starts on the far left.
- **Gone devices are forgotten:** a device not connected for 7 days loses its history, place, name and settings, so a device plugged in once does not stay around.
- **Reset measurements** of a device (its menu, or the dashboard): after a warning, its history and statistics are deleted and measuring starts again.
- **Windows 11 style menu:** the right-click menu is drawn like Windows 11's own context menus, dark or light like the taskbar.
- **Request support for a device** from the menu: pick a device that is not shown, and a GitHub issue opens filled in with what is needed to add it.
- **Tiny:** about 0.1–0.2 MB of RAM when idle (Task Manager "Memory"), about 0.1 s of CPU per minute.
- **One copy only:** starting BatteryHub again while it runs does nothing.
- **Left or right taskbar** (Windows 11): the widget becomes a column above the notification area, and the hover card opens beside the taskbar.
- **Safe fallback** to notification-area icons on an unknown Explorer layout.

## Using it

| On the strip | Meaning |
|---|---|
| **Red** percentage | At or below the device's warning level (default 15%) |
| **Green ⚡** | Plugged in and charging |
| **Blue ⚡** | Plugged in and charged to 100% |
| Dimmed | Asleep (receiver still plugged in), showing the last known value |

Left click does nothing (on purpose: the strip stays out of the way). **Right click** opens the menu:

| Menu item | What it does |
|---|---|
| *Device name (level, state)* ▸ | *Dashboard...*, *Battery history*, *Statistics* (the dashboard on that tab), *Rename device...*, *Red warning at or below* 50 / 40 / 30 / 20 / **15** / 10 / 5%, *Warn at 5% (notification)* (**on** by default), and *Show on the taskbar* (**on** by default; turned off, the device leaves the strip, its tooltip and the hover card but stays in this menu, still read, with its history and the 5% warning), *Move left* / *Move right* (its place on the strip; dragging it there works too) and *Reset measurements...* (asks first, then deletes the device's history and statistics) |
| *Refresh devices* | Full rescan; devices that are gone disappear |
| *Refresh battery levels* | Reads the levels again |
| *Refresh battery every* ▸ | **1 minute** / 5 / 10 / 30 minutes / 1 hour |
| *Refresh when a device connects or disconnects* | **On** by default; see below |
| *Show as notification-area icons (Windows arranges them)* | **Off** by default. Instead of the strip, one notification-area icon per device with its level drawn in it. Windows places these icons like any other: drag them to reorder or in and out of the hidden icons. New icons start hidden behind ^ (Windows 11 does that for every app): drag them onto the taskbar or turn them on in *Settings → Personalization → Taskbar → Other system tray icons* once; Windows remembers it per device |
| *Background behind taskbar text (for translucent taskbars)* | **Off** by default. A rounded plate in the taskbar's colour behind the widget, so it stays readable on a transparent taskbar (TranslucentTB, Windhawk Taskbar Styler...) |
| *Position on the taskbar* ▸ | **Next to the notification area** or 20 to 200 px to the left of it (in 20 px steps), e.g. to make room for another taskbar widget (iCUE, network meters...). *Move 10 px left / right* at the bottom moves it in 10 px steps to any place (shown as *Custom*). Also applies on other monitors |
| *Height on the taskbar* ▸ | 5 / 10 / 15 px up, **Centred**, or 5 / 10 / 15 px down; *Move 10 px up / down* at the bottom moves it in 10 px steps (up to 40 px, shown as *Custom*). Also applies on other monitors |
| *Show widget on* ▸ | Only with more than one monitor: *All monitors* / **Main monitor only** / *Display 2 (left) only*... The monitor is remembered by its identity, not its number; while it is unplugged the widget is on the main taskbar. Other monitors need *Show my taskbar on all displays* in Windows (taskbars of other programs, e.g. DisplayFusion, are not supported) |
| *Language* ▸ | **Windows language (...)** (the default, with the language it resolves to in brackets: the Windows *display* language, or English when BatteryHub has no translation for it), or one of the languages listed. The region and format setting (date, number format) does not count: with an English Windows and Polish formats, BatteryHub is in English until you pick *Polski*. *Create your own language...* and *Open the languages folder*: see [Your own language](#translations-create-your-own-language) |
| *Automatically start with Windows* | Adds or removes BatteryHub's autostart entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\BatteryHub Taskbar`) |
| *Automatically check for updates* | **Off** by default. When on, BatteryHub checks GitHub once, about 2 s after it starts (not while it runs; if the network is not up yet it tries again every 15 s, up to 8 times); if there is a new version you get the same question as with *Check for updates...* |
| *Install updates silently* | **Off** by default; needs the automatic check. When the check finds a new version, BatteryHub downloads it, verifies its SHA-256 checksum and installs it in the background: no question, no setup window, no "Update finished" message. Settings and autostart are kept. |
| *Request support for a device...* | Lists every device BatteryHub can see (HID, USB and Bluetooth; vendor-specific and audio devices first). Pick yours, check the name and type, add notes, and *Send* opens a new GitHub issue in your browser, filled in with exactly what the window shows: name, type, your name (optional, the issue is public), notes, vendor / product id, HID and USB interface summary, Windows build. You submit it there (a GitHub account is needed); nothing is sent from the app. *Copy full report* copies the same plus the list of all devices. Also available as `batteryhub-taskbar.exe --report` |
| *Install another version...* | Opens the setup's list of all releases on GitHub, to go back to an older one (or install a newer one) |
| *Check for updates...* | "No new updates", or "A new update is available: v1.1 → v1.x - update now?". *Yes* downloads the setup, checks it against the release's SHA-256 checksum and starts it; it updates and restarts BatteryHub, keeping your settings and autostart |
| *Exit* | Closes BatteryHub |

The hover card and the menu follow the taskbar theme (dark or light); the *Request support* window follows the Windows app theme.

**When a device comes or goes** (with *Refresh when a device connects or disconnects* on):

| Event | Shown after |
|---|---|
| USB receiver / cable plugged in or removed | ~1–3 s (Windows device notifications; every read also checks whether the set of HID devices changed) |
| Bluetooth device (AirPods, headphones, controllers) connects or disconnects | a few seconds (connections are checked every 3 s); AirPods only while connected to this PC |
| Headset on a 2.4 GHz dongle switched on or off | ~10 s (headset dongles are checked every 10 s) |
| Mouse / keyboard asleep, receiver still plugged in | stays, dimmed, with its last level |
| PC wakes up | a few seconds |

## Translations: create your own language

BatteryHub comes in 21 languages (the files are in [`lang/`](lang)). If yours is missing, or a translation is wrong, you can write your own in a few minutes, with no programming:

1. Right-click BatteryHub → *Language* → **Create your own language...**. A file `my-language.txt` is written to `%LOCALAPPDATA%\BatteryHub\Languages` and opens in Notepad. It lists every text of BatteryHub, one per line:

   ```
   @code = custom
   @name = My language

   Battery history = Battery history
   Charging = Charging
   {name}: battery at {level}% = {name}: battery at {level}%
   ```

2. Set **`@code`** to the language's code (e.g. `eo`, `ro`, `pt-PT`) and **`@name`** to its own name (e.g. `Română`), as it should appear in the menu.
3. Translate the text **after** each `=`; leave the English text before it as it is. Keep `{placeholders}` such as `{name}` or `{level}` and `\n` (a line break) unchanged. A line left empty, or removed, shows the English text.
4. Save the file (UTF-8, Notepad's default), right-click BatteryHub → *Language* and pick your language. To see a change, pick it again.

**Fixing a built-in translation:** give the file the code of that language (e.g. `@code = pl`) and keep only the lines you want to change; they replace the built-in ones, everything else stays.

*Open the languages folder* in the same menu shows the folder with your files. They stay there across updates.

### Share your language

A finished or improved language can become part of BatteryHub for everyone:

- **As an issue (no Git needed):** open a [**new issue**](../../issues/new) titled e.g. *Language: Română*, attach your `.txt` file (drag it into the text box) and write which language it is, so it can be added in a next version.
- **As a pull request:** add the file to [`lang/`](lang) as `<code>.txt` (e.g. `lang/ro.txt`) and add it to the list of built-in languages in [`src/i18n.rs`](src/i18n.rs). `cargo test` checks that the file contains every text and keeps all placeholders.

## Setup: Install / Update / Uninstall

Since v1.6 the setup is a Windows 11 style window. It first checks what is on this PC (the installed version and where, whether it is running, the original C++ BatteryHub, which it never touches) and offers the one action that fits:

| Button | When | What it does |
|---|---|---|
| **Install** | not installed | Installs, with a Start menu shortcut, and the choices *Desktop shortcut*, *Start automatically when I sign in to Windows* and *Start BatteryHub when setup is done* |
| **Update to ...** | an older version is installed | Replaces the program and keeps your settings, history and autostart; restarts it if it was running |
| **Reinstall** | the same version is installed | Writes the program again (repairs a damaged install) |
| **Go back to ...** | a newer version is installed | Installs this setup's older version, keeping your settings |
| **Other versions...** | always | Lists every release on GitHub; the chosen one is downloaded, checked against its SHA-256 checksum and installed |
| **Uninstall...** | installed | Closes BatteryHub, removes it from every autostart location, the shortcuts, files and the Settings → Apps entry. Your settings and battery history **stay** unless you tick *Also remove my settings and battery history* |

The new program file is written next to the old one, read back and checked (SHA-256) before it replaces it, so a failed write never leaves a broken install. Only BatteryHub processes started from the install folder are closed. *Show details* shows the log, also saved to `%TEMP%\BatteryHub-Setup.log`.

BatteryHub also appears in **Settings → Apps → Installed apps** (name, version, publisher, size), where *Uninstall* opens the same window.

Command line for scripts: `/install`, `/update`, `/uninstall`, `/S` (silent), `/autostart=yes|no`, `/desktop=yes|no`, `/nolaunch`, `/nonotify` (no "Update finished" message afterwards), `/purge` (uninstall: also remove settings and history), `/versions` (open the version list).

| Where the files go | |
|---|---|
| Program | `%LOCALAPPDATA%\Programs\BatteryHub\Taskbar\batteryhub-taskbar.exe`, plus `BatteryHub-Setup.exe` (a copy of the setup that *Uninstall* in Settings runs) |
| Settings → Apps entry | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\BatteryHub Taskbar` |
| Shortcuts | Desktop and `Start Menu\Programs\BatteryHub\BatteryHub Taskbar.lnk` |
| Settings | `HKCU\Software\BatteryHub\Taskbar`: `Thresholds` (per device), `RefreshMinutes`, `RefreshOnDeviceChange`, `TaskbarOffset` (DWORD, gap from the clock in pixels), `TaskbarOffsetY` (DWORD, up / down shift in pixels, + = down), `WidgetMonitor` (`all`, `main` or a monitor id), `WidgetBackground`, `AutoCheckUpdates`, `SilentUpdates`, `NotificationIcons`, `History` (binary, last 24 hours of levels) |
| Autostart | `HKCU\...\Run` → `BatteryHub Taskbar` (if you chose it) |

The original C++ BatteryHub uses different names (`Program Files\BatteryHub`, `HKCU\Software\WirelessBattery`, autostart `WirelessBattery`) and is never touched.

## Resource usage vs. the original

Both programs running **side by side on the same PC** (Windows 11 build 26300, x64, two monitors), the same devices connected (Logitech PRO 2 LIGHTSPEED mouse, SteelSeries 2.4 GHz headset dongle, Bluetooth), default settings (refresh every minute). The original was given 75 s to start, then both were sampled every 2 s for 3 minutes on 25 November 2026. Method, raw samples and older runs: [bench/RESULTS.md](bench/RESULTS.md).

| | **BatteryHub (Rust) v1.7.3** | BatteryHub (C++) 1.1.1 | |
|---|---:|---:|---:|
| **RAM in Task Manager** ("Memory" column), median | **0.09 MB** | 2.88 MB | **32× less** |
| RAM in Task Manager, average / max | 0.20 / 1.82 MB | 2.84 / 3.73 MB | |
| Working set incl. shared system DLLs | **0.58 MB** | 10.51 MB | 18× less |
| Private bytes (commit) | **5.3 MB** | 6.8 MB | |
| CPU time per minute | **0.23 s** | 1.52 s | 6.6× less |
| Threads | **10** | 17 | |
| Handles | **379** | 427 | |
| Program file | **1,598 KB** | 2,630 KB | 1.6× smaller |

The short peaks (max 1.8 MB) come while the devices are read; the memory goes back to Windows right after. Since v1.1 the program grew from about 500 KB to 1.6 MB with about 60 device drivers, 21 languages, the hover card with its forecast, the dashboard and the animations, and it still uses less memory than then (median 0.20 MB in v1.1).

What keeps it small: memory needed only for a moment (a read, a Bluetooth check) is handed back to Windows right after; settings are cached; Bluetooth LE is listened to only while reading; the device list is cached and rescanned only when it changes; only Bluetooth device nodes are enumerated; the Xbox input APIs and WinHTTP are loaded only when needed.

**To be fair:** the original has brand colours and options this version doesn't have. Part of the difference is less code doing less work, not only the language.

## Taskbar compatibility

The strip is a small layered window placed inside the taskbar, left of the notification area, the same technique the original uses. It depends on Explorer's window layout, so BatteryHub checks it every second and falls back safely:

| Situation | What BatteryHub does |
|---|---|
| Windows 11, Windows 10, ExplorerPatcher (also the small Windows 10 taskbar) | Strip next to the clock; on the Windows 10 layout the task buttons are narrowed so they never slide under it |
| Explorer restarts | Recreates exactly one strip on the new taskbar (stray strip windows are removed every second) |
| Taskbar height, DPI or theme changes (e.g. after leaving full screen) | Re-renders the strip |
| Vertical taskbar (left or right, Windows 11) | Strip as a column above the notification area; the hover card opens beside the taskbar |
| Unknown Explorer layout, or the strip cannot be placed for 3 s | **One notification-area icon per device** (level drawn in the icon, a short tooltip and the same menu), back to the strip when possible |

The optional experimental XAML layout also offers `Grouped` and `Individual` peripheral layouts.
Grouped uses one XAML strip; Individual uses one independently anchored XAML module per device,
sharing the existing rendered surface, battery monitor, history and host. Native notification icons
remain the original user-selectable option and the fallback when XAML integration is unavailable. See
[`docs/experimental-xaml-layout.md`](docs/experimental-xaml-layout.md) for the supported anchors
and the Explorer-owned nested collections that cannot be modified safely.

## Supported devices

| Family | Notes |
|---|---|
| Logitech | HID++ 1.0 / 2.0 over Unifying, Bolt, LIGHTSPEED, Bluetooth and USB, plus G headsets (voltage curves) |
| SteelSeries | Arctis 1 / 7 / 9 / Pro, Arctis Nova (3P, 5, 7, 7+, Pro, Pro Omni, Elite; with the dock battery on Pro Wireless, Pro Omni and Elite; Pro Omni and Scimitar below contributed by [Simdrom](https://github.com/Simdrom) in #189), GameBuds, GameBuds X, Aerox, Prime, Rival |
| Razer | Mice, keyboards and receivers use the generic 90-byte feature-report controller; Viper V3 HyperSpeed `1532:00B8` is covered when it exposes the expected 91-byte HID feature collection (physical validation pending). BlackShark V3 headset over its HyperSpeed dongle `1532:057A` or USB cable `1532:0579` (protocol from OpenRazer PR #2794 and Joule); BlackShark V2 Pro (2023) over its HyperSpeed dongle `1532:0555` (protocol from OpenRazer PR #2862 and [blackshark-battery](https://github.com/matej-basic/blackshark-battery), MIT); Barracuda X Chroma over its 2.4 GHz dongle `1532:0574` (protocol from [batterywatch](https://github.com/itayavra/batterywatch), GPL-3.0); Nari over its 2.4 GHz dongle `1532:051C` (request and answer from [RazerNariBatteryLevel](https://github.com/Modzeleczek/RazerNariBatteryLevel)'s Synapse capture, field layout from [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT, and [BrimDeck](https://github.com/MoFeng2223/BrimDeck), Apache-2.0; level in coarse steps, none while charging) |
| Razer Barracuda X (2022) | Existing upstream support, unchanged: 2.4 GHz dongle `1532:0550` or `1532:0552` (protocol from [barracuda-x-battery](https://github.com/nightdevx/barracuda-x-battery) and [razer-barracuda-x-battery](https://github.com/mehdibouchami/razer-barracuda-x-battery), both MIT); not physically retested for this contribution |
| ASUS ROG Pelta | Headset over its 2.4 GHz dongle `0B05:1B84` or USB cable `0B05:1B82` (protocol from [pelta-control](https://github.com/jimm0thy/pelta-control), MIT, from captures of Armoury Crate, as used by the original C++ BatteryHub) |
| Redragon (Areson) | M913 Impact Elite, M915RGB-WL and other devices on the "2.4G Wireless Receiver" `25A7:FA07` / `FA08`; level in 10 % steps (protocol as documented by the original C++ BatteryHub from a capture of Redragon's software, [m913-ctl](https://github.com/Qehbr/m913-ctl) and [redragon-m913-battery-tray](https://github.com/Doomguy-BFG9000/redragon-m913-battery-tray)) |
| Corsair | Void family headsets incl. HS70 Pro Wireless `1B1C:0A4F` over the 2.4 GHz receiver (protocol from [HeadsetControl](https://github.com/Sapd/HeadsetControl) and the Linux `hid-corsair-void` driver) |
| Endgame Gear 4K | OP1w 4k (v1 / v2) and XM2w 4k (v1 / v2) on the 4K dongle `3367:1970` (read-only feature report `A1`, battery command `B4`; protocol from [vole](https://github.com/johanneszab/vole), MIT, and [Endgame_Gear_Web_Configurator](https://github.com/Janlhin/Endgame_Gear_Web_Configurator), GPL-3.0; no charging indication) |
| MCHOSE keyboards (3033 receiver) | G75 V2, G87 V2, K99 V3 on the common receiver `3837:3033` ("GLW" func-config read from MCHOSE's M HUB, as documented by the original C++ BatteryHub; asked at most every 10 minutes; the K87S on the same receiver uses another protocol and is not read) |
| SinoWealth receiver | Redragon M602-KS, M690 Pro and other mice on SinoWealth's stock receiver `258A:002F` (contributed by NeroHole, [#111](../../pull/111); protocol as documented by [BatteryTray](https://github.com/qu1ncky/BatteryTray), MIT, and OpenMouse's M690 Pro notes; no charging indication) |
| Corsair (Bragi) | Virtuoso XT on its receiver `1B1C:0A64`, HS80 MAX Wireless on `1B1C:0A97`, Void Wireless v2 on `1B1C:2A08`, HS80 RGB Wireless on `1B1C:0A6B`, and the headsets on the Bragi receivers `0A62` / `0A3E` / `0A3D` (read-only Bragi GET of the battery and charge properties, as documented by the original C++ BatteryHub; untested here); Scimitar Elite Wireless SE mouse `1B1C:2B22` and other mice (e.g. M65 RGB Ultra Wireless, #202) on the Slipstream receivers `2B00` / `1BDC` / `1BA6` (verified on `2B00` in #189; cable untested) |
| ATK headsets (serial) | ATK N9 Pro (dongle `373B:11CB`, cable `11CA`) and first-generation N9 Ultra (`1204` / `1203`), read over the dongle's USB serial port as ATK HUB does (protocol facts from [myfinder-device-battery](https://github.com/wushuai0802/myfinder-device-battery) and the original C++ BatteryHub; not while ATK's app holds the port) |
| AULA / MCHOSE | AULA F75 / F87 Pro / F99 (Pro) and MCHOSE keyboards on the "BY" receiver `3554:FA09` / `3554:FB40` / `41E4:2001` (protocol from [Device-Battery-Info](https://github.com/PyFlat/Device-Battery-Info), [womier-l65-linux](https://github.com/deepan-alve/womier-l65-linux) and MCHOSE's M HUB, as collected in the original project); MCHOSE mice (K7, L7, M7, A7) on VID `5253` and on the receivers `3837:1001` (K7 Ultra) / `100A` / `100B` / `100C` / `1012` (protocol from [mchose-linux](https://github.com/alexfrih/mchose-linux); the `3837` ids from the original project); MCHOSE V9 Pro headset `291D:385D` (protocol from [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT) |
| ATK Mercury | Mercury I / M1 headset on its 2.4 GHz receiver `373B:107C`, headset firmware 0007 (protocol from [Joule](https://github.com/DillanCidral/Joule) PR #79, validated on hardware there, and [TaskbarVitals](https://github.com/symNistaken/TaskbarVitals); no charging state) |
| ATK / VXE | Mice on the NK mouse NANO dongle `373B:1216` (e.g. ATK A9 PLUS 2.0, VXE R1 NK; protocol from [HaloBattery](https://github.com/HeyOkay/HaloBattery) and [mouse-battery](https://github.com/qwr79/mouse-battery)) |
| Darmoshark | M3 and M3S on their Telink 2.4 GHz receivers `248A:FF30` / `248A:FF31` (protocol from [darmoshark-m3-configurator](https://github.com/Barba2k2/darmoshark-m3-configurator), MIT; M3S ids from the original project) |
| VGN / VXE / ATK keyboards (Evision) | VGN S99, V98 PRO, V98pro V2, N75 PRO, VXE75 (V2), VXE V87, ATK V75X and "Wireless Gaming Keyboard" (e.g. Feker Alice80) on the 2.4 GHz receiver `320F:5088` or USB cable `320F:5055` (protocol from [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT, and VGN HUB, as collected in the original project) |
| Pulsar / Kysona / VXE / LAMZU (CompX) | Pulsar X2 V2 / X2H / X2A / Xlite V3 (`3554:F507` / `F508` / `F509`), Kysona M600 (`F57C` / `F57D` / `F5D5`), VXE R1 SE+ (`373B:1085`, `3554:F58F`), VXE Dragonfly R1 Pro (Max) (`3554:F58A` receiver, `F58C` cable), LAMZU Atlantis / Thorn generation on the LAMZU 4K Receiver (`3554:F510`, cable `F50F`) (protocol from the Linux `hid-pulsar` driver, [vxe-r1-pro-max-battery](https://github.com/S1berla/vxe-r1-pro-max-battery) and [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT) |
| Pulsar XS-1 TenZ | 8K Dongle Gen.2 (`3710:5406`) or USB cable (`3710:3415`); one logical device across both modes, with battery and charging status (CompX report 8; protocol details cross-checked against the original C++ BatteryHub, GPL-3.0); contributed by [fevegit](https://github.com/fevegit) in #174 |
| Scyrox / ATK 8K (CompX) | Scyrox V6 / V8 on the SCYROX 8K Dongle `3554:F5F7` or USB cable `3554:F5F6`; ATK mice (e.g. Fire X 8K) on the ATK Mouse 8K Dongle `373B:101B`, ATK A9 Ultimate / F1 V2 on the "8k dongle-L" `373B:11D9`; Gravastar Mercury X `3554:F575`, LAMZU 1K Champion Receiver `3554:F50D`, IO Aurora `3554:FB08`, Red Square Kiro 1K `3554:FB5A`, VGN DragonFly F2 Ultra+ `3554:FB49`, Ardor Gaming Phantom Pro `3554:F5C4` and Phantom Pro V2 `3554:F52D` (CompX report 8; ids and protocol from [HaloBattery](https://github.com/HeyOkay/HaloBattery) and [MouseDock](https://github.com/late-autumn-gif/mousedock), both MIT, and [BrimDeck](https://github.com/MoFeng2223/BrimDeck), Apache-2.0) |
| AULA mice | X9 Ultra, V9 / V9 Max+ and other AULA mice on the AULA-8K Dongle `320F:22A0` (protocol from [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT, and the original C++ BatteryHub) |
| MCHOSE (newer mice) | K7 V2 Ultra+, A7 V3 Ultra+ and the other mice on MCHOSE's newer receivers `3837:1014` / `1018` / `1016` / `102C` / `102D`, K7 V2 Ultra+ cable `3837:4028` (M HUB's device-info read, as documented by [MchoseBatteryTray](https://github.com/HifumiDaisuki01/MchoseBatteryTray), MIT, and the original C++ BatteryHub) |
| MCHOSE G3 V2 | G3 V2 (Pro) on its receiver `3837:4245` or USB cable `3837:418C` (battery read from [MCHOSE-Monitor](https://github.com/Jstdi001/MCHOSE-Monitor), MIT, verified on a G3 V2, as used by the original C++ BatteryHub) |
| AJAZZ AJ139 V2 MC; Smailwolf RS6 | AJAZZ on its 2.4 GHz receiver `A8A5:2255` or USB cable `A8A4:2255` (protocol from [ajazz-aj139-macos](https://github.com/werquess/ajazz-aj139-macos), MIT); Smailwolf RS6 on `A8A5:2255`, using the 64/64 F0 `30` battery query (verified on RS6 hardware) |
| Jabra | Headsets behind a Jabra Link 390 dongle `0B0E:2E50` / `2E51` / `2E56` / `2E57` (GN Protocol as documented by [HeadsetControl](https://github.com/Sapd/HeadsetControl), GPL-3.0, and the original C++ BatteryHub) |
| Rongyuan receivers | Mice and keyboards on the Rongyuan 2.4 GHz receivers of Attack Shark's web driver (88 ids, e.g. Attack Shark R3 `3151:402D`, EWEADN GS01 `3151:5031`, AJAZZ, Akko Air 01) (protocol from [BatteryTray](https://github.com/qu1ncky/BatteryTray), MIT) |
| SONiX keyboards | AULA F108 Pro, AJAZZ AK820 MAX and AJAZZ AK35I Pro V3 on the `05AC:024F` dongle, AJAZZ AK980 V2 / AK820 Pro on `0C45:FEFE` / `0C45:FDFD`, 2.4G mode only (protocol from [sonix-keyboard-studio](https://github.com/donglover69/sonix-keyboard-studio), [ak820-rs](https://github.com/deddroid/ak820-rs) and [ak820-studio](https://github.com/usharik/ak820-studio)) |
| WLMOUSE | Beast X, Beast X Mini, Mini Pro and Max over their receivers (protocol from [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT) |
| DriverCore mice (VID `373E`) | Mice on the shared `373E` vendor id with its 65-byte status feature report, e.g. "R6 Mouse 2.4G" `373E:0022`, LAMZU and Arbiter Studio receivers (the WLMOUSE status query, as documented by the original C++ BatteryHub from the LAMZU / Arbiter web drivers, GPL-3.0) |
| Flydigi | Apex 6 / Apex 6 Pro `37D7:2502` on the 2.4 GHz dongle or cable (protocol from [SDL](https://github.com/libsdl-org/SDL)'s `SDL_hidapi_flydigi.c`, zlib) |
| GameSir | G7 Pro 8K PC (Aimlabs), XInput mode: exact percentage and charging flag from the vendor HID report. Hardware tested with the 2.4 GHz receiver `3537:10C6` and USB cable `3537:10C5`. |
| HyperX | Cloud III S Wireless headset over its 2.4 GHz dongle `03F0:06BE` / `03F0:02CC` (protocol as documented by [HyperX-Battery-Tray](https://github.com/GuacamoleGH/HyperX-Battery-Tray), [hyperx-battery-logger](https://github.com/OZX-OG/hyperx-battery-logger) and [HyperHeadset](https://github.com/LennardKittner/HyperHeadset), all MIT) |
| HyperX Cloud II Core Wireless | Over its 2.4 GHz dongle `03F0:0995` (second id `03F0:0795`) (protocol from [HaloBattery](https://github.com/HeyOkay/HaloBattery) and [HyperHeadset](https://github.com/LennardKittner/HyperHeadset), both MIT) |
| HyperX Cloud II Wireless | Over its 2.4 GHz dongle `0951:1718` (protocol from [HeadsetControl](https://github.com/Sapd/HeadsetControl), GPL-3.0) |
| HyperX Cloud Flight | Over its 2.4 GHz dongle `0951:1723` (and `0951:16C4`); level in 5 % steps, no level while charging; contributed by [estebannoemr](https://github.com/estebannoemr) in #93, tested on hardware (protocol from [hyperx-cloud-flight](https://github.com/kondinskis/hyperx-cloud-flight) and [hyperx-cloud-flight-wireless](https://github.com/srn/hyperx-cloud-flight-wireless), both MIT) |
| HyperX (older headsets) | Cloud Stinger Core Wireless (`0951:170B`, PlayStation receivers `0951:1713` / `03F0:0A93` / `03F0:0B93`) and Cloud Alpha Wireless (`03F0:098D`) over their 2.4 GHz dongles (protocols from [HeadsetControl](https://github.com/Sapd/HeadsetControl), GPL-3.0, [HyperHeadset](https://github.com/LennardKittner/HyperHeadset), MIT, and [HyperBatteryMonitor](https://github.com/davidsantanaalves/HyperBatteryMonitor), MIT, as used by the original C++ BatteryHub) |
| Logitech G321 | G321 LIGHTSPEED headset over its 2.4 GHz receiver `046D:0B39`; contributed by [Bacurovisk](https://github.com/Bacurovisk) in #107, tested on hardware (protocol from [G321 Studio](https://github.com/gleyssonsc/g321-studio), MIT) |
| Logitech G PRO X 2 | PRO X 2 LIGHTSPEED headset over its 2.4 GHz receiver `046D:0AF7` (Logitech's "Centurion" transport, as documented by [HeadsetControl](https://github.com/Sapd/HeadsetControl), GPL-3.0, [Solaar](https://github.com/pwr-Solaar/Solaar) and [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT) |
| Logitech G533 | Headset over its dongle `046D:0A66`, read with its fixed HID++ battery request (protocol and voltage calibration from [HeadsetControl](https://github.com/Sapd/HeadsetControl), GPL-3.0) |
| Logitech ASTRO A50 Gen 5 | USB base station `046D:0B1C`; read-only battery request on `FF32:0074` (interface 8), reports battery percentage and charging status (protocol documented by [HeadsetControl](https://github.com/Sapd/HeadsetControl), GPL-3.0, and implemented in the original C++ BatteryHub, GPL-3.0); contributed by [fevegit](https://github.com/fevegit) in #174 |
| ASTRO A50 (Gen 4) | Base station `9886:002C` in PC mode; battery level and charging over its vendor channel `FF32:0074` (protocol from [HeadsetControl](https://github.com/Sapd/HeadsetControl), GPL-3.0, [#248](../../issues/248)) |
| ZOWIE | DW-series mice (FK2-DW, U2-DW, S2-DW, EC-DW ...) on the DW receiver `04A5:800A`, ZOWIE U2 on `04A5:8008`; level estimated from the battery voltage, charging on the dock inferred from its trend (protocol from [zowie-battery-tray](https://github.com/IgnacioBarb/zowie-battery-tray), MIT, as used by the original C++ BatteryHub) |
| NZXT Lift Elite | Battery percentage, charging and cell voltage over receiver `1E71:2101` (tested on hardware); USB cable `1E71:2129` implemented from the C++ protocol, not yet hardware-tested. Cable takes priority with one shared device entry. Close NZXT CAM if telemetry does not answer. Protocol from the C++ BatteryHub `nzxt.cpp`, crediting HaloBattery (MIT); Windows report framing corrected against the actual receiver. Diagnostic: `cargo run --example nzxt`.; contributed by [BatuDege](https://github.com/BatuDege) in #184 |
| Keychron | Mice (M1, M2, M3, M3 mini, M6, M7) on the Keychron Link receiver `3434:D030` / `3434:D031`; keyboards on it only with the `KC_GET_BATTERY` firmware patch ([ldkv/qmk_firmware#1](https://github.com/ldkv/qmk_firmware/pull/1)), as stock keyboard firmware reports its battery over Bluetooth only (protocol as documented by the original C++ BatteryHub, GPL-3.0) |
| Keyboards on the "Wireless 2.4G Dongle" | `36B0:3002` (e.g. Cidoo QK61 V2, URX Core75, EVO80, PMO / WOBKEY Wave 75, Aurora 80, P75, PMO 80) and PMO Hub keyboards on their USB cable (`36B0:3009` / `3004` / `302B` / `3012`); level only, no charging state (PMO Hub session read and VIA custom value 7, as documented by pmohub.cn's web driver, [HaloBattery](https://github.com/HeyOkay/HaloBattery), MIT, and the original C++ BatteryHub) |
| Keychron Nape Pro | Trackball on the Link-KM receiver `3434:D026` or its USB cable `3434:0440` (raw HID `B2` slot list and `A7 31` battery read, as documented by [nape_pro](https://github.com/nvalis/nape_pro) and the original C++ BatteryHub) |
| ROCCAT | Syn Max Air headset over its 2.4 GHz receiver `1E7D:3A3F` (voltage curve; coarse level while charging) |
| FNATIC | BOLT mouse over its receiver `2F0E:0203` (the USB cable `2F0E:0204` only marks it as charging); contributed by [Jere2201](https://github.com/Jere2201) in #48, from USBPcap captures, tested on hardware |
| Attack Shark / Delux / Inphic | Mice (e.g. X1, X6 / R1, X11, IO Nova SE) on the Xenta 2.4 GHz receiver `1D57:FA60`, and the IO Nova SE on its USB cable (`1D57:2015`, same frames, from a USB capture in [#172](../../issues/172)) (protocol from [Attack Shark Battery KDE](https://gitlab.com/gabriel17166/Attack-Shark-Battery-KDE) and [attackshark-battery-windows](https://github.com/daeoxide/attackshark-battery-windows), both MIT); Delux M900 Pro and Inphic IN10 (R) MAX on the `1D57:FA65` receiver (frames from [omarchy-delux-m900](https://github.com/AeonArcHQ/omarchy-delux-m900), MIT, and the original project) |
| Sony | DualShock 4, DualSense, DualSense Edge (USB and Bluetooth) |
| Apple | AirPods and Beats, from BLE proximity broadcasts (L / R / case) |
| Bluetooth | Any device Windows knows a battery level for, plus the Google Fast Pair L / R / case split |
| Generic HID | Devices exposing the standard Battery Strength usage |
| Xbox | Windows.Gaming.Input (exact %) and XInput (coarse levels) |

Device not shown? Use *Request support for a device...* in the right-click menu (see above). More brands from the original project can be added as one file each in `src/drivers/`; pull requests are welcome (the ROCCAT, FNATIC BOLT and HyperX Cloud Flight drivers came from them). Devices can be excluded with `HKCU\Software\WirelessBattery\IgnoreDevices` (`VID:PID` list, the same setting as the original).

## Unsupported devices

Devices that were requested but cannot be supported for now, and why. Most of them can still show their battery over **Bluetooth**, if the device has it: whenever Windows shows the level in *Settings → Bluetooth & devices*, BatteryHub shows it too. If you know how one of these reports its battery (a capture, a project that reads it), please comment on its issue; `batteryhub-taskbar.exe --listen VID:PID` records what a receiver sends without sending anything to it.

**Think one of these is supported after all?** If you know how it reports its battery, or you have it working, please [open a pull request](../../pulls) with the driver (and a note on how it was checked on real hardware), or comment on the device's issue.

| Device | VID:PID | Why | Issue |
|---|---|---|---|
| Logitech G435 headset (LIGHTSPEED) | `046D:0ACB`, `046D:0ADF` | Its dongle gives the battery only in bootloader mode, which cuts the audio; BatteryHub never touches it. Over Bluetooth it works. | [#10](../../issues/10), [#56](../../issues/56), [#92](../../issues/92), [#119](../../issues/119), [#138](../../issues/138) |
| Havit Fuxi-H3 and the same dongle in Redragon / Rexus / Ardor headsets | `040B:0897` | The dongle sends only volume / mute keys; the command Havit's app uses is not known | [#6](../../issues/6), [#79](../../issues/79) |
| Mad Dog GH750K, IO Graphite 2 Pro Wireless (JieLi dongle), "GAMING HEADSET B2", unbranded G435 look-alike, Attack Shark L80 Pro, "TA3000" headset | `4C4A:4155`, `3634:4155` | No documented battery query; the dongle's vendor channels belong to the audio chip (firmware / settings), so commands are not guessed | [#1](../../issues/1), [#30](../../issues/30), [#97](../../issues/97), [#114](../../issues/114), [#120](../../issues/120), [#132](../../issues/132) |
| DP-HS-1015 headset | `10D6:B011` | An Actions-chip dongle: audio, media keys and the chip's own vendor channels (`FF90` / `FF82`), with no documented battery query. Over Bluetooth it works when Windows shows the level. | [#178](../../issues/178) |
| Bloody MR710 headset | `10D6:4801` | The Actions-chip dongle has only audio, media keys and the chip's own channels; no documented battery query. Over Bluetooth it works when Windows shows the level. | [#31](../../issues/31) |
| 8BitDo Ultimate 2C Wireless in X-input mode | `2DC8:310A` | The dongle shows Windows a wired Xbox pad (no level), and its configuration channel's battery read is documented only for other 8BitDo models; commands are not guessed | [#106](../../issues/106) |
| UGREEN 2.4 GHz receiver | `2B89:0043` | The receiver has no battery or link-state channel (UGREEN's own software doesn't read it either). Over Bluetooth it works when Windows shows the level. | [#108](../../issues/108) |
| 8BitDo Retro Mechanical Keyboard receiver | `2DC8:5201` | No documented battery command; its vendor channel is also the firmware-update channel, so commands are not guessed | [#5](../../issues/5) |
| Mojhon Storm / BIGBIG WON controller | `E2B7:122C` | XInput dongle that reports no battery level | [#8](../../issues/8) |
| Rapoo VT9PRO 4K | `24AE:1211` | No documented protocol | [#11](../../issues/11) |
| Edifier HECATE G5 Wireless | `35BB:A008` | No documented protocol for its headset dongle (the supported HECATE mice use other ids) | [#15](../../issues/15), [#131](../../issues/131) |
| "RS 8" / ELECFOX keyboards on `36B0:3005` | `36B0:3005` | A VIA-firmware raw HID channel only; a battery read over it is documented only for other `36B0` firmware, so commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#135](../../issues/135) |
| "2.4G Wireless Device" keyboard | `1A2C:7FFF` | No documented protocol (not the Semico `AA 55 CC 33` layout of the `1A2C:8FFF` keyboards). Over Bluetooth it works when Windows shows the level. | [#136](../../issues/136) |
| Maono DM40 / DM40 Pro wireless microphone (receiver) | `352F:0405` | The receiver only has a consumer-control collection with 65-byte reports; the battery level is read by Maono's own app over an undocumented protocol, so commands are not guessed. | [#140](../../issues/140) |
| Royal Kludge RK96 (2.4G Wireless Receiver) | `25A7:FA70` | No documented protocol for this receiver (the RK MG6 Pro driver is for `2E3C` ids). Over Bluetooth it works when Windows shows the level. | [#137](../../issues/137) |
| Unbranded "2.4G Wireless Receiver" mice | `260D:1026`, `3151:3020` | No documented protocol | [#14](../../issues/14), [#16](../../issues/16) |
| Mice on the AULA "BY" receiver | `3554:FA09` | Only AULA / MCHOSE keyboards answer on this receiver (those are supported) | [#17](../../issues/17) |
| Sinowealth "Gaming Keyboard" | `258A:010C` | No documented protocol | [#19](../../issues/19) |
| VGN Dragonfly 3 Pro | `391D:1A03` | No documented protocol for this receiver (not one of the Rongyuan-protocol VGN ids, and its vendor collections differ); V HUB's query is not known, so commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#125](../../issues/125) |
| Dark Project ALU87A Onionite Wireless ("ONIONITE87_2.4G") | `342D:E411` | No documented protocol for this dongle; its raw HID channel is not known to answer a battery query, so commands are not guessed | [#122](../../issues/122) |
| Bloody R36 Ultra | `1956:3001` | The documented Bloody protocol does not answer on this mouse | [#24](../../issues/24) |
| ThinkPad X1 Presenter Mouse, ProtoArc EM11 NL (Bluetooth) | Bluetooth | Windows reports no battery level for them over Bluetooth; their 2.4 GHz receivers are not identified yet | [#49](../../issues/49), [#50](../../issues/50) |
| Dark Project Nexus | `3554:F53E` | Not among the documented CompX (Pulsar / Kysona) ids; commands are not guessed | [#52](../../issues/52) |
| Glorious GHS Wireless InfinitePlay headset | `3794:D005` | No documented battery protocol for its dongle | [#54](../../issues/54) |
| Keychron Q6 HE (and other Keychron QMK keyboards on the cable) | `3434:0B60` | Keychron's stock firmware reports the battery only over Bluetooth (works there) | [#59](../../issues/59) |
| Corsair Sabre v3 Pro Wireless (Slipstream V2 receiver) | `1B1C:2B42` | The only documented battery read ([OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub)) first logs in and switches the receiver to software mode, which takes over the mouse's onboard settings; BatteryHub only reads | [#98](../../issues/98) |
| Keychron keyboards on the Keychron Link receiver (stock firmware), e.g. Q1 HE | `3434:D030` | Stock firmware reports the keyboard's battery over Bluetooth only, never through the receiver. Over Bluetooth it works. | [#61](../../issues/61) |
| DAREU EK75 Pro | `260D:0042` | No documented protocol. Over Bluetooth it works when Windows shows the level. | [#63](../../issues/63) |
| L90 headset (C-Media dongle) | `0D8C:0371` | The dongle has only audio and media keys, no vendor channel | [#64](../../issues/64) |
| ONIKUMA GT808 headset | `3206:0861` | The dongle has only audio and media keys, no vendor channel. Over Bluetooth it works when Windows shows the level. | [#65](../../issues/65) |
| "2.4G Wireless Device" mouse | `25A7:FA17` | No documented protocol (not the CompX / Areson layout used on other `25A7` receivers) | [#66](../../issues/66) |
| Akko Verge S9 Ultra headset | `36EB:1087` | No documented battery protocol for its dongle; commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#75](../../issues/75) |
| EKSA E1000 WT headset | `3206:109D` | No documented battery protocol for its dongle; commands are not guessed | [#76](../../issues/76) |
| HATOR Pulsar 3 Wireless | `379A:3907` | No documented protocol for HATOR's `379A` mouse receiver; commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#73](../../issues/73) |
| beyerdynamic MMX 200 Wireless | `2E50:002F` | No documented battery protocol for its dongle; its vendor channels are not guessed. Over Bluetooth it works when Windows shows the level. | [#40](../../issues/40) |
| Zoom75 Wireless (Meletrix) | `6D66:8888` | Its VIA raw HID channel has no documented battery command. Over Bluetooth it works when Windows shows the level. | [#42](../../issues/42) |
| T87 keyboard | `056D:C077` | No documented protocol | [#44](../../issues/44) |
| Keychron TurboLink 8K receiver | `3434:D05A` | Not among the documented Keychron receiver ids; which Launcher protocol it uses is not confirmed, so commands are not guessed | [#45](../../issues/45) |
| Redragon K1NG 4K PRO (M916 PRO 4K) | `3554:F54F` | Not among the documented CompX (Pulsar / Kysona) ids; commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#46](../../issues/46) |
| PIAO-2.4G mouse | `372E:1014` | No documented protocol for this `372E` receiver | [#47](../../issues/47) |
| AJAZZ NJ07 (Bluetooth LE) | — | Windows reads no battery level from it over Bluetooth; its 2.4 GHz dongle has not been reported yet | [#69](../../issues/69) |
| VXE V1 (VGN Siren V1) headset | `10D6:0016` | The Actions-chip dongle's vendor channels are undocumented; no battery query is known. Over Bluetooth it works when Windows shows the level. | [#85](../../issues/85) |
| ASTRO A50 X (base station) | `046D:0B0B` | Its only vendor channel (`FF32:0074`) is not HID++ and has no documented battery command; G HUB reads it with its own protocol, which is not published | [#105](../../issues/105) |
| AJAZZ AJ159P | `249A:5C2F` | No documented protocol for this receiver | [#216](../../issues/216) |
| MCHOSE G11 headset | `3554:FD01` | No documented battery query for its dongle; commands are not guessed | [#213](../../issues/213) |
| BTP-KP70A controller (X-input dongle) | `20BC:5130` | The dongle shows Windows an Xbox pad that reports no level; its vendor channel is undocumented | [#212](../../issues/212) |
| Bloody R72 Pro | — | No documented protocol; the documented Bloody protocol is for other models | [#211](../../issues/211) |
| L7 Mix mouse | `248A:FF4C` | No documented protocol for this receiver | [#207](../../issues/207) |
| Logitech G325 LIGHTSPEED headset | `046D:0B21` | No documented battery read for this model (the PRO X 2 protocol is not confirmed on it); commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#206](../../issues/206) |
| ATK ZERO | `373B:11FE` | Not among the documented ATK receiver ids; commands are not guessed | [#204](../../issues/204) |
| HyperX Cloud Stinger 2 Wireless | not reported | The dongle's id was not in the request and no protocol for it has been checked; please comment with its VID:PID | [#203](../../issues/203) |
| HyperX Pulsefire Haste 2 S | `03F0:0BB8` | No documented battery protocol for this mouse | [#201](../../issues/201) |
| Razer BlackShark V3 Pro | `1532:0577` | No documented battery read for this dongle (the BlackShark V3 `057A` read is not confirmed on it); commands are not guessed | [#199](../../issues/199) |
| Ninjutso Sora V3 | `093A:EB02` | No documented protocol for this receiver | [#198](../../issues/198) |
| K68 keyboard (Ziyoulang) | `1EA7:0002` | No documented protocol | [#196](../../issues/196) |
| Royal Kludge S98-ES | `258A:0150` | No documented protocol for this receiver. Over Bluetooth it works when Windows shows the level. | [#195](../../issues/195) |
| AULA Kunlun X5 | `38A6:28F9` | Not among the documented AULA ids; commands are not guessed | [#194](../../issues/194) |
| MCHOSE L7 Ultra+ | `5253:1020` | No documented protocol for this `5253` receiver | [#192](../../issues/192) |
| "2.4G Wireless Receiver" mouse | `25A7:FA7C` | No documented protocol (not the CompX / Areson layout used on other `25A7` receivers) | [#225](../../issues/225) |
| Attack Shark MEGA B2 TMR | `3554:F517` | Not among the documented CompX / Attack Shark ids; commands are not guessed | [#223](../../issues/223) |
| HATOR Rockfall 3 Wireless | `379A:0300` | No documented battery read for this keyboard's dongle; commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#221](../../issues/221) |
| HELLYBERRY HM47 | `379A:A202` | No documented protocol for this `379A` receiver; commands are not guessed | [#220](../../issues/220) |
| Corsair K63 Wireless | `1B1C:1B50` | No documented battery protocol for this keyboard's receiver; commands are not guessed | [#163](../../issues/163) |
| ASTRO A30 | `9886:0050` | No documented battery protocol for its dongle; commands are not guessed | [#162](../../issues/162) |
| AJAZZ AK820 MAX on `0C45:FEFC` | `0C45:FEFC` | Not one of the documented SONiX dongle ids (the AK820 MAX on `05AC:024F` is supported); commands are not guessed | [#160](../../issues/160) |
| Poly BT700 adapter | `047F:02E6` | Its vendor channel differs from the supported BT600 and no battery query is documented | [#159](../../issues/159) |
| "GAMING HEADSET B100" | `1024:8087` | The dongle has only audio and media keys, no vendor channel. Over Bluetooth it works when Windows shows the level. | [#165](../../issues/165) |
| RS322 ROX headset | `4C49:4255` | The dongle has only audio and media keys, no vendor channel. Over Bluetooth it works when Windows shows the level. | [#148](../../issues/148) |
| QCY H3 Pro | `3654:4A55` | Only audio and media keys over USB, no vendor channel. Over Bluetooth it works when Windows shows the level. | [#144](../../issues/144) |
| MCHOSE X9 headset | `3837:6045` | No documented battery protocol for its dongle; commands are not guessed | [#147](../../issues/147) |
| SteelSeries Apex Pro Mini Wireless | `1038:1624` | No documented battery read for this keyboard; commands are not guessed | [#146](../../issues/146) |
| Keychron K8 Pro (Bluetooth) | Bluetooth | Works whenever Windows shows its level in *Settings → Bluetooth & devices*; Keychron keyboards send no level otherwise | [#145](../../issues/145) |
| IO Airox | `249A:5C2F` | No documented protocol for this receiver (shared with the AJAZZ AJ159P) | [#236](../../issues/236) |
| Keychron Q1 Max | `320F:5088` | Keychron's stock firmware reports the battery only over Bluetooth (works there) | [#235](../../issues/235) |
| Razer Kraken V3 Pro | `1532:052C` | Its dongle shows Windows only a media-keys collection, no vendor channel to ask | [#234](../../issues/234) |
| Lenovo Legion M410 Wireless | `17EF:6197` | No documented battery protocol for this receiver; commands are not guessed | [#231](../../issues/231) |
| Apple Watch Magnetic Charging Cable | `05AC:1392` | A charger: the watch does not report its battery to Windows through it | [#230](../../issues/230) |
| KRUX Plasma Wireless | not reported | The receiver's id was not in the request; please comment with its VID:PID | [#239](../../issues/239) |
| Ardor Gaming Edge Wireless | `3206:0881` | The dongle has only media keys, no vendor channel. Over Bluetooth it works when Windows shows the level. | [#244](../../issues/244) |
| ASUS ROG Delta II (2.4 GHz) | `0B05:1AFA` | No documented battery read for its dongle; commands are not guessed. Over Bluetooth it works when Windows shows the level. | [#252](../../issues/252) |
| Rapoo gaming receiver | `24AE:1416` | No documented protocol for this receiver (like the VT9PRO on `24AE:1211`); commands are not guessed | [#249](../../issues/249) |
| H2015 Pro headset | `3206:0871` | No documented battery query for its dongle; commands are not guessed | [#242](../../issues/242) |

## Testing

- **Unit tests** (`cargo test --lib`) decode recorded device reports without the hardware: SteelSeries Arctis Nova status blocks, DualSense / DualShock 4 input reports, AirPods proximity broadcasts, Google Fast Pair battery fields, Logitech voltage curves and receiver grouping, and the report decoders of every vendor driver (frames taken from the protocol sources). They also cover the 24-hour history, the card's blur, when a device is removed or shown as asleep, warning levels, colours, menu ids, the GitHub release parser, SHA-256 and version comparison.
- The **setup** has its own tests (autostart matching never touches the original app, version handling).
- **CI** ([GitHub Actions](.github/workflows/ci.yml)) runs all tests and builds the program and the setup on every push.
- **Resource usage** is measured with [bench/compare.ps1](bench/compare.ps1) (two programs side by side) and [bench/measure.ps1](bench/measure.ps1).

## Building from source

Requirements: Rust 1.99 or newer (stable, MSVC toolchain; the crates use the 2024 edition) and Visual Studio Build Tools with the Windows SDK (`rc.exe` embeds the icon).

```powershell
.\build-release.ps1
```

This builds the program, then the setup with the program embedded (`installer/`, `BH_PAYLOAD_DIR`), and writes `release\BatteryHub-Taskbar-Setup-v1.7.4.exe` and `SHA256SUMS.txt`. The version comes from the `VERSION` file (up to four parts, e.g. `1.0.1.1`).

| Path | Contents |
|---|---|
| `src/` | Library: HID layer, device drivers, BLE watcher, settings, update check, UI and setup helpers |
| `src/bin/taskbar.rs` | The taskbar program |
| `installer/` | The setup (`src/setup.rs`) |
| `res/` | Icon (`make_icon.py` draws it), manifest, resource script, README screenshots |
| `bench/` | Resource measurement scripts and results |
| `.github/workflows/ci.yml` | CI: tests and builds on every push |
| `examples/list.rs` | Console diagnostics: `cargo run --example list -- --dump --ble`, `-- --update` |

## Contributors

**Physical validation in this contribution:** no new device driver is added. The existing Barracuda X
(2022) driver is retained unchanged; the generic Razer driver covers the Viper V3 HyperSpeed only
when its expected HID collection is present, but that model has not been physically validated here.

BatteryHub is not made by one person alone. Thank you to everyone who helped!

- **[@ferloom](https://github.com/ferloom)**: author and maintainer.
- **Claude (AI, by Anthropic)**: writes much of the code together with ferloom: drivers, the taskbar strip, the dashboard and the setup.
- **Daniel Gutschalk**: ROCCAT Syn Max Air headset driver.
- **[@Simdrom](https://github.com/Simdrom)**: Corsair Scimitar, Arctis Nova Pro Omni, dock battery (#189), Arctis GameBuds X and snappier card switching (#246).
- **[@fevegit](https://github.com/fevegit)**: ASTRO A50 Gen 5, Pulsar XS-1 TenZ (#174).
- **[@BatuDege](https://github.com/BatuDege)**: NZXT Lift Elite (#184).
- **[@NeroHole](https://github.com/NeroHole)**: SinoWealth receivers, Redragon M602-KS (#111).
- **[@Bacurovisk](https://github.com/Bacurovisk)**: Logitech G321 LIGHTSPEED (#107), Portuguese (Brazil) and translatable connection names (#222, #237).
- **[@denwir](https://github.com/denwir)**: GameSir G7 Pro 8K, tested on hardware (#238).
- **[@elcio96](https://github.com/elcio96)**: Smailwolf RS6, tested on hardware (#233).
- **[@estebannoemr](https://github.com/estebannoemr)**: HyperX Cloud Flight, tested on hardware (#93).
- **[@Jere2201](https://github.com/Jere2201)**: FNATIC BOLT (#48).

**Device requests, dumps and testing.** Every new device comes from someone who asked for it, sent a dump or tested a version. Thank you to all of them:

<details><summary>173 people (click to show)</summary>

[@12545521](https://github.com/12545521) · [@3khz913](https://github.com/3khz913) · [@489677891](https://github.com/489677891) · [@6y6Jluk-p](https://github.com/6y6Jluk-p) · [@aaeronwydoodt](https://github.com/aaeronwydoodt) · [@absana692](https://github.com/absana692) · [@agumusfu](https://github.com/agumusfu) · [@AlanRooMJ](https://github.com/AlanRooMJ) · [@Alaxonator](https://github.com/Alaxonator) · [@amiantosss](https://github.com/amiantosss) · [@Artstup](https://github.com/Artstup) · [@ashlikesnow0](https://github.com/ashlikesnow0) · [@Azidy](https://github.com/Azidy) · [@Bacurovisk](https://github.com/Bacurovisk) · [@BiggestGit98](https://github.com/BiggestGit98) · [@Botzoi](https://github.com/Botzoi) · [@Chanaphonpa](https://github.com/Chanaphonpa) · [@ciscovti](https://github.com/ciscovti) · [@cockychen104-lab](https://github.com/cockychen104-lab) · [@cuognguyen123hhkk-a11y](https://github.com/cuognguyen123hhkk-a11y) · [@D4wiXD](https://github.com/D4wiXD) · [@DAFI2MuCH](https://github.com/DAFI2MuCH) · [@danceingby](https://github.com/danceingby) · [@daniluwka](https://github.com/daniluwka) · [@daostltt-ops](https://github.com/daostltt-ops) · [@darlandk](https://github.com/darlandk) · [@Delamier](https://github.com/Delamier) · [@DelnarS](https://github.com/DelnarS) · [@Der228gt](https://github.com/Der228gt) · [@deviloff123456-eng](https://github.com/deviloff123456-eng) · [@dicock](https://github.com/dicock) · [@DKG486](https://github.com/DKG486) · [@DNAnoU](https://github.com/DNAnoU) · [@Draling2077](https://github.com/Draling2077) · [@drobs3](https://github.com/drobs3) · [@Eindominus](https://github.com/Eindominus) · [@elcio96](https://github.com/elcio96) · [@EliteGoogle](https://github.com/EliteGoogle) · [@ensinlel](https://github.com/ensinlel) · [@ereklelabadze](https://github.com/ereklelabadze) · [@erlikkk1](https://github.com/erlikkk1) · [@estebannoemr](https://github.com/estebannoemr) · [@Estylowy](https://github.com/Estylowy) · [@fabianf4](https://github.com/fabianf4) · [@FantyTry](https://github.com/FantyTry) · [@FearSoul111](https://github.com/FearSoul111) · [@Ferrrrrit](https://github.com/Ferrrrrit) · [@foxdiggory](https://github.com/foxdiggory) · [@FremenCrow](https://github.com/FremenCrow) · [@gamedez](https://github.com/gamedez) · [@ganz1k1488-debug](https://github.com/ganz1k1488-debug) · [@gegekekPro](https://github.com/gegekekPro) · [@glebkurlevic-bot](https://github.com/glebkurlevic-bot) · [@gregstaruck-ops](https://github.com/gregstaruck-ops) · [@GrustnyD](https://github.com/GrustnyD) · [@guangusq](https://github.com/guangusq) · [@Handrei-2](https://github.com/Handrei-2) · [@heiseeen](https://github.com/heiseeen) · [@hgiang0808](https://github.com/hgiang0808) · [@htomoya16](https://github.com/htomoya16) · [@huypk10](https://github.com/huypk10) · [@iaopier](https://github.com/iaopier) · [@iluvlean2604](https://github.com/iluvlean2604) · [@iMulo](https://github.com/iMulo) · [@Izznolu](https://github.com/Izznolu) · [@jakubmarek203-art](https://github.com/jakubmarek203-art) · [@JanOdwazny](https://github.com/JanOdwazny) · [@JM4110](https://github.com/JM4110) · [@joacoavilautn](https://github.com/joacoavilautn) · [@jotaeverton122-png](https://github.com/jotaeverton122-png) · [@judyrain7-tech](https://github.com/judyrain7-tech) · [@kafder100](https://github.com/kafder100) · [@kamikaze221](https://github.com/kamikaze221) · [@kanar101](https://github.com/kanar101) · [@KaP1X0](https://github.com/KaP1X0) · [@karanshukla](https://github.com/karanshukla) · [@karsky2006-star](https://github.com/karsky2006-star) · [@KazamyXD](https://github.com/KazamyXD) · [@killer202](https://github.com/killer202) · [@kirick0789-debug](https://github.com/kirick0789-debug) · [@kkz06038183-ui](https://github.com/kkz06038183-ui) · [@klimenok01-hash](https://github.com/klimenok01-hash) · [@komkrich2lo-cmd](https://github.com/komkrich2lo-cmd) · [@Konnowo](https://github.com/Konnowo) · [@kontofaceity-tech](https://github.com/kontofaceity-tech) · [@kostya02102000-afk](https://github.com/kostya02102000-afk) · [@KR-Yeon78](https://github.com/KR-Yeon78) · [@krysti777](https://github.com/krysti777) · [@Ledinhvanh2405](https://github.com/Ledinhvanh2405) · [@Lee060217](https://github.com/Lee060217) · [@leminh17410-netizen](https://github.com/leminh17410-netizen) · [@lilsedze-prog](https://github.com/lilsedze-prog) · [@Lourran2](https://github.com/Lourran2) · [@lucas-nobrega](https://github.com/lucas-nobrega) · [@luizbuscarini654-hub](https://github.com/luizbuscarini654-hub) · [@Luxitino](https://github.com/Luxitino) · [@maciekpat2-netizen](https://github.com/maciekpat2-netizen) · [@marzzep](https://github.com/marzzep) · [@masciuch](https://github.com/masciuch) · [@MaslenaDude](https://github.com/MaslenaDude) · [@mateo081324](https://github.com/mateo081324) · [@Matig28](https://github.com/Matig28) · [@matteo4229](https://github.com/matteo4229) · [@Max-Vely](https://github.com/Max-Vely) · [@Merki118](https://github.com/Merki118) · [@Miko986123](https://github.com/Miko986123) · [@MnL59](https://github.com/MnL59) · [@momoshiaoi](https://github.com/momoshiaoi) · [@MRieznik](https://github.com/MRieznik) · [@n0vv10r](https://github.com/n0vv10r) · [@nadnite](https://github.com/nadnite) · [@ndang3110](https://github.com/ndang3110) · [@nextsanek2-pixel](https://github.com/nextsanek2-pixel) · [@ninja28289898](https://github.com/ninja28289898) · [@None85](https://github.com/None85) · [@onodaHiroo](https://github.com/onodaHiroo) · [@oWillario](https://github.com/oWillario) · [@p1nk1sjr](https://github.com/p1nk1sjr) · [@pablomarinmorales](https://github.com/pablomarinmorales) · [@PhucDHTB](https://github.com/PhucDHTB) · [@phucnghia9a-ux](https://github.com/phucnghia9a-ux) · [@Player3960nh](https://github.com/Player3960nh) · [@quangminh119](https://github.com/quangminh119) · [@qw1xx12](https://github.com/qw1xx12) · [@r96unimagu](https://github.com/r96unimagu) · [@Rafaration](https://github.com/Rafaration) · [@ravlikpavlik073-dot](https://github.com/ravlikpavlik073-dot) · [@RedJohnJS8](https://github.com/RedJohnJS8) · [@ReisRx](https://github.com/ReisRx) · [@River551231](https://github.com/River551231) · [@RostikProgrammer](https://github.com/RostikProgrammer) · [@rusiktuzik2003-design](https://github.com/rusiktuzik2003-design) · [@s18672207603-web](https://github.com/s18672207603-web) · [@Sakuya-San-Git](https://github.com/Sakuya-San-Git) · [@sashatkachenko20160805-hash](https://github.com/sashatkachenko20160805-hash) · [@sfeldmen](https://github.com/sfeldmen) · [@sgjcfg2009](https://github.com/sgjcfg2009) · [@shaaker-sudo](https://github.com/shaaker-sudo) · [@shtomo](https://github.com/shtomo) · [@Sk1vvy](https://github.com/Sk1vvy) · [@slugZ777](https://github.com/slugZ777) · [@ssebxss](https://github.com/ssebxss) · [@Supphanat-P](https://github.com/Supphanat-P) · [@tam263-iu](https://github.com/tam263-iu) · [@teiichisora1-ops](https://github.com/teiichisora1-ops) · [@tekichaxxxx](https://github.com/tekichaxxxx) · [@thereds8](https://github.com/thereds8) · [@thomjofre25](https://github.com/thomjofre25) · [@TiaGod1994](https://github.com/TiaGod1994) · [@timinside](https://github.com/timinside) · [@tkacch](https://github.com/tkacch) · [@tomaski](https://github.com/tomaski) · [@truxxyt-code](https://github.com/truxxyt-code) · [@ttwintypink](https://github.com/ttwintypink) · [@tyybest-coder](https://github.com/tyybest-coder) · [@vadimciuntu2013-oss](https://github.com/vadimciuntu2013-oss) · [@VELMORRA](https://github.com/VELMORRA) · [@VenancioSantos](https://github.com/VenancioSantos) · [@vohoainam112112-source](https://github.com/vohoainam112112-source) · [@volunteer280-sys](https://github.com/volunteer280-sys) · [@vsewvs-beep](https://github.com/vsewvs-beep) · [@vukkashin](https://github.com/vukkashin) · [@vuongego-netizen](https://github.com/vuongego-netizen) · [@vvuhoainam94-afk](https://github.com/vvuhoainam94-afk) · [@wanerzin](https://github.com/wanerzin) · [@WholeHorse](https://github.com/WholeHorse) · [@yanwend11](https://github.com/yanwend11) · [@YossarianBGD](https://github.com/YossarianBGD) · [@yurakolesnikov1810-maker](https://github.com/yurakolesnikov1810-maker) · [@ZhangProj](https://github.com/ZhangProj) · [@ZHEEZL](https://github.com/ZHEEZL) · [@ZhengHu92](https://github.com/ZhengHu92) · [@zikuhi](https://github.com/zikuhi)

</details>

Want to be on this list? [Request a device](../../issues), test a new version, send a pull request or [a translation](#share-your-language).

## License

GPL-3.0-or-later, the same as the original project. See [LICENSE](LICENSE).
