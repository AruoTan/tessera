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
readout for **暂停监控** / **继续监控** and **退出**. The menu has no Settings item,
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
The portable executable requires Windows 10 or 11 and stores settings in the
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
alongside it as `tessera.png`; Windows builds embed only the ICO file.

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

Before releasing a build, validate Windows 10 and 11, mixed-DPI displays,
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
pause/resume and exit, restart persistence, and saving an already-open settings
form after repositioning. The readout must not snap back during sampling or after Save.
Check right-clicking text and blank space: only the readout menu should appear,
without opening Settings or the taskbar menu. Verify menu cancellation with Escape
and outside clicks, and that sampling continues while the menu is open. Check
pause/resume menu labels against the settings button and repeat after Explorer
restart. No Tessera notification-area icon should appear.

There is no telemetry or application-initiated external probing. Sampling reads
local system counters. Settings stay local; acquisition runs only while the app runs.

Released under [GNU GPL v3](../LICENSE). See the [privacy policy](privacy.md).
