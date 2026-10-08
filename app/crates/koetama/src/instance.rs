//! One Koetama at a time: two would fight over the game's files.

/// False if another Koetama already runs. The lock lives as long as the program.
#[cfg(windows)]
pub fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = format!("Local\\{}\0", kd_common::paths::APP_NAME)
        .encode_utf16()
        .collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        // (the handle stays open until the program ends, or release(): that is the lock)
        HANDLE.store(h as usize, std::sync::atomic::Ordering::SeqCst);
        !h.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

#[cfg(windows)]
static HANDLE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(not(windows))]
static LOCK_FILE: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

#[cfg(not(windows))]
pub fn single_instance() -> bool {
    let dir = kd_common::paths::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let Ok(f) = std::fs::File::create(dir.join("lock")) else {
        return true;
    };
    match f.try_lock() {
        Ok(()) => {
            let _ = LOCK_FILE.set(f);
            true
        }
        Err(_) => false,
    }
}

/// Let go of the lock (a new copy of the program is about to take over: the window's renderer fallback).
#[cfg(windows)]
pub fn release() {
    use windows_sys::Win32::Foundation::CloseHandle;
    let h = HANDLE.swap(0, std::sync::atomic::Ordering::SeqCst);
    if h != 0 {
        unsafe {
            CloseHandle(h as _);
        }
    }
}

#[cfg(not(windows))]
pub fn release() {
    if let Some(f) = LOCK_FILE.get() {
        let _ = f.unlock();
    }
}
