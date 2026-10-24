//! The two API styles an NPC's LLM may speak: OpenAI-compatible chat completions (OpenAI, OpenRouter, Groq, ... any
//! server with POST <url>/chat/completions) and Anthropic's messages (POST <url>/v1/messages). The conversation is
//! kept provider-neutral (Turn); body() makes each one's request, parse_reply() reads its answer, complete() does the
//! call.
use kd_common::feed::NpcFunction;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Api {
    #[default]
    OpenAi,
    Anthropic,
}

impl Api {
    pub fn name(self) -> &'static str {
        match self {
            Api::OpenAi => "openai",
            Api::Anthropic => "anthropic",
        }
    }

    pub fn from_name(s: &str) -> Option<Api> {
        match s {
            "openai" => Some(Api::OpenAi),
            "anthropic" => Some(Api::Anthropic),
            _ => None,
        }
    }

    /// Where it is when the host gives no URL.
    pub fn default_url(self) -> &'static str {
        match self {
            Api::OpenAi => "https://api.openai.com/v1",
            Api::Anthropic => "https://api.anthropic.com",
        }
    }
}

/// The host's LLM: which API, where, the model and key, and the limits Koetama keeps (not the mod's to change).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub api: Api,
    /// "" for Api::default_url
    pub url: String,
    pub model: String,
    pub key: String,
    /// this month's budget and what was spent of it so far (US dollars)
    pub budget: f64,
    pub spent: f64,
    /// the model's price, dollars a million tokens in (the prompt) and out (the reply)
    pub price_in: f64,
    pub price_out: f64,
    /// requests a minute, at most
    pub per_minute: u32,
}

impl Config {
    pub fn url(&self) -> &str {
        if self.url.trim().is_empty() {
            self.api.default_url()
        } else {
            self.url.trim()
        }
    }

    /// What a reply's tokens cost (dollars).
    pub fn cost(&self, r: &Reply) -> f64 {
        (r.tokens_in as f64 * self.price_in + r.tokens_out as f64 * self.price_out) / 1e6
    }
}

/// A function the LLM asks the game to run: its call id (the game answers with it), name, arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// One turn of an NPC's conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Turn {
    /// what was said near the NPC (its lines, as Koetama wrote them)
    User(String),
    /// what the NPC said, and the functions it called
    Assistant { text: String, calls: Vec<Call> },
    /// the game's results for those calls: (call id, result)
    Results(Vec<(String, String)>),
}

/// What the LLM answered: its words, its function calls, and the tokens it counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<Call>,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// The API's address for a request.
pub fn endpoint(api: Api, url: &str) -> String {
    let base = url.trim_end_matches('/');
    match api {
        Api::OpenAi => format!("{base}/chat/completions"),
        Api::Anthropic => format!("{base}/v1/messages"),
    }
}

/// The turns as the APIs accept them: a call without results gets "(not run)" ones (a request that failed after
/// it), and turns of the same side next to each other are made one (a failed request leaves its turn out, so two
/// players' turns can meet).
fn mended(turns: &[Turn]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for (i, t) in turns.iter().enumerate() {
        out.push(t.clone());
        if let Turn::Assistant { calls, .. } = t {
            if !calls.is_empty() && !matches!(turns.get(i + 1), Some(Turn::Results(_))) {
                out.push(Turn::Results(calls.iter().map(|c| (c.id.clone(), "(not run)".to_string())).collect()));
            }
        }
    }
    let mut merged: Vec<Turn> = Vec::new();
    for t in out {
        match (merged.last_mut(), t) {
            (Some(Turn::User(a)), Turn::User(b)) => {
                a.push('\n');
                a.push_str(&b);
            }
            (_, t) => merged.push(t),
        }
    }
    merged
}

