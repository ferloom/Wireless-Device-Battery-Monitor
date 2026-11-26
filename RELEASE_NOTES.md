# BatteryHub v1.9 (Rust)

An ultra-lightweight Rust implementation of BatteryHub for Windows, a recode of [krisgfx9/Wireless-Device-Battery-Monitor](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor). All credit for the original idea, look and device protocols goes to the original project.

## Download

| File | |
|---|---|
| `BatteryHub-Taskbar-Setup-v1.9.exe` | Setup: Install / Update / Uninstall |
| `SHA256SUMS.txt` | Checksum |

Already installed? Use *Check for updates...* in the right-click menu (v1.1 and later), or run the new setup and click **Update**. Your settings and autostart choice are kept.

## New in v1.9

- **Every monitor's widget is drawn for that monitor:** right size with different scaling or resolutions (#392, #595), and its own orientation.
- **Mirror other widgets** (on by default): turn it off and each taskbar's widget gets its own place, height and device order.
- **Widget position:** *Before the icons* (centred taskbar icons), and *Change position*: move the widget anywhere along the taskbar with the mouse, a click puts it down.
- **Lock device positions**, **Ring style** (icon inside a ring filled to the level), **Charging animation**, and per device a **Glowing icon**.
- **Phones (experimental):** a level that stays the same keeps "charging" for 10 minutes, then it ends.
- **Request support for a device** now goes to the device-requests channel on [Discord](https://discord.gg/hAbyqpT3EA).

## New in v1.8

### BatteryHub is free
- **BatteryHub is free and will stay free.** Nobody should pay for it; if you did, ask the seller for a refund. Shown once after a new installation, in *Help* and at the top of the README.

### Per device (right-click a device)
- **On hover:** the card, only a short tooltip, or nothing (the hover highlight stays).
- **Card:** its chart and its time left can each be turned off; without time left the card has no empty gap.
- **Battery only:** only the level is shown, nothing is recorded or worked out for the device. It asks first, because turning it on deletes the device's history.
- **Delete device** in the dashboard, for devices that are not connected.

### Phones over Bluetooth (experimental)
- **Phones** (e.g. iPhones) are read over the standard Bluetooth LE Battery Service and kept connected between reads, so the level keeps updating. Otherwise the level is the one Windows has, which many phones only update when they reconnect, so it can stay old.
- **Rising level means charging** (optional, also experimental, per phone): for phones whose charging state can't be read, a rising level counts as charging until it falls again.
- Tested on an iPhone 16.

### Taskbar
- **Widget position:** *Left*, *Next to the icons* (also with centred icons) or *Right*; the widget glides to its new place, on every monitor.
- **Taskbar full:** the widget hides where it would cover task buttons (per monitor); with no room on any taskbar, the levels move to notification-area icons, with a notification, until there is room again.
- **Compact layout:** icons above percentages (by [DyAvr](https://github.com/DyAvr), #456), popping in on every monitor.
- **Stardock Start11** works (by [artxamer](https://github.com/artxamer), #512).
- The tray icon can hide when no device is connected, and the refresh icon animates on every monitor.

### Right-click menu
- **Shorter:** *Widget*, *Settings*, *Help* and *Exit* (by [artxamer](https://github.com/artxamer), #520).
- **Help:** check for updates, install another version, request support for a device, GitHub, Discord, the free notice and the latest releases.

### New devices
- Logitech G435 on the older dongle (by [DopleZZ](https://github.com/DopleZZ), #481), HyperX Pulsefire Saga Pro (by [stanzeman](https://github.com/stanzeman), #553), Darmoshark M3 PRO (by [4679](https://github.com/4679), #511), ROG OMNI receiver (by [TIGamingTV](https://github.com/TIGamingTV), #452), JBL Quantum 800 (by [basziee](https://github.com/basziee), #443), Hitscan Hyperlight X (by [Dyzson](https://github.com/Dyzson), #427) and Hyperlight, Sony INZONE H5, G-Wolves mice, BEITONG KP70, Flydigi Vader 5 Pro, more ASUS ROG / TUF mice, VGN Dragonfly 4K and F2 Pro Max (#524), AJAZZ 8K receiver, MEETION mice.

### Fixed
- Logitech G435 on its newer dongle was never read; Audeze Maxwell; G733 level (by [TIGamingTV](https://github.com/TIGamingTV), #478); Smailwolf / AJAZZ `A8A5:2255` below 100 % (by [elcio96](https://github.com/elcio96), #494); a PMO wired keyboard without a battery showed 0 % (#434).
- The widget no longer disappears for up to a minute after it gets wider.
- Updates come only from this repository.

## New in v1.7.6

- **New devices:** Logitech G435 on its newer dongle (`046D:0ADF`) and ASTRO A20 X (`046D:0B35`), Razer BlackShark V2 Pro for Xbox (`1532:0A49`), Audeze Maxwell and Maxwell 2 (`3329:4B19` / `4B29`), HyperX Cloud II Wireless on the HP dongle (`03F0:0696`), Fractal Design Scape (`36BC:0001`), Attack Shark R11 Ultra (`3554:FB44`), ROG Strix Impact II Wireless (`0B05:1949`) and mice on the Incott 8K receiver (`093A:522C`).
- **Logi Bolt:** a sleeping keyboard or mouse on a Bolt receiver stays listed (by [NanoBitrin](https://github.com/NanoBitrin), #363).
- **Icon colour:** per device in its menu: the taskbar's, the brand's (e.g. Razer green, Logitech blue), a ready-made one or your own. Optionally red at the warning level.
- **Earbuds:** both buds in the right-click menu (L / R), and each bud turns red on its own.
- **Auto-hiding taskbars:** the right-click menu stays open with StartAllBack too.

## New in v1.7.5

- **New devices:** Logitech G522 LIGHTSPEED (`046D:0B18`) and G735 (`046D:0AD8`), HyperX Cloud Core Wireless (`0951:173F`), Razer BlackShark V3 X (`1532:057D`) and Kraken V3 Pro (`1532:052C`), MCHOSE V9 Pro 2 (`3837:6011`) and V9 Turbo+ (`3837:600A`), DAREU EM901X (`260D:1074`), Fantech Helios Go XD5 (`25A7:FA7C`) and Raigor (`1EA7:0066`), mice on the Niceboy MW400 receiver (`248A:FA02`), the Zaopin 8K (`3554:F527`) and ATK NK (`373B:10C9`) receivers, HyperX Cloud III Wireless (`03F0:05B7`), HyperX Cloud Stinger 3 Wireless (`03F0:03C6`, tested by [adskiykrik](https://github.com/adskiykrik), #313), VGN Dragonfly 3 Pro (`391D:1A03`, tested by [PromiseTzZ](https://github.com/PromiseTzZ), #272), and from [Simdrom](https://github.com/Simdrom)'s #311: ASTRO A50 X, ATK ZERO, MCHOSE L7 Ultra+ and X9, Machenike L7 Mix, DAREU EK75 Pro, IPI PIAO, AJAZZ AK820 MAX on `0C45:FEFC` and AJAZZ AJ159P.
- **Numbers in your language:** decimal comma where your language uses one (by [Bacurovisk](https://github.com/Bacurovisk), #255).
- **Battery time estimates:** a drop is no longer counted as a rise when charging starts, and the hours a device keeps showing the level it reached after a charge no longer make the battery look longer-lasting (a mouse at 3 days could jump to 8).
- **Right-click menu:** belongs to the widget you clicked, so it should stay open on auto-hiding taskbars.

## New in v1.7.4

- **Other monitors:** the widget no longer disappears from another monitor's taskbar when a borderless window covers that monitor (e.g. Wallpaper Engine after a click on the desktop); it steps aside only for real full-screen games and videos, exactly when Windows moves the taskbar behind them.

## New in v1.7.3

- **New devices:** SteelSeries Arctis GameBuds X (`1038:2317`, by [Simdrom](https://github.com/Simdrom), #246) and ASTRO A50 Gen 4 on its base station (`9886:002C`, #248).
- **Dashboard:** right at the top of the right-click menu, and it now lists every device with saved history, also the ones not connected right now.
- **Earbuds:** both buds' levels on the taskbar (left over right on a vertical taskbar) and in the card, and the charging case's level next to the name (AirPods).
- **Vertical, auto-hiding and other monitors' taskbars:** the widget shows on every monitor's vertical taskbar right above its clock, pops in and out when it appears on or leaves a monitor, animates smoothly there, and steps aside for full-screen games; the right-click menu stays open on an auto-hiding taskbar.
- **Card:** red chart and forecast at or below the device's warning level, and smoother switching between devices (also between cards of different height).

## New in v1.7.2

- **New devices:** RAWM Leviathan V4 on the RAWM HS Receiver (`1915:2346`, #188), GameSir G7 Pro 8K with its exact level instead of the Xbox one (by [denwir](https://github.com/denwir), #238) and Smailwolf RS6 (by [elcio96](https://github.com/elcio96), #233, #208); Corsair mice other than the Scimitar on Slipstream receivers, e.g. M65 RGB Ultra Wireless (#202).
- **Translations:** receiver names and the display side ("Display 2 (left)") are now translated in every language (by [Bacurovisk](https://github.com/Bacurovisk), #237).

## New in v1.7.1

- **New devices:** Razer Barracuda X (2022) on its 2.4 GHz dongle (`1532:0550` / `1532:0552`, #214, #186) and HyperX Cloud II Wireless (`0951:1718`, #200).
- **Taskbar animations:** devices pop in and out, the others slide aside to make room (with motion blur), and the "no devices" icon no longer flashes before the first check.
- **Better Portuguese (Brazil)** by [Bacurovisk](https://github.com/Bacurovisk) (#222).

## New in v1.7

- **21 languages** (#177): besides English, 20 new ones: Polish, German, French, Spanish, Portuguese (Brazil), Italian, Dutch, Czech, Hungarian, Swedish, Russian, Ukrainian, Turkish, Japanese, Korean, Chinese (Simplified), Chinese (Traditional), Vietnamese, Indonesian and Thai. BatteryHub follows the Windows language by default; pick another or make your own under *Language*.
- **Move and swap devices on the taskbar:** drag a device onto another place with the mouse (the others slide aside), or use *Move left / Move right* in its menu. Every device keeps its place, and a new device starts on the far left.
- **Hide a device** from the taskbar from its menu (#176).
- **AirPods and other earbuds:** their own icon, the weaker bud on the taskbar, a chart per bud in the card, and charging updates within seconds.
- **New dashboard look**, and the card's forecast per hour when you move the mouse along the chart.
- **New devices:** ASTRO A50 Gen 5 and Pulsar XS-1 TenZ (by [fevegit](https://github.com/fevegit), #174), NZXT Lift Elite (by [BatuDege](https://github.com/BatuDege), #184), Corsair Scimitar, SteelSeries Arctis Nova Pro Omni and the dock battery (by [Simdrom](https://github.com/Simdrom), #189), Cidoo QK61 V2 and other keyboards on `36B0:3002` (#175), Ardor Gaming receivers (#181, #185), IO Nova SE on its cable (#172).
- **Fixes:** Corsair headsets answering on another report (#161); devices not seen for 7 days are forgotten; times follow your current time zone.

## New in v1.6.1

- **New devices:** Gravastar Mercury X (#170), LAMZU 1K Champion (#149), IO Aurora (#151), Red Square Kiro (#153), VGN DragonFly F2 Ultra+ (#166), Corsair Void Wireless v2 (#168) and HS80 RGB Wireless (#161), Keychron Nape Pro on the Link-KM (#142), MCHOSE G3 V2 (#158).
- **Attack Shark X6 / R1** showed 10 % when full: its level is now read correctly (#8). Mice on the Xenta receiver show their model name (X11, X3 Max, V3 Pro, V6, IO Nova SE).
- **Refresh battery every 1 / 5 minutes** works again instead of opening another window (#171).

## New in v1.6

- **New devices:** Logitech G PRO X 2 LIGHTSPEED (#112) and G321 (#107), Razer Nari (#110), Corsair Virtuoso XT (#118) and HS80 MAX (#121), HyperX Cloud Alpha Wireless (#116), ASUS ROG Pelta (#133), ATK N9 Pro / N9 Ultra (#115, #129), ATK Mercury I (#134), ATK F1 V2 8K dongle (#130), MCHOSE K99 V3 / G75 V2 keyboards (#123, #126), Endgame Gear XM2w / OP1w 4k (#128), Redragon 2.4G receivers (#109, and SinoWealth from NeroHole in #111).
- **New setup** in a Windows 11 style window: install, update, reinstall, go back to any older release, and uninstall from Settings > Apps, keeping your settings and history unless you choose otherwise.
- **Device dashboard** (right-click a device): 7-day chart with charging marked, charge and discharge sessions, battery life and time left.
- **Battery history kept 7 days** and across restarts; nothing is made up where BatteryHub wasn't running.
- **Rename devices** and get a **notification at 5%**; device menu with icons.
- **Left or right taskbar** works (#127).
- **Hover card about 10x lighter** on the CPU when moving between devices.
- **Safer updates:** a release is installed only with a matching SHA-256 checksum.

## New in v1.5

- **About 40 newly supported devices** from your requests: HyperX (Cloud III S, Cloud II Core, Cloud Flight, Stinger Core), Razer BlackShark V2 Pro and Barracuda X Chroma, Corsair HS70 Pro, Logitech G533, Jabra Link 390, BenQ ZOWIE, Keychron Link, Flydigi Apex 6, and many AULA, MCHOSE, ATK, VXE, LAMZU, Scyrox, VGN, AJAZZ, EWEADN, Epomaker, Darmoshark and Delux mice and keyboards. Full list in the README, plus a list of devices that can't be supported and why.
- **Logitech receivers work without G HUB** (#41).
- **Other monitors:** the widget sits right next to each taskbar's clock and appears on all taskbars at once.
- **New option:** show as notification-area icons that Windows arranges (drag between other tray icons, no overlap with other widgets).
- **Hover card** with the device's level and a chart of the last 24 hours (kept for 24 hours only).
- **Shares the taskbar:** sits left of other programs' taskbar widgets instead of on top.
- **Contributions:** FNATIC BOLT (#48) and HyperX Cloud Flight tested on hardware (#93).
- **Hover highlight** like the taskbar's buttons, **only one copy** can run, and **less memory** (about 0.1–0.2 MB most of the time).

## New in v1.4.1

- Built with the Rust 2024 edition (was 2021). No change in behaviour.

## New in v1.4

- **Install updates silently** (menu, with the automatic check): new versions install in the background without asking or showing anything.
- **Razer BlackShark V3** also over its USB cable.

## New in v1.3.4

- **WLMOUSE** Beast X / Mini / Mini Pro / Max mice.
- **Razer BlackShark V3** headset over its HyperSpeed dongle.

## New in v1.3.3

- **Attack Shark mice:** fixed the mouse not showing up (e.g. X1); now also shows when it is on power.
- **`--listen VID:PID`** diagnostic: records what a device sends, for device requests.

## New in v1.3.2

- **Attack Shark mice** (e.g. X1) on the Xenta 2.4 GHz receiver (1D57:FA60): battery level and *Charging*, shown as asleep while the mouse is off. Protocol from [Attack Shark Battery KDE](https://gitlab.com/gabriel17166/Attack-Shark-Battery-KDE) by Gabriel Marques Ferrarezi (MIT). Not yet confirmed on hardware under Windows; please report in [#2](https://github.com/ferloom/Wireless-Device-Battery-Monitor/issues/2) whether it works for you.

## Fixed in v1.3.1

- **Request support for a device:** the issue it opens showed the version as `vv1.3`; it now says `v1.3.1`.

## New in v1.3

- **New black-and-white icon** for the program, the setup and the shortcuts.
- **Position on the taskbar** now moves the widget in 20 px steps (20 to 200 px).
- **Height on the taskbar:** new menu to move the widget 5 / 10 / 15 px up or down.
- **Listed in Settings → Apps:** BatteryHub now shows up under *Installed apps* with its version, and *Uninstall* there removes it cleanly. The setup keeps a copy of itself in the program folder for this. This also prepares BatteryHub for winget.
- **No Visual C++ Redistributable needed:** the program and the setup now start on a clean Windows. Before, they needed `VCRUNTIME140.dll`, which most PCs have but some don't.
- **Faster automatic update check:** about 2 s after start instead of 20 s. If the network is not up yet (e.g. right after sign-in), it tries again every 15 s, up to 8 times.

## New in v1.2.2

- **Update finished:** after an update (from the menu or with the setup), BatteryHub says once which version it was updated from and to.
- **No flash on the main taskbar** at start-up when the widget is set to show only on another monitor.

## New in v1.2.1

- **Show widget on** (right-click menu, with more than one monitor), like the original: *All monitors*, *Main monitor only*, or *Display 2 (left) only* for any other monitor. The monitor is remembered by its identity; while it is unplugged, the widget is on the main taskbar.
- **Position on the taskbar:** next to the notification area or 40 / 80 / 120 / 160 / 200 px from it, to make room for other taskbar widgets (e.g. an iCUE sensor).
- **Background behind taskbar text (for translucent taskbars):** optional rounded plate in the taskbar's colour, off by default.

## New in v1.2

- **ROCCAT Syn Max Air** headset (1E7D:3A3F) over its 2.4 GHz receiver: level from the battery voltage, *Charging* / *Full* on the dock, disappears when switched off. Contributed by [BrooklynBanana](https://github.com/BrooklynBanana). ROCCAT Swarm must not run at the same time (it holds the receiver).
- **Request support for a device...** (right-click menu): lists every device BatteryHub can see (HID, USB, Bluetooth). Pick the one that is not shown, check its name and type, add notes, and *Send* opens a GitHub issue in your browser, filled in with exactly what the window shows (name, type, notes, vendor / product id, HID and USB interface summary, Windows build). You submit it there; nothing is sent from the app. *Copy full report* copies the same plus the list of all devices. Also `batteryhub-taskbar.exe --report`. Light or dark, following the Windows app theme.
- **Automatically start with Windows** (menu): switch the autostart entry on or off without the setup.
- **Automatically check for updates** (menu, off by default): checks once when BatteryHub starts and, if there is a new version, asks whether to update.
- **Shorter tooltip:** `PRO 2 LIGHTSPEED: 96%` (plus charging / full / asleep).
- **Setup:** the autostart line shows the registry entry (`HKCU\...\Run\BatteryHub Taskbar`) instead of the program path.

## v1.1

- **Check for updates** (right-click menu): asks GitHub for the latest release and shows "No new updates" or "A new update is available: v1.1 -> v1.x - update now?". On *Yes* the setup is downloaded, verified against the release's SHA-256 checksum and started; it updates and restarts BatteryHub.
- **About 0.2 MB of RAM when idle** (Task Manager "Memory" column; was about 1.1 MB): memory that is only needed for a moment is handed back to Windows after every read and Bluetooth check, and settings are cached instead of read from the registry every second.
- **Setup:** when it is done (install, update, uninstall or exit), it thanks you, links to the project for a star and closes itself after 5 seconds. On an error it stays open.

## Fixes since v1.0 (v1.0.1 / v1.0.1.1)

- Unplugged receiver / cable: its device disappears at once instead of showing "asleep".
- Bluetooth devices (AirPods...) disappear / appear right when they disconnect / connect; AirPods only while connected to this PC.
- Headsets switched on or off on a 2.4 GHz dongle, and mice waking up behind a receiver, show within ~10 s.
- No duplicate, garbled strip after an Explorer restart (seen on the small Windows 10 taskbar of ExplorerPatcher).
- Safe fallback to notification-area icons on a vertical taskbar or an unknown Explorer layout; unit tests and CI.

Windows may show a SmartScreen warning because the file is not code-signed: click *More info* -> *Run anyway*.
