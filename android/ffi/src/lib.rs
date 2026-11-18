//! appshell-core for the Android shell (app.appshell.Native in Kotlin).
//!
//! Kotlin calls in with JSON and gets JSON back: a list of actions for it to
//! carry out. Files are reached through the Kotlin `Platform` object (the
//! Storage Access Framework); the core never sees anything but its handles.
//! Every call happens under one lock, so the WebView's request thread (which
//! fetches /blob/ tokens) and the main thread can both come in.

use appshell_core::folio::Folio;
use appshell_core::leaflet::Leaflet;
use appshell_core::{Action, FileError, Platform};
use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jbyteArray, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

struct JniPlatform {
    vm: JavaVM,
    obj: GlobalRef,
}

impl JniPlatform {
    fn env(&self) -> JNIEnv<'_> {
        self.vm.attach_current_thread_permanently().expect("a JNI thread")
    }

    fn call(&self, name: &str, sig: &str, args: &[JValue]) -> Option<jni::objects::JValueOwned<'_>> {
        let mut env = self.env();
        let r = env.call_method(self.obj.as_obj(), name, sig, args);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return None;
        }
        r.ok()
    }

    fn string(&self, name: &str, h: &str) -> String {
        let mut env = self.env();
        let Ok(js) = env.new_string(h) else { return String::new() };
        let r = env.call_method(self.obj.as_obj(), name, "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&js)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return String::new();
        }
        match r.and_then(|v| v.l()) {
            Ok(o) if !o.is_null() => env.get_string(&JString::from(o)).map(String::from).unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn bytes(&self, o: JObject) -> Option<Vec<u8>> {
        if o.is_null() {
            return None;
        }
        self.env().convert_byte_array(JByteArray::from(o)).ok()
    }
}

impl Platform for JniPlatform {
    fn read(&self, h: &str, limit: u64) -> Result<Vec<u8>, FileError> {
        let mut env = self.env();
        let js = env.new_string(h).map_err(|e| FileError::Other(e.to_string()))?;
        let r = env.call_method(self.obj.as_obj(), "read", "(Ljava/lang/String;J)[B", &[JValue::Object(&js), JValue::Long(limit as i64)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        let data = r.ok().and_then(|v| v.l().ok()).and_then(|o| self.bytes(o));
        match data {
            Some(d) => Ok(d),
            None => {
                let e = self.string("lastError", "");
                Err(match e.as_str() {
                    "NOT_FOUND" => FileError::NotFound,
                    "TOO_LARGE" => FileError::TooLarge,
                    "IS_DIR" => FileError::IsDir,
                    other => FileError::Other(if other.is_empty() { "it can't be read".into() } else { other.into() }),
                })
            }
        }
    }

    fn write(&self, h: &str, data: &[u8]) -> Result<(), String> {
        let mut env = self.env();
        let js = env.new_string(h).map_err(|e| e.to_string())?;
        let arr = env.byte_array_from_slice(data).map_err(|e| e.to_string())?;
        let r = env.call_method(self.obj.as_obj(), "write", "(Ljava/lang/String;[B)Ljava/lang/String;", &[JValue::Object(&js), JValue::Object(&arr)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return Err("it can't be written".into());
        }
        match r.and_then(|v| v.l()) {
            Ok(o) if o.is_null() => Ok(()),
            Ok(o) => Err(env.get_string(&JString::from(o)).map(String::from).unwrap_or_default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn mtime(&self, h: &str) -> Option<i64> {
        let mut env = self.env();
        let js = env.new_string(h).ok()?;
        let v = env.call_method(self.obj.as_obj(), "mtime", "(Ljava/lang/String;)J", &[JValue::Object(&js)]).ok()?.j().ok()?;
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return None;
        }
        (v >= 0).then_some(v)
    }

    fn exists(&self, h: &str) -> bool {
        let mut env = self.env();
        let Ok(js) = env.new_string(h) else { return false };
        let r = env.call_method(self.obj.as_obj(), "exists", "(Ljava/lang/String;)Z", &[JValue::Object(&js)]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return false;
        }
        r.and_then(|v| v.z()).unwrap_or(false)
    }

    fn name(&self, h: &str) -> String {
        self.string("name", h)
    }

    fn folder(&self, h: &str) -> String {
        self.string("folder", h)
    }

    fn with_extension(&self, _h: &str, _ext: &str) -> Option<String> {
        None // a document's name is the provider's, not a path to add to
    }

    fn load_settings(&self) -> Option<Vec<u8>> {
        let o = self.call("loadSettings", "()[B", &[])?.l().ok()?;
        self.bytes(o)
    }

    fn store_settings(&self, data: &[u8]) {
        let env = self.env();
        let Ok(arr) = env.byte_array_from_slice(data) else { return };
        let _ = self.call("storeSettings", "([B)V", &[JValue::Object(&arr)]);
    }

    fn system_dark(&self) -> bool {
        self.call("systemDark", "()Z", &[]).and_then(|v| v.z().ok()).unwrap_or(false)
    }

    fn supports_recent(&self) -> bool {
        false // a file can't be reached again once the app has closed
    }
}

enum Core {
    Folio(Folio),
    Leaflet(Leaflet),
}

// The Kotlin side only reaches a core under the lock below.
unsafe impl Send for Core {}

fn cores() -> &'static Mutex<HashMap<i64, Core>> {
    static CORES: OnceLock<Mutex<HashMap<i64, Core>>> = OnceLock::new();
    CORES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn actions(a: Vec<Action>) -> String {
    serde_json::to_string(&a).unwrap_or_else(|_| "[]".into())
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(String::from).unwrap_or_default()
}

fn out(env: &mut JNIEnv, s: String) -> jstring {
    env.new_string(s).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

fn handles(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect()
}

/// Native.create(app, platform): a core for "folio" or "leaflet".
#[no_mangle]
pub extern "system" fn Java_app_appshell_Native_create<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, app: JString<'l>, platform: JObject<'l>, files: JString<'l>) -> jlong {
    let app = jstr(&mut env, &app);
    let files: Value = serde_json::from_str(&jstr(&mut env, &files)).unwrap_or(Value::Null);
    let (Ok(vm), Ok(obj)) = (env.get_java_vm(), env.new_global_ref(platform)) else { return 0 };
    let p = Box::new(JniPlatform { vm, obj });
    let core = if app == "folio" { Core::Folio(Folio::new(p, handles(&files))) } else { Core::Leaflet(Leaflet::new(p)) };
    static NEXT: Mutex<i64> = Mutex::new(1);
    let mut n = NEXT.lock().unwrap();
    let id = *n;
    *n += 1;
    cores().lock().unwrap().insert(id, core);
    id
}

#[no_mangle]
pub extern "system" fn Java_app_appshell_Native_destroy<'l>(_env: JNIEnv<'l>, _c: JClass<'l>, id: jlong) {
    cores().lock().unwrap().remove(&id);
}

/// Native.call(id, json): {"op": ..., ...} -> actions (or a value, for "background" and "size").
#[no_mangle]
pub extern "system" fn Java_app_appshell_Native_call<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, id: jlong, request: JString<'l>) -> jstring {
    let req: Value = serde_json::from_str(&jstr(&mut env, &request)).unwrap_or(Value::Null);
    let result = {
        let mut map = cores().lock().unwrap();
        match map.get_mut(&id) {
            Some(core) => dispatch(core, &req),
            None => "[]".into(),
        }
    };
    out(&mut env, result)
}

fn dispatch(core: &mut Core, r: &Value) -> String {
    let op = r["op"].as_str().unwrap_or("");
    let win = r["win"].as_u64().unwrap_or(0) as u32;
    let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
    match core {
        Core::Folio(f) => actions(match op {
            "start" => vec![Action::NewWindow { win: appshell_core::folio::WIN }],
            "message" => f.message(&s("msg")),
            "result" => f.result(r["req"].as_u64().unwrap_or(0) as u32, r["value"].clone()),
            "openFiles" => f.open_files(handles(&r["files"])),
            "navigation" => f.navigation(&s("url"), r["user"].as_bool().unwrap_or(false)),
            "closeRequested" => f.close_requested(),
            "forceClose" => {
                f.force_close();
                vec![Action::Close { win }]
            }
            "closed" => {
                f.closed(None);
                vec![]
            }
            "crashed" => f.crashed(),
            "background" => {
                let (c, d) = f.background();
                return json!({"color": c, "dark": d}).to_string();
            }
            _ => vec![],
        }),
        Core::Leaflet(l) => actions(match op {
            "start" => l.start(handles(&r["files"])),
            "activate" => l.activate(),
            "message" => l.message(win, &s("msg")),
            "result" => l.result(r["req"].as_u64().unwrap_or(0) as u32, r["value"].clone()),
            "openFiles" => l.open_files(handles(&r["files"])),
            "navigation" => l.navigation(win, &s("url"), r["user"].as_bool().unwrap_or(false)),
            "closeRequested" => l.close_requested(win),
            "closed" => {
                let last = l.closed(win, None);
                return json!({"last": last}).to_string();
            }
            "crashed" => l.crashed(win),
            "background" => {
                let (c, d) = l.background();
                return json!({"color": c, "dark": d}).to_string();
            }
            _ => vec![],
        }),
    }
}

/// Native.blobTake(id, win, token): the bytes, behind their MIME type and a newline.
#[no_mangle]
pub extern "system" fn Java_app_appshell_Native_blobTake<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, id: jlong, win: jint, token: JString<'l>) -> jbyteArray {
    let token = jstr(&mut env, &token);
    let taken = {
        let mut map = cores().lock().unwrap();
        match map.get_mut(&id) {
            Some(Core::Leaflet(l)) => l.blob_take(win as u32, &token),
            _ => None,
        }
    };
    match taken {
        Some((data, kind)) => {
            let mut v = kind.into_bytes();
            v.push(b'\n');
            v.extend_from_slice(&data);
            env.byte_array_from_slice(&v).map(|a| a.into_raw()).unwrap_or(std::ptr::null_mut())
        }
        None => std::ptr::null_mut(),
    }
}

/// Native.blobPut(id, win, bytes, kind): a token, or null if it's too large.
#[no_mangle]
pub extern "system" fn Java_app_appshell_Native_blobPut<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, id: jlong, win: jint, data: JByteArray<'l>, kind: JString<'l>) -> jstring {
    let kind = jstr(&mut env, &kind);
    let Ok(bytes) = env.convert_byte_array(&data) else { return std::ptr::null_mut() };
    let token = {
        let mut map = cores().lock().unwrap();
        match map.get_mut(&id) {
            Some(Core::Leaflet(l)) => l.blob_put(win as u32, bytes, &kind),
            _ => None,
        }
    };
    match token {
        Some(t) => out(&mut env, t),
        None => std::ptr::null_mut(),
    }
}

/// Native.exportDone(id, req, pdf or null, error): the page printed to a PDF, or didn't.
#[no_mangle]
pub extern "system" fn Java_app_appshell_Native_exportDone<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, id: jlong, req: jint, pdf: JByteArray<'l>, error: JString<'l>) -> jstring {
    let result = if pdf.is_null() {
        Err(if error.is_null() { "the page couldn't be printed".to_string() } else { jstr(&mut env, &error) })
    } else {
        env.convert_byte_array(&pdf).map_err(|e| e.to_string())
    };
    let a = {
        let mut map = cores().lock().unwrap();
        match map.get_mut(&id) {
            Some(Core::Leaflet(l)) => l.export_done(req as u32, result),
            _ => vec![],
        }
    };
    out(&mut env, actions(a))
}

/// Native.script(app, kind, arrayBuffers): the scripts that go in front of the page.
#[no_mangle]
pub extern "system" fn Java_app_appshell_Native_script<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, app: JString<'l>, buffers: jni::sys::jboolean) -> jstring {
    let app = jstr(&mut env, &app);
    let send = "function (m) { __appshell.postMessage(m); }";
    let mut s = appshell_core::web::bridge_script(&app, send);
    if app == "leaflet" {
        s.push('\n');
        s.push_str(&appshell_core::web::put_script(send, buffers != 0));
    }
    out(&mut env, s)
}
