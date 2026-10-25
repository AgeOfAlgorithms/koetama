//! The host's LLM for game mods' agents (docs/llm-agents.md), as the window keeps it in settings.json ("llm"): the games
//! it is on for (off unless the host turns it on for a game), the API, address, model and key (sealed: secret.rs), the
//! month's limit in tokens in and out, requests a minute, and the tokens used each month. Tokens, not dollars (the
//! user, 2026-10-25): a provider's bill can only be guessed at, its token counts are exact. (An older "budget" and
//! "spent" in dollars are let go.)
use crate::settings::Settings;
use kd_llm::Tokens;
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
    /// tokens a month, in and out
    pub limit: Tokens,
    pub per_minute: u32,
    /// tokens used, by month ("2026-10": [in, out])
    pub used: Map<String, Value>,
}

/// The month's limit unless the host sets one: about 2,000 replies of an agent with a long history (~2,300 tokens in,
/// ~100 out each).
pub const LIMIT: Tokens = Tokens { input: 5_000_000, output: 500_000 };

impl Default for LlmSetting {
    fn default() -> Self {
        LlmSetting {
            games: Vec::new(),
            api: kd_llm::Api::OpenAi,
            url: String::new(),
            model: String::new(),
            key: String::new(),
            limit: LIMIT,
            per_minute: 20,
            used: Map::new(),
        }
    }
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
        let limit = o.get("limit").and_then(Value::as_array).and_then(|a| Some(Tokens::new(a.first()?.as_u64()?, a.get(1)?.as_u64()?)));
        LlmSetting {
            games: o.get("games").and_then(Value::as_array).map_or_else(Vec::new, |a| a.iter().filter_map(|g| g.as_str().map(String::from)).collect()),
            api: o.get("api").and_then(Value::as_str).and_then(kd_llm::Api::from_name).unwrap_or(d.api),
            url: o.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
            model: o.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
            key: o.get("key").and_then(Value::as_str).and_then(crate::secret::open).unwrap_or_default(),
            limit: limit.unwrap_or(d.limit),
            per_minute: num("per_minute", d.per_minute as f64).clamp(1.0, 600.0) as u32,
            used: o.get("used").and_then(Value::as_object).cloned().unwrap_or_default(),
        }
    }

    pub fn save(&self, s: &mut Settings) {
        s.set(
            "llm",
            json!({
                "games": self.games, "api": self.api.name(), "url": self.url, "model": self.model,
                "key": crate::secret::seal(&self.key), "limit": [self.limit.input, self.limit.output],
                "per_minute": self.per_minute, "used": self.used,
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

    pub fn used_this_month(&self) -> Tokens {
        let m = self.used.get(&this_month()).and_then(Value::as_array);
        let n = |i: usize| m.and_then(|a| a.get(i)).and_then(Value::as_u64).unwrap_or(0);
        Tokens::new(n(0), n(1))
    }

    pub fn set_used(&mut self, t: Tokens) {
        self.used.insert(this_month(), json!([t.input, t.output]));
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
            limit: self.limit,
            per_minute: self.per_minute,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_and_read_back() {
        let mut l = LlmSetting { model: "gpt-x".into(), key: "sk-secret".into(), limit: Tokens::new(1_000, 100), ..Default::default() };
        l.set_on("dst", true);
        l.set_used(Tokens::new(250, 25));
        let mut s = Settings::default();
        l.save(&mut s);
        let text = serde_json::to_string(&s.0).unwrap();
        assert!(!text.contains("sk-secret"), "the key is sealed: {text}");
        let back = LlmSetting::load(&s);
        assert_eq!(back, l);
        assert!(back.config_for("dst").is_some_and(|c| c.key == "sk-secret" && c.limit == Tokens::new(1_000, 100)));
        assert_eq!(back.used_this_month(), Tokens::new(250, 25));
        assert!(back.config_for("teardown").is_none(), "off for a game the host did not turn it on for");
        let mut empty = back.clone();
        empty.key.clear();
        assert!(empty.config_for("dst").is_none(), "no key: off");
        assert_eq!(this_month().len(), 7);
    }

    /// An older setting in dollars: its limit is the default, its dollars are let go.
    #[test]
    fn dollars_are_let_go() {
        let mut s = Settings::default();
        s.set("llm", json!({"model": "m", "budget": 5.0, "spent": {"2026-10": 0.85}, "price_in": 1.0}));
        let l = LlmSetting::load(&s);
        assert_eq!((l.limit, l.used_this_month()), (LIMIT, Tokens::default()));
        l.save(&mut s);
        let text = serde_json::to_string(&s.0).unwrap();
        assert!(!text.contains("budget") && !text.contains("spent") && !text.contains("price"), "{text}");
    }
}
