//! A secret kept in the settings (the agents' LLM key): on Windows sealed with the user's own Windows protection
//! (DPAPI: only this Windows account on this PC can open it - "dpapi:<hex>"); elsewhere only hex ("plain:<hex>": the
//! settings file is the user's own).

/// The secret as it goes into the settings ("" for none).
pub fn seal(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    #[cfg(windows)]
    if let Some(b) = dpapi::protect(s.as_bytes()) {
        return format!("dpapi:{}", hex(&b));
    }
    format!("plain:{}", hex(s.as_bytes()))
}

/// The secret from the settings (None: none, or it cannot be opened here - another PC, another account).
pub fn open(stored: &str) -> Option<String> {
    if let Some(h) = stored.strip_prefix("plain:") {
        return String::from_utf8(unhex(h)?).ok();
    }
    #[cfg(windows)]
    if let Some(h) = stored.strip_prefix("dpapi:") {
        return String::from_utf8(dpapi::unprotect(&unhex(h)?)?).ok();
    }
    None
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(h: &str) -> Option<Vec<u8>> {
    if !h.len().is_multiple_of(2) {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(h.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn run(data: &[u8], seal: bool) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // (no description, no extra entropy, no prompt: the current user's key)
        let ok = unsafe {
            if seal {
                CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        Some(v)
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, true)
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_and_opened() {
        let s = seal("sk-test-123");
        assert!(!s.contains("sk-test"), "never in the clear: {s}");
        #[cfg(windows)]
        assert!(s.starts_with("dpapi:"));
        assert_eq!(open(&s).as_deref(), Some("sk-test-123"));
        assert_eq!(seal(""), "");
        assert_eq!(open(""), None);
        assert_eq!(open("dpapi:zz"), None);
        assert_eq!(open("plain:736b"), Some("sk".into()));
    }
}
