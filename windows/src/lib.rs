//! Folio and Leaflet on Windows. The interface (each app's ui/, built into
//! the executable) runs in WebView2 with everything a browser would do on its
//! own turned off; what the page asks for is decided by appshell-core.
//!
//! Privacy: the WebView2 profile is InPrivate and lives in a private
//! temporary folder that is deleted when the app closes (and, if it was
//! killed, the next time it starts); every request the page makes goes to
//! the app itself or to a closed proxy port; no SmartScreen, autofill,
//! password saving, downloads, devtools or permissions.
#![cfg(windows)]

mod platform;
mod webview2;

use appshell_core::folio::Folio;
use appshell_core::leaflet::Leaflet;
use appshell_core::web::{bridge_script, mime, rgb, ui_path};
use appshell_core::{Action, Filter};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Fullscreen, Theme, Window, WindowBuilder, WindowId};
use wry::http::{header, Request, Response, StatusCode};
use wry::{RequestAsyncResponder, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

pub use platform::WinFiles;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum App {
    Folio,
    Leaflet,
}

impl App {
    fn name(self) -> &'static str {
        match self {
            App::Folio => "Folio",
            App::Leaflet => "Leaflet",
        }
    }
    fn scheme(self) -> &'static str {
        match self {
            App::Folio => "folio",
            App::Leaflet => "leaflet",
        }
    }
    /// Where the page lives: wry serves the app's own scheme as
    /// https://<scheme>.localhost/ on Windows.
    fn origin(self) -> String {
        format!("https://{}.localhost", self.scheme())
    }
}

/// The interface built into this executable: (path, bytes), sorted.
pub type Ui = &'static [(&'static str, &'static [u8])];

enum Core {
    Folio(Folio),
    Leaflet(Leaflet),
}

pub enum UserEvent {
    Ipc { win: u32, msg: String },
    Protocol { win: u32, req: Request<Vec<u8>>, responder: RequestAsyncResponder },
    Navigate { win: u32, url: String, user: bool },
    Crashed { win: u32 },
    Drop { win: u32, paths: Vec<PathBuf> },
    DropHint { win: u32, show: bool },
    Exported { req: u32, result: Result<Vec<u8>, String> },
    CloseCheck { win: u32, ok: bool },
    PrintLoaded { key: u32 },
}

struct Win {
    window: Window,
    webview: WebView,
}

/// A window showing a PDF in WebView2's own viewer, for its print dialog.
struct PrintWin {
    window: Window,
    #[allow(dead_code)]
    webview: WebView,
}

struct Shell {
    app: App,
    ui: Ui,
    core: Core,
    proxy: EventLoopProxy<UserEvent>,
    wins: HashMap<u32, Win>,
    ids: HashMap<WindowId, u32>,
    prints: HashMap<u32, PrintWin>,
    print_ids: HashMap<WindowId, u32>,
    print_data: std::sync::Arc<std::sync::Mutex<HashMap<u32, Vec<u8>>>>,
    next_print: u32,
    context: WebContext,
    data_dir: PathBuf,
    fullscreen: HashMap<u32, bool>,
    #[cfg(feature = "selftest")]
    test: Option<selftest::Test>,
}

/// The browser arguments: wry's defaults, a closed proxy port with no
/// exceptions (not even this machine), and none of the browser's own
/// background traffic.
fn browser_args() -> String {
    [
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,AutofillServerCommunication,OptimizationHints,MediaRouter,DialMediaRouteProvider,msEdgeTranslate,msDiagnosticDataForceEnabled",
        "--proxy-server=http://127.0.0.1:9",
        "--proxy-bypass-list=<-loopback>",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-domain-reliability",
        "--disable-sync",
        "--no-pings",
        "--disable-breakpad",
        "--disable-client-side-phishing-detection",
        "--no-first-run",
        "--webrtc-ip-handling-policy=disable_non_proxied_udp",
    ]
    .join(" ")
}

/// A private folder for the WebView2 profile of this run. Folders a killed
/// or crashed run left behind are removed once they're a day old (so not
/// one that another running copy is using).
fn make_data_dir(app: App) -> PathBuf {
    let base = std::env::temp_dir();
    let prefix = format!("{}-webview-", app.scheme());
    if let Ok(entries) = std::fs::read_dir(&base) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 24 * 3600);
            if name.starts_with(&prefix) && old && e.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let dir = base.join(format!("{prefix}{}", &appshell_core::token()[..16]));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The WebView2 browser process lets go of its folder a moment after the
