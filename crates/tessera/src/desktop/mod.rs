//! Rust-owned Windows application lifecycle and native settings controls.

#[cfg(windows)]
mod settings;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{Handle, run};

#[cfg(not(windows))]
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct Handle;

#[cfg(not(windows))]
pub fn run() -> Result<(), String> {
    Err("Tessera requires Windows 10 or 11. Portable Rust tests can run on other platforms.".into())
}

/// Keep a stable settings directory across application updates.
#[cfg(any(windows, test))]
fn settings_path(roaming: &std::path::Path) -> std::path::PathBuf {
    roaming.join("io.tessera").join("settings.json")
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_location_uses_tessera_application_identity() {
        let root = std::path::Path::new("roaming");
        assert_eq!(
            super::settings_path(root),
            root.join("io.tessera/settings.json")
        );
    }
}
