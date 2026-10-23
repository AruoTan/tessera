# Development

Run the following commands from the repository root. The root `Cargo.toml`
defines the workspace; `target/` holds the build cache and `dist/` holds packaged
outputs. Both output directories are ignored by Git. See [Architecture](architecture.md)
for sampling, rendering, and source module responsibilities.

## Run on Windows

Install Rust stable (MSVC, Rust 1.95 or newer), Visual Studio Build Tools with
**Desktop development with C++**, and a Windows SDK. The SDK resource compiler
embeds the application icon, visual styles, and DPI/Windows compatibility manifest.

From the repository root:

```powershell
cargo run -p tessera --locked
```

The settings window appears on first launch. Subsequently, double-click the
readout with the left mouse button to open Settings. Right-click anywhere in the
readout for **固定位置** / **取消固定**, **暂停监控** / **继续监控**, and **退出**. The menu has no Settings item,
and the application has no notification-area icon. Closing Settings leaves the
application running; use **退出** in the readout's context menu to exit.
The settings window includes **网络配置**, **任务栏位置**, and **关于**.
Use **保存** to apply adapter changes, **撤销更改** to reload saved choices,
and **查看许可证** to open the bundled GPL text. Position reset and pause/resume
apply immediately. A disconnected selected adapter remains available in the list;
refreshing the list preserves unsaved selections. Launching a second copy
focuses the existing settings window. Closing and reopening settings reloads saved
preferences. The readout reattaches automatically after Explorer restarts.

Hold the left mouse button anywhere in the taskbar readout, including the blank
space between and around the two rows, and drag it to choose a
position. Movement stays on the taskbar and stops before the notification tray.
Release to save automatically; small click movements do not reposition it.
The saved position is relative to the usable taskbar, so it adjusts to resolution
and DPI changes. In **任务栏位置**, use **恢复默认位置**
to return beside the tray immediately. Dragging remains independent of unsaved
adapter changes in Settings. Losing mouse capture cancels the current drag.
The readout reserves only its icon, number, and unit columns, so it can be placed
at the left edge without a large invisible gutter. Its width also stays fixed
during a drag to keep the grabbed point steady.

```powershell
# Standalone executable: target/release/tessera.exe
cargo build --release -p tessera --locked

# Install NSIS 3, then create the installer, portable ZIP and checksums:
# Output: dist/windows-msvc/
./packaging/build-installer.ps1
```

Tessera uses the application identity `io.tessera` and stores settings at
`%APPDATA%\io.tessera\settings.json`.
The installer is per-user and adds a Start menu shortcut and an uninstall entry.
The portable executable stores settings in the same per-user directory.
Uninstalling preserves settings.

## Build Windows packages in the Linux container

Rebuild the development container to install MinGW, NSIS, ZIP tools and the
Windows x64 Rust target. Then, from the repository root:

```sh
bash packaging/build-installer.sh
```

The script resolves paths independently of the current directory and builds a
release executable for `x86_64-pc-windows-gnu`. It creates these unsigned outputs
in `dist/windows-gnu/`:

- `Tessera-<version>-x64-setup.exe`: per-user NSIS installer.
- `Tessera-portable.zip`: executable and GPL license.
- `tessera.exe` and `LICENSE.txt`: standalone portable files.
- `checksums.txt`: SHA-256 hashes of the four files above.

No Windows SDK, PowerShell or Wine is needed for cross-compilation or packaging.
The portable executable requires Windows 10 1903 or newer, or Windows 11 and stores settings in the
same roaming profile directory as the installed app. Test Windows desktop behavior
on Windows; successful packaging does not verify Explorer integration.

On a Debian/Ubuntu Linux host outside the development container, install Rust
stable and the packaging prerequisites first:

```sh
sudo apt-get update
sudo apt-get install --no-install-recommends build-essential ca-certificates gcc-mingw-w64-x86-64-posix nsis jq zip unzip
rustup target add x86_64-pc-windows-gnu
cargo fetch --locked
bash packaging/build-installer.sh
```

