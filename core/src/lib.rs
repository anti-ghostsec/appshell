//! What Folio's and Leaflet's windows do for their pages, the same on every
//! platform. A platform shell (Windows, Android) hands the page's messages in
//! and carries out the [`Action`]s that come back: dialogs, printing, window
//! changes, script to run in the page. Files are read and written through
//! the [`Platform`] it provides.
//!
//! The rules are the Linux apps' (folio.py, leaflet.py): the page never names
//! a file; a file is reached by a token the page was given when the file was
//! opened by a route the page has no say in (the command line or "Open with",
//! a file dialog, a drop from the file manager), and where a file is saved is
//! only ever chosen by the user in a dialog.

pub mod folio;
pub mod leaflet;
pub mod web;

use serde::Serialize;

/// How a platform reaches files. A handle is whatever the platform uses to
/// name a file: a path on Windows, a content:// URI on Android. The page
/// never sees one.
pub trait Platform {
    /// Up to `limit` bytes; more than that is FileError::TooLarge.
    fn read(&self, handle: &str, limit: u64) -> Result<Vec<u8>, FileError>;
    /// Replace the file's contents, as safely as the platform allows
    /// (a temporary file renamed over it, where it can).
    fn write(&self, handle: &str, data: &[u8]) -> Result<(), String>;
    /// Last change, in milliseconds.
    fn mtime(&self, handle: &str) -> Option<i64>;
    fn exists(&self, handle: &str) -> bool;
    /// The file's name, for showing.
    fn name(&self, handle: &str) -> String;
    /// The folder it's in, for showing ("" if the platform can't say).
    fn folder(&self, handle: &str) -> String;
    /// The same file with ".ext" added, where names can be changed like that.
    fn with_extension(&self, handle: &str, ext: &str) -> Option<String>;

    /// The settings file's contents, if there is one.
    fn load_settings(&self) -> Option<Vec<u8>>;
    fn store_settings(&self, data: &[u8]);

    /// The user's home folder, "/"-separated (Folio shortens folders with it).
    fn home(&self) -> String {
        String::new()
    }
    fn system_dark(&self) -> bool {
        false
    }
    /// A theme forced from outside (LEAFLET_THEME on Windows).
    fn theme_override(&self) -> Option<String> {
        None
    }
    /// Whether a list of recent files can work: on Android a file can't be
    /// reached again after the app closes, so there's no such list.
    fn supports_recent(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    NotFound,
    IsDir,
    TooLarge,
    Other(String),
}

/// A choice in a file dialog: patterns for Windows, MIME types for Android.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Filter {
    pub name: String,
    pub patterns: Vec<String>,
    pub mime: Vec<String>,
}

impl Filter {
    pub fn new(name: &str, exts: &[&str], mime: &[&str]) -> Filter {
        Filter {
            name: name.into(),
            patterns: exts.iter().map(|e| if *e == "*" { "*".to_string() } else { format!("*.{e}") }).collect(),
            mime: mime.iter().map(|m| m.to_string()).collect(),
        }
    }
}

/// What the platform is asked to do. `win` names the window (Folio has one,
/// number 0). Answers to dialogs come back through `result(req, value)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "do", rename_all = "camelCase")]
pub enum Action {
    /// Run this script in the window's page.
    Eval { win: u32, js: String },
    /// An Open dialog. Answer: a list of handles, or null if cancelled.
    #[serde(rename_all = "camelCase")]
    OpenDialog { win: u32, req: u32, title: String, multiple: bool, filters: Vec<Filter>, near: Option<String> },
    /// A Save dialog. Answer: a handle, or null.
    #[serde(rename_all = "camelCase")]
    SaveDialog { win: u32, req: u32, title: String, name: String, filters: Vec<Filter>, near: Option<String>, mime: String },
    /// A question with buttons. Answer: the button's index, or null.
    #[serde(rename_all = "camelCase")]
    Ask { win: u32, req: u32, message: String, detail: String, buttons: Vec<String>, cancel: usize, default: usize },
    /// Hand a link the user agreed to open to the system's browser or mail program.
    OpenUrl { url: String },
    Title { win: u32, title: String },
    /// The colour behind the page, "#rrggbb".
    Background { win: u32, color: String, dark: bool },
    Fullscreen { win: u32 },
    /// Close the window now: everything that needed asking was asked.
    Close { win: u32 },
    /// Make a new window for this id and load the page in it.
    NewWindow { win: u32 },
    /// Bring the window to the front.
    Present { win: u32 },
    /// Reload the page (after its renderer stopped).
    Reload { win: u32 },
    /// Print the page as it's laid out.
    PrintPage { win: u32 },
    /// Print a PDF; its bytes are fetched once with `blob_take(win, token)`.
    PrintPdf { win: u32, token: String, name: String },
    /// Print the page to a PDF (A4, margins 20 mm top and bottom, 22 mm at
    /// the sides) and hand the bytes to `export_done(req, ...)`.
    ExportPdf { win: u32, req: u32 },
}

/// A random token, hex: what the page holds instead of a file.
pub fn token() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the system's random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// JSON for a script: also safe inside a JS string literal of the page.
pub fn js_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap().replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// A link shown in a question, cut short like the Linux apps do.
pub fn shown_url(url: &str) -> String {
    if url.chars().count() <= 300 {
        url.to_string()
    } else {
        url.chars().take(300).collect::<String>() + "…"
    }
}

/// Only these go to another program, and only after the user says so.
pub fn is_openable_link(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:")
}

pub fn ask_link(win: u32, req: u32, url: &str) -> Action {
    Action::Ask {
        win,
        req,
        message: "Open this link in your web browser?".into(),
        detail: shown_url(url),
        buttons: vec!["Cancel".into(), "Open link".into()],
        cancel: 0,
        default: 0,
    }
}