/// last window closes.
fn remove_data_dir(dir: &Path) {
    for _ in 0..30 {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn serve_static(ui: Ui, path: &str) -> Response<Cow<'static, [u8]>> {
    let found = ui_path(path).and_then(|rel| ui.binary_search_by(|(p, _)| p.cmp(&rel.as_str())).ok().map(|i| ui[i]));
    match found {
        Some((p, data)) => respond(StatusCode::OK, mime(p), Cow::Borrowed(data)),
        None => respond(StatusCode::NOT_FOUND, "text/plain", Cow::Borrowed(b"")),
    }
}

fn respond(status: StatusCode, kind: &str, body: Cow<'static, [u8]>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, kind)
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .unwrap()
}

/// Run the app until its last window closes.
pub fn run(app: App, ui: Ui) -> ! {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().to_string_lossy().starts_with("--"))
        .map(|p| std::path::absolute(&p).unwrap_or(p).to_string_lossy().into_owned())
        .collect();
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let files = Box::new(WinFiles { app: app.name() });
    let core = match app {
        App::Folio => Core::Folio(Folio::new(files, args.clone())),
        App::Leaflet => Core::Leaflet(Leaflet::new(files)),
    };
    let data_dir = make_data_dir(app);
    let mut shell = Shell {
        app,
        ui,
        core,
        proxy,
        wins: HashMap::new(),
        ids: HashMap::new(),
        prints: HashMap::new(),
        print_ids: HashMap::new(),
        print_data: Default::default(),
        next_print: 1,
        context: WebContext::new(Some(data_dir.clone())),
        data_dir,
        fullscreen: HashMap::new(),
        #[cfg(feature = "selftest")]
        test: selftest::Test::from_env(),
    };
    let first = match &mut shell.core {
        Core::Folio(_) => vec![Action::NewWindow { win: appshell_core::folio::WIN }],
        Core::Leaflet(l) => l.start(args),
    };
    shell.perform(&event_loop, first);

    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                if let Some(key) = shell.print_ids.remove(&window_id) {
                    shell.prints.remove(&key);
                    shell.print_data.lock().unwrap().remove(&key);
                } else if let Some(&win) = shell.ids.get(&window_id) {
                    let actions = match &mut shell.core {
                        Core::Folio(f) => f.close_requested(),
                        Core::Leaflet(l) => l.close_requested(win),
                    };
                    shell.perform(target, actions);
                }
            }
            Event::UserEvent(ev) => shell.user_event(target, ev),
            _ => {}
        }
        if shell.wins.is_empty() {
            shell.prints.clear();
            *flow = ControlFlow::Exit;
        }
        if *flow == ControlFlow::Exit {
            remove_data_dir(&shell.data_dir);
        }
    })
}

impl Shell {
    fn user_event(&mut self, target: &EventLoopWindowTarget<UserEvent>, ev: UserEvent) {
        let actions = match ev {
            UserEvent::Ipc { win, msg } => {
                #[cfg(feature = "selftest")]
                if let Some(t) = &self.test {
                    if t.message(&msg) {
                        return;
                    }
                }
                match &mut self.core {
                    Core::Folio(f) => f.message(&msg),
                    Core::Leaflet(l) => l.message(win, &msg),
                }
            }
            UserEvent::Protocol { win, req, responder } => {
                self.protocol(win, req, responder);
                vec![]
            }
            UserEvent::Navigate { win, url, user } => match &mut self.core {
                Core::Folio(f) => f.navigation(&url, user),
                Core::Leaflet(l) => l.navigation(win, &url, user),
            },
            UserEvent::Crashed { win } => match &mut self.core {
                Core::Folio(f) => f.crashed(),
                Core::Leaflet(l) => l.crashed(win),
            },
            UserEvent::Drop { win, paths } => {
                let handles: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                match &mut self.core {
                    Core::Folio(f) => {
                        let mut a = f.drop_hint(false);
                        a.extend(f.open_files(handles));
                        a
                    }
                    Core::Leaflet(l) => l.dropped(win, handles),
                }
            }
            UserEvent::DropHint { show, .. } => match &self.core {
                Core::Folio(f) => f.drop_hint(show),
                Core::Leaflet(_) => vec![],
            },
            UserEvent::Exported { req, result } => match &mut self.core {
                Core::Leaflet(l) => l.export_done(req, result),
                Core::Folio(_) => vec![],
            },
            UserEvent::CloseCheck { win, ok } => {
                if ok {
                    vec![]
                } else {
                    // The page is broken; don't trap the user.
                    if let Core::Folio(f) = &mut self.core {
                        f.force_close();
                    }
                    vec![Action::Close { win }]
                }
            }
            UserEvent::PrintLoaded { key } => {
                if let Some(p) = self.prints.get(&key) {
                    webview2::show_print_dialog(&p.webview);
                }
                vec![]
            }
        };
        self.perform(target, actions);
    }

