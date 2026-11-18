//! What the page may and may not do, as folio.py's and leaflet.py's tests check it.

use appshell_core::folio::{clean_settings, Folio};
use appshell_core::leaflet::Leaflet;
use appshell_core::{Action, FileError, Platform};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Default)]
struct Disk {
    files: HashMap<String, Vec<u8>>,
    settings: Option<Vec<u8>>,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<Disk>>);

impl Fake {
    fn put(&self, h: &str, data: &[u8]) {
        self.0.borrow_mut().files.insert(h.into(), data.to_vec());
    }
    fn get(&self, h: &str) -> Option<Vec<u8>> {
        self.0.borrow().files.get(h).cloned()
    }
}

impl Platform for Fake {
    fn read(&self, h: &str, limit: u64) -> Result<Vec<u8>, FileError> {
        let d = self.0.borrow().files.get(h).cloned().ok_or(FileError::NotFound)?;
        if d.len() as u64 > limit {
            return Err(FileError::TooLarge);
        }
        Ok(d)
    }
    fn write(&self, h: &str, data: &[u8]) -> Result<(), String> {
        if h.starts_with("/readonly/") {
            return Err("Permission denied".into());
        }
        self.put(h, data);
        Ok(())
    }
    fn mtime(&self, h: &str) -> Option<i64> {
        self.0.borrow().files.get(h).map(|d| d.len() as i64)
    }
    fn exists(&self, h: &str) -> bool {
        self.0.borrow().files.contains_key(h)
    }
    fn name(&self, h: &str) -> String {
        h.rsplit('/').next().unwrap().into()
    }
    fn folder(&self, h: &str) -> String {
        h.rsplit_once('/').map(|x| x.0).unwrap_or("").into()
    }
    fn with_extension(&self, h: &str, ext: &str) -> Option<String> {
        Some(format!("{h}.{ext}"))
    }
    fn load_settings(&self) -> Option<Vec<u8>> {
        self.0.borrow().settings.clone()
    }
    fn store_settings(&self, data: &[u8]) {
        self.0.borrow_mut().settings = Some(data.to_vec());
    }
    fn home(&self) -> String {
        "/home/you".into()
    }
}

/// The value a Folio command resolved with.
fn resolved(actions: &[Action]) -> Value {
    for a in actions {
        if let Action::Eval { js, .. } = a {
            if let Some(rest) = js.strip_prefix("folio.resolve(") {
                let (_, json) = rest.split_once(", ").unwrap();
                return serde_json::from_str(json.strip_suffix(')').unwrap()).unwrap();
            }
        }
    }
    panic!("no reply in {actions:?}")
}

fn folio_with(files: &[&str]) -> (Folio, Fake) {
    let fake = Fake::default();
    fake.put("/docs/opened.txt", b"opened\n");
    fake.put("/docs/other.txt", b"private\n");
    let f = Folio::new(Box::new(fake.clone()), files.iter().map(|s| s.to_string()).collect());
    (f, fake)
}

fn cmd(f: &mut Folio, cmd: &str, args: Value) -> Vec<Action> {
    f.message(&json!({"id": 1, "cmd": cmd, "args": args}).to_string())
}

fn opened(f: &mut Folio) -> Value {
    let init = resolved(&cmd(f, "init", json!({})));
    init["files"][0].clone()
}

#[test]
fn folio_command_line_files_get_tokens() {
    let (mut f, _) = folio_with(&["/docs/opened.txt"]);
    let doc = opened(&mut f);
    assert_eq!(doc["content"], "opened\n");
    assert_eq!(doc["path"], "/docs/opened.txt");
    assert!(doc["doc"].as_str().unwrap().len() >= 32);
}

#[test]
fn folio_a_path_is_not_a_token() {
    let (mut f, fake) = folio_with(&["/docs/opened.txt"]);
    opened(&mut f);
    for guess in ["/docs/other.txt", "other.txt", "", "../other.txt"] {
        let r = resolved(&cmd(&mut f, "save", json!({"doc": guess, "content": "pwned"})));
        assert!(r.get("error").is_some(), "{guess}");
        let r = resolved(&cmd(&mut f, "reload", json!({"doc": guess})));
        assert!(r.get("error").is_some() && r.get("content").is_none());
    }
    assert_eq!(fake.get("/docs/other.txt").unwrap(), b"private\n");
}

