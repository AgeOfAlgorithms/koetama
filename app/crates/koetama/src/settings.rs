//! The window's settings (settings.json in Koetama's folder): the game, the microphone and speakers (by name), the
//! volume, whether to look for updates at start, the languages the player speaks, the translation ("translate_into":
//! the language the game's chat is translated into, "" off - the default; "translate_downloads": models download
//! when needed, default true).
use kd_common::paths;
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default)]
pub struct Settings(pub Map<String, Value>);

impl Settings {
    pub fn load() -> Settings {
        std::fs::read_to_string(paths::settings_path())
            .ok()
            // (a byte order mark - Notepad, PowerShell - is not JSON: without this the whole file was dropped)
            .and_then(|s| serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')).ok())
            .and_then(|v| v.as_object().cloned())
            .map(Settings)
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(paths::data_dir());
        if let Ok(s) = serde_json::to_string_pretty(&Value::Object(self.0.clone())) {
            let _ = std::fs::write(paths::settings_path(), s);
        }
    }

    pub fn str(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(|v| v.as_str()).map(String::from)
    }

    pub fn f64(&self, key: &str, default: f64) -> f64 {
        self.0.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn set(&mut self, key: &str, v: impl Into<Value>) {
        self.0.insert(key.to_string(), v.into());
    }
}
