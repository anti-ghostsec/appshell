//! WebView2 settings and events wry doesn't cover.

use crate::UserEvent;
use std::path::PathBuf;
use tao::event_loop::EventLoopProxy;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{take_pwstr, ContextMenuRequestedEventHandler, NavigationStartingEventHandler, PrintToPdfCompletedHandler, ProcessFailedEventHandler};
use windows::core::{Interface, BOOL, HSTRING, PWSTR};
use wry::{WebView, WebViewExtWindows};

/// Context menu items that would leave the page, save or share it, or reach the network.
fn unwanted(name: &str, debug: bool) -> bool {
    let n = name.to_ascii_lowercase();
    if n == "inspectelement" {
        return !debug;
    }
    ["back", "forward", "reload", "saveas", "print", "share", "webcapture", "qr", "link", "image", "search", "translate", "readaloud", "visualsearch", "sendto", "openin", "copytoclipboard"]
        .iter()
        .any(|w| n.contains(w))
}

unsafe fn settings(wv: &ICoreWebView2) {
    let Ok(s) = (unsafe { wv.Settings() }) else { return };
    unsafe {
        let _ = s.SetAreHostObjectsAllowed(false);
        let _ = s.SetIsBuiltInErrorPageEnabled(false);
        let _ = s.SetIsZoomControlEnabled(false);
        let _ = s.SetAreDefaultScriptDialogsEnabled(true);
        if let Ok(s4) = s.cast::<ICoreWebView2Settings4>() {
            let _ = s4.SetIsPasswordAutosaveEnabled(false);
        }
        if let Ok(s6) = s.cast::<ICoreWebView2Settings6>() {
            let _ = s6.SetIsSwipeNavigationEnabled(false);
        }
        if let Ok(s8) = s.cast::<ICoreWebView2Settings8>() {
            let _ = s8.SetIsReputationCheckingRequired(false);
        }
    }
}

/// Only the app's own page may load in the window; anything else is
/// refused and handed to appshell-core, which decides whether it was a link
/// worth asking about.
pub fn lock_down(webview: &WebView, proxy: EventLoopProxy<UserEvent>, win: u32, origin: &str, debug: bool) {
    let wv = webview.webview();
    unsafe {
        settings(&wv);
        let page = format!("{origin}/index.html");
        let p = proxy.clone();
        let mut token = 0i64;
        let _ = wv.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                let bare = uri.split(['#', '?']).next().unwrap_or("");
                if bare == page {
                    return Ok(());
                }
                args.SetCancel(true)?;
                let mut user = BOOL(0);
                let _ = args.IsUserInitiated(&mut user);
                let _ = p.send_event(UserEvent::Navigate { win, url: uri, user: user.as_bool() });
                Ok(())
            })),
            &mut token,
        );
        let p = proxy.clone();
        let _ = wv.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                args.ProcessFailedKind(&mut kind)?;
                if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                    || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
                    || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED
                {
                    let _ = p.send_event(UserEvent::Crashed { win });
                }
                Ok(())
            })),
            &mut token,
        );
        if let Ok(wv11) = wv.cast::<ICoreWebView2_11>() {
            let _ = wv11.add_ContextMenuRequested(
                &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let items = args.MenuItems()?;
                    let mut n = 0u32;
                    items.Count(&mut n)?;
                    for i in (0..n).rev() {
                        let item = items.GetValueAtIndex(i)?;
                        let mut name = PWSTR::null();
                        item.Name(&mut name)?;
                        if unwanted(&take_pwstr(name), debug) {
                            items.RemoveValueAtIndex(i)?;
                        }
                    }
                    let mut left = 0u32;
                    items.Count(&mut left)?;
                    if left == 0 {
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            );
        }
    }
}

/// The print window: only its own PDF.
pub fn lock_down_viewer(webview: &WebView) {
    let wv = webview.webview();
    unsafe {
        settings(&wv);
        let mut token = 0i64;
        let _ = wv.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                if !take_pwstr(uri).starts_with("https://print.localhost/") {
                    args.SetCancel(true)?;
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

/// The system's print dialog for what the view shows.
pub fn show_print_dialog(webview: &WebView) {
    let wv = webview.webview();
    let shown = unsafe { wv.cast::<ICoreWebView2_16>().and_then(|w| w.ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_SYSTEM)) };
    if shown.is_err() {
        let _ = webview.print();
    }
}

/// Print the page to a PDF: A4, 20 mm top and bottom, 22 mm at the sides,
/// as Leaflet does on Linux. The file is written in `dir` (inside the
/// app's private temporary folder), read back and removed.
pub fn print_to_pdf(webview: &WebView, dir: PathBuf, done: impl FnOnce(Result<Vec<u8>, String>) + 'static) {
    let wv = webview.webview();
    let env = webview.environment();
    let slot = std::rc::Rc::new(std::cell::RefCell::new(Some(Box::new(done) as Box<dyn FnOnce(Result<Vec<u8>, String>)>)));
    let finish = {
        let slot = slot.clone();
        move |r: Result<Vec<u8>, String>| {
            if let Some(d) = slot.borrow_mut().take() {
                d(r);
            }
        }
    };
    let result = (|| -> windows::core::Result<()> {
        unsafe {
            let wv7 = wv.cast::<ICoreWebView2_7>()?;
            let env6 = env.cast::<ICoreWebView2Environment6>()?;
            let s = env6.CreatePrintSettings()?;
            s.SetOrientation(COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT)?;
            s.SetPageWidth(210.0 / 25.4)?;
            s.SetPageHeight(297.0 / 25.4)?;
            s.SetMarginTop(20.0 / 25.4)?;
            s.SetMarginBottom(20.0 / 25.4)?;
            s.SetMarginLeft(22.0 / 25.4)?;
            s.SetMarginRight(22.0 / 25.4)?;
            s.SetScaleFactor(1.0)?;
            s.SetShouldPrintBackgrounds(true)?;
            s.SetShouldPrintHeaderAndFooter(false)?;
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("export.pdf");
            let target = HSTRING::from(path.as_os_str());
            let finish = finish.clone();
            let dir = dir.clone();
            wv7.PrintToPdf(
                &target,
                &s,
                &PrintToPdfCompletedHandler::create(Box::new(move |hr, ok| {
                    let r = if hr.is_ok() && ok {
                        std::fs::read(&path).map_err(|e| crate::platform::describe(&e))
                    } else {
                        Err("the page couldn't be printed".into())
                    };
                    let _ = std::fs::remove_dir_all(&dir);
                    finish(r);
                    Ok(())
                })),
            )
        }
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&dir);
        finish(Err(format!("printing to PDF isn't available here ({e})")));
    }
}
