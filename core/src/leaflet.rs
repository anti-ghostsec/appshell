//! Leaflet's windows: leaflet.py's bridge, without GTK.
//!
//! The page posts `webkit.messageHandlers.leaflet.postMessage(JSON {cmd, ...})`
//! and is told things with `Leaflet.handle({what, ...})`. File contents go
//! through `/put` (page to Leaflet, answered with a token) and `/blob/<token>`
//! (Leaflet to page, fetched once). The page never names a file: which file
//! is read or written is decided here, by a dialog, the command line, a file
//! dropped on the window, the open document's own path, or the recent list,
//! which the page refers to by number.

use crate::{ask_link, js_json, token, Action, FileError, Filter, Platform};
use leaflet_formats as formats;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

pub const MAX_FILE: u64 = 512 * 1024 * 1024;
pub const MAX_RECENT: usize = 10;

pub fn theme_bg(theme: &str) -> &'static str {
    match theme {
        "dark" => "#25262b",
        "classic" => "#c0c0c0",
        _ => "#e9eaee",
    }
}

fn save_filter(fmt: &str) -> Option<Filter> {
    Some(match fmt {
        "pdf" => Filter::new("PDF", &["pdf"], &["application/pdf"]),
        "docx" => Filter::new("Word document (.docx)", &["docx"], &["application/vnd.openxmlformats-officedocument.wordprocessingml.document"]),
        "odt" => Filter::new("OpenDocument text (.odt)", &["odt"], &["application/vnd.oasis.opendocument.text"]),
        "txt" => Filter::new("Plain text (.txt)", &["txt"], &["text/plain"]),
        _ => return None,
    })
}