Windows packaging uses `packaging/build-installer.ps1` and the MSVC toolchain,
with outputs in `dist/windows-msvc/`. The NSIS definition is
`packaging/windows/installer.nsi`; both packaging scripts use the application icon
from `crates/tessera/assets/icons/Tessera.ico`. The matching PNG artwork is kept
alongside it as `tessera.png`; Windows builds embed only the ICO file. Its 256-pixel
frame uses lossless PNG compression; the five smaller frames remain uncompressed
32-bit bitmaps. All six resolutions and their original RGBA pixels are preserved.
This follows the [Windows icon compression guidance](https://learn.microsoft.com/en-us/windows/win32/uxguide/vis-icons).

## Build with GitHub Actions

Open **Actions → Build Installer → Run workflow** and choose the branch to build. Download
`Tessera-windows-x64-unsigned` from the run's Artifacts. It contains
the setup executable, portable ZIP, and SHA-256 checksums and is retained for 14 days.
No release or Store submission is performed and no signing secret is required.

CI and installer builds cache Cargo dependencies and compiled crates. Packaging
runs formatting and core tests; full desktop tests and Clippy remain in
**Rust CI**. Packaging builds the Rust release executable and invokes NSIS directly.
Hosted runners use four build jobs; local builds retain the
two-job memory limit. Release builds use size optimization (`opt-level = "s"`),
ThinLTO, 16 codegen units, stripped symbols, and abort-on-panic. The icon SVG parser
is a build-time dependency; it is excluded from the release executable.
Cold caches and Rust/dependency upgrades still require a full build;
compare subsequent runs with a cache hit when measuring the improvement.

## Check the project

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Portable workspace tests require no GUI libraries or desktop. The native settings,
context menu and taskbar widget are Windows-only; Linux builds run the portable
tests and report the supported platform when the executable is launched. Windows
CI runs the native control lifecycle/persistence and rendering tests as well.

## Limitations and Windows validation

The widget targets the primary taskbar. Settings currently use Simplified Chinese.
There is no persistent measurement history, startup registration, or automatic
update service. CPU temperature depends on available local sensor providers.

Before releasing a build, validate Windows 10 1903+ and Windows 11, mixed-DPI displays,
taskbar auto-hide, full-screen applications, Explorer restart, sleep/resume, adapter
disconnect/reconnect, pause/resume, and installation/uninstallation.

Verify that launching from Explorer, a shortcut, and the installer displays the
readout without clicking the taskbar.
Check that normal first presentation already has CPU/GPU/RAM usage values, and
that network disconnection does not delay it. On a machine with no DXGI hardware
GPU, confirm CPU and RAM are adjacent and the readout width shrinks by one column.
Disable GPU performance counters separately: an installed GPU must remain visible,
and unavailable readings must not prevent presentation beyond the startup limit.
Also verify eGPU connect/disconnect and a device change during dragging.
Hover taskbar thumbnails to enter and leave Aero Peek (including desktop preview);
the readout should remain visible whenever the taskbar is visible. Also check the
Start menu, tray flyouts, and calendar for normal stacking and focus behavior.
With auto-hide enabled, verify both animation directions, rapid reversals, and
disabled system animations: the readout must stay attached throughout, without
an independent fade, delayed pop-in, or leftover desktop overlay. Restart Explorer
while the taskbar is visible and while it is hidden; the readout should disappear
with the old taskbar and return on the replacement at its saved relative position.
Repeat with auto-hide already active when launching the app, and with Settings
open during Explorer restart. Check that dragging still works after recovery.
Repeat several Explorer restarts with sampling paused so recovery cannot depend
on changing speed text. The readout must remain above taskbar content after the
buttons and notification area finish loading, without another taskbar click.
During a slow Explorer restart, verify that no readout is shown over the desktop
between shell window creation and taskbar initialization. Repeat with auto-hide,
and confirm that starting the app on an already-running taskbar still shows it
without waiting for another Explorer restart. Validate visual timing on Windows;
the state-machine tests cannot establish frame alignment of Explorer rendering.

For dragging, also verify grabbing blank space between the rows, horizontal and
vertical taskbars, both ends of the available track, release outside the readout,
interrupted capture, left-double-click to open Settings, right-click for
lock/unlock, pause/resume and exit, restart persistence, and saving an already-open settings
form after repositioning. The readout must not snap back during sampling or after Save.
When locked, text, icons, and blank areas must all use the normal arrow cursor
and refuse dragging, while double-click Settings and the context menu still work.
The menu uses real inbox XAML `MenuFlyout` controls with system pin/unpin,
pause/play and exit font icons. The custom XAML presenter uses 8-DIP rounded
corners, Fluent Desktop Acrylic (`HostBackdrop`) material with an animated
hollow edge shadow, a subtle outline
and compact rows.
Check that corners remain transparent and the first/last item highlights stay
inside the rounded background. Inspect DPI scales, large system text, light/dark
system themes and high contrast (which bypasses custom styling). Check
card entry/exit and shadow animations with
Windows animation effects both enabled and disabled. Compare opening with the
system sound/network context menu, including the shadow that continues to
appear only after the 200-ms card entry completes, followed by a 400-ms opacity
reveal on the separate edge image. The shadow must remain absent throughout card movement;
`Loaded`/`Opened` alone must not trigger it. On a static light desktop, compare
the menu interior immediately after entry and after the shadow settles: its
color must not get darker as the shadow appears. Inspect the hollow edge image:
no shadow above the top edge, light sides, stronger bottom, smooth rounded corners, and fully
transparent outer limits. No shadow pixels may cover the menu surface. The
reserved gutter must not change the card's taskbar gap or introduce clipping.
The bottom shadow should fade across that gap and disappear at the taskbar's
upper edge; both sides should have a similarly extended fade rather than only
a narrow dark outline. At the top of each side, check the mirrored 45-degree
cutoff: the farther outward the shadow, the lower it starts, with a soft boundary.
The diagonal must keep its angle at different menu sizes and DPI scales.
Measure the opaque card's bottom (excluding the transparent shadow gutter) against
the taskbar top at 100%, 125%, 150% and 200% scaling: the gap should be 12 DIP,
rounded to physical pixels. Compare first/repeated openings and a secondary
monitor with negative screen coordinates; the card must not jump after entry.
The Windows smoke test checks the transformed visible bottom after the shadow
starts, so changing only an anchor constant cannot satisfy it.
The menu must remain compact: its height must equal the three row heights plus
padding, border and shadow gutter. The image must never make it a large square.
Verify both the card slide/fade and subsequent shadow fade on first and repeated
openings; an animation initialization failure must not be mistaken for success.
Compare light/dark themes and Windows animations disabled. Reopen and
close quickly during the reveal to catch lingering shadows or stale callbacks.
With Windows animations disabled the full shadow must appear immediately.
After settling, move the pointer across the
menu and widget, wait at least a second, and confirm there is no second jump
or placement correction. With animations disabled the menu must appear directly
at its final position. Verify arrow keys, Enter, Escape, outside-click dismissal,
screen edges, and repeated opening/closing.
After selecting an item, the action should run after the close animation.
Sampling and widget updates must continue during first-use XAML initialization.
The menu thread/XAML source are created on first use and retained until shutdown;
compare memory before and after first use if measuring the cost.

An opt-in smoke test exercises real XAML controls, popup state, close events and
stale callback rejection on an interactive Windows desktop:

```powershell
cargo test -p tessera desktop_flyout_uses_system_controls --locked -- --ignored --test-threads=1
```

It is ignored in ordinary CI because hosted/server or noninteractive Windows
sessions need not provide a usable inbox XAML desktop. Cross-compilation only
checks the code and linking; it cannot verify appearance or animation frames.
Verify lock persistence after restarting Tessera and Explorer, and that saving
an already-open settings form cannot undo either locking or unlocking.
Unlocking must immediately restore dragging and the move cursor. An explicit
position reset remains available while locked and preserves the lock.
Check right-clicking text and blank space: only the readout menu should appear,
without opening Settings or the taskbar menu. Verify menu cancellation with Escape
and outside clicks, and that sampling continues while the menu is open. Check
pause/resume menu labels against the settings button and repeat after Explorer
restart. No Tessera notification-area icon should appear.

There is no telemetry or application-initiated external probing. Sampling reads
local system counters. Settings stay local; acquisition runs only while the app runs.

Released under [GNU GPL v3](../LICENSE). See the [privacy policy](privacy.md).
