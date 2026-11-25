# Experimental Windows taskbar integration

The optional `experimental-xaml` build hosts BatteryHub's existing Rust-rendered strip inside the
verified primary horizontal taskbar's `SystemTrayFrameGrid`. Battery polling, rendering, history,
settings and device support remain in BatteryHub; the native XAML host handles presentation and
pointer forwarding. The adapter does not open HID devices.

## Layout and interaction

- `Grouped` uses one XAML surface and one persisted logical anchor.
- `Individual` uses one independently anchored module per visible device. Device keys preserve
  anchors across reorder and reconnect. The existing renderer owns the pixels and hover card; each
  module forwards enter, leave and pointer movement to it. Hover highlights only the pointed-to
  module. Dragging uses the shared insertion marker; the global position lock applies to all
  modules.
- Anchors are direct children or verified grid columns, not screen coordinates. The default is
  after `NotifyIconStack`. A missing saved anchor falls back to that slot without overwriting the
  saved preference.

## Compatibility and fallback

The host checks the Windows build, taskbar module identity, PDB identity, XAML layout, and visible
presentation before hiding the traditional widget. The verified development target is Windows 11
build `26300.9818` with a horizontal primary taskbar. Unknown builds or layouts, vertical or
secondary taskbars, missing/mismatched symbols, unsupported offsets, or failed readiness checks
keep or restore the traditional strip. The adapter removes its own XAML slots when it detaches;
it does not restart or patch Explorer on disk.

The implementation changes only verified direct children of the outer tray container. Nested
generated notification-icon collections, the grouped Control Center button, the clock, and Show
desktop are outside the supported anchor set.

## Build and local preview

Build the standard release with `build-release.ps1`. The experimental build additionally requires
Rust `1.99.0`, Visual Studio 2022 Build Tools with the Windows SDK, and the exact matching
`taskbar.dll` public PDB at `target/xaml-preview/artifacts/Taskbar.pdb`. The host verifies the
module and PDB identities before attaching. Obtain symbols locally through the Microsoft Symbol
Server; do not commit or redistribute the PDB. [Microsoft's symbol-server license terms](https://learn.microsoft.com/en-us/legal/windows-sdk/microsoft-symbol-server-license-terms)
allow symbol use for debugging/testing with Microsoft software and prohibit sharing or publishing
the symbol items as standalone offerings.

Run `scripts/Start-ExperimentalPreview.ps1` to start the isolated Rust supervisor, candidate and
XAML host. Run `scripts/Stop-ExperimentalPreview.ps1` to detach and close them. The preview does
not install an autostart entry. Its settings are under
`HKCU\Software\BatteryHub-Experimental\Taskbar`; the stable edition uses a separate registry
key. To remove the local preview completely, stop it, delete `target/xaml-preview/candidate`, and
remove only the experimental registry key if its settings and history are no longer needed.

No experimental installer or distributable portable archive is produced: the preview needs a
matching Microsoft PDB that cannot be bundled for redistribution, and the standard setup embeds
only one executable rather than the multi-file XAML host. The standard installer remains
independent and unchanged.
