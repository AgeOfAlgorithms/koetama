//! The two API styles an agent's LLM may speak: OpenAI-compatible chat completions (OpenAI, OpenRouter, Groq, ... any
//! server with POST <url>/chat/completions) and Anthropic's messages (POST <url>/v1/messages). The conversation is
//! kept provider-neutral (Turn); body() makes each one's request, parse_reply() reads its answer, complete() does the
//! call.
use kd_common::feed::AgentFunction;
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
    /// this month's limit, tokens in and out (what was used is the Ledger's)
    pub limit: crate::Tokens,
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

}

/// A function the LLM asks the game to run: its call id (the game answers with it), name, arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// One turn of an agent's conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Turn {
    /// what was said near the agent (its lines, as Koetama wrote them)
    User(String),
    /// what the agent said, and the functions it called
    Assistant { text: String, calls: Vec<Call> },
    /// the game's results for those calls: (call id, result)
    Results(Vec<(String, String)>),
}

/// What the LLM answered: its words, its function calls, and the tokens it counted - in (the whole prompt; of them,
/// those read from the provider's cache and those written to it) and out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<Call>,
    pub tokens_in: u64,
    pub tokens_cached: u64,
    pub tokens_cache_write: u64,
    pub tokens_out: u64,
}

impl Reply {
    /// What it used: every token of the prompt in (cached ones too, as the providers count them), the reply's out.
    pub fn tokens(&self) -> crate::Tokens {
        crate::Tokens::new(self.tokens_in, self.tokens_out)
    }
}

/// A response's `usage` into the reply: OpenAI's prompt_tokens (cached: prompt_tokens_details.cached_tokens, or
/// DeepSeek's prompt_cache_hit_tokens) and completion_tokens; Anthropic's input_tokens, cache_read_input_tokens and
/// cache_creation_input_tokens (all three the prompt) and output_tokens. Counts it does not give are left as they are.
fn take_usage(api: Api, u: &Value, r: &mut Reply) {
    let n = |k: &str| u.get(k).and_then(Value::as_u64);
    match api {
        Api::OpenAi => {
            if let Some(x) = n("prompt_tokens") {
                r.tokens_in = x;
            }
            if let Some(x) = u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64).or(n("prompt_cache_hit_tokens")) {
                r.tokens_cached = x;
            }
            if let Some(x) = n("completion_tokens") {
                r.tokens_out = x;
            }
        }
        Api::Anthropic => {
            let (fresh, read, write) = (n("input_tokens"), n("cache_read_input_tokens"), n("cache_creation_input_tokens"));
            if fresh.is_some() || read.is_some() || write.is_some() {
                r.tokens_in = fresh.unwrap_or(0) + read.unwrap_or(0) + write.unwrap_or(0);
                r.tokens_cached = read.unwrap_or(0);
                r.tokens_cache_write = write.unwrap_or(0);
            }
            if let Some(x) = n("output_tokens") {
                r.tokens_out = x.max(r.tokens_out);
            }
        }
    }
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
pub fn body(api: Api, url: &str, model: &str, system: &str, functions: &[AgentFunction], turns: &[Turn], max_tokens: u32) -> Value {
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
    let mut r = Reply::default();
    if let Some(u) = v.get("usage") {
        take_usage(api, u, &mut r);
    }
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
            Ok(Reply { text: m.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string(), calls, ..r })
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
            Ok(Reply { text: text.trim().to_string(), calls, ..r })
        }
    }
}

/// One request to the host's LLM (blocking: the service runs it on a thread of its own).
pub fn complete(cfg: &Config, system: &str, functions: &[AgentFunction], turns: &[Turn], max_tokens: u32, timeout: Duration) -> Result<Reply, String> {
    request(cfg, system, functions, turns, max_tokens, timeout, None)
}