#[test]
fn folio_save_writes_the_opened_document_only() {
    let (mut f, fake) = folio_with(&["/docs/opened.txt"]);
    let doc = opened(&mut f);
    let r = resolved(&cmd(&mut f, "save", json!({"doc": doc["doc"], "path": "/docs/other.txt", "content": "changed\n"})));
    assert!(r.get("error").is_none());
    assert_eq!(fake.get("/docs/opened.txt").unwrap(), b"changed\n");
    assert_eq!(fake.get("/docs/other.txt").unwrap(), b"private\n");
}

#[test]
fn folio_keeps_line_endings_and_encoding() {
    let (mut f, fake) = folio_with(&["/docs/crlf.txt"]);
    fake.put("/docs/crlf.txt", b"\xef\xbb\xbfa\r\nb\r\n");
    let doc = opened(&mut f);
    assert_eq!(doc["eol"], "\r\n");
    assert_eq!(doc["encoding"], "utf-8-sig");
    assert_eq!(doc["content"], "a\nb\n");
    cmd(&mut f, "save", json!({"doc": doc["doc"], "content": "a\nb\nc\n", "eol": doc["eol"], "encoding": doc["encoding"]}));
    assert_eq!(fake.get("/docs/crlf.txt").unwrap(), b"\xef\xbb\xbfa\r\nb\r\nc\r\n");
    fake.put("/docs/latin.txt", b"caf\xe9\n");
    let r = f.open_files(vec!["/docs/latin.txt".into()]);
    let Action::Eval { js, .. } = &r[0] else { panic!() };
    assert!(js.contains("\"encoding\":\"latin-1\"") && js.contains("café"));
}

#[test]
fn folio_closed_document_can_no_longer_be_written() {
    let (mut f, fake) = folio_with(&["/docs/opened.txt"]);
    let doc = opened(&mut f);
    cmd(&mut f, "close", json!({"doc": doc["doc"]}));
    let r = resolved(&cmd(&mut f, "save", json!({"doc": doc["doc"], "content": "late"})));
    assert!(r.get("error").is_some());
    assert_eq!(fake.get("/docs/opened.txt").unwrap(), b"opened\n");
}

#[test]
fn folio_save_as_goes_where_the_dialog_says() {
    let (mut f, fake) = folio_with(&[]);
    cmd(&mut f, "init", json!({}));
    let a = cmd(&mut f, "saveAs", json!({"content": "one\n", "name": "../../evil/x.txt"}));
    let Action::SaveDialog { req, name, .. } = &a[0] else { panic!("{a:?}") };
    assert_eq!(name, "x.txt");
    let r = resolved(&f.result(*req, json!("/docs/new.txt")));
    assert_eq!(fake.get("/docs/new.txt").unwrap(), b"one\n");
    let again = resolved(&cmd(&mut f, "save", json!({"doc": r["doc"], "content": "two\n"})));
    assert!(again.get("error").is_none());
    assert_eq!(fake.get("/docs/new.txt").unwrap(), b"two\n");
    // cancelled
    let a = cmd(&mut f, "saveAs", json!({"content": "x"}));
    let Action::SaveDialog { req, .. } = &a[0] else { panic!() };
    assert_eq!(resolved(&f.result(*req, Value::Null)), Value::Null);
}

#[test]
fn folio_stat_tells_nothing_about_other_files() {
    let (mut f, _) = folio_with(&["/docs/opened.txt"]);
    let doc = opened(&mut f);
    let r = resolved(&cmd(&mut f, "stat", json!({"docs": [doc["doc"], "/docs/other.txt", "nope"]})));
    assert_eq!(r.as_object().unwrap().len(), 1);
    assert!(r[doc["doc"].as_str().unwrap()].is_i64());
}

#[test]
fn folio_unreadable_files_get_no_token() {
    let (mut f, fake) = folio_with(&["/docs/missing.txt", "/docs/bin"]);
    fake.put("/docs/bin", b"\x00\x01\x02");
    let init = resolved(&cmd(&mut f, "init", json!({})));
    assert_eq!(init["files"][0]["error"], "missing.txt not found");
    assert!(init["files"][1]["error"].as_str().unwrap().contains("binary"));
    assert!(init["files"][0].get("doc").is_none());
}

