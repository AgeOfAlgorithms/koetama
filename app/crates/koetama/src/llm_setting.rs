//! The host's LLM for game mods' agents (docs/llm-agents.md), as the window keeps it in settings.json ("llm"): the games
//! it is on for (off unless the host turns it on for a game), the API, address, model and key (sealed: secret.rs), the
//! month's budget, requests a minute, and what was spent each month. The model's price is not kept: it comes from
//! OpenRouter's list (kd_llm::Prices, `prices`), or kd_llm::prices::UNKNOWN for a model the list does not have.
use crate::settings::Settings;
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct LlmSetting {
    /// the games (profile ids) whose agents may use it
    pub games: Vec<String>,
    pub api: kd_llm::Api,
    pub url: String,
    pub model: String,
    /// the key, opened (kept sealed in the settings)
    pub key: String,
    pub budget: f64,
    pub per_minute: u32,
    /// dollars spent, by month ("2026-10")
    pub spent: Map<String, Value>,
    /// OpenRouter's price list, once loaded (not kept in the settings: its own file, kd_llm::Prices::load)
    pub prices: Option<kd_llm::Prices>,
}

impl Default for LlmSetting {
    fn default() -> Self {
        LlmSetting {
            games: Vec::new(),
            api: kd_llm::Api::OpenAi,
            url: String::new(),
            model: String::new(),
            key: String::new(),
            budget: 5.0,
            per_minute: 20,
            spent: Map::new(),
            prices: None,
        }
    }
}

/// OpenRouter's price list as kept on this PC (llm-prices.json in Koetama's folder), fetched again at most once a day
/// (blocking: a thread's work).
pub fn load_prices() -> Option<kd_llm::Prices> {
    let path = kd_common::paths::data_dir().join("llm-prices.json");
    kd_llm::Prices::load(&path, &|url| {
        kd_common::fetch::get_text(url, Some("application/json"), std::time::Duration::from_secs(20)).map_err(|e| e.to_string())
    })
}

/// This month, "YYYY-MM" (UTC; from the clock's days since 1970).
pub fn this_month() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // (Howard Hinnant's civil_from_days)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

impl LlmSetting {
    pub fn load(s: &Settings) -> LlmSetting {
        let d = LlmSetting::default();
        let Some(o) = s.0.get("llm").and_then(Value::as_object) else { return d };
        let num = |k: &str, def: f64| o.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).unwrap_or(def);
        LlmSetting {
            games: o.get("games").and_then(Value::as_array).map_or_else(Vec::new, |a| a.iter().filter_map(|g| g.as_str().map(String::from)).collect()),
            api: o.get("api").and_then(Value::as_str).and_then(kd_llm::Api::from_name).unwrap_or(d.api),
            url: o.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
            model: o.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
            key: o.get("key").and_then(Value::as_str).and_then(crate::secret::open).unwrap_or_default(),
            budget: num("budget", d.budget),
            per_minute: num("per_minute", d.per_minute as f64).clamp(1.0, 600.0) as u32,
            spent: o.get("spent").and_then(Value::as_object).cloned().unwrap_or_default(),
            prices: None,
        }
    }

    pub fn save(&self, s: &mut Settings) {
        s.set(
            "llm",
            json!({
                "games": self.games, "api": self.api.name(), "url": self.url, "model": self.model,
                "key": crate::secret::seal(&self.key), "budget": self.budget, "per_minute": self.per_minute,
                "spent": self.spent,
            }),
        );
    }

    pub fn on_for(&self, game: &str) -> bool {
        self.games.iter().any(|g| g == game)
    }

    pub fn set_on(&mut self, game: &str, on: bool) {
        self.games.retain(|g| g != game);
        if on {
            self.games.push(game.to_string());
        }
    }

    pub fn spent_this_month(&self) -> f64 {
        self.spent.get(&this_month()).and_then(Value::as_f64).unwrap_or(0.0)
    }

    pub fn set_spent(&mut self, x: f64) {
        self.spent.insert(this_month(), json!(x));
    }

    /// The model's price, dollars a million tokens in and out, and the list's id for it (None: not in OpenRouter's
    /// list, or the list is not loaded - counted at kd_llm::prices::UNKNOWN).
    pub fn price(&self) -> ((f64, f64), Option<String>) {
        match self.prices.as_ref().and_then(|p| p.find(self.api, self.url_or_default(), &self.model)) {
            Some((i, o, id)) => ((i, o), Some(id)),
            None => (kd_llm::prices::UNKNOWN, None),
        }
    }

    fn url_or_default(&self) -> &str {
        if self.url.trim().is_empty() {
            self.api.default_url()
        } else {
            self.url.trim()
        }
    }

    /// Ready to use: a model and a key.
    pub fn complete(&self) -> bool {
        !self.model.trim().is_empty() && !self.key.trim().is_empty()
    }

    /// The service's config, for a game it is on for (None: off for it, or not complete).
    pub fn config_for(&self, game: &str) -> Option<kd_llm::Config> {
        self.on_for(game).then(|| self.config()).flatten()
    }

    /// The config as the setting is now, whatever game (None: not complete) - the window's "Try it".
    pub fn config(&self) -> Option<kd_llm::Config> {
        self.complete().then(|| kd_llm::Config {
            api: self.api,
            url: self.url.trim().to_string(),
            model: self.model.trim().to_string(),
            key: self.key.trim().to_string(),
            budget: self.budget,
            price_in: self.price().0 .0,
            price_out: self.price().0 .1,
            per_minute: self.per_minute,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_and_read_back() {
        let mut l = LlmSetting { model: "gpt-x".into(), key: "sk-secret".into(), ..Default::default() };
        l.set_on("dst", true);
        l.set_spent(0.25);
        let mut s = Settings::default();
        l.save(&mut s);
        let text = serde_json::to_string(&s.0).unwrap();
        assert!(!text.contains("sk-secret"), "the key is sealed: {text}");
        let back = LlmSetting::load(&s);
        assert_eq!(back, l);
        assert!(back.config_for("dst").is_some_and(|c| c.key == "sk-secret" && c.price_in == kd_llm::prices::UNKNOWN.0));
        assert_eq!(back.spent_this_month(), 0.25);
        assert!(back.config_for("teardown").is_none(), "off for a game the host did not turn it on for");
        let mut empty = back.clone();
        empty.key.clear();
        assert!(empty.config_for("dst").is_none(), "no key: off");
        assert_eq!(this_month().len(), 7);
    }
}
