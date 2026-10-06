//! Where Steam keeps a game (engine/steam.py): its libraries, an app's install folder, its Workshop content, and on
//! Linux its Proton prefix (a Windows game's own drive_c: its Documents and AppData). For game modules: Windows and
//! Linux.
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static VDF_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""path"\s+"([^"]+)""#).unwrap());
static INSTALLDIR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""installdir"\s+"([^"]+)""#).unwrap());

/// Steam's folder, or None.
pub fn steam_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        for (hive, key) in
            [(HKEY_CURRENT_USER, r"Software\Valve\Steam"), (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam")]
        {
            for name in ["SteamPath", "InstallPath"] {
                if let Some(p) = win::reg_string(hive, key, name) {
                    if !p.is_empty() && Path::new(&p).is_dir() {
                        return Some(PathBuf::from(normpath(&p)));
                    }
                }
            }
        }
        let p = Path::new(r"C:\Program Files (x86)\Steam");
        p.is_dir().then(|| p.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        let home = kd_common::paths::home();
        for p in [
            home.join(".steam").join("steam"),
            home.join(".local").join("share").join("Steam"),
            home.join(".var").join("app").join("com.valvesoftware.Steam").join(".local").join("share").join("Steam"), // (Flatpak)
            home.join("Library").join("Application Support").join("Steam"), // (macOS)
        ] {
            if p.is_dir() {
                return Some(std::fs::canonicalize(&p).unwrap_or(p));
            }
        }
        None
    }
}

/// Every Steam library folder (libraryfolders.vdf), Steam's own first.
pub fn libraries(root: Option<&Path>) -> Vec<PathBuf> {
    let root = match root.map(Path::to_path_buf).or_else(steam_root) {
        Some(r) if !r.as_os_str().is_empty() => r,
        _ => return Vec::new(),
    };
    let mut out = vec![root.clone()];
    let vdf = root.join("steamapps").join("libraryfolders.vdf");
    let Ok(bytes) = std::fs::read(vdf) else {
        return out;
    };
    let text = String::from_utf8_lossy(&bytes);
    for m in VDF_PATH.captures_iter(&text) {
        let p = PathBuf::from(normpath(&m[1].replace(r"\\", r"\")));
        if p.is_dir() && !out.iter().any(|q| normcase(q) == normcase(&p)) {
            out.push(p);
        }
    }
    out
}

fn manifest(lib: &Path, appid: u32) -> PathBuf {
    lib.join("steamapps").join(format!("appmanifest_{appid}.acf"))
}

/// The library that has app appid installed (its appmanifest), or None.
pub fn app_library(appid: u32, root: Option<&Path>) -> Option<PathBuf> {
    libraries(root).into_iter().find(|lib| manifest(lib, appid).exists())
}

/// The app's install folder (steamapps/common/<installdir>), or None.
pub fn install_dir(appid: u32, root: Option<&Path>) -> Option<PathBuf> {
    let lib = app_library(appid, root)?;
    let bytes = std::fs::read(manifest(&lib, appid)).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let m = INSTALLDIR.captures(&text)?;
    let p = lib.join("steamapps").join("common").join(&m[1]);
    p.is_dir().then_some(p)
}

/// The app's Workshop content folder (subscribed items, one folder each), or None.
pub fn workshop_dir(appid: u32, root: Option<&Path>) -> Option<PathBuf> {
    libraries(root)
        .into_iter()
        .map(|lib| lib.join("steamapps").join("workshop").join("content").join(appid.to_string()))
        .find(|p| p.is_dir())
}

/// Linux: the Windows user folder inside the app's Proton prefix (its Documents, AppData), or None.
pub fn proton_user(appid: u32, root: Option<&Path>) -> Option<PathBuf> {
    libraries(root)
        .into_iter()
        .map(|lib| {
            lib.join("steamapps")
                .join("compatdata")
                .join(appid.to_string())
                .join("pfx")
                .join("drive_c")
                .join("users")
                .join("steamuser")
        })
        .find(|p| p.is_dir())
}

/// This user's Documents folder (Windows: where it really is - OneDrive moves it).
pub fn documents_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(p) = win::known_documents() {
        return p;
    }
    kd_common::paths::home().join("Documents")
}

/// Python's os.path.normpath: one kind of separator (Windows: backslashes), no "." or "x/.." parts, no doubled ones.
fn normpath(p: &str) -> String {
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let s = if cfg!(windows) { p.replace('/', "\\") } else { p.to_string() };
    if s.is_empty() {
        return ".".into();
    }
    // (the part kept as it is: a drive "C:", a share "\\server\share", the leading separators)
    let (mut prefix, rest) = if cfg!(windows) {
        let (drive, rest) = if let Some(unc) = s.strip_prefix(r"\\") {
            let mut it = unc.splitn(3, '\\');
            let (server, share) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            let n = 2 + server.len() + if share.is_empty() && !unc.contains('\\') { 0 } else { 1 + share.len() };
            let n = n.min(s.len());
            (s[..n].to_string(), s[n..].to_string())
        } else if s.len() >= 2 && s.as_bytes()[1] == b':' && s.is_char_boundary(2) {
            (s[..2].to_string(), s[2..].to_string())
        } else {
            (String::new(), s.clone())
        };
        if let Some(r) = rest.strip_prefix('\\') {
            (drive + "\\", r.trim_start_matches('\\').to_string())
        } else {
            (drive, rest)
        }
    } else {
        let lead = s.len() - s.trim_start_matches('/').len();
        let prefix = match lead {
            0 => "",
            2 => "//",
            _ => "/",
        };
        (prefix.to_string(), s[lead..].to_string())
    };
    let rooted = prefix.ends_with(sep);
    let mut comps: Vec<&str> = Vec::new();
    for c in rest.split(sep) {
        match c {
            "" | "." => {}
            ".." => {
                if comps.last().is_some_and(|l| *l != "..") {
                    comps.pop();
                } else if !rooted {
                    comps.push("..");
                }
            }
            _ => comps.push(c),
        }
    }
    prefix.push_str(&comps.join(&sep.to_string()));
    if prefix.is_empty() {
        ".".into()
    } else {
        prefix
    }
}

/// Python's os.path.normcase: Windows compares paths without case and with either separator.
fn normcase(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.to_lowercase().replace('/', "\\")
    } else {
        s.into_owned()
    }
}

#[cfg(windows)]
mod win {
    use std::path::PathBuf;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, RRF_RT_REG_SZ};
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A string value from the registry, or None.
    pub fn reg_string(hive: HKEY, key: &str, name: &str) -> Option<String> {
        let (k, n) = (wide(key), wide(name));
        let mut size: u32 = 0;
        // SAFETY: valid nul-terminated strings; a null buffer asks for the size.
        let r = unsafe {
            RegGetValueW(hive, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size)
        };
        if r != 0 || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: buf holds `size` bytes.
        let r = unsafe {
            RegGetValueW(hive, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
        };
        if r != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// FOLDERID_Documents (where the user's Documents really are), or None.
    pub fn known_documents() -> Option<PathBuf> {
        let mut p: *mut u16 = std::ptr::null_mut();
        // SAFETY: a valid GUID and out pointer; the string it gives is freed with CoTaskMemFree.
        unsafe {
            let hr = SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut p);
            if p.is_null() {
                return None;
            }
            let mut len = 0;
            while *p.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            CoTaskMemFree(p as *const _);
            (hr == 0 && !s.is_empty()).then(|| PathBuf::from(s))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normpath_as_python() {
        if cfg!(windows) {
            assert_eq!(normpath("c:/program files (x86)/steam"), r"c:\program files (x86)\steam");
            assert_eq!(normpath(r"D:\\Games\.\x\..\SteamLibrary\"), r"D:\Games\SteamLibrary");
            assert_eq!(normpath(r"\\server\share\a\..\b"), r"\\server\share\b");
            assert_eq!(normpath(r"C:\.."), r"C:\");
            assert_eq!(normpath(r"a\..\.."), "..");
        } else {
            assert_eq!(normpath("/home//me/./x/../Steam/"), "/home/me/Steam");
            assert_eq!(normpath("//a/b"), "//a/b");
            assert_eq!(normpath("///a"), "/a");
            assert_eq!(normpath("a/../.."), "..");
        }
        assert_eq!(normpath(""), ".");
    }
}
