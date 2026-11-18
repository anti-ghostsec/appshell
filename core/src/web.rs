//! Serving the interface, and the small script each shell puts in front of it.

/// MIME types for the interface's files (leaflet.py's table, plus fonts Folio ships).
pub fn mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "gif" => "image/gif",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// The interface file a request path names, as a path relative to ui/, or
/// None for anything that would leave ui/ or isn't a plain file path.
pub fn ui_path(url_path: &str) -> Option<String> {
    let path = url_path.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(path)?;
    let mut parts: Vec<&str> = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains('\\') || p.contains(':') || p.contains('\0') => return None,
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A CSS colour from the page as "#rrggbb", or None: #rgb, #rrggbb (with or
/// without alpha) and rgb()/rgba() are what the interfaces send.
pub fn colour(value: &str) -> Option<String> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let full: String = match hex.len() {
            3 | 4 => hex.chars().take(3).flat_map(|c| [c, c]).collect(),
            6 | 8 => hex[..6].to_string(),
            _ => return None,
        };
        return Some(format!("#{full}"));
    }
    let inner = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb("))?.strip_suffix(')')?;
    let nums: Vec<&str> = inner.split([',', ' ', '/']).filter(|s| !s.is_empty()).collect();
    if nums.len() < 3 {
        return None;
    }
    let mut rgb = [0u8; 3];
    for (k, n) in nums[..3].iter().enumerate() {
        let f: f64 = n.trim_end_matches('%').parse().ok()?;
        let f = if n.ends_with('%') { f * 2.55 } else { f };
        rgb[k] = f.round().clamp(0.0, 255.0) as u8;
    }
    Some(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]))
}

pub fn rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    (p(0), p(2), p(4))
}

/// The script every page gets before its own: it gives the page the
/// `webkit.messageHandlers.<app>.postMessage` it already uses on Linux, over
/// the platform's own channel (`send` is a JS expression taking a string).
pub fn bridge_script(app: &str, send: &str) -> String {
    format!(
        r#"(function () {{
  'use strict';
  if (window.webkit && window.webkit.messageHandlers) return;
  var send = {send};
  var handler = Object.freeze({{ postMessage: function (m) {{ send(String(m)); }} }});
  var handlers = {{}};
  handlers[{app:?}] = handler;
  Object.defineProperty(window, 'webkit', {{ value: Object.freeze({{ messageHandlers: Object.freeze(handlers) }}) }});
}})();"#
    )
}

/// Android only: the WebView can't hand a POST body to the app, so Leaflet's
/// `fetch('/put', ...)` goes over the message channel instead. `send` takes a
/// string or an ArrayBuffer; `buffers` says whether ArrayBuffers can be sent.
pub fn put_script(send: &str, buffers: bool) -> String {
    format!(
        r#"(function () {{
  'use strict';
  var send = {send};
  var realFetch = window.fetch.bind(window);
  var waiting = new Map();
  var next = 0;
  Object.defineProperty(window, '__appshellPutDone', {{ value: function (id, token) {{
    var done = waiting.get(id);
    waiting.delete(id);
    if (done) done(token);
  }} }});
  function bytesOf(body) {{
    if (typeof body === 'string') return Promise.resolve(new TextEncoder().encode(body));
    if (body instanceof ArrayBuffer) return Promise.resolve(new Uint8Array(body));
    if (ArrayBuffer.isView(body)) return Promise.resolve(new Uint8Array(body.buffer, body.byteOffset, body.byteLength));
    if (body instanceof Blob) return body.arrayBuffer().then(function (b) {{ return new Uint8Array(b); }});
    return Promise.resolve(new Uint8Array(0));
  }}
  function b64(bytes) {{
    var s = '';
    for (var i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  }}
  window.fetch = function (url, opts) {{
    if (url !== '/put' || !opts || opts.method !== 'POST') return realFetch(url, opts);
    var type = (opts.headers && (opts.headers['Content-Type'] || opts.headers['content-type'])) || 'application/octet-stream';
    return bytesOf(opts.body).then(function (bytes) {{
      var id = ++next;
      var answer = new Promise(function (resolve) {{ waiting.set(id, resolve); }});
      if ({buffers}) {{
        send(JSON.stringify({{ put: id, type: String(type) }}));
        send(bytes.slice().buffer);
      }} else {{
        send(JSON.stringify({{ put: id, type: String(type), b64: b64(bytes) }}));
      }}
      return answer.then(function (token) {{
        return token ? new Response(token, {{ status: 200 }}) : new Response('too large', {{ status: 413 }});
      }});
    }});
  }};
}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_ui() {
        assert_eq!(ui_path("/index.html").as_deref(), Some("index.html"));
        assert_eq!(ui_path("/vendor/pdfjs/pdf.min.mjs?x=1").as_deref(), Some("vendor/pdfjs/pdf.min.mjs"));
        assert_eq!(ui_path("/../secret"), None);
        assert_eq!(ui_path("/a/%2e%2e/%2e%2e/x"), None);
        assert_eq!(ui_path("/C:/x"), None);
        assert_eq!(ui_path("/a\\b"), None);
        assert_eq!(ui_path("/"), None);
    }

    #[test]
    fn colours() {
        assert_eq!(colour("#1d1e22").as_deref(), Some("#1d1e22"));
        assert_eq!(colour(" #FFF ").as_deref(), Some("#ffffff"));
        assert_eq!(colour("rgb(29, 30, 34)").as_deref(), Some("#1d1e22"));
        assert_eq!(colour("red; background: url(x)"), None);
        assert_eq!(colour("#12345"), None);
    }
}
