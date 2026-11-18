//! Puts each app's interface (ui/, fetched at the pinned commit by the build
//! script or CI) into its executable, and gives the executables a manifest:
//! per-monitor DPI awareness and the common controls dialogs need.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn files(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            files(&p, base, out);
        } else {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, p.canonicalize().unwrap()));
        }
    }
}

fn table(var: &str, out_name: &str) {
    println!("cargo:rerun-if-env-changed={var}");
    let mut list = Vec::new();
    if let Ok(dir) = std::env::var(var) {
        let dir = PathBuf::from(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        files(&dir, &dir, &mut list);
    }
    list.sort();
    let mut src = String::from("/// The interface's files, sorted by path.\npub static FILES: &[(&str, &[u8])] = &[\n");
    for (rel, path) in &list {
        writeln!(src, "    ({rel:?}, include_bytes!({:?})),", path.to_string_lossy()).unwrap();
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join(out_name);
    std::fs::write(out, src).unwrap();
}

fn main() {
    table("APPSHELL_FOLIO_UI", "ui_folio.rs");
    table("APPSHELL_LEAFLET_UI", "ui_leaflet.rs");
    #[cfg(windows)]
    {
        use embed_manifest::manifest::{DpiAwareness, Setting};
        use embed_manifest::{embed_manifest, new_manifest};
        if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
            let m = new_manifest("Appshell")
                .dpi_awareness(DpiAwareness::PerMonitorV2)
                .long_path_aware(Setting::Enabled);
            embed_manifest(m).expect("unable to embed the manifest");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
