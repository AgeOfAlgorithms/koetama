//! Picking a file (a game mod profile to add): Windows' own Open dialog; on Linux zenity or kdialog when there.
//! Blocking: call it from a thread (the window keeps drawing).
use std::path::PathBuf;

/// The file the player picked, None if they cancelled (or there is no dialog here).
#[cfg(windows)]
pub fn pick_profile() -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let filter: Vec<u16> = "Game mod profiles (*.json)\0*.json\0All files\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "Add a game mod profile\0".encode_utf16().collect();
    let mut buf = vec![0u16; 4096];
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

#[cfg(not(windows))]
pub fn pick_profile() -> Option<PathBuf> {
    let tries: [(&str, &[&str]); 2] = [
        ("zenity", &["--file-selection", "--title=Add a game mod profile", "--file-filter=*.json"]),
        ("kdialog", &["--getopenfilename", ".", "*.json"]),
    ];
    for (cmd, args) in tries {
        if let Ok(out) = std::process::Command::new(cmd).args(args).output() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path));
        }
    }
    None
}