/// The request body for `api`: the system prompt, the functions, the conversation, the longest reply.
pub fn body(api: Api, url: &str, model: &str, system: &str, functions: &[NpcFunction], turns: &[Turn], max_tokens: u32) -> Value {
    let turns = mended(turns);
    match api {
        Api::OpenAi => {
            let mut messages = vec![json!({"role": "system", "content": system})];
            for t in &turns {
                match t {
                    Turn::User(s) => messages.push(json!({"role": "user", "content": s})),
                    Turn::Assistant { text, calls } => {
                        let mut m = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                        if !calls.is_empty() {
                            m["tool_calls"] = calls
                                .iter()
                                .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args.to_string()}}))
                                .collect();
                        }
                        messages.push(m);
                    }
                    Turn::Results(rs) => {
                        for (id, r) in rs {
                            messages.push(json!({"role": "tool", "tool_call_id": id, "content": r}));
                        }
                    }
                }
            }
            // (OpenAI's own newer models refuse max_tokens; other OpenAI-style servers know only max_tokens)
            let limit = if url.contains("api.openai.com") { "max_completion_tokens" } else { "max_tokens" };
            let mut b = json!({"model": model, "messages": messages});
            b[limit] = json!(max_tokens);
            if !functions.is_empty() {
                b["tools"] = functions
                    .iter()
                    .map(|f| json!({"type": "function", "function": {"name": f.name, "description": f.description, "parameters": f.parameters}}))
                    .collect();
            }
            b
        }
        Api::Anthropic => {
            let mut messages: Vec<Value> = Vec::new();
            for t in &turns {
                match t {
                    Turn::User(s) => messages.push(json!({"role": "user", "content": s})),
                    Turn::Assistant { text, calls } => {
                        let mut content: Vec<Value> = Vec::new();
                        if !text.is_empty() || calls.is_empty() {
                            content.push(json!({"type": "text", "text": if text.is_empty() { "..." } else { text }}));
                        }
                        for c in calls {
                            content.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.args}));
                        }
                        messages.push(json!({"role": "assistant", "content": content}));
                    }
                    Turn::Results(rs) => {
                        let content: Vec<Value> = rs
                            .iter()
                            .map(|(id, r)| json!({"type": "tool_result", "tool_use_id": id, "content": r}))
                            .collect();
                        messages.push(json!({"role": "user", "content": content}));
                    }
                }
            }
            // (the persona and functions are the same every time: cached, they cost a fraction after the first)
            let mut b = json!({
                "model": model,
                "max_tokens": max_tokens,
                "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
                "messages": messages,
            });
            if !functions.is_empty() {
                b["tools"] = functions
                    .iter()
                    .map(|f| json!({"name": f.name, "description": f.description, "input_schema": f.parameters}))
                    .collect();
            }
            b
        }
    }
}

/// What the API's answer says: the words, the calls, the tokens - or its error.
pub fn parse_reply(api: Api, v: &Value) -> Result<Reply, String> {
    if let Some(e) = v.get("error") {
        let msg = e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.to_string());
        return Err(msg);
    }
    let tokens = |k: &str| v.get("usage").and_then(|u| u.get(k)).and_then(Value::as_u64).unwrap_or(0);
    match api {
        Api::OpenAi => {
            let m = v.pointer("/choices/0/message").ok_or("the answer has no choices[0].message")?;
            let mut calls = Vec::new();
            for c in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let args = c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
                calls.push(Call {
                    id: c.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: c.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string(),
                    args: serde_json::from_str(args).unwrap_or_else(|_| json!(args)),
                });
            }
            Ok(Reply {
                text: m.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                calls,
                tokens_in: tokens("prompt_tokens"),
                tokens_out: tokens("completion_tokens"),
            })
        }
        Api::Anthropic => {
            let content = v.get("content").and_then(Value::as_array).ok_or("the answer has no content")?;
            let (mut text, mut calls) = (String::new(), Vec::new());
            for b in content {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => text.push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
                    Some("tool_use") => calls.push(Call {
                        id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                        name: b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                        args: b.get("input").cloned().unwrap_or(json!({})),
                    }),
                    _ => {}
                }
            }
            // (cached prompt tokens are counted apart: they are still tokens in)
            let tokens_in = tokens("input_tokens") + tokens("cache_read_input_tokens") + tokens("cache_creation_input_tokens");
            Ok(Reply { text: text.trim().to_string(), calls, tokens_in, tokens_out: tokens("output_tokens") })
        }
    }
}

