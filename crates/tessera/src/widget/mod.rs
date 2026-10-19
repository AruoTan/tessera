#[cfg(windows)]
mod bitmap;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
mod text;

#[cfg(any(windows, test))]
mod surface;

#[cfg(any(windows, test))]
mod icons;

#[cfg(any(windows, test))]
mod gauge;

#[cfg(any(windows, test))]
mod presentation;

#[cfg(any(windows, test))]
mod startup;

#[cfg(windows)]
pub use windows::start;

pub fn show_startup_error(message: &str) {
    eprintln!("{message}");
    #[cfg(windows)]
    windows::show_error(message);
}

#[cfg(not(windows))]
pub fn start(_app: crate::desktop::Handle, _state: crate::state::AppState) -> std::io::Result<()> {
    // Native windows and live monitoring are Windows-only.
    Ok(())
}
