# Tessera

A Windows taskbar monitor written entirely in Rust, using native Win32 settings, a XAML context menu styled after Windows 11,
and DirectWrite/Direct2D rendering. Current version: `0.1.0`.

- Live upload/download speeds from physical Ethernet and Wi-Fi adapters, sampled every second.
- CPU utilization and optional temperature, GPU utilization and memory usage, and physical RAM usage.
- Automatic GPU detection with a compact CPU/RAM layout when no GPU is present.
- A draggable taskbar widget that follows taskbar visibility and recovers after Explorer restarts.
- Native settings for adapter selection and taskbar position; double-click the widget to open them, or use the XAML MenuFlyout to lock/unlock its position, pause/resume, or exit.
- System fonts, DPI-aware rendering, and automatic light/dark taskbar colors.

The widget targets the primary taskbar on Windows 10 1903 or newer and Windows 11 (x64). CPU temperature
availability depends on firmware or a compatible local hardware-monitor provider.
The context menu styles Windows' built-in XAML controls with 8-DIP rounded corners,
Fluent Desktop Acrylic (`HostBackdrop`) material, compact rows and system font icons. It follows the system
light/dark theme and animation preference, including a separate edge-shadow reveal after card entry completes;
high contrast retains the inbox style.
It initializes on the first right-click and needs no separate Windows App SDK runtime.
Settings are stored at `%APPDATA%\io.tessera\settings.json`.

## Develop on Windows

Install Rust stable (MSVC, Rust 1.95 or newer), Visual Studio Build Tools with
**Desktop development with C++**, and a Windows SDK.

```powershell
cargo run -p tessera --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Build packages

On Windows, install NSIS 3 and run:

```powershell
./packaging/build-installer.ps1
```

The [development container](.devcontainer/README.md) can cross-compile Windows x64
installers and portable executables from the repository root:

```sh
bash packaging/build-installer.sh
```

Windows packages are written to `dist/windows-msvc/`; container packages are under
`dist/windows-gnu/`. Both include an unsigned installer, portable ZIP,
and SHA-256 checksums. Running the application requires Windows.

**GitHub Actions:** **Rust CI** runs Linux and Windows checks. Run **Build Installer**
manually to download an unsigned Windows x64 installer and portable ZIP.

See [Development](docs/development.md) for setup, packaging, limitations, and
desktop validation, and [Architecture](docs/architecture.md) for sampling and rendering details.

## Repository

| Path | Contents |
| --- | --- |
| `Cargo.toml` | Root Rust workspace and shared package configuration |
| `crates/tessera/` | Native Windows application, sampling workers, settings, and taskbar widget |
| `crates/tessera-core/` | Portable settings, accounting, formatting, and geometry logic |
| `crates/tessera/assets/` | Application icon and SVG widget icons |
| `crates/tessera/resources/windows/` | Windows application manifest |
| `packaging/` | Windows and Linux packaging scripts and NSIS installer |
| `docs/` | Architecture, development guide, and privacy policy |
| `.devcontainer/` | Rust development and Windows cross-compilation environment |
| `.github/workflows/` | Rust checks and manual installer builds |
| `target/` | Ignored Cargo build cache |
| `dist/` | Ignored installer and portable packages, separated by toolchain |

See the [privacy policy](docs/privacy.md). Released under [GNU GPL v3](LICENSE).
