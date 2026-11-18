//! The Rust port against Leaflet's Python, on the same files.
//!
//!     python3 tools/formats_oracle.py ../leaflet target/oracle
//!     ORACLE_DIR=target/oracle cargo test -p leaflet-formats --test oracle
//!
//! Without ORACLE_DIR the test says so and passes (CI always sets it).

use leaflet_formats::{self as lf, model::plain_text, zipio::Zip};
use serde_json::Value;
use std::path::PathBuf;

fn members(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = Zip::open(data).expect("zip");
    let names: Vec<String> = z.names().map(str::to_string).collect();
    names.into_iter().map(|n| { let d = z.read(&n, true).unwrap().unwrap(); (n, d) }).collect()
}

#[test]
fn same_as_python() {
    let Ok(dir) = std::env::var("ORACLE_DIR") else {
        eprintln!("ORACLE_DIR isn't set: skipping the comparison with leaflet_formats.py");
        return;
    };
    let dir = PathBuf::from(dir);
    let oracle: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("oracle.json")).unwrap()).unwrap();
    let mut failures = Vec::new();
    let mut checked = 0;
    for (name, want) in oracle.as_object().unwrap() {
        let mut fail = |what: &str, detail: String| failures.push(format!("{name}: {what}: {detail}"));
        if let Some(src) = name.strip_prefix("html:") {
            let _ = src;
            let source = want["source"].as_str().unwrap();
            let back = lf::from_html(source);
            let got = lf::to_html(&back);
            if got != want["rehtml"].as_str().unwrap() {
                fail("from_html -> to_html", format!("\n  python: {}\n  rust:   {got}", want["rehtml"]));
            }
            if plain_text(&back.blocks) != want["plain"].as_str().unwrap() {
                fail("plain text", format!("{:?} vs {:?}", want["plain"], plain_text(&back.blocks)));
            }
            checked += 1;
            continue;
        }
        let data = std::fs::read(dir.join("corpus").join(name)).unwrap();
        let kind = lf::detect(&data);
        if kind != want["kind"].as_str() {
            fail("detect", format!("{:?} vs {:?}", want["kind"], kind));
            continue;
        }
        if let Some(strip) = want.get("strip").and_then(Value::as_str) {
            let expect = std::fs::read(dir.join("out").join(strip)).unwrap();
            if lf::strip_image_metadata(&data) != expect {
                fail("strip_image_metadata", "differs".into());
            }
            let size = lf::images::image_size(&data);
            if [size.0, size.1] != [want["size"][0].as_i64().unwrap(), want["size"][1].as_i64().unwrap()] {
                fail("image_size", format!("{:?} vs {:?}", want["size"], size));
            }
        }
        let Some(kind) = kind.filter(|k| ["docx", "odt", "rtf", "txt"].contains(k)) else { checked += 1; continue };
        let doc = match lf::load(&data, kind) {
            Ok(d) => d,
            Err(e) => {
                if want.get("error").is_none() {
                    fail("load", format!("rust failed: {e}"));
                }
                continue;
            }
        };
        if let Some(e) = want.get("error") {
            fail("load", format!("python failed ({e}) but rust read it"));
            continue;
        }
        let html = lf::to_html(&doc);
        if html != want["html"].as_str().unwrap() {
            fail("to_html", format!("\n  python: {}\n  rust:   {html}", want["html"]));
        }
        let warnings: Vec<&str> = want["warnings"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        if doc.warnings != warnings {
            fail("warnings", format!("{warnings:?} vs {:?}", doc.warnings));
        }
        if plain_text(&doc.blocks) != want["plain"].as_str().unwrap() {
            fail("plain text", format!("{:?} vs {:?}", want["plain"], plain_text(&doc.blocks)));
        }
        let back = lf::from_html(&html);
        if lf::to_html(&back) != want["rehtml"].as_str().unwrap() {
            fail("re-read html", format!("\n  python: {}\n  rust:   {}", want["rehtml"], lf::to_html(&back)));
        }
        for (fname, _) in want["outputs"].as_object().unwrap() {
            let mut parts = fname.rsplitn(3, '.');
            let fmt = parts.next().unwrap();
            let how = parts.next().unwrap();
            let src = if how == "direct" { &doc } else { &back };
            let got = lf::save(src, fmt).unwrap();
            let expect = std::fs::read(dir.join("out").join(fname)).unwrap();
            if fmt == "txt" {
                if got != expect {
                    fail(fname, format!("{:?} vs {:?}", String::from_utf8_lossy(&expect), String::from_utf8_lossy(&got)));
                }
                continue;
            }
            let (a, b) = (members(&expect), members(&got));
            let an: Vec<&str> = a.iter().map(|m| m.0.as_str()).collect();
            let bn: Vec<&str> = b.iter().map(|m| m.0.as_str()).collect();
            if an != bn {
                fail(fname, format!("members {an:?} vs {bn:?}"));
                continue;
            }
            for ((n, x), (_, y)) in a.iter().zip(&b) {
                if x != y {
                    let (xs, ys) = (String::from_utf8_lossy(x), String::from_utf8_lossy(y));
                    let at = xs.chars().zip(ys.chars()).position(|(p, q)| p != q).unwrap_or(xs.len().min(ys.len()));
                    let ctx = |s: &str| s.chars().skip(at.saturating_sub(60)).take(160).collect::<String>();
                    fail(fname, format!("{n} differs at char {at}\n  python: {}\n  rust:   {}", ctx(&xs), ctx(&ys)));
                }
            }
        }
        checked += 1;
    }
    eprintln!("compared {checked} cases with leaflet_formats.py");
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// Damaged files (bytes flipped, cut short) must give an error, never a panic.
#[test]
fn damaged_files_never_panic() {
    let Ok(dir) = std::env::var("ORACLE_DIR") else { return };
    let corpus = PathBuf::from(dir).join("corpus");
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut tried = 0;
    for entry in std::fs::read_dir(corpus).unwrap() {
        let data = std::fs::read(entry.unwrap().path()).unwrap();
        if data.is_empty() {
            continue;
        }
        for round in 0..40 {
            let mut d = data.clone();
            if round % 4 == 0 {
                d.truncate((next() as usize) % d.len());
            } else {
                for _ in 0..1 + round % 8 {
                    let i = (next() as usize) % d.len();
                    d[i] = next() as u8;
                }
            }
            let _ = lf::detect(&d);
            for kind in ["docx", "odt", "rtf", "txt"] {
                if let Ok(doc) = lf::load(&d, kind) {
                    let html = lf::to_html(&doc);
                    let back = lf::from_html(&html);
                    let _ = lf::save(&back, "docx");
                    let _ = lf::save(&doc, "odt");
                }
            }
            let _ = lf::strip_image_metadata(&d);
            let _ = lf::images::image_size(&d);
            tried += 1;
        }
    }
    eprintln!("{tried} damaged files read without a panic");
}

#[test]
fn odd_html_never_panics() {
    for s in ["<", "</", "<a", "<a href", "<a href=", "<a href='", "&", "&#", "&#x", "&#99999999999999;", "&amp", "<p><b><i>x</p>",
        "</p></div>", "<table><tr><td colspan=x>", "<img src=\"data:image/png;base64,\">", "<ul><ul><ul>", "<!--", "<!x", "<?x",
        "<script>", "<style>a", "\u{0}", "<p style=\"text-align\">x</p>", "<li>a</li>"] {
        let d = lf::from_html(s);
        let _ = lf::save(&d, "docx");
        let _ = lf::to_html(&d);
    }
}
