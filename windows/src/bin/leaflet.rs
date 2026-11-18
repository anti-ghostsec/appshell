//! Leaflet for Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod ui {
    include!(concat!(env!("OUT_DIR"), "/ui_leaflet.rs"));
}

#[cfg(windows)]
fn main() {
    appshell_windows::run(appshell_windows::App::Leaflet, ui::FILES)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This is the Windows build of Leaflet; on Linux, run leaflet.py.");
}
