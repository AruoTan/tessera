# Tessera development container

The container installs Rust stable,
the Windows x64 GNU target, MinGW cross-compiler/resource tools, NSIS, jq and ZIP
tools, then fetches the locked Cargo dependencies at the repository root.

From the repository root, build a Windows installer and portable executable:

```sh
bash packaging/build-installer.sh
```

Outputs are in `dist/windows-gnu/`: the unsigned NSIS installer,
portable ZIP, standalone `tessera.exe`, `LICENSE.txt`, and SHA-256 checksums.
You can also run **Rust: package Windows (container)** from VS Code's build tasks.
This uses `x86_64-pc-windows-gnu`; the Windows GitHub Actions installer uses MSVC.
The container can build packages without Wine or a Windows SDK.

Run the portable Rust checks in the [development guide](../docs/development.md).
Running the application and validating its native settings, context menu and taskbar
integration require Windows 10 or 11. Windows SDK/MSVC tools are not included here.
