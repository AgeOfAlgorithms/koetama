//! What a model costs, for the host's budget: OpenRouter's public list of models and their prices
//! (https://openrouter.ai/api/v1/models - no key; it lists the providers' own prices for the models it resells), kept on
//! this PC and fetched again at most once a day. A model it does not list is counted at UNKNOWN (dear on purpose: the
//! budget stops early rather than late).
use crate::api::Api;
use serde_json::Value;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const LIST_URL: &str = "https://openrouter.ai/api/v1/models";
/// the kept list is fetched again when it is older than this
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// dollars a million tokens in and out for a model the list does not have
pub const UNKNOWN: (f64, f64) = (3.0, 15.0);

/// A model in the list: its id ("openai/gpt-4o-mini"), its price in dollars a million tokens - in, out, and the
/// prompt's tokens read from the provider's cache and written to it (when the list says) - and when it came out (Unix
/// seconds; 0 not said).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Price {
    pub id: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub created: i64,
}

impl Price {
    /// A model the list does not have: UNKNOWN, no id.
    pub fn unknown() -> Price {
        Price { input: UNKNOWN.0, output: UNKNOWN.1, ..Default::default() }
    }

    /// The service's config, counting with this price.
    pub fn apply(&self, cfg: &mut crate::Config) {
        (cfg.price_in, cfg.price_out, cfg.price_cached, cfg.price_cache_write) = (self.input, self.output, self.cache_read, self.cache_write);
    }
}

/// OpenRouter's models, in its order (the newest first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prices(pub Vec<Price>);

/// A price as OpenRouter writes it: dollars a token, as a string ("0.0000001") - a million of them, to a millionth of
/// a dollar (0.1, not 0.09999999999999999).
fn per_million(v: Option<&Value>) -> Option<f64> {
    let x: f64 = match v? {
        Value::String(s) => s.trim().parse().ok()?,
        n => n.as_f64()?,
    };
    (x.is_finite() && x >= 0.0).then_some((x * 1e12).round() / 1e6)
}

/// "claude-haiku-5.5" and "claude-haiku-5-5" alike: lower case, dots as dashes.
fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace('.', "-")
}

/// The list's own name for the company behind an address (its ids start "<company>/").
fn company(api: Api, url: &str) -> Option<&'static str> {
    if api == Api::Anthropic {
        return Some("anthropic");
    }
    let u = url.to_ascii_lowercase();
    [
        ("api.openai.com", "openai"),
        ("anthropic.com", "anthropic"),
        ("deepseek.com", "deepseek"),
        ("moonshot", "moonshotai"),
        ("mistral.ai", "mistralai"),
        ("x.ai", "x-ai"),
        ("googleapis.com", "google"),
    ]
    .iter()
    .find(|(host, _)| u.contains(host))
    .map(|(_, c)| *c)
}

