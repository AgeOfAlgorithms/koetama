//! The test speakers' recorded voices: made once with the Windows computer voices (System.Speech; none on other
//! systems - the dummies are then silent). A profile's test_voices name a voice, a rate and a text; they reach
//! PowerShell only as environment variables, never inside the script (a profile's text cannot run code).
use crate::profile::{Profile, TestVoice};
use crate::teardown;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The voices as wav files in work, named by name(voice); {src: its wav} for those that are there.
pub fn make_in(work: &Path, voices: &[TestVoice], name: &dyn Fn(&TestVoice) -> String) -> HashMap<i64, PathBuf> {
    let mut out = HashMap::new();
    if !cfg!(windows) || voices.is_empty() {
        return out;
    }
    let _ = fs::create_dir_all(work);
    for v in voices {
        let path = work.join(name(v));
        if !path.exists() {
            // (the rate is a number -10..10 - validated; the rest come in as environment variables)
            let ps = format!(
                "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                 try {{ $s.SelectVoice($env:KD_VOICE) }} catch {{}}; $s.Rate = {}; $s.SetOutputToWaveFile($env:KD_WAV); \
                 $s.Speak($env:KD_SAID); $s.Dispose()",
                v.rate.clamp(-10, 10)
            );
            let mut cmd = std::process::Command::new("powershell");
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps]).stdin(std::process::Stdio::null());
            cmd.env("KD_VOICE", &v.voice).env("KD_WAV", &path).env("KD_SAID", &v.text);
            cmd.env_remove("PSModulePath"); // (PowerShell 7's module path breaks Windows PowerShell's own modules)
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // (CREATE_NO_WINDOW)
            }
            match cmd.status() {
                Ok(st) if st.success() => {}
                _ => continue,
            }
        }
        if fs::metadata(&path).is_ok_and(|m| m.len() > 44) {
            out.insert(v.src, path);
        }
    }
    out
}

/// FNV-1a: a short name for a voice's settings (a changed text makes a new file).
fn fnv(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// A profile's test voices. Teardown's (built in): voice<src>.wav in teardown::work_dir() (the files the Python made);
/// a profile file's: work_dir()/profiles/<id>/voice<src>-<hash of its settings>.wav.
pub fn for_profile(p: &Profile, builtin: bool) -> HashMap<i64, PathBuf> {
    if builtin {
        return make_in(&teardown::work_dir(), &p.test_voices, &|v| format!("voice{}.wav", v.src));
    }
    let dir = teardown::work_dir().join("profiles").join(&p.id);
    make_in(&dir, &p.test_voices, &|v| format!("voice{}-{:08x}.wav", v.src, fnv(&format!("{}|{}|{}", v.voice, v.rate, v.text))))
}