    /// /put and /blob/<token> for Leaflet's page; everything else is ui/.
    fn protocol(&mut self, win: u32, req: Request<Vec<u8>>, responder: RequestAsyncResponder) {
        let path = req.uri().path().to_string();
        let Core::Leaflet(l) = &mut self.core else {
            responder.respond(serve_static(self.ui, &path));
            return;
        };
        if path == "/put" && req.method() == "POST" {
            let kind = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream").to_string();
            let r = match l.blob_put(win, req.into_body(), &kind) {
                Some(token) => respond(StatusCode::OK, "text/plain", Cow::Owned(token.into_bytes())),
                None => respond(StatusCode::PAYLOAD_TOO_LARGE, "text/plain", Cow::Borrowed(b"too large")),
            };
            responder.respond(r);
        } else if let Some(token) = path.strip_prefix("/blob/") {
            let r = match l.blob_take(win, token) {
                Some((data, kind)) => respond(StatusCode::OK, &kind, Cow::Owned(data)),
                None => respond(StatusCode::NOT_FOUND, "text/plain", Cow::Borrowed(b"")),
            };
            responder.respond(r);
        } else {
            responder.respond(serve_static(self.ui, &path));
        }
    }

    fn window(&self, win: u32) -> Option<&Win> {
        self.wins.get(&win)
    }

    fn perform(&mut self, target: &EventLoopWindowTarget<UserEvent>, actions: Vec<Action>) {
        let mut queue: std::collections::VecDeque<Action> = actions.into();
        while let Some(a) = queue.pop_front() {
            let more = self.perform_one(target, a);
            queue.extend(more);
        }
    }

    fn perform_one(&mut self, target: &EventLoopWindowTarget<UserEvent>, action: Action) -> Vec<Action> {
        match action {
            Action::Eval { win, js } => {
                if let Some(w) = self.window(win) {
                    let _ = w.webview.evaluate_script(&js);
                }
            }
            Action::NewWindow { win } => self.new_window(target, win),
            Action::Title { win, title } => {
                if let Some(w) = self.window(win) {
                    w.window.set_title(&title);
                }
            }
            Action::Background { win, color, dark } => {
                if let Some(w) = self.window(win) {
                    let (r, g, b) = rgb(&color);
                    let _ = w.webview.set_background_color((r, g, b, 255));
                    w.window.set_theme(Some(if dark { Theme::Dark } else { Theme::Light }));
                }
            }
            Action::Fullscreen { win } => {
                if let Some(w) = self.wins.get(&win) {
                    let on = !self.fullscreen.get(&win).copied().unwrap_or(false);
                    w.window.set_fullscreen(if on { Some(Fullscreen::Borderless(None)) } else { None });
                    self.fullscreen.insert(win, on);
                }
            }
            Action::Present { win } => {
                if let Some(w) = self.window(win) {
                    w.window.set_minimized(false);
                    w.window.set_focus();
                }
            }
            Action::Close { win } => return self.close_window(win),
            Action::Reload { win } => {
                if let Some(w) = self.window(win) {
                    let _ = w.webview.load_url(&format!("{}/index.html", self.app.origin()));
                }
            }
            Action::OpenUrl { url } => open_url(&url),
            Action::OpenDialog { win, req, title, multiple, filters, near } => {
                let answer = self.open_dialog(win, &title, multiple, &filters, near.as_deref());
                return self.answer(req, answer);
            }
            Action::SaveDialog { win, req, title, name, filters, near, .. } => {
                let answer = self.save_dialog(win, &title, &name, &filters, near.as_deref());
                return self.answer(req, answer);
            }
            Action::Ask { win, req, message, detail, buttons, cancel, .. } => {
                let answer = self.ask(win, &message, &detail, &buttons, cancel);
                return self.answer(req, answer);
            }
            Action::PrintPage { win } => {
                if let Some(w) = self.window(win) {
                    webview2::show_print_dialog(&w.webview);
                }
            }
            Action::PrintPdf { win, token, name } => {
                let data = match &mut self.core {
                    Core::Leaflet(l) => l.blob_take(win, &token).map(|d| d.0),
                    Core::Folio(_) => None,
                };
                if let Some(data) = data {
                    self.print_pdf(target, data, &name);
                }
            }
            Action::ExportPdf { win, req } => {
                if let Some(w) = self.window(win) {
                    let proxy = self.proxy.clone();
                    let dir = self.data_dir.join(format!("export-{}", &appshell_core::token()[..12]));
                    webview2::print_to_pdf(&w.webview, dir, move |result| {
                        let _ = proxy.send_event(UserEvent::Exported { req, result });
                    });
                }
            }
        }
        vec![]
    }