/// As complete, the reply streamed: `on_text` gets all the words so far each time more come (several times a second
/// or more often; the caller thins them out). Function calls come whole in the Reply, as complete's.
pub fn complete_streaming(
    cfg: &Config,
    system: &str,
    functions: &[AgentFunction],
    turns: &[Turn],
    max_tokens: u32,
    timeout: Duration,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply, String> {
    request(cfg, system, functions, turns, max_tokens, timeout, Some(on_text))
}

fn request(
    cfg: &Config,
    system: &str,
    functions: &[AgentFunction],
    turns: &[Turn],
    max_tokens: u32,
    timeout: Duration,
    mut on_text: Option<&mut dyn FnMut(&str)>,
) -> Result<Reply, String> {
    let url = endpoint(cfg.api, cfg.url());
    let local = ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| url.starts_with(p));
    let agent = ureq::Agent::config_builder()
        .https_only(!local)
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut b = body(cfg.api, cfg.url(), &cfg.model, system, functions, turns, max_tokens);
    if on_text.is_some() {
        b["stream"] = json!(true);
        let asks = ["api.openai.com", "api.deepseek.com", "openrouter.ai"];
        if cfg.api == Api::OpenAi && asks.iter().any(|h| cfg.url().contains(h)) {
            // (these send the tokens counted - cached ones too - only when asked; other OpenAI-style servers may refuse
            // the field: theirs are counted from the characters)
            b["stream_options"] = json!({"include_usage": true});
        }
    }
    let send = |b: &Value| -> Result<ureq::http::Response<ureq::Body>, String> {
        let mut req = agent.post(&url).header("Content-Type", "application/json").header("User-Agent", "Koetama");
        req = match cfg.api {
            Api::OpenAi => req.header("Authorization", &format!("Bearer {}", cfg.key)),
            Api::Anthropic => req.header("x-api-key", &cfg.key).header("anthropic-version", "2023-06-01"),
        };
        req.send(b.to_string()).map_err(|e| format!("the LLM could not be reached: {e}"))
    };
    let whole = |resp: ureq::http::Response<ureq::Body>| -> Result<(u16, Value), String> {
        let status = resp.status().as_u16();
        let text = resp.into_body().read_to_string().map_err(|e| format!("the LLM's answer could not be read: {e}"))?;
        let v: Value = serde_json::from_str(&text).map_err(|_| format!("the LLM answered {status}, not JSON"))?;
        Ok((status, v))
    };
    let mut resp = send(&b)?;
    if resp.status().as_u16() >= 400 {
        // (an error comes whole, streamed or not)
        let (status, v) = whole(resp)?;
        if cfg.api == Api::OpenAi && status == 400 && b.get("reasoning_effort").is_none() && wants_no_reasoning(&v) {
            // (a reasoning model refuses functions on chat completions unless its reasoning is off - and a character's
            //  reply wants no long thinking anyway; a model that knows no reasoning_effort is never sent it)
            b["reasoning_effort"] = json!("none");
            resp = send(&b)?;
        } else {
            return Err(match parse_reply(cfg.api, &v) {
                Err(e) => format!("the LLM answered {status}: {e}"),
                Ok(_) => format!("the LLM answered {status}"),
            });
        }
    }
    let status = resp.status().as_u16();
    if status >= 400 {
        let (status, v) = whole(resp)?;
        return Err(match parse_reply(cfg.api, &v) {
            Err(e) => format!("the LLM answered {status}: {e}"),
            Ok(_) => format!("the LLM answered {status}"),
        });
    }
    match on_text.as_mut() {
        None => {
            let (_, v) = whole(resp)?;
            parse_reply(cfg.api, &v).map_err(|e| format!("the LLM answered {status}: {e}"))
        }
        Some(f) => {
            let mut r = read_stream(cfg.api, std::io::BufReader::new(resp.into_body().into_reader()), *f)?;
            if r.tokens_in == 0 && r.tokens_out == 0 {
                // (a server that streams no counts: about 4 characters a token, for the budget)
                let sent: usize = system.len() + turns.iter().map(turn_len).sum::<usize>();
                r.tokens_in = (sent / 4) as u64;
                r.tokens_out = (r.text.len() / 4 + r.calls.iter().map(|c| c.args.to_string().len() / 4).sum::<usize>()) as u64;
            }
            Ok(r)
        }
    }
}

