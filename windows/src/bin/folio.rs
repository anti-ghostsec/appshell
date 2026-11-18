//! Folio for Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod ui {
    include!(concat!(env!("OUT_DIR"), "/ui_folio.rs"));
}

#[cfg(windows)]
fn main() {
    appshell_windows::run(appshell_windows::App::Folio, ui::FILES)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This is the Windows build of Folio; on Linux, run folio.py.");
}
