//! Folio's window: folio.py's bridge, without GTK.
//!
//! The page calls `webkit.messageHandlers.folio.postMessage(JSON {id, cmd, args})`
//! and is answered with `folio.resolve(id, data)`. Documents are known to the
//! page by tokens; settings are the only thing written on Folio's own.

use crate::{js_json, token, Action, FileError, Filter, Platform};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
pub const WIN: u32 = 0;

pub fn theme_bg(theme: &str) -> &'static str {
    match theme {
        "ink" => "#0f1218",
        "paper" => "#fbfaf7",
        "sepia" => "#f5edda",
        _ => "#1d1e22",
    }
}

enum Wait {
    Open { cid: i64 },
    SaveAs { cid: i64, content: String, eol: String, encoding: String },
    Link { url: String },
}

pub struct Folio {
    p: Box<dyn Platform>,
    ready: bool,
    allow_close: bool,
    pending_files: Vec<String>,
    docs: HashMap<String, String>,
    settings: Map<String, Value>,
    waits: HashMap<u32, Wait>,
    next_req: u32,
}

/// What folio.py's read_file gives the page.
pub fn read_file(p: &dyn Platform, handle: &str) -> Value {
    let name = p.name(handle);
    let path = show_path(p, handle);
    let data = match p.read(handle, MAX_BYTES) {
        Ok(d) => d,
        Err(FileError::TooLarge) => return json!({"path": path, "error": format!("{name} is too large to open (over 50 MB)")}),
        Err(FileError::NotFound) => return json!({"path": path, "error": format!("{name} not found")}),
        Err(FileError::IsDir) => return json!({"path": path, "error": format!("{name} is a folder")}),
        Err(FileError::Other(e)) => return json!({"path": path, "error": format!("Could not open {name}: {e}")}),
    };
    if data[..data.len().min(8192)].contains(&0) {
        return json!({"path": path, "error": format!("{name} looks like a binary file")});
    }
    let (mut encoding, body) = match data.strip_prefix(b"\xef\xbb\xbf") {
        Some(rest) => ("utf-8-sig", rest),
        None => ("utf-8", &data[..]),
    };
    let text = match std::str::from_utf8(body) {
        Ok(t) => t.to_string(),
        Err(_) => {
            encoding = "latin-1";
            body.iter().map(|&b| b as char).collect()
        }
    };
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let text = text.replace("\r\n", "\n");
    json!({"path": path, "content": text, "eol": eol, "encoding": encoding, "mtime": p.mtime(handle)})
}

/// What the page shows as the file's path: "/"-separated.
fn show_path(p: &dyn Platform, handle: &str) -> String {
    let folder = p.folder(handle).replace('\\', "/");
    let name = p.name(handle);
    if folder.is_empty() {
        format!("/{name}")
    } else if folder.ends_with('/') {
        format!("{folder}{name}")
    } else {
        format!("{folder}/{name}")
    }
}

/// folio.py's write_file: the text in the file's own line endings and encoding.
pub fn encode_text(content: &str, eol: &str, encoding: &str) -> Vec<u8> {
    let content = if eol == "\r\n" { content.replace('\n', "\r\n") } else { content.to_string() };
    match encoding.to_ascii_lowercase().replace('_', "-").as_str() {
        "utf-8-sig" | "utf8-sig" => {
            let mut v = b"\xef\xbb\xbf".to_vec();
            v.extend_from_slice(content.as_bytes());
            v
        }
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" | "l1" => {
            if content.chars().all(|c| (c as u32) < 256) {
                content.chars().map(|c| c as u8).collect()
            } else {
                content.into_bytes() // can't be written in latin-1: UTF-8, as folio.py does
            }
        }
        _ => content.into_bytes(),
    }
}