#[test]
fn folio_settings_are_plain_values() {
    let got = clean_settings(&json!({"theme": "ink", "fontSize": 19, "fade": true, "lineHeight": 1.7, "_window": {"w": 1}, "nested": {"a": 1}, "list": [1], "long": "x".repeat(201)}));
    assert_eq!(Value::Object(got), json!({"theme": "ink", "fontSize": 19, "fade": true, "lineHeight": 1.7}));
    let (mut f, fake) = folio_with(&[]);
    cmd(&mut f, "saveSettings", json!({"settings": {"theme": "paper", "_window": {"w": 5}}}));
    f.closed(Some((800, 600, false)));
    let s: Value = serde_json::from_slice(&fake.0.borrow().settings.clone().unwrap()).unwrap();
    assert_eq!(s, json!({"theme": "paper", "_window": {"w": 800, "h": 600, "max": false}}));
}

#[test]
fn folio_theme_colours_cannot_carry_css() {
    let (mut f, _) = folio_with(&[]);
    let a = cmd(&mut f, "setTheme", json!({"bg": "red; } * { background: url(http://x)", "dark": true}));
    let Action::Background { color, .. } = &a[0] else { panic!() };
    assert_eq!(color, "#1d1e22");
}

#[test]
fn folio_links_only_after_a_click_and_a_yes() {
    let (mut f, _) = folio_with(&[]);
    assert!(f.navigation("https://example.test/", false).is_empty());
    assert!(f.navigation("file:///etc/passwd", true).is_empty());
    assert!(f.navigation("javascript:alert(1)", true).is_empty());
    let a = f.navigation("https://example.test/", true);
    let Action::Ask { req, .. } = &a[0] else { panic!() };
    assert!(f.result(*req, json!(0)).is_empty());
    let a = f.navigation("https://example.test/", true);
    let Action::Ask { req, .. } = &a[0] else { panic!() };
    assert_eq!(f.result(*req, json!(1)), vec![Action::OpenUrl { url: "https://example.test/".into() }]);
}

#[test]
fn folio_unknown_commands_resolve_to_null() {
    let (mut f, _) = folio_with(&[]);
    assert_eq!(resolved(&cmd(&mut f, "read", json!({"path": "/docs/other.txt"}))), Value::Null);
    assert!(f.message("not json").is_empty());
    assert!(f.message("{\"cmd\":\"init\"}").is_empty());
}

// ------------------------------------------------------------ Leaflet

/// The messages Leaflet sent the page: (what, payload).
fn sent(actions: &[Action]) -> Vec<(String, Value)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Eval { js, .. } => {
                let j = js.strip_prefix("window.Leaflet && window.Leaflet.handle(")?.strip_suffix(')')?;
                let v: Value = serde_json::from_str(j).unwrap();
                Some((v["what"].as_str().unwrap().to_string(), v))
            }
            _ => None,
        })
        .collect()
}

fn leaflet() -> (Leaflet, Fake, u32) {
    let fake = Fake::default();
    fake.put("/docs/report.docx", &leaflet_formats::save(&leaflet_formats::from_html("<p>Report</p>"), "docx").unwrap());
    fake.put("/docs/pages.pdf", b"%PDF-1.4\n%%EOF\n");
    fake.put("/docs/secret.txt", b"secret");
    let mut l = Leaflet::new(Box::new(fake.clone()));
    let a = l.start(vec!["/docs/report.docx".into()]);
    let Action::NewWindow { win } = a[0] else { panic!() };
    (l, fake, win)
}

fn msg(l: &mut Leaflet, win: u32, v: Value) -> Vec<Action> {
    l.message(win, &v.to_string())
}

#[test]
fn leaflet_opens_what_it_was_started_with() {
    let (mut l, _, win) = leaflet();
    let a = msg(&mut l, win, json!({"cmd": "ready"}));
    let s = sent(&a);
    assert_eq!(s[0].0, "state");
    assert_eq!(s[1].0, "load");
    assert_eq!(s[1].1["kind"], "doc");
    assert_eq!(s[1].1["html"], "<p>Report</p>");
    assert!(a.iter().any(|x| matches!(x, Action::Title { title, .. } if title == "report.docx — Leaflet")));
}