    fn answer(&mut self, req: u32, value: Value) -> Vec<Action> {
        match &mut self.core {
            Core::Folio(f) => f.result(req, value),
            Core::Leaflet(l) => l.result(req, value),
        }
    }

    fn open_dialog(&self, win: u32, title: &str, multiple: bool, filters: &[Filter], near: Option<&str>) -> Value {
        #[cfg(feature = "selftest")]
        if let Some(t) = &self.test {
            return t.answer_open();
        }
        let mut d = rfd::FileDialog::new().set_title(title);
        if let Some(w) = self.window(win) {
            d = d.set_parent(&w.window);
        }
        for f in filters {
            let exts: Vec<String> = f.patterns.iter().map(|p| p.trim_start_matches("*.").to_string()).collect();
            d = d.add_filter(&f.name, &exts);
        }
        if let Some(dir) = near.and_then(|n| Path::new(n).parent()).filter(|d| d.is_dir()) {
            d = d.set_directory(dir);
        }
        let picked: Option<Vec<PathBuf>> = if multiple { d.pick_files() } else { d.pick_file().map(|p| vec![p]) };
        match picked {
            Some(list) => json!(list.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>()),
            None => Value::Null,
        }
    }

    fn save_dialog(&self, win: u32, title: &str, name: &str, filters: &[Filter], near: Option<&str>) -> Value {
        #[cfg(feature = "selftest")]
        if let Some(t) = &self.test {
            return t.answer_save(name);
        }
        let mut d = rfd::FileDialog::new().set_title(title).set_file_name(name);
        if let Some(w) = self.window(win) {
            d = d.set_parent(&w.window);
        }
        for f in filters {
            let exts: Vec<String> = f.patterns.iter().map(|p| p.trim_start_matches("*.").to_string()).collect();
            d = d.add_filter(&f.name, &exts);
        }
        let folder = near
            .and_then(|n| Path::new(n).parent().map(Path::to_path_buf))
            .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents")))
            .filter(|d| d.is_dir());
        if let Some(dir) = folder {
            d = d.set_directory(dir);
        }
        match d.save_file() {
            Some(p) => json!(p.to_string_lossy()),
            None => Value::Null,
        }
    }

    fn ask(&self, win: u32, message: &str, detail: &str, buttons: &[String], cancel: usize) -> Value {
        use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
        #[cfg(feature = "selftest")]
        if let Some(t) = &self.test {
            return t.answer_ask(buttons);
        }
        let others: Vec<(usize, &String)> = buttons.iter().enumerate().filter(|(i, _)| *i != cancel).collect();
        let cancel_label = buttons.get(cancel).cloned().unwrap_or_else(|| "Cancel".into());
        let set = match others.as_slice() {
            [(_, a)] => MessageButtons::OkCancelCustom((*a).clone(), cancel_label),
            [(_, a), (_, b)] => MessageButtons::YesNoCancelCustom((*b).clone(), (*a).clone(), cancel_label),
            _ => MessageButtons::OkCancel,
        };
        let mut d = MessageDialog::new().set_level(MessageLevel::Info).set_title(self.app.name()).set_description(format!("{message}\n\n{detail}")).set_buttons(set);
        if let Some(w) = self.window(win) {
            d = d.set_parent(&w.window);
        }
        match d.show() {
            MessageDialogResult::Custom(label) => match buttons.iter().position(|b| *b == label) {
                Some(i) => json!(i),
                None => Value::Null,
            },
            MessageDialogResult::Ok | MessageDialogResult::Yes => others.first().map(|(i, _)| json!(i)).unwrap_or(Value::Null),
            _ => json!(cancel),
        }
    }