impl Prices {
    pub fn parse(json: &str) -> Result<Prices, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| format!("the price list is not JSON: {e}"))?;
        let data = v.get("data").and_then(Value::as_array).ok_or("the price list has no \"data\"")?;
        let mut out = Vec::new();
        for m in data {
            let (Some(id), Some(p)) = (m.get("id").and_then(Value::as_str), m.get("pricing")) else { continue };
            if let (Some(input), Some(output)) = (per_million(p.get("prompt")), per_million(p.get("completion"))) {
                out.push(Price {
                    id: id.to_string(),
                    input,
                    output,
                    cache_read: per_million(p.get("input_cache_read")),
                    cache_write: per_million(p.get("input_cache_write")),
                    created: m.get("created").and_then(Value::as_i64).unwrap_or(0),
                });
            }
        }
        if out.is_empty() {
            return Err("the price list has no prices".into());
        }
        Ok(Prices(out))
    }

    /// The model as `model` is named at that address: OpenRouter's own ids ("openai/gpt-4o-mini"), or a company's
    /// ("gpt-4o-mini" at api.openai.com, "claude-haiku-5-5" at Anthropic's). Batch prices (":batch") are not a
    /// request's. Else a company's alias for its newest model ("deepseek-flash" at api.deepseek.com - DeepSeek's
    /// DeepSeek-V4.1-Flash-1017): that company's NEWEST model with every word of the name and only version numbers
    /// besides ("deepseek-v4.1-flash-1017"; not "deepseek-flash-vision").
    pub fn find(&self, api: Api, url: &str, model: &str) -> Option<Price> {
        let want = norm(model);
        let live = |m: &&Price| !m.id.contains(':');
        let hit = |id: &str| self.0.iter().filter(live).find(|m| norm(&m.id) == id);
        hit(&want)
            .or_else(|| company(api, url).and_then(|c| hit(&format!("{c}/{want}"))))
            .or_else(|| self.0.iter().filter(live).find(|m| m.id.rsplit('/').next().map(norm).as_deref() == Some(want.as_str())))
            .or_else(|| company(api, url).and_then(|c| self.closest(c, model)))
            .cloned()
    }

    /// The company's newest model whose name has every word of `model` and otherwise only version numbers ("v4.1",
    /// "1017").
    fn closest(&self, company: &str, model: &str) -> Option<&Price> {
        let words = |s: &str| -> Vec<String> { s.trim().to_ascii_lowercase().split('-').filter(|w| !w.is_empty()).map(String::from).collect() };
        let version = |w: &str| {
            let w = w.strip_prefix('v').unwrap_or(w);
            w.starts_with(|c: char| c.is_ascii_digit()) && w.chars().all(|c| c.is_ascii_digit() || c == '.')
        };
        let want = words(model);
        let prefix = format!("{company}/");
        let mut best: Option<&Price> = None;
        for m in self.0.iter().filter(|m| !m.id.contains(':')) {
            let Some(name) = m.id.to_ascii_lowercase().strip_prefix(&prefix).map(String::from) else { continue };
            let have = words(&name);
            let mut rest = have.clone();
            if !want.iter().all(|w| rest.iter().position(|h| h == w).map(|i| rest.remove(i)).is_some()) {
                continue;
            }
            if rest.is_empty() || !rest.iter().all(|w| version(w)) {
                continue;
            }
            // (the newest; the same day: the first in the list)
            if best.is_none_or(|b| m.created > b.created) {
                best = Some(m);
            }
        }
        best
    }

    /// The kept list at `path` if it is younger than MAX_AGE; else fetched (with `get`) and kept; else the kept one
    /// however old (offline); else None.
    pub fn load(path: &Path, get: &dyn Fn(&str) -> Result<String, String>) -> Option<Prices> {
        let kept = std::fs::read_to_string(path).ok().and_then(|s| Prices::parse(&s).ok());
        let fresh = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < MAX_AGE);
        if kept.is_some() && fresh {
            return kept;
        }
        match get(LIST_URL).and_then(|s| Prices::parse(&s).map(|p| (p, s))) {
            Ok((p, s)) => {
                if let Some(d) = path.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(path, s);
                Some(p)
            }
            Err(_) => kept,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Prices {
        Prices::parse(
            r#"{"data": [
                {"id": "openai/gpt-6-luna", "pricing": {"prompt": "0.0000001", "completion": "0.0000005"}},
                {"id": "openai/gpt-6-luna:batch", "pricing": {"prompt": "0.00000005", "completion": "0.00000025"}},
                {"id": "anthropic/claude-haiku-5.5", "pricing": {"prompt": "0.000001", "completion": "0.000005"}},
                {"id": "deepseek/deepseek-chat", "pricing": {"prompt": "0.00000027", "completion": "0.0000011"}},
                {"id": "free/thing", "pricing": {"prompt": "0", "completion": "0"}},
                {"id": "no-price"}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn models_found_as_each_address_names_them() {
        let p = list();
        let near = |a: Option<Price>, i: f64, o: f64| a.is_some_and(|p| (p.input - i).abs() < 1e-9 && (p.output - o).abs() < 1e-9);
        assert!(near(p.find(Api::OpenAi, "https://api.openai.com/v1", "gpt-6-luna"), 0.1, 0.5), "not the batch price");
        assert!(near(p.find(Api::OpenAi, "https://openrouter.ai/api/v1", "openai/gpt-6-luna"), 0.1, 0.5));
        assert!(near(p.find(Api::Anthropic, "", "claude-haiku-5-5"), 1.0, 5.0), "dashes for dots");
        assert!(near(p.find(Api::OpenAi, "https://api.deepseek.com", "deepseek-chat"), 0.27, 1.1));
        assert!(near(p.find(Api::OpenAi, "https://example.com/v1", "deepseek-chat"), 0.27, 1.1), "any company's, by its name");
        assert!(p.find(Api::OpenAi, "https://api.openai.com/v1", "gpt-9").is_none());
        assert_eq!(p.0.len(), 5, "one without a price left out");
    }

    /// A company's alias for its newest model: the newest of its own with the alias's words and version numbers -
    /// deepseek-flash is DeepSeek-V4.1-Flash-1017 (DeepSeek's price page, 2026-10-25), at its cache price too.
    #[test]
    fn an_alias_is_its_newest_model() {
        let p = Prices::parse(
            r#"{"data": [
                {"id": "deepseek/deepseek-v4.1-flash-1017", "created": 1792225624, "pricing": {"prompt": "0.00000015", "completion": "0.0000006", "input_cache_read": "0.000000003"}},
                {"id": "deepseek/deepseek-v4.1-flash", "created": 1789021285, "pricing": {"prompt": "0.00000002", "completion": "0.000001"}},
                {"id": "deepseek/deepseek-v4.1-flash:batch", "created": 1789021285, "pricing": {"prompt": "0.00000001", "completion": "0.0000001"}},
                {"id": "deepseek/deepseek-v4-flash-vision-exp", "created": 1787311563, "pricing": {"prompt": "0.0000002", "completion": "0.0000006"}},
                {"id": "deepseek/deepseek-v4-pro-0813", "created": 1786549364, "pricing": {"prompt": "0.00000066", "completion": "0.00000198"}},
                {"id": "deepseek/deepseek-v4-pro", "created": 1777000679, "pricing": {"prompt": "0.0000002", "completion": "0.0000004"}},
                {"id": "deepseek/deepseek-v4-flash", "created": 1777000666, "pricing": {"prompt": "0.00000009", "completion": "0.00000018"}},
                {"id": "openai/gpt-6-sol", "pricing": {"prompt": "0.000002", "completion": "0.00001"}}
            ]}"#,
        )
        .unwrap();
        let id = |url: &str, m: &str| p.find(Api::OpenAi, url, m).map(|p| p.id);
        let flash = p.find(Api::OpenAi, "https://api.deepseek.com", "deepseek-flash").unwrap();
        assert_eq!((flash.id.as_str(), flash.input, flash.output, flash.cache_read), ("deepseek/deepseek-v4.1-flash-1017", 0.15, 0.6, Some(0.003)), "the newest");
        assert_eq!(id("https://api.deepseek.com", "deepseek-pro").as_deref(), Some("deepseek/deepseek-v4-pro-0813"));
        assert_eq!(id("https://api.deepseek.com", "deepseek-v4-flash").as_deref(), Some("deepseek/deepseek-v4-flash"), "an exact id first");
        assert_eq!(id("https://example.com/v1", "deepseek-flash"), None, "only at the company's own address");
        assert_eq!(id("https://api.openai.com/v1", "gpt-6"), None, "\"sol\" is not a version");
        assert_eq!(id("https://api.deepseek.com", "deepseek-flash-vision"), None, "\"exp\" is not a version");
    }

    #[test]
    fn kept_and_fetched_once_a_day() {
        let dir = std::env::temp_dir().join(format!("kd-llm-prices-{}", std::process::id()));
        let path = dir.join("prices.json");
        let _ = std::fs::remove_dir_all(&dir);
        let body = r#"{"data": [{"id": "a/b", "pricing": {"prompt": "0.000001", "completion": "0.000002"}}]}"#;
        let calls = std::cell::Cell::new(0);
        let get = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(body.to_string())
        };
        assert_eq!(Prices::load(&path, &get).map(|p| p.0.len()), Some(1));
        assert_eq!(Prices::load(&path, &get).map(|p| p.0.len()), Some(1));
        assert_eq!(calls.get(), 1, "the kept list is fresh: not fetched again");
        let offline = |_: &str| Err("offline".to_string());
        let _ = std::fs::write(&path, body);
        assert!(Prices::load(&path, &offline).is_some(), "the kept one when it cannot be fetched");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(Prices::load(&path, &offline).is_none());
    }
}