#[test]
fn leaflet_saves_only_to_the_open_file_or_a_dialog() {
    let (mut l, fake, win) = leaflet();
    msg(&mut l, win, json!({"cmd": "ready"}));
    let t = l.blob_put(win, b"<p>Changed</p>".to_vec(), "text/html").unwrap();
    // a path sent along is ignored: the file is the open one
    let a = msg(&mut l, win, json!({"cmd": "save", "token": t, "format": "docx", "path": "/docs/secret.txt"}));
    assert_eq!(sent(&a)[0].0, "saved");
    assert_eq!(fake.get("/docs/secret.txt").unwrap(), b"secret");
    let doc = leaflet_formats::load(&fake.get("/docs/report.docx").unwrap(), "docx").unwrap();
    assert_eq!(leaflet_formats::to_html(&doc), "<p>Changed</p>");
    // another format needs the dialog
    let t = l.blob_put(win, b"<p>x</p>".to_vec(), "text/html").unwrap();
    let a = msg(&mut l, win, json!({"cmd": "save", "token": t, "format": "odt", "suggest": "../../x/new.docx"}));
    let Action::SaveDialog { req, name, .. } = &a[0] else { panic!("{a:?}") };
    assert_eq!(name, "new.odt");
    let a = l.result(*req, json!("/docs/out.odt"));
    assert_eq!(sent(&a)[0].0, "saved");
    assert_eq!(leaflet_formats::detect(&fake.get("/docs/out.odt").unwrap()), Some("odt"));
}

#[test]
fn leaflet_a_token_is_good_once() {
    let (mut l, _, win) = leaflet();
    let t = l.blob_put(win, b"%PDF-1.4".to_vec(), "application/pdf").unwrap();
    assert!(l.blob_take(win, &t).is_some());
    assert!(l.blob_take(win, &t).is_none());
    assert!(l.blob_take(win, "/docs/secret.txt").is_none());
    let a = msg(&mut l, win, json!({"cmd": "save", "token": t, "format": "pdf"}));
    assert_eq!(sent(&a)[0].1["text"], "Nothing to save.");
}

#[test]
fn leaflet_only_a_pdf_is_saved_as_a_pdf() {
    let (mut l, _, win) = leaflet();
    msg(&mut l, win, json!({"cmd": "ready"}));
    let t = l.blob_put(win, b"<p>x</p>".to_vec(), "text/html").unwrap();
    let a = msg(&mut l, win, json!({"cmd": "save", "token": t, "format": "pdf"}));
    assert_eq!(sent(&a)[0].1["text"], "Leaflet can't save that.");
    let t = l.blob_put(win, b"not a pdf".to_vec(), "application/pdf").unwrap();
    let a = msg(&mut l, win, json!({"cmd": "save", "token": t, "format": "pdf"}));
    assert_eq!(sent(&a)[0].1["text"], "Leaflet can't save that.");
    let t = l.blob_put(win, b"x".to_vec(), "text/html").unwrap();
    let a = msg(&mut l, win, json!({"cmd": "save", "token": t, "format": "exe"}));
    assert_eq!(sent(&a)[0].1["text"], "Leaflet can't save that.");
}

#[test]
fn leaflet_export_goes_where_the_ticket_says() {
    let (mut l, fake, win) = leaflet();
    msg(&mut l, win, json!({"cmd": "ready"}));
    let a = msg(&mut l, win, json!({"cmd": "export-pdf"}));
    let Action::SaveDialog { req, title, .. } = &a[0] else { panic!() };
    assert_eq!(title, "Export as PDF");
    let a = l.result(*req, json!("/docs/report.pdf"));
    let Action::ExportPdf { req, .. } = a[0] else { panic!("{a:?}") };
    let a = l.export_done(req, Ok(b"%PDF-1.7 printed".to_vec()));
    let (_, ex) = &sent(&a)[0];
    let printed = l.blob_take(win, ex["token"].as_str().unwrap()).unwrap();
    assert_eq!(printed.0, b"%PDF-1.7 printed");
    let clean = l.blob_put(win, b"%PDF-1.7 clean".to_vec(), "application/pdf").unwrap();
    // a made-up ticket goes nowhere
    let bad = l.blob_put(win, b"%PDF-1.7 x".to_vec(), "application/pdf").unwrap();
    assert!(msg(&mut l, win, json!({"cmd": "write-export", "token": bad, "ticket": "/docs/secret.txt"})).is_empty());
    let a = msg(&mut l, win, json!({"cmd": "write-export", "token": clean, "ticket": ex["ticket"]}));
    assert_eq!(sent(&a)[0].1["export"], true);
    assert_eq!(fake.get("/docs/report.pdf").unwrap(), b"%PDF-1.7 clean");
    assert_eq!(fake.get("/docs/secret.txt").unwrap(), b"secret");
}