/// About how long a turn is (characters): the prompt's size, for a cost before the answer counts it.
pub fn turn_len(t: &Turn) -> usize {
    match t {
        Turn::User(s) => s.len(),
        Turn::Assistant { text, calls } => text.len() + calls.iter().map(|c| c.args.to_string().len()).sum::<usize>(),
        Turn::Results(rs) => rs.iter().map(|(_, r)| r.len()).sum(),
    }
}

/// A streamed answer (server-sent events: `data: {...}` lines), read as it comes: the words to `on_text` (all of them
/// so far, each time more come), the calls put together from their pieces, the tokens counted.
pub fn read_stream(api: Api, r: impl std::io::BufRead, on_text: &mut dyn FnMut(&str)) -> Result<Reply, String> {
    let mut reply = Reply::default();
    // (calls by their index in the answer: id, name, the arguments' JSON as it comes)
    let mut parts: Vec<(usize, String, String, String)> = Vec::new();
    for line in r.lines() {
        let line = line.map_err(|e| format!("the LLM's answer broke off: {e}"))?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
        if let Some(e) = v.get("error") {
            return Err(e.get("message").and_then(Value::as_str).map_or_else(|| e.to_string(), str::to_string));
        }
        let before = reply.text.len();
        match api {
            Api::OpenAi => {
                if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                    take_usage(api, u, &mut reply);
                }
                let Some(d) = v.pointer("/choices/0/delta") else { continue };
                if let Some(t) = d.get("content").and_then(Value::as_str) {
                    reply.text.push_str(t);
                }
                for c in d.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    let i = c.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let at = match parts.iter().position(|p| p.0 == i) {
                        Some(at) => at,
                        None => {
                            parts.push((i, String::new(), String::new(), String::new()));
                            parts.len() - 1
                        }
                    };
                    let p = &mut parts[at];
                    if let Some(id) = c.get("id").and_then(Value::as_str) {
                        p.1.push_str(id);
                    }
                    if let Some(name) = c.pointer("/function/name").and_then(Value::as_str) {
                        p.2.push_str(name);
                    }
                    if let Some(a) = c.pointer("/function/arguments").and_then(Value::as_str) {
                        p.3.push_str(a);
                    }
                }
            }
            Api::Anthropic => match v.get("type").and_then(Value::as_str) {
                Some("message_start") => {
                    if let Some(u) = v.pointer("/message/usage") {
                        take_usage(api, u, &mut reply);
                    }
                }
                Some("content_block_start") => {
                    let i = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    if let Some(b) = v.get("content_block").filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")) {
                        let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                        parts.push((i, s("id"), s("name"), String::new()));
                    }
                }
                Some("content_block_delta") => {
                    let i = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    match v.pointer("/delta/type").and_then(Value::as_str) {
                        Some("text_delta") => reply.text.push_str(v.pointer("/delta/text").and_then(Value::as_str).unwrap_or("")),
                        Some("input_json_delta") => {
                            if let Some(p) = parts.iter_mut().find(|p| p.0 == i) {
                                p.3.push_str(v.pointer("/delta/partial_json").and_then(Value::as_str).unwrap_or(""));
                            }
                        }
                        _ => {}
                    }
                }
                Some("message_delta") => {
                    if let Some(u) = v.get("usage") {
                        take_usage(api, u, &mut reply);
                    }
                }
                Some("message_stop") => break,
                _ => {}
            },
        }
        if reply.text.len() > before {
            on_text(reply.text.trim_start());
        }
    }
    reply.text = reply.text.trim().to_string();
    parts.sort_by_key(|p| p.0);
    reply.calls = parts
        .into_iter()
        .map(|(_, id, name, args)| Call {
            id,
            name,
            args: if args.trim().is_empty() { json!({}) } else { serde_json::from_str(&args).unwrap_or(json!(args)) },
        })
        .collect();
    Ok(reply)
}