    fn new_window(&mut self, target: &EventLoopWindowTarget<UserEvent>, win: u32) {
        let (w, h, max) = match &self.core {
            Core::Folio(f) => f.window_size(),
            Core::Leaflet(l) => l.window_size(),
        };
        let (bg, dark) = match &self.core {
            Core::Folio(f) => f.background(),
            Core::Leaflet(l) => l.background(),
        };
        let window = match WindowBuilder::new()
            .with_title(self.app.name())
            .with_inner_size(LogicalSize::new(w as f64, h as f64))
            .with_maximized(max)
            .with_theme(Some(if dark { Theme::Dark } else { Theme::Light }))
            .build(target)
        {
            Ok(w) => w,
            Err(_) => return,
        };
        let proxy = self.proxy.clone();
        let p2 = self.proxy.clone();
        let p3 = self.proxy.clone();
        let origin = self.app.origin();
        let ui = self.ui;
        let (r, g, b) = rgb(&bg);
        let mut builder = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_url(format!("{origin}/index.html"))
            .with_https_scheme(true)
            .with_incognito(true)
            .with_additional_browser_args(browser_args())
            .with_browser_accelerator_keys(false)
            .with_devtools(std::env::var_os("APPSHELL_DEBUG").is_some())
            .with_general_autofill_enabled(false)
            .with_hotkeys_zoom(false)
            .with_back_forward_navigation_gestures(false)
            .with_clipboard(true)
            .with_autoplay(false)
            .with_background_color((r, g, b, 255))
            .with_user_agent(self.app.name())
            .with_initialization_script_for_main_only(bridge_script(self.app.scheme(), "function (s) { window.ipc.postMessage(s); }"), true)
            .with_asynchronous_custom_protocol(self.app.scheme().to_string(), move |_id, req, responder| {
                // ui/ is answered here; the rest goes to the window's own state
                let path = req.uri().path();
                if path == "/put" || path.starts_with("/blob/") {
                    let _ = proxy.send_event(UserEvent::Protocol { win, req, responder });
                } else {
                    responder.respond(serve_static(ui, path));
                }
            })
            .with_ipc_handler({
                let origin = origin.clone();
                move |req: Request<String>| {
                    // only the app's own page may talk to it
                    let uri = req.uri().to_string();
                    if uri.starts_with(&format!("{origin}/")) {
                        let _ = p2.send_event(UserEvent::Ipc { win, msg: req.into_body() });
                    }
                }
            })
            .with_drag_drop_handler(move |e| {
                match e {
                    wry::DragDropEvent::Enter { .. } => {
                        let _ = p3.send_event(UserEvent::DropHint { win, show: true });
                    }
                    wry::DragDropEvent::Leave => {
                        let _ = p3.send_event(UserEvent::DropHint { win, show: false });
                    }
                    wry::DragDropEvent::Drop { paths, .. } => {
                        let _ = p3.send_event(UserEvent::Drop { win, paths });
                    }
                    _ => {}
                }
                true
            })
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_download_started_handler(|_, _| false);
        #[cfg(feature = "selftest")]
        if let Some(t) = &self.test {
            builder = builder.with_initialization_script_for_main_only(t.script(), true);
        }
        let webview = match builder.build(&window) {
            Ok(v) => v,
            Err(e) => {
                error_box(&format!("{} can't start: {e}\n\nIt needs the Microsoft Edge WebView2 Runtime.", self.app.name()));
                return;
            }
        };
        webview2::lock_down(&webview, self.proxy.clone(), win, &origin, std::env::var_os("APPSHELL_DEBUG").is_some());
        self.ids.insert(window.id(), win);
        self.wins.insert(win, Win { window, webview });
    }

    fn close_window(&mut self, win: u32) -> Vec<Action> {
        let Some(w) = self.wins.remove(&win) else { return vec![] };
        self.ids.remove(&w.window.id());
        let scale = w.window.scale_factor();
        let size = w.window.inner_size().to_logical::<f64>(scale);
        let state = Some((size.width.round() as u32, size.height.round() as u32, w.window.is_maximized()));
        drop(w);
        match &mut self.core {
            Core::Folio(f) => f.closed(state),
            Core::Leaflet(l) => {
                l.closed(win, state);
            }
        }
        vec![]
    }

