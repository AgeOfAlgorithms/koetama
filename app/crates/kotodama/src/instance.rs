//! One Kotodama at a time: two would fight over the game's files.

/// False if another Kotodama already runs. The lock lives as long as the program.
#[cfg(windows)]
pub fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = format!("Local\\{}\0", kd_common::paths::APP_NAME).encode_utf16().collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        // (the handle stays open until the program ends: that is the lock)
        !h.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

#[cfg(not(windows))]
pub fn single_instance() -> bool {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::fs::File> = OnceLock::new();
    let dir = kd_common::paths::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let Ok(f) = std::fs::File::create(dir.join("lock")) else { return true };
    match f.try_lock() {
        Ok(()) => {
            let _ = LOCK.set(f);
            true
        }
        Err(_) => false,
    }
}
