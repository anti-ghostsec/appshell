//! Files on Windows: plain paths, written to a temporary file in the same
//! folder and swapped in, keeping the original's attributes and permissions.

use appshell_core::{FileError, Platform};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct WinFiles {
    pub app: &'static str,
}

/// %LOCALAPPDATA%\<App>: settings only. Local, not roaming, so it isn't
/// copied to other machines with the profile.
pub fn config_dir(app: &str) -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"))?;
    Some(PathBuf::from(base).join(app))
}

fn io_error(e: &std::io::Error) -> FileError {
    match e.kind() {
        std::io::ErrorKind::NotFound => FileError::NotFound,
        std::io::ErrorKind::IsADirectory => FileError::IsDir,
        _ => FileError::Other(describe(e)),
    }
}

/// An OS error the way people read it, without the "(os error 5)".
pub fn describe(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.rfind(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Replace `target` with `tmp`. ReplaceFileW keeps the original's
/// attributes, permissions and creation time; a new file is just renamed.
fn swap_in(tmp: &Path, target: &Path) -> std::io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};
    if target.exists() {
        let (t, n) = (wide(target), wide(tmp));
        let ok = unsafe { ReplaceFileW(PCWSTR(t.as_ptr()), PCWSTR(n.as_ptr()), PCWSTR::null(), REPLACE_FILE_FLAGS(0), None, None) };
        if ok.is_ok() {
            return Ok(());
        }
    }
    std::fs::rename(tmp, target)
}

pub fn write_atomic(target: &Path, data: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let dir = target.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", &appshell_core::token()[..12]));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        swap_in(&tmp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

impl Platform for WinFiles {
    fn read(&self, h: &str, limit: u64) -> Result<Vec<u8>, FileError> {
        let p = Path::new(h);
        let meta = std::fs::metadata(p).map_err(|e| io_error(&e))?;
        if meta.is_dir() {
            return Err(FileError::IsDir);
        }
        if meta.len() > limit {
            return Err(FileError::TooLarge);
        }
        let f = std::fs::File::open(p).map_err(|e| io_error(&e))?;
        let mut data = Vec::with_capacity(meta.len() as usize);
        f.take(limit + 1).read_to_end(&mut data).map_err(|e| io_error(&e))?;
        if data.len() as u64 > limit {
            return Err(FileError::TooLarge);
        }
        Ok(data)
    }

    fn write(&self, h: &str, data: &[u8]) -> Result<(), String> {
        write_atomic(Path::new(h), data).map_err(|e| describe(&e))
    }

    fn mtime(&self, h: &str) -> Option<i64> {
        let t = std::fs::metadata(h).ok()?.modified().ok()?;
        Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64)
    }

    fn exists(&self, h: &str) -> bool {
        std::fs::symlink_metadata(h).is_ok()
    }

    fn name(&self, h: &str) -> String {
        Path::new(h).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| h.to_string())
    }

    fn folder(&self, h: &str) -> String {
        Path::new(h).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn with_extension(&self, h: &str, ext: &str) -> Option<String> {
        Some(format!("{h}.{ext}"))
    }

    fn load_settings(&self) -> Option<Vec<u8>> {
        std::fs::read(config_dir(self.app)?.join("settings.json")).ok()
    }

    fn store_settings(&self, data: &[u8]) {
        if let Some(dir) = config_dir(self.app) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = write_atomic(&dir.join("settings.json"), data);
        }
    }

    fn home(&self) -> String {
        std::env::var("USERPROFILE").unwrap_or_default().replace('\\', "/")
    }

    fn system_dark(&self) -> bool {
        use windows::core::w;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut _),
                Some(&mut size),
            )
        };
        r.is_ok() && value == 0
    }

    fn theme_override(&self) -> Option<String> {
        if self.app == "Leaflet" {
            std::env::var("LEAFLET_THEME").ok().filter(|t| !t.is_empty())
        } else {
            None
        }
    }
}