#[test]
fn leaflet_recent_files_by_number_only_when_switched_on() {
    let (mut l, _, win) = leaflet();
    let s = sent(&msg(&mut l, win, json!({"cmd": "ready"})));
    assert_eq!(s[0].1["recent"], json!([]));
    msg(&mut l, win, json!({"cmd": "settings", "rememberRecent": true}));
    let a = l.start(vec!["/docs/pages.pdf".into()]);
    let Action::NewWindow { win: w2 } = a[0] else { panic!() };
    let s = sent(&msg(&mut l, w2, json!({"cmd": "ready"})));
    assert_eq!(s[0].1["recent"], json!([]));
    let a = l.activate();
    let Action::NewWindow { win: w3 } = a[0] else { panic!() };
    let s = sent(&msg(&mut l, w3, json!({"cmd": "ready"})));
    assert_eq!(s[0].1["recent"], json!([{"name": "pages.pdf", "folder": "/docs"}]));
    let s = sent(&msg(&mut l, w3, json!({"cmd": "open", "recent": 0})));
    assert_eq!(s[0].1["kind"], "pdf");
    assert!(msg(&mut l, w3, json!({"cmd": "open", "recent": 5})).is_empty());
    assert!(msg(&mut l, w3, json!({"cmd": "open", "recent": "/docs/secret.txt"})).is_empty());
}

#[test]
fn leaflet_asks_before_closing_with_changes() {
    let (mut l, fake, win) = leaflet();
    msg(&mut l, win, json!({"cmd": "ready"}));
    msg(&mut l, win, json!({"cmd": "title", "dirty": true}));
    let a = l.close_requested(win);
    let Some(Action::Ask { req, buttons, .. }) = a.iter().find(|x| matches!(x, Action::Ask { .. })) else { panic!() };
    assert_eq!(buttons.len(), 3);
    assert!(l.result(*req, json!(0)).is_empty()); // cancel
    let a = l.close_requested(win);
    let Some(Action::Ask { req, .. }) = a.iter().find(|x| matches!(x, Action::Ask { .. })) else { panic!() };
    let s = sent(&l.result(*req, json!(2))); // save
    assert_eq!(s[0].0, "save-request");
    let t = l.blob_put(win, b"<p>Saved</p>".to_vec(), "text/html").unwrap();
    let a = msg(&mut l, win, json!({"cmd": "save", "token": t, "format": "docx"}));
    assert!(a.contains(&Action::Close { win }));
    assert!(fake.get("/docs/report.docx").is_some());
    assert!(l.closed(win, Some((900, 700, true))));
}

#[test]
fn leaflet_links_only_after_a_yes() {
    let (mut l, _, win) = leaflet();
    assert!(msg(&mut l, win, json!({"cmd": "link", "url": "file:///etc/passwd"})).is_empty());
    let a = msg(&mut l, win, json!({"cmd": "link", "url": "https://example.test/"}));
    let Action::Ask { req, .. } = &a[0] else { panic!() };
    assert_eq!(l.result(*req, json!(1)), vec![Action::OpenUrl { url: "https://example.test/".into() }]);
}

#[test]
fn leaflet_pick_checks_what_was_picked() {
    let (mut l, _, win) = leaflet();
    msg(&mut l, win, json!({"cmd": "ready"}));
    let a = msg(&mut l, win, json!({"cmd": "pick", "for": "pdf", "at": 2}));
    let Action::OpenDialog { req, .. } = &a[0] else { panic!() };
    let s = sent(&l.result(*req, json!(["/docs/secret.txt"])));
    assert_eq!(s[0].1["text"], "That file isn't a PDF.");
    let a = msg(&mut l, win, json!({"cmd": "pick", "for": "pdf", "at": 2}));
    let Action::OpenDialog { req, .. } = &a[0] else { panic!() };
    let s = sent(&l.result(*req, json!(["/docs/pages.pdf"])));
    assert_eq!(s[0].0, "picked");
    assert_eq!(s[0].1["at"], 2);
}

#[test]
fn leaflet_a_crash_reopens_from_disk() {
    let (mut l, _, win) = leaflet();
    msg(&mut l, win, json!({"cmd": "ready"}));
    assert_eq!(l.crashed(win), vec![Action::Reload { win }]);
    let s = sent(&msg(&mut l, win, json!({"cmd": "ready"})));
    assert_eq!(s.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["state", "error", "load"]);
}
