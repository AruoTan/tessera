#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod desktop;
#[cfg_attr(not(windows), allow(dead_code))]
mod monitor;
#[cfg_attr(not(windows), allow(dead_code))]
mod state;
#[cfg_attr(not(windows), allow(dead_code))]
mod widget;

fn main() {
    if let Err(error) = desktop::run() {
        widget::show_startup_error(&format!("Unable to start Tessera: {error}"));
    }
}