/// What the page may store: plain named values, nothing nested or huge.
pub fn clean_settings(settings: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(obj) = settings.as_object() else { return out };
    for (k, v) in obj.iter().take(64) {
        if k.is_empty() || k.starts_with('_') || k.chars().count() > 40 {
            continue;
        }
        let ok = match v {
            Value::Bool(_) | Value::Number(_) => true,
            Value::String(s) => s.chars().count() <= 200,
            _ => false,
        };
        if ok {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn text_args(args: &Map<String, Value>) -> (String, String, String) {
    let content = args.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let eol = if args.get("eol").and_then(Value::as_str) == Some("\r\n") { "\r\n" } else { "\n" };
    let encoding = args.get("encoding").and_then(Value::as_str).unwrap_or("utf-8").to_string();
    (content, eol.to_string(), encoding)
}

impl Folio {
    /// `files`: what Folio was opened with (command line, "Open with").
    pub fn new(p: Box<dyn Platform>, files: Vec<String>) -> Folio {
        let settings = p
            .load_settings()
            .and_then(|d| serde_json::from_slice::<Value>(&d).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Folio { p, ready: false, allow_close: false, pending_files: files, docs: HashMap::new(), settings, waits: HashMap::new(), next_req: 1 }
    }

    pub fn platform(&self) -> &dyn Platform {
        &*self.p
    }

    /// The window size remembered from last time: (width, height, maximized).
    pub fn window_size(&self) -> (u32, u32, bool) {
        let w = self.settings.get("_window");
        let n = |k: &str, d: u64| w.and_then(|w| w.get(k)).and_then(Value::as_u64).unwrap_or(d).clamp(200, 10000) as u32;
        (n("w", 1000), n("h", 760), w.and_then(|w| w.get("max")).and_then(Value::as_bool).unwrap_or(false))
    }

    /// The theme's background, before the page has said anything.
    pub fn background(&self) -> (String, bool) {
        let theme = self.settings.get("theme").and_then(Value::as_str).unwrap_or("night");
        (theme_bg(theme).to_string(), matches!(theme, "night" | "ink"))
    }

    fn add_doc(&mut self, handle: &str) -> String {
        let t = token();
        self.docs.insert(t.clone(), handle.to_string());
        t
    }

    fn doc_path(&self, args: &Map<String, Value>) -> Option<String> {
        args.get("doc").and_then(Value::as_str).and_then(|t| self.docs.get(t)).cloned()
    }

    fn open_doc(&mut self, handle: &str) -> Value {
        let mut f = read_file(&*self.p, handle);
        if f.get("error").is_none() {
            let t = self.add_doc(handle);
            f["doc"] = json!(t);
        }
        f
    }

    fn reply(cid: i64, data: Value) -> Action {
        Action::Eval { win: WIN, js: format!("folio.resolve({cid}, {})", js_json(&data)) }
    }

    fn req(&mut self, w: Wait) -> u32 {
        let r = self.next_req;
        self.next_req += 1;
        self.waits.insert(r, w);
        r
    }

    /// A message the page posted.
    pub fn message(&mut self, raw: &str) -> Vec<Action> {
        let Ok(msg) = serde_json::from_str::<Value>(raw) else { return vec![] };
        let Some(cid) = msg.get("id").and_then(Value::as_i64).filter(|_| msg["id"].is_i64() || msg["id"].is_u64()) else { return vec![] };
        let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
        let args = msg.get("args").and_then(Value::as_object).cloned().unwrap_or_default();
        match self.command(&cmd, &args, cid) {
            Some(Ok(v)) => vec![Self::reply(cid, v)],
            Some(Err(actions)) => actions,
            None => vec![Self::reply(cid, Value::Null)],
        }
    }

    /// Some(Ok(reply)), Some(Err(actions)) for an answer that comes later, None for no such command.
    fn command(&mut self, cmd: &str, args: &Map<String, Value>, cid: i64) -> Option<Result<Value, Vec<Action>>> {
        Some(match cmd {
            "init" => {
                self.ready = true;
                self.docs.clear(); // a page that starts over holds no tokens
                let pending = std::mem::take(&mut self.pending_files);
                let files: Vec<Value> = pending.iter().map(|h| self.open_doc(h)).collect();
                let settings: Map<String, Value> = self.settings.iter().filter(|(k, _)| !k.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect();
                Ok(json!({"settings": settings, "files": files, "home": self.p.home()}))
            }
            "reload" => match self.doc_path(args) {
                None => Ok(json!({"error": "That document isn't open"})),
                Some(h) => Ok(read_file(&*self.p, &h)),
            },
            "stat" => {
                let mut out = Map::new();
                for d in args.get("docs").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(h) = d.as_str().and_then(|t| self.docs.get(t)) {
                        out.insert(d.as_str().unwrap().to_string(), json!(self.p.mtime(h)));
                    }
                }
                Ok(Value::Object(out))
            }
            "save" => match self.doc_path(args) {
                None => Ok(json!({"error": "Could not save: that document isn't open"})),
                Some(h) => {
                    let (c, e, enc) = text_args(args);
                    Ok(self.write_file(&h, &c, &e, &enc))
                }
            },
            "saveAs" => {
                // Where it goes is the user's answer to this dialog and nothing else;
                // the page only suggests a file name.
                let (content, eol, encoding) = text_args(args);
                let name = args.get("name").and_then(Value::as_str).map(basename).unwrap_or_default();
                let near = self.doc_path(args);
                let req = self.req(Wait::SaveAs { cid, content, eol, encoding });
                Err(vec![Action::SaveDialog {
                    win: WIN,
                    req,
                    title: "Save As".into(),
                    name: if name.is_empty() { "Untitled.txt".into() } else { name },
                    filters: vec![],
                    near,
                    mime: "text/plain".into(),
                }])
            }
            "open" => {
                let near = self.doc_path(args);
                let req = self.req(Wait::Open { cid });
                Err(vec![Action::OpenDialog {
                    win: WIN,
                    req,
                    title: "Open".into(),
                    multiple: true,
                    filters: vec![Filter::new("All files", &["*"], &["*/*"])],
                    near,
                }])
            }
            "close" => {
                // The tab is gone, and with it the page's way to reach the file.
                if let Some(t) = args.get("doc").and_then(Value::as_str) {
                    self.docs.remove(t);
                }
                Ok(json!(true))
            }
            "setTitle" => {
                let title: String = args.get("title").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("Folio").chars().take(200).collect();
                let dirty = if args.get("dirty").and_then(Value::as_bool).unwrap_or(false) { "• " } else { "" };
                let sub: String = args.get("subtitle").and_then(Value::as_str).unwrap_or("").chars().take(400).collect();
                let sub = if sub.is_empty() { String::new() } else { format!(" ({sub})") };
                Err(vec![Action::Title { win: WIN, title: format!("{dirty}{title}{sub} — Folio") }, Self::reply(cid, json!(true))])
            }
            "setTheme" => {
                let bg = args.get("bg").and_then(Value::as_str).and_then(crate::web::colour).unwrap_or_else(|| theme_bg("night").into());
                let dark = args.get("dark").and_then(Value::as_bool).unwrap_or(false);
                Err(vec![Action::Background { win: WIN, color: bg, dark }, Self::reply(cid, json!(true))])
            }
            "saveSettings" => {
                let window = self.settings.get("_window").cloned();
                self.settings = clean_settings(args.get("settings").unwrap_or(&Value::Null));
                if let Some(w) = window {
                    self.settings.insert("_window".into(), w);
                }
                self.store_settings();
                Ok(json!(true))
            }
            "fullscreen" => Err(vec![Action::Fullscreen { win: WIN }, Self::reply(cid, json!(true))]),
            "quit" => {
                self.allow_close = true;
                Err(vec![Self::reply(cid, json!(true)), Action::Close { win: WIN }])
            }
            _ => return None,
        })
    }

    fn store_settings(&self) {
        if let Ok(data) = serde_json::to_vec_pretty(&Value::Object(self.settings.clone())) {
            self.p.store_settings(&data);
        }
    }

    fn write_file(&self, handle: &str, content: &str, eol: &str, encoding: &str) -> Value {
        let data = encode_text(content, eol, encoding);
        match self.p.write(handle, &data) {
            Ok(()) => json!({"path": show_path(&*self.p, handle), "name": self.p.name(handle), "mtime": self.p.mtime(handle)}),
            Err(e) => json!({"error": format!("Could not save {}: {e}", self.p.name(handle))}),
        }
    }

    /// The platform's answer to a dialog or question.
    pub fn result(&mut self, req: u32, value: Value) -> Vec<Action> {
        let Some(wait) = self.waits.remove(&req) else { return vec![] };
        match wait {
            Wait::Open { cid } => {
                let Some(list) = value.as_array() else { return vec![Self::reply(cid, Value::Null)] };
                let handles: Vec<String> = list.iter().filter_map(|v| v.as_str().map(str::to_string)).filter(|s| !s.is_empty()).collect();
                let files: Vec<Value> = handles.iter().map(|h| self.open_doc(h)).collect();
                vec![Self::reply(cid, Value::Array(files))]
            }
            Wait::SaveAs { cid, content, eol, encoding } => {
                let Some(h) = value.as_str().filter(|s| !s.is_empty()).map(str::to_string) else {
                    return vec![Self::reply(cid, Value::Null)];
                };
                let mut r = self.write_file(&h, &content, &eol, &encoding);
                if r.get("error").is_none() {
                    let t = self.add_doc(&h);
                    r["doc"] = json!(t);
                }
                vec![Self::reply(cid, r)]
            }
            Wait::Link { url } => {
                if value.as_u64() == Some(1) {
                    vec![Action::OpenUrl { url }]
                } else {
                    vec![]
                }
            }
        }
    }

    /// Files from outside the page: a drop from the file manager, a second
    /// "Open with", a share.
    pub fn open_files(&mut self, handles: Vec<String>) -> Vec<Action> {
        if !self.ready {
            self.pending_files.extend(handles);
            return vec![Action::Present { win: WIN }];
        }
        let files: Vec<Value> = handles.iter().map(|h| self.open_doc(h)).collect();
        vec![Action::Eval { win: WIN, js: format!("folio.openFiles({})", js_json(&Value::Array(files))) }, Action::Present { win: WIN }]
    }

    pub fn drop_hint(&self, show: bool) -> Vec<Action> {
        if !self.ready {
            return vec![];
        }
        vec![Action::Eval { win: WIN, js: format!("folio.dropHint({show})") }]
    }

    /// The page tried to go somewhere else. Only a link the user clicked goes
    /// out, and only once they say so: a note can't send the browser anywhere
    /// by itself. The navigation itself is always refused.
    pub fn navigation(&mut self, url: &str, user_clicked: bool) -> Vec<Action> {
        if user_clicked && crate::is_openable_link(url) {
            let req = self.req(Wait::Link { url: url.to_string() });
            return vec![crate::ask_link(WIN, req, url)];
        }
        vec![]
    }

    /// The window's close button. Folio asks the page, which saves or asks
    /// and then calls quit. If running the script fails, the platform closes
    /// anyway: a broken page mustn't trap the user.
    pub fn close_requested(&mut self) -> Vec<Action> {
        if self.allow_close || !self.ready {
            return vec![Action::Close { win: WIN }];
        }
        vec![Action::Eval { win: WIN, js: "folio.requestClose()".into() }]
    }

    pub fn force_close(&mut self) {
        self.allow_close = true;
    }

    /// The window is going; remember its size.
    pub fn closed(&mut self, size: Option<(u32, u32, bool)>) {
        if let Some((w, h, max)) = size {
            self.settings.insert("_window".into(), json!({"w": w, "h": h, "max": max}));
            self.store_settings();
        }
        self.docs.clear();
    }

    /// The page's renderer stopped: load it again; it calls init and starts over.
    pub fn crashed(&mut self) -> Vec<Action> {
        self.ready = false;
        self.docs.clear();
        vec![Action::Reload { win: WIN }]
    }
}

fn basename(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or("").to_string()
}