/// The API refused the request over its reasoning (OpenAI: "... set reasoning_effort to 'none'").
fn wants_no_reasoning(v: &Value) -> bool {
    v.pointer("/error/message").and_then(Value::as_str).is_some_and(|m| m.contains("reasoning_effort"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade() -> Vec<AgentFunction> {
        vec![AgentFunction {
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
    }

    /// The prompt's cached tokens, as each API counts them (they are tokens in all the same).
    #[test]
    fn cached_tokens_are_counted() {
        let deepseek = json!({"choices": [{"message": {"content": "Hi"}}],
            "usage": {"prompt_tokens": 2000, "prompt_cache_hit_tokens": 1800, "prompt_cache_miss_tokens": 200, "completion_tokens": 50}});
        let r = parse_reply(Api::OpenAi, &deepseek).unwrap();
        assert_eq!((r.tokens_in, r.tokens_cached, r.tokens_out), (2000, 1800, 50));
        let openai = json!({"choices": [{"message": {"content": "Hi"}}],
            "usage": {"prompt_tokens": 2000, "prompt_tokens_details": {"cached_tokens": 1536}, "completion_tokens": 50}});
        assert_eq!(parse_reply(Api::OpenAi, &openai).unwrap().tokens_cached, 1536);
        let anthropic = json!({"content": [{"type": "text", "text": "Hi"}],
            "usage": {"input_tokens": 20, "cache_read_input_tokens": 1500, "cache_creation_input_tokens": 480, "output_tokens": 50}});
        let a = parse_reply(Api::Anthropic, &anthropic).unwrap();
        assert_eq!((a.tokens_in, a.tokens_cached, a.tokens_cache_write), (2000, 1500, 480));
        assert_eq!(r.tokens(), crate::Tokens::new(2000, 50), "cached tokens are tokens in");
        let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n\
                      data: {\"choices\":[],\"usage\":{\"prompt_tokens\":900,\"prompt_cache_hit_tokens\":640,\"completion_tokens\":3}}\n\n\
                      data: [DONE]\n";
        let s = read_stream(Api::OpenAi, stream.as_bytes(), &mut |_| {}).unwrap();
        assert_eq!((s.tokens_in, s.tokens_cached, s.tokens_out), (900, 640, 3));
    }

    #[test]
    fn streams_read() {
        let o = "data: {\"choices\":[{\"delta\":{\"content\":\"Twel\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"ve.\",\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"offer_trade\",\"arguments\":\"{\\\"item\\\":\"}}]}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"hat\\\"}\"}}]}}]}\n\n\
                 data: {\"choices\":[],\"usage\":{\"prompt_tokens\":50,\"completion_tokens\":9}}\n\n\
                 data: [DONE]\n";
        let mut seen = Vec::new();
        let r = read_stream(Api::OpenAi, o.as_bytes(), &mut |t| seen.push(t.to_string())).unwrap();
        assert_eq!(seen, ["Twel", "Twelve."]);
        assert_eq!((r.text.as_str(), r.tokens_in, r.tokens_out), ("Twelve.", 50, 9));
        assert_eq!(r.calls, vec![Call { id: "c1".into(), name: "offer_trade".into(), args: json!({"item": "hat"}) }]);
        let a = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":30,\"output_tokens\":1}}}\n\n\
                 data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
                 data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Oink\"}}\n\n\
                 data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"!\"}}\n\n\
                 data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"f\",\"input\":{}}}\n\n\
                 data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\": 1}\"}}\n\n\
                 data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":12}}\n\n\
                 data: {\"type\":\"message_stop\"}\n";
        let mut seen = Vec::new();
        let r = read_stream(Api::Anthropic, a.as_bytes(), &mut |t| seen.push(t.to_string())).unwrap();
        assert_eq!(seen, ["Oink", "Oink!"]);
        assert_eq!((r.text.as_str(), r.tokens_in, r.tokens_out), ("Oink!", 30, 12));
        assert_eq!(r.calls[0].args, json!({"a": 1}));
        let e = "data: {\"error\":{\"message\":\"overloaded\"}}\n";
        assert_eq!(read_stream(Api::OpenAi, e.as_bytes(), &mut |_| {}), Err("overloaded".into()));
    }
}