/// A file name the page suggested, as a plain name: no folders, nothing odd.
pub fn clean_name(name: &str, fallback: &str) -> String {
    let n = name.replace('\\', "/");
    let base = n.rsplit('/').next().unwrap_or("");
    let printable: String = base.chars().filter(|c| !c.is_control() && !matches!(*c, '\u{2028}' | '\u{2029}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')).collect();
    let trimmed: String = printable.trim().trim_start_matches('.').chars().take(120).collect();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

fn stem(name: &str) -> &str {
    // os.path.splitext: a leading dot doesn't start an extension
    match name.rfind('.') {
        Some(i) if i > 0 && !name[..i].chars().all(|c| c == '.') => &name[..i],
        _ => name,
    }
}

#[derive(Clone)]
enum Pending {
    Path(String),
    Data(Vec<u8>, String),
    New,
}

#[derive(Clone, Copy, PartialEq)]
enum After {
    Close,
    /// Close, then go on asking about the other windows.
    CloseAndQuit,
}

enum Then {
    Write { data: Vec<u8>, fmt: String, export: bool },
    Export,
}

enum Wait {
    Open,
    Pick { what: String, at: Value },
    SaveWhere { fmt: String, then: Then },
    Replace { handle: String, then: Then },
    Export { target: String },
    Link { url: String },
    Unsaved { then: After },
}

#[derive(Default)]
struct Win {
    path: Option<String>,
    kind: Option<String>,
    dirty: bool,
    name: String,
    closing: bool,
    store: HashMap<String, (Vec<u8>, String)>,
    pending: Option<Pending>,
    ready: bool,
    after_save: Option<After>,
    recent_shown: Vec<String>,
    exports: HashMap<String, String>,
    queued_error: Option<String>,
}

impl Win {
    fn put(&mut self, data: Vec<u8>, kind: &str) -> String {
        let t = token();
        self.store.insert(t.clone(), (data, kind.to_string()));
        t
    }
    fn take(&mut self, t: &str) -> Option<(Vec<u8>, String)> {
        self.store.remove(t)
    }
}

pub struct Leaflet {
    p: Box<dyn Platform>,
    settings: Map<String, Value>,
    wins: BTreeMap<u32, Win>,
    next_win: u32,
    waits: HashMap<u32, (u32, Wait)>,
    next_req: u32,
}

fn send(win: u32, what: &str, mut payload: Value) -> Action {
    payload["what"] = json!(what);
    Action::Eval { win, js: format!("window.Leaflet && window.Leaflet.handle({})", js_json(&payload)) }
}

fn error(win: u32, text: impl Into<String>) -> Action {
    send(win, "error", json!({"text": text.into()}))
}

fn file_error(e: &FileError) -> String {
    match e {
        FileError::NotFound => "No such file or directory".into(),
        FileError::IsDir => "Is a directory".into(),
        FileError::TooLarge => "the file is larger than 512 MB".into(),
        FileError::Other(s) => s.clone(),
    }
}

impl Leaflet {
    pub fn new(p: Box<dyn Platform>) -> Leaflet {
        let settings = p
            .load_settings()
            .and_then(|d| serde_json::from_slice::<Value>(&d).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Leaflet { p, settings, wins: BTreeMap::new(), next_win: 1, waits: HashMap::new(), next_req: 1 }
    }

    pub fn platform(&self) -> &dyn Platform {
        &*self.p
    }

    pub fn windows(&self) -> Vec<u32> {
        self.wins.keys().copied().collect()
    }

    pub fn window_size(&self) -> (u32, u32, bool) {
        let w = self.settings.get("_window");
        let n = |k: &str, d: u64| w.and_then(|w| w.get(k)).and_then(Value::as_u64).unwrap_or(d).clamp(200, 10000) as u32;
        (n("w", 1100), n("h", 800), w.and_then(|w| w.get("max")).and_then(Value::as_bool).unwrap_or(false))
    }

    /// A new window: empty, or opening a file, or a new document.
    fn make_window(&mut self, pending: Option<Pending>) -> (u32, Action) {
        let id = self.next_win;
        self.next_win += 1;
        self.wins.insert(id, Win { pending, ..Default::default() });
        (id, Action::NewWindow { win: id })
    }

    /// Leaflet started: one window per file it was opened with, or an empty one.
    pub fn start(&mut self, handles: Vec<String>) -> Vec<Action> {
        if handles.is_empty() {
            return vec![self.make_window(None).1];
        }
        handles.into_iter().map(|h| self.make_window(Some(Pending::Path(h))).1).collect()
    }

    /// Opened again from outside (a second "Open with"): new windows.
    pub fn open_files(&mut self, handles: Vec<String>) -> Vec<Action> {
        self.start(handles)
    }

    /// An empty window (Leaflet's icon clicked again).
    pub fn activate(&mut self) -> Vec<Action> {
        vec![self.make_window(None).1]
    }

    pub fn theme(&self) -> String {
        let theme = self.p.theme_override().unwrap_or_else(|| self.settings.get("theme").and_then(Value::as_str).unwrap_or("auto").to_string());
        let theme = if theme == "auto" { if self.p.system_dark() { "dark".to_string() } else { "light".to_string() } } else { theme };
        if matches!(theme.as_str(), "light" | "dark" | "classic") {
            theme
        } else {
            "light".into()
        }
    }

    /// The colour behind the page.
    pub fn background(&self) -> (String, bool) {
        let t = self.theme();
        (theme_bg(&t).to_string(), t == "dark")
    }

    fn remember_recent(&self) -> bool {
        self.p.supports_recent() && self.settings.get("remember_recent").and_then(Value::as_bool).unwrap_or(false)
    }

    fn state(&mut self, win: u32) -> Value {
        let recent: Vec<String> = if self.remember_recent() {
            self.settings.get("recent").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect()
        } else {
            vec![]
        };
        let shown: Vec<String> = recent.into_iter().filter(|h| self.p.exists(h)).collect();
        let list: Vec<Value> = shown.iter().map(|h| json!({"name": self.p.name(h), "folder": self.p.folder(h)})).collect();
        let theme_setting = self.p.theme_override().unwrap_or_else(|| self.settings.get("theme").and_then(Value::as_str).unwrap_or("auto").to_string());
        let remember = self.remember_recent();
        if let Some(w) = self.wins.get_mut(&win) {
            w.recent_shown = shown;
        }
        json!({"theme": self.theme(), "themeSetting": theme_setting, "rememberRecent": remember, "recent": list})
    }

    fn req(&mut self, win: u32, w: Wait) -> u32 {
        let r = self.next_req;
        self.next_req += 1;
        self.waits.insert(r, (win, w));
        r
    }

    fn store_settings(&self) {
        if let Ok(d) = serde_json::to_vec_pretty(&Value::Object(self.settings.clone())) {
            self.p.store_settings(&d);
        }
    }

    fn remember(&mut self, handle: &str) {
        if !self.remember_recent() {
            return;
        }
        let mut recent: Vec<Value> = vec![json!(handle)];
        for v in self.settings.get("recent").and_then(Value::as_array).into_iter().flatten() {
            if v.as_str() != Some(handle) && recent.len() < MAX_RECENT {
                recent.push(v.clone());
            }
        }
        self.settings.insert("recent".into(), Value::Array(recent));
        self.store_settings();
    }

    // ------------------------------------------------------------ bytes for the page

    /// GET /blob/<token>: fetched once.
    pub fn blob_take(&mut self, win: u32, token: &str) -> Option<(Vec<u8>, String)> {
        self.wins.get_mut(&win)?.take(token)
    }

    /// POST /put: the page hands over bytes and gets a token back.
    pub fn blob_put(&mut self, win: u32, data: Vec<u8>, kind: &str) -> Option<String> {
        if data.len() as u64 > MAX_FILE {
            return None;
        }
        let kind = kind.split(';').next().unwrap_or("application/octet-stream").trim();
        Some(self.wins.get_mut(&win)?.put(data, kind))
    }

    // ------------------------------------------------------------ the page's messages

    pub fn message(&mut self, win: u32, raw: &str) -> Vec<Action> {
        if !self.wins.contains_key(&win) {
            return vec![];
        }
        let Ok(msg) = serde_json::from_str::<Value>(raw) else { return vec![] };
        let Some(cmd) = msg.get("cmd").and_then(Value::as_str).map(str::to_string) else { return vec![] };
        let msg = msg.as_object().cloned().unwrap_or_default();
        match cmd.as_str() {
            "ready" => self.cmd_ready(win),
            "title" => {
                let dirty = msg.get("dirty").and_then(Value::as_bool).unwrap_or(false);
                self.set_title(win, dirty)
            }
            "open" => self.cmd_open(win, &msg),
            "new" => self.cmd_new(win),
            "drop" => {
                let t = msg.get("token").and_then(Value::as_str).unwrap_or("");
                let name = clean_name(msg.get("name").and_then(Value::as_str).unwrap_or(""), "Dropped file");
                match self.wins.get_mut(&win).unwrap().take(t) {
                    Some((data, _)) => self.open_in(win, Pending::Data(data, name)),
                    None => vec![],
                }
            }
            "pick" => self.cmd_pick(win, &msg),
            "save" => self.cmd_save(win, &msg),
            "export-pdf" => {
                let a = self.ask_where(win, "pdf", None, Then::Export);
                vec![a]
            }
            "write-export" => self.cmd_write_export(win, &msg),
            "print" => self.cmd_print(win, &msg),
            "link" => {
                let url = msg.get("url").and_then(Value::as_str).unwrap_or("").to_string();
                if !crate::is_openable_link(&url) {
                    return vec![];
                }
                let req = self.req(win, Wait::Link { url: url.clone() });
                vec![ask_link(win, req, &url)]
            }
            "settings" => self.cmd_settings(&msg),
            "close" => {
                self.wins.get_mut(&win).unwrap().closing = true;
                vec![Action::Close { win }]
            }
            "quit" => self.ask_quit(),
            "fullscreen" => vec![Action::Fullscreen { win }],
            _ => vec![],
        }
    }

    fn cmd_ready(&mut self, win: u32) -> Vec<Action> {
        let state = self.state(win);
        let w = self.wins.get_mut(&win).unwrap();
        w.ready = true;
        let mut out = vec![send(win, "state", state)];
        if let Some(e) = w.queued_error.take() {
            out.push(error(win, e));
        }
        match w.pending.take() {
            Some(Pending::Path(h)) => out.extend(self.open_path(win, Some(&h), None)),
            Some(Pending::Data(d, n)) => out.extend(self.open_path(win, None, Some((d, n)))),
            Some(Pending::New) => out.extend(self.cmd_new(win)),
            None => {}
        }
        out
    }

    fn set_title(&mut self, win: u32, dirty: bool) -> Vec<Action> {
        let w = self.wins.get_mut(&win).unwrap();
        w.dirty = dirty;
        let name = if w.name.is_empty() { "Untitled" } else { &w.name };
        vec![Action::Title { win, title: format!("{}{name} — Leaflet", if dirty { "• " } else { "" }) }]
    }

    fn cmd_open(&mut self, win: u32, msg: &Map<String, Value>) -> Vec<Action> {
        if let Some(n) = msg.get("recent") {
            // a file in the recent list, by its place in it
            let w = &self.wins[&win];
            if let Some(h) = n.as_u64().and_then(|n| w.recent_shown.get(n as usize)).cloned() {
                return self.open_in(win, Pending::Path(h));
            }
            return vec![];
        }
        let req = self.req(win, Wait::Open);
        let all = [
            "pdf", "docx", "odt", "rtf", "txt",
        ];
        vec![Action::OpenDialog {
            win,
            req,
            title: "Open".into(),
            multiple: false,
            filters: vec![
                Filter::new(
                    "PDFs and documents",
                    &all,
                    &[
                        "application/pdf",
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                        "application/vnd.oasis.opendocument.text",
                        "application/rtf",
                        "text/rtf",
                        "text/plain",
                    ],
                ),
                Filter::new("All files", &["*"], &["*/*"]),
            ],
            near: self.wins[&win].path.clone(),
        }]
    }

    fn cmd_new(&mut self, win: u32) -> Vec<Action> {
        if self.wins[&win].kind.is_some() {
            return vec![self.make_window(Some(Pending::New)).1];
        }
        let w = self.wins.get_mut(&win).unwrap();
        w.kind = Some("docx".into());
        w.path = None;
        w.name = String::new();
        let mut out = vec![send(win, "load", json!({"kind": "doc", "name": "Untitled", "format": "docx", "saveAs": true, "html": "<p><br></p>", "warnings": []}))];
        out.extend(self.set_title(win, false));
        out
    }

    /// Open a file in this window if it's empty, otherwise in a new one.
    fn open_in(&mut self, win: u32, what: Pending) -> Vec<Action> {
        let w = &self.wins[&win];
        if w.kind.is_none() && !w.dirty {
            match what {
                Pending::Path(h) => self.open_path(win, Some(&h), None),
                Pending::Data(d, n) => self.open_path(win, None, Some((d, n))),
                Pending::New => self.cmd_new(win),
            }
        } else {
            vec![self.make_window(Some(what)).1]
        }
    }

    fn open_path(&mut self, win: u32, handle: Option<&str>, data: Option<(Vec<u8>, String)>) -> Vec<Action> {
        let shown = match (handle, &data) {
            (Some(h), _) => self.p.name(h),
            (None, Some((_, n))) => n.clone(),
            _ => "the file".into(),
        };
        let bytes = match (handle, data) {
            (_, Some((d, _))) => d,
            (Some(h), None) => match self.p.read(h, MAX_FILE) {
                Ok(d) => d,
                Err(e) => return vec![error(win, format!("Couldn't open {shown}: {}", file_error(&e)))],
            },
            _ => return vec![],
        };
        let kind = match formats::detect(&bytes) {
            None => return vec![error(win, format!("Couldn't open {shown}: Leaflet can't open this kind of file."))],
            Some("ole") => {
                return vec![error(
                    win,
                    format!("Couldn't open {shown}: This is an old binary Word file (.doc). Leaflet opens .docx, .odt, .rtf, .txt and PDF files."),
                )]
            }
            Some(k) => k.to_string(),
        };
        if let Some(h) = handle {
            self.remember(h);
        }
        let w = self.wins.get_mut(&win).unwrap();
        w.path = handle.map(str::to_string);
        w.kind = Some(kind.clone());
        w.name = shown.clone();
        let mut out = Vec::new();
        if kind == "pdf" {
            let t = w.put(bytes, "application/pdf");
            out.push(send(win, "load", json!({"kind": "pdf", "name": shown, "token": t, "saveAs": handle.is_none()})));
        } else {
            let doc = match formats::load(&bytes, &kind) {
                Ok(d) => d,
                Err(e) => {
                    w.kind = None;
                    return vec![error(win, format!("Couldn't open {shown}: {e}"))];
                }
            };
            // RTF and text can be read but are saved as DOCX (or text).
            let save_as = handle.is_none() || kind == "rtf";
            out.push(send(
                win,
                "load",
                json!({"kind": "doc", "name": shown, "format": kind, "saveAs": save_as, "html": formats::to_html(&doc), "warnings": doc.warnings}),
            ));
        }
        out.extend(self.set_title(win, false));
        out
    }

    fn cmd_pick(&mut self, win: u32, msg: &Map<String, Value>) -> Vec<Action> {
        let what = if msg.get("for").and_then(Value::as_str) == Some("pdf") { "pdf" } else { "image" };
        let filter = if what == "pdf" {
            Filter::new("PDF", &["pdf"], &["application/pdf"])
        } else {
            Filter::new("Pictures", &["png", "jpg", "jpeg", "gif", "bmp"], &["image/png", "image/jpeg", "image/gif", "image/bmp"])
        };
        let at = msg.get("at").cloned().unwrap_or(Value::Null);
        let req = self.req(win, Wait::Pick { what: what.into(), at });
        vec![Action::OpenDialog {
            win,
            req,
            title: if what == "pdf" { "Insert pages from a PDF".into() } else { "Insert a picture".into() },
            multiple: false,
            filters: vec![filter],
            near: None,
        }]
    }

    fn cmd_save(&mut self, win: u32, msg: &Map<String, Value>) -> Vec<Action> {
        let token = msg.get("token").and_then(Value::as_str).unwrap_or("").to_string();
        let w = self.wins.get_mut(&win).unwrap();
        let fmt = msg.get("format").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).or_else(|| w.kind.clone()).unwrap_or_default();
        let Some((mut data, kind)) = w.take(&token) else { return vec![error(win, "Nothing to save.")] };
        // A document is saved as a document, and only a PDF as a PDF.
        let is_html = kind == "text/html";
        if save_filter(&fmt).is_none() || (fmt == "pdf") == is_html || (fmt == "pdf" && formats::detect(&data) != Some("pdf")) {
            return vec![error(win, "Leaflet can't save that.")];
        }
        if is_html {
            let Ok(html) = String::from_utf8(data) else { return vec![error(win, "Leaflet can't save that.")] };
            let doc = formats::from_html(&html);
            data = match formats::save(&doc, &fmt) {
                Ok(d) => d,
                Err(e) => return vec![error(win, e.to_string())],
            };
        }
        let same_format = w.kind.as_deref() == Some(fmt.as_str());
        let as_ = msg.get("as").and_then(Value::as_bool).unwrap_or(false);
        let export = msg.get("export").and_then(Value::as_bool).unwrap_or(false);
        let then = Then::Write { data, fmt: fmt.clone(), export };
        if let (Some(path), false, true) = (w.path.clone(), as_, same_format) {
            return self.run_then(win, &path, then);
        }
        let suggest = msg.get("suggest").and_then(Value::as_str).map(str::to_string);
        vec![self.ask_where(win, &fmt, suggest, then)]
    }

    /// Where a file goes is only ever chosen here, in a dialog; the page can
    /// suggest a name for it and nothing more.
    fn ask_where(&mut self, win: u32, fmt: &str, suggest: Option<String>, then: Then) -> Action {
        let w = &self.wins[&win];
        let title = if fmt != "pdf" || w.kind.as_deref() == Some("pdf") { "Save as" } else { "Export as PDF" };
        let base = clean_name(suggest.as_deref().filter(|s| !s.is_empty()).unwrap_or(&w.name), "Untitled");
        let st = stem(&base);
        let name = format!("{}.{fmt}", if st.is_empty() { "Untitled" } else { st });
        let filter = save_filter(fmt).unwrap();
        let mime = filter.mime[0].clone();
        let near = w.path.clone();
        let req = self.req(win, Wait::SaveWhere { fmt: fmt.into(), then });
        Action::SaveDialog { win, req, title: title.into(), name, filters: vec![filter], near, mime }
    }

    fn run_then(&mut self, win: u32, handle: &str, then: Then) -> Vec<Action> {
        match then {
            Then::Write { data, fmt, export } => self.write(win, handle, &data, &fmt, export),
            Then::Export => {
                let req = self.req(win, Wait::Export { target: handle.to_string() });
                vec![Action::ExportPdf { win, req }]
            }
        }
    }

    fn write(&mut self, win: u32, handle: &str, data: &[u8], fmt: &str, export: bool) -> Vec<Action> {
        if let Err(e) = self.p.write(handle, data) {
            return vec![error(win, format!("Couldn't save: {e}"))];
        }
        let name = self.p.name(handle);
        if !export {
            let w = self.wins.get_mut(&win).unwrap();
            w.path = Some(handle.to_string());
            w.kind = Some(fmt.to_string());
            w.name = name.clone();
            self.remember(handle);
        }
        let mut out = vec![send(win, "saved", json!({"name": name, "export": export, "format": fmt}))];
        if !export {
            out.extend(self.set_title(win, false));
            if let Some(after) = self.wins.get_mut(&win).unwrap().after_save.take() {
                out.extend(self.after(win, after));
            }
        }
        out
    }

    fn after(&mut self, win: u32, after: After) -> Vec<Action> {
        if let Some(w) = self.wins.get_mut(&win) {
            w.closing = true;
        }
        let mut out = vec![Action::Close { win }];
        if after == After::CloseAndQuit {
            out.extend(self.ask_quit());
        }
        out
    }

    fn cmd_write_export(&mut self, win: u32, msg: &Map<String, Value>) -> Vec<Action> {
        let w = self.wins.get_mut(&win).unwrap();
        let target = msg.get("ticket").and_then(Value::as_str).and_then(|t| w.exports.remove(t));
        let data = w.take(msg.get("token").and_then(Value::as_str).unwrap_or(""));
        match (target, data) {
            (Some(t), Some((d, _))) if formats::detect(&d) == Some("pdf") => self.write(win, &t, &d, "pdf", true),
            _ => vec![],
        }
    }

    fn cmd_print(&mut self, win: u32, msg: &Map<String, Value>) -> Vec<Action> {
        let w = self.wins.get_mut(&win).unwrap();
        match w.take(msg.get("token").and_then(Value::as_str).unwrap_or("")) {
            None => vec![Action::PrintPage { win }], // a document: print the page as it's laid out
            Some((data, _)) => {
                if formats::detect(&data) != Some("pdf") {
                    return vec![error(win, "Couldn't print: that isn't a PDF.")];
                }
                let name = format!("{}.pdf", stem(&clean_name(&w.name, "document")));
                let token = w.put(data, "application/pdf");
                vec![Action::PrintPdf { win, token, name }]
            }
        }
    }

    fn cmd_settings(&mut self, msg: &Map<String, Value>) -> Vec<Action> {
        if let Some(t) = msg.get("theme").and_then(Value::as_str).filter(|t| matches!(*t, "auto" | "light" | "dark" | "classic")) {
            self.settings.insert("theme".into(), json!(t));
        }
        if let Some(r) = msg.get("rememberRecent") {
            let on = r.as_bool().unwrap_or(false) && self.p.supports_recent();
            self.settings.insert("remember_recent".into(), json!(on));
            if !on {
                self.settings.remove("recent");
            }
        }
        if msg.get("clearRecent").and_then(Value::as_bool).unwrap_or(false) {
            self.settings.remove("recent");
        }
        self.store_settings();
        let (color, dark) = self.background();
        let mut out = Vec::new();
        for win in self.windows() {
            out.push(Action::Background { win, color: color.clone(), dark });
            let st = self.state(win);
            out.push(send(win, "state", st));
        }
        out
    }

    // ------------------------------------------------------------ answers from the platform

    pub fn result(&mut self, req: u32, value: Value) -> Vec<Action> {
        let Some((win, wait)) = self.waits.remove(&req) else { return vec![] };
        if !self.wins.contains_key(&win) {
            return vec![];
        }
        let handle = || value.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        match wait {
            Wait::Open => {
                let h = value.as_array().and_then(|a| a.first()).and_then(Value::as_str).map(str::to_string).or_else(handle);
                match h {
                    Some(h) => self.open_in(win, Pending::Path(h)),
                    None => vec![],
                }
            }
            Wait::Pick { what, at } => {
                let Some(h) = value.as_array().and_then(|a| a.first()).and_then(Value::as_str).map(str::to_string).or_else(handle) else {
                    return vec![];
                };
                let data = match self.p.read(&h, MAX_FILE) {
                    Ok(d) => d,
                    Err(e) => return vec![error(win, format!("Couldn't read that file: {}", file_error(&e)))],
                };
                if what == "pdf" && formats::detect(&data) != Some("pdf") {
                    return vec![error(win, "That file isn't a PDF.")];
                }
                if what == "image" {
                    if formats::sniff_image(&data).is_none() {
                        return vec![error(win, "Leaflet can insert PNG, JPEG, GIF and BMP pictures.")];
                    }
                    if data.len() > formats::MAX_IMAGE {
                        return vec![error(win, "That picture is too large (over 32 MB).")];
                    }
                }
                let name = self.p.name(&h);
                let token = self.wins.get_mut(&win).unwrap().put(data, "application/octet-stream");
                vec![send(win, "picked", json!({"token": token, "kind": what, "name": name, "at": at}))]
            }
            Wait::SaveWhere { fmt, then } => {
                let Some(h) = handle() else { return vec![] };
                if self.p.name(&h).to_lowercase().ends_with(&format!(".{fmt}")) {
                    return self.run_then(win, &h, then);
                }
                // The name chosen had no extension: the dialog asked about
                // replacing that name, not this one, so ask here.
                match self.p.with_extension(&h, &fmt) {
                    None => self.run_then(win, &h, then),
                    Some(h2) if self.p.exists(&h2) => {
                        let name = self.p.name(&h2);
                        let folder = self.p.folder(&h2);
                        let req = self.req(win, Wait::Replace { handle: h2, then });
                        vec![Action::Ask {
                            win,
                            req,
                            message: format!("{name} already exists. Replace it?"),
                            detail: format!("It's in {folder}. Replacing it overwrites what's there."),
                            buttons: vec!["Cancel".into(), "Replace".into()],
                            cancel: 0,
                            default: 0,
                        }]
                    }
                    Some(h2) => self.run_then(win, &h2, then),
                }
            }
            Wait::Replace { handle, then } => {
                if value.as_u64() == Some(1) {
                    self.run_then(win, &handle, then)
                } else {
                    vec![]
                }
            }
            Wait::Export { .. } => vec![], // answered through export_done
            Wait::Link { url } => {
                if value.as_u64() == Some(1) {
                    vec![Action::OpenUrl { url }]
                } else {
                    vec![]
                }
            }
            Wait::Unsaved { then } => match value.as_u64() {
                Some(1) => self.after(win, then),
                Some(2) => {
                    self.wins.get_mut(&win).unwrap().after_save = Some(then);
                    vec![send(win, "save-request", json!({}))]
                }
                _ => vec![],
            },
        }
    }

    /// The page printed to a PDF (ExportPdf). The page then strips its
    /// metadata, and the ticket, not the page, says where it's written.
    pub fn export_done(&mut self, req: u32, pdf: Result<Vec<u8>, String>) -> Vec<Action> {
        let Some((win, Wait::Export { target })) = self.waits.remove(&req) else { return vec![] };
        let Some(w) = self.wins.get_mut(&win) else { return vec![] };
        match pdf {
            Err(e) => vec![error(win, format!("Couldn't export: {e}"))],
            Ok(data) if data.len() as u64 > MAX_FILE || formats::detect(&data) != Some("pdf") => {
                vec![error(win, "Couldn't export: the printed file isn't a PDF.")]
            }
            Ok(data) => {
                let ticket = token();
                w.exports.insert(ticket.clone(), target);
                let t = w.put(data, "application/pdf");
                vec![send(win, "exported", json!({"token": t, "ticket": ticket}))]
            }
        }
    }

    // ------------------------------------------------------------ windows

    /// Files dropped on the window, straight from the system: the page isn't asked.
    pub fn dropped(&mut self, win: u32, handles: Vec<String>) -> Vec<Action> {
        let mut out = Vec::new();
        for h in handles.into_iter().take(8) {
            if self.wins.contains_key(&win) {
                out.extend(self.open_in(win, Pending::Path(h)));
            }
        }
        out
    }

    /// The page tried to go somewhere: never allowed; a link the user
    /// clicked may go to the browser once they say so.
    pub fn navigation(&mut self, win: u32, url: &str, user_gesture: bool) -> Vec<Action> {
        if user_gesture && crate::is_openable_link(url) && self.wins.contains_key(&win) {
            let req = self.req(win, Wait::Link { url: url.to_string() });
            return vec![ask_link(win, req, url)];
        }
        vec![]
    }

    pub fn close_requested(&mut self, win: u32) -> Vec<Action> {
        let Some(w) = self.wins.get(&win) else { return vec![Action::Close { win }] };
        if w.dirty && !w.closing {
            return self.ask_unsaved(win, After::Close);
        }
        vec![Action::Close { win }]
    }

    fn ask_unsaved(&mut self, win: u32, then: After) -> Vec<Action> {
        let name = self.wins[&win].name.clone();
        let req = self.req(win, Wait::Unsaved { then });
        vec![
            Action::Present { win },
            Action::Ask {
                win,
                req,
                message: format!("Save changes to {}?", if name.is_empty() { "Untitled" } else { &name }),
                detail: "Your changes will be lost if you don't save them.".into(),
                buttons: vec!["Cancel".into(), "Don't save".into(), "Save".into()],
                cancel: 0,
                default: 2,
            },
        ]
    }

    pub fn ask_quit(&mut self) -> Vec<Action> {
        let dirty = self.wins.iter().find(|(_, w)| w.dirty && !w.closing).map(|(id, _)| *id);
        if let Some(win) = dirty {
            return self.ask_unsaved(win, After::CloseAndQuit);
        }
        let mut out = Vec::new();
        for (id, w) in self.wins.iter_mut() {
            if !w.closing {
                w.closing = true;
                out.push(Action::Close { win: *id });
            }
        }
        out
    }

    /// The platform closed the window; `size` is remembered for next time.
    /// Returns true when it was the last one.
    pub fn closed(&mut self, win: u32, size: Option<(u32, u32, bool)>) -> bool {
        self.wins.remove(&win);
        self.waits.retain(|_, (w, _)| *w != win);
        if let Some((w, h, max)) = size {
            self.settings.insert("_window".into(), json!({"w": w, "h": h, "max": max}));
            self.store_settings();
        }
        self.wins.is_empty()
    }

    /// The page's renderer stopped. Load it again and reopen the file from disk.
    pub fn crashed(&mut self, win: u32) -> Vec<Action> {
        let Some(w) = self.wins.get_mut(&win) else { return vec![] };
        w.exports.clear();
        w.store.clear();
        w.ready = false;
        w.dirty = false;
        let path = w.path.take();
        w.kind = None;
        w.pending = path.map(Pending::Path);
        w.queued_error = Some("The document view stopped and was restarted. Unsaved changes were lost.".into());
        vec![Action::Reload { win }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_name("../../etc/passwd", "x"), "passwd");
        assert_eq!(clean_name("C:\\a\\b.pdf", "x"), "b.pdf");
        assert_eq!(clean_name("...", "x"), "x");
        assert_eq!(clean_name("a\u{202e}fdp.exe", "x"), "afdp.exe");
        assert_eq!(stem("a.tar.gz"), "a.tar");
        assert_eq!(stem(".bashrc"), ".bashrc");
    }
}