    /// A PDF to print: a small window with WebView2's own PDF viewer, whose
    /// print dialog opens as soon as the file is in.
    fn print_pdf(&mut self, target: &EventLoopWindowTarget<UserEvent>, data: Vec<u8>, name: &str) {
        let key = self.next_print;
        self.next_print += 1;
        self.print_data.lock().unwrap().insert(key, data);
        let Ok(window) = WindowBuilder::new().with_title(format!("Print {name} — Leaflet")).with_inner_size(LogicalSize::new(820.0, 900.0)).build(target) else { return };
        let store = self.print_data.clone();
        let proxy = self.proxy.clone();
        let origin = format!("https://print.localhost/{key}/{}", name.replace(['/', '\\', '?', '#', '%'], "_"));
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_url(origin)
            .with_https_scheme(true)
            .with_incognito(true)
            .with_additional_browser_args(browser_args())
            .with_devtools(false)
            .with_custom_protocol("print".into(), move |_id, req| {
                let k = req.uri().path().trim_start_matches('/').split('/').next().and_then(|s| s.parse::<u32>().ok());
                match k.and_then(|k| store.lock().unwrap().get(&k).cloned()) {
                    Some(d) => respond(StatusCode::OK, "application/pdf", Cow::Owned(d)),
                    None => respond(StatusCode::NOT_FOUND, "text/plain", Cow::Borrowed(b"")),
                }
            })
            .with_on_page_load_handler(move |e, _| {
                if matches!(e, wry::PageLoadEvent::Finished) {
                    let _ = proxy.send_event(UserEvent::PrintLoaded { key });
                }
            })
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .build(&window);
        if let Ok(webview) = webview {
            webview2::lock_down_viewer(&webview);
            self.print_ids.insert(window.id(), key);
            self.prints.insert(key, PrintWin { window, webview });
        }
    }
}

/// Only links the user said yes to get here (appshell-core asks first).
fn open_url(url: &str) {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !appshell_core::is_openable_link(url) {
        return;
    }
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
}

fn error_box(text: &str) {
    rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("Error").set_description(text).set_buttons(rfd::MessageButtons::Ok).show();
}

#[cfg(feature = "selftest")]
mod selftest {
    //! The CI's test build only: APPSHELL_TEST_SCRIPT is run in the page, it
    //! reports with `window.ipc.postMessage(JSON.stringify({__test: ...}))`,
    //! the report goes to APPSHELL_TEST_OUT and the app quits. Dialogs are
    //! answered from APPSHELL_TEST_OPEN / APPSHELL_TEST_SAVE / APPSHELL_TEST_ASK.
    use serde_json::{json, Value};

    pub struct Test {
        script: String,
        out: String,
    }

    impl Test {
        pub fn from_env() -> Option<Test> {
            let path = std::path::PathBuf::from(std::env::var("APPSHELL_TEST_SCRIPT").ok()?);
            let common = std::fs::read_to_string(path.with_file_name("common.js")).ok()?;
            let test = std::fs::read_to_string(&path).ok()?;
            let script = format!(
                "window.__appshellReport = function (o) {{ window.ipc.postMessage(JSON.stringify({{ __test: o }})); }};\n{common}\n{test}"
            );
            Some(Test { script, out: std::env::var("APPSHELL_TEST_OUT").unwrap_or_else(|_| "selftest.json".into()) })
        }
        pub fn script(&self) -> String {
            self.script.clone()
        }
        pub fn message(&self, msg: &str) -> bool {
            let Ok(v) = serde_json::from_str::<Value>(msg) else { return false };
            let Some(report) = v.get("__test") else { return false };
            let _ = std::fs::write(&self.out, report.to_string());
            std::process::exit(if report.get("ok").and_then(Value::as_bool) == Some(true) { 0 } else { 1 });
        }
        pub fn answer_open(&self) -> Value {
            match std::env::var("APPSHELL_TEST_OPEN") {
                Ok(p) => json!([p]),
                Err(_) => Value::Null,
            }
        }
        pub fn answer_save(&self, name: &str) -> Value {
            match std::env::var("APPSHELL_TEST_SAVE") {
                Ok(dir) => json!(std::path::Path::new(&dir).join(name).to_string_lossy()),
                Err(_) => Value::Null,
            }
        }
        pub fn answer_ask(&self, buttons: &[String]) -> Value {
            let want = std::env::var("APPSHELL_TEST_ASK").unwrap_or_default();
            buttons.iter().position(|b| *b == want).map(|i| json!(i)).unwrap_or(json!(0))
        }
    }
}