/// One request to the host's LLM (blocking: the service runs it on a thread of its own).
pub fn complete(cfg: &Config, system: &str, functions: &[NpcFunction], turns: &[Turn], max_tokens: u32, timeout: Duration) -> Result<Reply, String> {
    let url = endpoint(cfg.api, cfg.url());
    let local = ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| url.starts_with(p));
    let agent = ureq::Agent::config_builder()
        .https_only(!local)
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let b = body(cfg.api, cfg.url(), &cfg.model, system, functions, turns, max_tokens);
    let mut req = agent.post(&url).header("Content-Type", "application/json").header("User-Agent", "Koetama");
    req = match cfg.api {
        Api::OpenAi => req.header("Authorization", &format!("Bearer {}", cfg.key)),
        Api::Anthropic => req.header("x-api-key", &cfg.key).header("anthropic-version", "2023-06-01"),
    };
    let resp = req.send(b.to_string()).map_err(|e| format!("the LLM could not be reached: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().map_err(|e| format!("the LLM's answer could not be read: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("the LLM answered {status}, not JSON"))?;
    match parse_reply(cfg.api, &v) {
        Err(e) => Err(format!("the LLM answered {status}: {e}")),
        Ok(_) if status >= 400 => Err(format!("the LLM answered {status}")),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade() -> Vec<NpcFunction> {
        vec![NpcFunction {
            name: "offer_trade".into(),
            description: "Offer an item".into(),
            parameters: json!({"type": "object", "properties": {"item": {"type": "string"}}}),
        }]
    }

    fn talk() -> Vec<Turn> {
        vec![
            Turn::User("Wendy: 10 meat for a helmet?".into()),
            Turn::Assistant {
                text: "Twelve.".into(),
                calls: vec![Call { id: "c1".into(), name: "offer_trade".into(), args: json!({"item": "helmet"}) }],
            },
            Turn::Results(vec![("c1".into(), "ok".into())]),
            Turn::Assistant { text: "Deal.".into(), calls: vec![] },
            Turn::User("Wendy: thanks".into()),
            Turn::User("Wes: (mimes)".into()),
        ]
    }

    #[test]
    fn openai_requests() {
        let b = body(Api::OpenAi, "https://api.openai.com/v1", "m", "You are a pig.", &trade(), &talk(), 300);
        assert_eq!(b["max_completion_tokens"], 300, "OpenAI's own: max_completion_tokens");
        assert_eq!(b["messages"][0], json!({"role": "system", "content": "You are a pig."}));
        assert_eq!(b["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"item\":\"helmet\"}");
        assert_eq!(b["messages"][3], json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}));
        assert_eq!(b["messages"][5], json!({"role": "user", "content": "Wendy: thanks\nWes: (mimes)"}), "two user turns made one");
        assert_eq!(b["tools"][0]["function"]["name"], "offer_trade");
        let other = body(Api::OpenAi, "https://openrouter.ai/api/v1", "m", "s", &[], &talk(), 300);
        assert_eq!(other["max_tokens"], 300);
        assert!(other.get("tools").is_none());
        assert_eq!(endpoint(Api::OpenAi, "https://openrouter.ai/api/v1/"), "https://openrouter.ai/api/v1/chat/completions");
    }

    #[test]
    fn anthropic_requests() {
        let b = body(Api::Anthropic, "https://api.anthropic.com", "m", "You are a pig.", &trade(), &talk(), 300);
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["messages"][1]["content"][1], json!({"type": "tool_use", "id": "c1", "name": "offer_trade", "input": {"item": "helmet"}}));
        assert_eq!(b["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(b["messages"].as_array().unwrap().len(), 5, "the two last user turns made one");
        assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(endpoint(Api::Anthropic, "https://api.anthropic.com"), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn a_call_left_without_results_is_mended() {
        let turns = vec![
            Turn::User("a".into()),
            Turn::Assistant { text: String::new(), calls: vec![Call { id: "c1".into(), name: "f".into(), args: json!({}) }] },
            Turn::User("b".into()),
        ];
        let b = body(Api::OpenAi, "x", "m", "s", &[], &turns, 10);
        assert_eq!(b["messages"][3], json!({"role": "tool", "tool_call_id": "c1", "content": "(not run)"}));
        let a = body(Api::Anthropic, "x", "m", "s", &[], &turns, 10);
        assert_eq!(a["messages"][1]["content"][0]["type"], "tool_use", "no empty text block before it");
        assert_eq!(a["messages"][2]["content"][0]["content"], "(not run)");
    }

    #[test]
    fn replies_read() {
        let o = json!({"choices": [{"message": {"content": " Twelve. ", "tool_calls": [
            {"id": "c9", "type": "function", "function": {"name": "offer_trade", "arguments": "{\"item\":\"helmet\"}"}}]}}],
            "usage": {"prompt_tokens": 120, "completion_tokens": 30}});
        let r = parse_reply(Api::OpenAi, &o).unwrap();
        assert_eq!((r.text.as_str(), r.tokens_in, r.tokens_out), ("Twelve.", 120, 30));
        assert_eq!(r.calls[0].args, json!({"item": "helmet"}));
        let a = json!({"content": [{"type": "text", "text": "Twelve."}, {"type": "tool_use", "id": "t1", "name": "offer_trade", "input": {"item": "helmet"}}],
            "usage": {"input_tokens": 20, "cache_read_input_tokens": 100, "output_tokens": 30}});
        let r = parse_reply(Api::Anthropic, &a).unwrap();
        assert_eq!((r.calls[0].id.as_str(), r.tokens_in), ("t1", 120));
        assert_eq!(parse_reply(Api::Anthropic, &json!({"type": "error", "error": {"message": "overloaded"}})), Err("overloaded".into()));
        let cfg = Config { price_in: 1.0, price_out: 5.0, ..Default::default() };
        assert!((cfg.cost(&Reply { tokens_in: 1_000_000, tokens_out: 200_000, ..Default::default() }) - 2.0).abs() < 1e-9);
    }
}
