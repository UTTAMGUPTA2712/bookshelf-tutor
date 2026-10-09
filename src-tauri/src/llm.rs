//! Provider-agnostic LLM access: presets, saved provider configs, roles, live model lists,
//! streaming chat, and JSON output with validation + one repair round.
//!
//! Two wire adapters cover every built-in provider: Ollama's native API and the
//! OpenAI-compatible chat completions API.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::secrets;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    #[serde(rename = "type")]
    pub provider_type: String,
    pub label: String,
    pub adapter: String,
    pub base_url: String,
    pub needs_key: bool,
    pub local: bool,
    pub docs_url: String,
    pub note: String,
}

pub fn presets() -> &'static [Preset] {
    static PRESETS: std::sync::OnceLock<Vec<Preset>> = std::sync::OnceLock::new();
    PRESETS.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/providers.json")).expect("resources/providers.json is valid")
    })
}

fn preset(provider_type: &str) -> Option<&'static Preset> {
    presets().iter().find(|p| p.provider_type == provider_type)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptions {
    /// Ollama context window. Ollama's default is too small for chapter excerpts.
    pub num_ctx: Option<u32>,
    /// Ollama CPU threads. Defaults to half the cores so the computer stays usable.
    pub num_thread: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub provider_type: String,
    pub label: String,
    pub base_url: String,
    pub enabled_models: Vec<String>,
    pub options: ProviderOptions,
    pub adapter: String,
    pub needs_key: bool,
    /// Whether THIS machine's keychain holds a key for it.
    pub has_key: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    pub id: Option<String>,
    pub provider_type: String,
    pub label: String,
    pub base_url: String,
    #[serde(default)]
    pub enabled_models: Vec<String>,
    #[serde(default)]
    pub options: ProviderOptions,
    /// `None` = leave the stored key alone, `Some("")` = remove it.
    pub api_key: Option<String>,
}

fn row_to_config(r: &rusqlite::Row) -> rusqlite::Result<ProviderConfig> {
    let provider_type: String = r.get(1)?;
    let p = preset(&provider_type);
    Ok(ProviderConfig {
        id: r.get(0)?,
        label: r.get(2)?,
        base_url: r.get(3)?,
        enabled_models: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        options: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        adapter: p.map(|p| p.adapter.clone()).unwrap_or_else(|| "openai".into()),
        needs_key: p.map(|p| p.needs_key).unwrap_or(false),
        has_key: false,
        provider_type,
    })
}

pub fn list_providers(conn: &Connection) -> AppResult<Vec<ProviderConfig>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_type, label, base_url, enabled_models, options FROM providers
         WHERE deleted = 0 ORDER BY created_at",
    )?;
    let mut out = stmt.query_map([], row_to_config)?.collect::<Result<Vec<_>, _>>()?;
    for p in &mut out {
        p.has_key = secrets::get_key(&p.id).ok().flatten().is_some();
    }
    Ok(out)
}

pub fn get_provider(conn: &Connection, id: &str) -> AppResult<ProviderConfig> {
    conn.query_row(
        "SELECT id, provider_type, label, base_url, enabled_models, options FROM providers WHERE id = ?1 AND deleted = 0",
        [id],
        row_to_config,
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound("Provider not found".into()))
}

pub fn save_provider(conn: &Connection, input: &ProviderInput, device_id: &str) -> AppResult<String> {
    let base_url = input.base_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err(AppError::Invalid("Base URL is required".into()));
    }
    let id = input.id.clone().unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let ts = now();
    conn.execute(
        "INSERT INTO providers(id, provider_type, label, base_url, enabled_models, options, created_at, updated_at, device_id, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, 0)
         ON CONFLICT(id) DO UPDATE SET provider_type = excluded.provider_type, label = excluded.label,
             base_url = excluded.base_url, enabled_models = excluded.enabled_models, options = excluded.options,
             updated_at = excluded.updated_at, device_id = excluded.device_id, deleted = 0",
        params![
            id,
            input.provider_type,
            input.label.trim(),
            base_url,
            serde_json::to_string(&input.enabled_models)?,
            serde_json::to_string(&input.options)?,
            ts,
            device_id
        ],
    )?;
    match input.api_key.as_deref().map(str::trim) {
        Some("") => secrets::delete_key(&id)?,
        Some(k) => secrets::set_key(&id, k)?,
        None => {}
    }
    Ok(id)
}

pub fn delete_provider(conn: &Connection, id: &str, device_id: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE providers SET deleted = 1, updated_at = ?2, device_id = ?3 WHERE id = ?1",
        params![id, now(), device_id],
    )?;
    secrets::delete_key(id)
}

// ---------- roles ----------

pub const ROLES: &[(&str, &str)] = &[
    ("question", "Writing questions"),
    ("grade", "Grading & reviews"),
    ("interviewer", "Interviewer's voice (optional — uses the grading model if empty)"),
    ("teacher", "Teaching & summaries (optional — uses the question model if empty)"),
];

/// Runs on this machine (Ollama or a localhost server) rather than in the cloud.
pub fn is_local(p: &ProviderConfig) -> bool {
    p.adapter == "ollama" || ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"].iter().any(|h| p.base_url.contains(h))
}

/// "7.6B" / "qwen2.5-coder:1.5b" / "smollm:135m" → billions of parameters.
pub fn param_billions(s: &str) -> Option<f32> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?i)(?:^|[^0-9.])(\d+(?:\.\d+)?)\s*([bm])(?:$|[^a-z])").expect("regex"));
    re.captures_iter(s).last().and_then(|c| {
        let n: f32 = c[1].parse().ok()?;
        Some(if c[2].eq_ignore_ascii_case("m") { n / 1000.0 } else { n })
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoleLink {
    pub provider_id: String,
    pub model: String,
}

pub fn get_roles(conn: &Connection) -> AppResult<HashMap<String, Vec<RoleLink>>> {
    let mut stmt = conn.prepare("SELECT role, chain FROM roles")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = HashMap::new();
    for row in rows {
        let (role, chain) = row?;
        out.insert(role, serde_json::from_str(&chain).unwrap_or_default());
    }
    Ok(out)
}

pub fn set_role(conn: &Connection, role: &str, chain: &[RoleLink], device_id: &str) -> AppResult<()> {
    if !ROLES.iter().any(|(r, _)| *r == role) {
        return Err(AppError::Invalid(format!("Unknown role {role}")));
    }
    conn.execute(
        "INSERT INTO roles(role, chain, updated_at, device_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(role) DO UPDATE SET chain = excluded.chain, updated_at = excluded.updated_at, device_id = excluded.device_id",
        params![role, serde_json::to_string(chain)?, now(), device_id],
    )?;
    Ok(())
}

/// Everything needed to call one model.
#[derive(Debug, Clone)]
pub struct Target {
    pub provider: ProviderConfig,
    pub key: Option<String>,
    pub model: String,
}

impl Target {
    pub fn describe(&self) -> Value {
        json!({ "providerId": self.provider.id, "provider": self.provider.label, "model": self.model })
    }
}

/// The usable links of a role's chain on this machine, in order.
pub fn resolve_role(conn: &Connection, role: &str) -> AppResult<Vec<Target>> {
    let label = ROLES.iter().find(|(r, _)| *r == role).map(|(_, l)| *l).unwrap_or(role);
    let chain = get_roles(conn)?.remove(role).unwrap_or_default();
    let mut targets = Vec::new();
    let mut problems = Vec::new();
    for link in chain {
        let Ok(provider) = get_provider(conn, &link.provider_id) else {
            problems.push(format!("{} (provider removed)", link.model));
            continue;
        };
        let key = secrets::get_key(&provider.id)?;
        if provider.needs_key && key.is_none() {
            problems.push(format!("{} needs an API key on this computer", provider.label));
            continue;
        }
        targets.push(Target { provider, key, model: link.model });
    }
    if targets.is_empty() {
        let why = if problems.is_empty() { String::new() } else { format!(" ({})", problems.join("; ")) };
        return Err(AppError::Invalid(format!(
            "No usable model for \u{201c}{label}\u{201d}{why}. Open Settings \u{2192} AI models to choose one."
        )));
    }
    Ok(targets)
}

// ---------- HTTP ----------

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(300))
        .user_agent("BookshelfTutor/0.1")
        .build()
        .expect("http client")
}

fn auth(req: reqwest::RequestBuilder, p: &ProviderConfig, key: Option<&str>) -> reqwest::RequestBuilder {
    let mut req = req;
    if let Some(k) = key {
        req = req.bearer_auth(k);
        if p.provider_type == "anthropic" {
            req = req.header("x-api-key", k).header("anthropic-version", "2023-06-01");
        }
    }
    if p.provider_type == "openrouter" {
        req = req.header("X-Title", "Bookshelf Tutor");
    }
    req
}

async fn check_status(resp: reqwest::Response, what: &str) -> AppResult<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("error"))
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str().map(String::from))
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    Err(match status.as_u16() {
        401 | 403 => AppError::Auth(format!("{what}: the provider rejected the API key ({status}). {detail}")),
        429 => AppError::RateLimited(format!("{what}: rate limited or out of credit ({status}). {detail}")),
        404 => AppError::Provider(format!("{what}: not found ({status}). {detail}")),
        _ => AppError::Provider(format!("{what}: {status}. {detail}")),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: Option<String>,
    pub context_length: Option<u64>,
    /// USD per million tokens, when the provider publishes it (OpenRouter).
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    pub size_bytes: Option<u64>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
}

const NON_CHAT_HINTS: &[&str] = &[
    "embed", "whisper", "tts", "dall-e", "moderation", "transcribe", "realtime", "image", "audio", "rerank", "guard",
];

pub async fn list_models(http: &reqwest::Client, p: &ProviderConfig, key: Option<&str>) -> AppResult<Vec<ModelInfo>> {
    let mut models = if p.adapter == "ollama" {
        let resp = http.get(format!("{}/api/tags", p.base_url)).send().await.map_err(|e| {
            AppError::Network(format!("Ollama isn't reachable at {} — is it running? ({e})", p.base_url))
        })?;
        let v: Value = check_status(resp, "List models").await?.json().await?;
        v["models"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|m| ModelInfo {
                id: m["name"].as_str().unwrap_or_default().to_string(),
                name: None,
                context_length: None,
                prompt_price: None,
                completion_price: None,
                size_bytes: m["size"].as_u64(),
                parameter_size: m.pointer("/details/parameter_size").and_then(|v| v.as_str()).map(String::from),
                quantization: m.pointer("/details/quantization_level").and_then(|v| v.as_str()).map(String::from),
            })
            .filter(|m| !m.id.is_empty())
            .collect::<Vec<_>>()
    } else {
        let resp = auth(http.get(format!("{}/models", p.base_url)), p, key).send().await?;
        let v: Value = check_status(resp, "List models").await?.json().await?;
        let price = |m: &Value, k: &str| -> Option<f64> {
            m.pointer(&format!("/pricing/{k}"))
                .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()))
                .map(|per_token| per_token * 1_000_000.0)
        };
        v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|m| ModelInfo {
                id: m["id"].as_str().unwrap_or_default().to_string(),
                name: m["name"].as_str().or_else(|| m["display_name"].as_str()).map(String::from),
                context_length: m["context_length"].as_u64().or_else(|| m["context_window"].as_u64()),
                prompt_price: price(&m, "prompt"),
                completion_price: price(&m, "completion"),
                size_bytes: None,
                parameter_size: None,
                quantization: None,
            })
            .filter(|m| !m.id.is_empty())
            .collect::<Vec<_>>()
    };
    models.retain(|m| {
        let id = m.id.to_lowercase();
        !NON_CHAT_HINTS.iter().any(|h| id.contains(h))
    });
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

// ---------- chat ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self {
        Self { role: "system".into(), content: c.into() }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self { role: "user".into(), content: c.into() }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: c.into() }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

pub async fn chat_stream(
    http: &reqwest::Client,
    t: &Target,
    messages: &[Message],
    json_mode: bool,
    max_tokens: u32,
    on_token: &mut (dyn FnMut(&str) + Send),
) -> AppResult<(String, Usage)> {
    let temperature = t.provider.options.temperature.unwrap_or(0.4);
    if t.provider.adapter == "ollama" {
        let mut body = json!({
            "model": t.model,
            "messages": messages,
            "stream": true,
            "options": {
                "num_ctx": t.provider.options.num_ctx.unwrap_or(8192),
                "temperature": temperature,
                "num_predict": max_tokens,
                "num_thread": t.provider.options.num_thread.unwrap_or_else(default_threads),
            },
        });
        if json_mode {
            body["format"] = json!("json");
        }
        // Thinking models spend minutes reasoning on a CPU before answering; our tasks don't need it.
        if ollama_can_think(http, &t.provider.base_url, &t.model).await {
            body["think"] = if t.model.starts_with("gpt-oss") { json!("low") } else { json!(false) };
        }
        let resp = http
            .post(format!("{}/api/chat", t.provider.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Network(format!("Ollama isn't reachable at {} ({e})", t.provider.base_url)))?;
        let resp = check_status(resp, &format!("{} / {}", t.provider.label, t.model)).await?;
        let mut text = String::new();
        let mut usage = Usage::default();
        let mut buf = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
                if let Some(err) = v["error"].as_str() {
                    return Err(AppError::Provider(err.to_string()));
                }
                if let Some(c) = v.pointer("/message/content").and_then(|c| c.as_str()) {
                    if !c.is_empty() {
                        text.push_str(c);
                        on_token(c);
                        if is_runaway(&text) {
                            return Ok((text, usage));
                        }
                    }
                }
                if v["done"].as_bool() == Some(true) {
                    usage.prompt_tokens = v["prompt_eval_count"].as_u64();
                    usage.completion_tokens = v["eval_count"].as_u64();
                }
            }
        }
        return Ok((text, usage));
    }

    // OpenAI-compatible
    let mut use_json_format = json_mode;
    loop {
        let mut body = json!({
            "model": t.model,
            "messages": messages,
            "stream": true,
            "temperature": temperature,
            "max_tokens": max_tokens,
        });
        if use_json_format {
            body["response_format"] = json!({ "type": "json_object" });
        }
        let resp = auth(http.post(format!("{}/chat/completions", t.provider.base_url)), &t.provider, t.key.as_deref())
            .json(&body)
            .send()
            .await?;
        // Some providers/models reject response_format; retry once without it.
        if use_json_format && resp.status().as_u16() == 400 {
            use_json_format = false;
            continue;
        }
        let resp = check_status(resp, &format!("{} / {}", t.provider.label, t.model)).await?;
        let mut text = String::new();
        let mut usage = Usage::default();
        let mut buf = String::new();
        let mut stream = resp.bytes_stream();
        'outer: while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(pos) = buf.find('\n') {
                let line: String = buf.drain(..=pos).collect();
                let line = line.trim();
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    break 'outer;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
                if let Some(err) = v.pointer("/error/message").and_then(|m| m.as_str()) {
                    return Err(AppError::Provider(err.to_string()));
                }
                if let Some(c) = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
                    if !c.is_empty() {
                        text.push_str(c);
                        on_token(c);
                        if is_runaway(&text) {
                            break 'outer;
                        }
                    }
                }
                if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                    usage.prompt_tokens = u["prompt_tokens"].as_u64();
                    usage.completion_tokens = u["completion_tokens"].as_u64();
                }
            }
        }
        return Ok((text, usage));
    }
}

fn default_threads() -> u32 {
    let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    (cores / 2).max(2)
}

/// Whether an Ollama model reports the "thinking" capability (cached per model).
async fn ollama_can_think(http: &reqwest::Client, base_url: &str, model: &str) -> bool {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, bool>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = format!("{base_url}|{model}");
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return *v;
    }
    let can = match http.post(format!("{base_url}/api/show")).json(&json!({ "model": model })).send().await {
        Ok(r) if r.status().is_success() => r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["capabilities"].as_array().map(|c| c.iter().any(|x| x == "thinking")))
            .unwrap_or(false),
        _ => return false,
    };
    cache.lock().unwrap().insert(key, can);
    can
}

/// Small models in JSON mode sometimes emit whitespace forever; stop early instead of waiting.
fn is_runaway(text: &str) -> bool {
    text.len() - text.trim_end().len() > 200
}

/// Pulls the first complete JSON object out of a model reply (handles code fences,
/// `<think>` blocks and chatter before/after).
pub fn extract_json(text: &str) -> Option<&str> {
    let text = match text.rfind("</think>") {
        Some(i) => &text[i + "</think>".len()..],
        None => text,
    };
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let (mut depth, mut in_str, mut escaped) = (0i32, false, false);
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_reply<T: DeserializeOwned>(text: &str, validate: &(dyn Fn(&T) -> Result<(), String> + Sync)) -> Result<T, String> {
    let raw = extract_json(text).ok_or("the reply did not contain a JSON object")?;
    let value: T = serde_json::from_str(raw).map_err(|e| format!("invalid JSON for the requested format: {e}"))?;
    validate(&value)?;
    Ok(value)
}

pub struct JsonReply<T> {
    pub value: T,
    pub raw: String,
    pub usage: Usage,
}

/// Asks for JSON, validates it, and gives the model one chance to repair its output.
pub async fn chat_json<T: DeserializeOwned>(
    http: &reqwest::Client,
    t: &Target,
    messages: Vec<Message>,
    validate: &(dyn Fn(&T) -> Result<(), String> + Sync),
    max_tokens: u32,
    on_token: &mut (dyn FnMut(&str) + Send),
) -> AppResult<JsonReply<T>> {
    let mut messages = messages;
    let mut last_err = String::new();
    for attempt in 0..2 {
        let (text, usage) = chat_stream(http, t, &messages, true, max_tokens, on_token).await?;
        match parse_reply::<T>(&text, validate) {
            Ok(value) => return Ok(JsonReply { value, raw: text, usage }),
            Err(e) => {
                last_err = e.clone();
                if attempt == 0 {
                    on_token("\n\n[repairing output…]\n\n");
                    messages.push(Message::assistant(text));
                    messages.push(Message::user(format!(
                        "Your reply could not be used: {e}. Reply again with ONLY the corrected JSON object, no other text."
                    )));
                }
            }
        }
    }
    Err(AppError::InvalidOutput(format!("{} returned unusable output: {last_err}", t.model)))
}

/// Runs `f` against each target in the role chain until one succeeds.
pub async fn with_fallback<T, F, Fut>(targets: &[Target], mut f: F) -> AppResult<(T, Target)>
where
    F: FnMut(Target) -> Fut,
    Fut: std::future::Future<Output = AppResult<T>>,
{
    let mut last = None;
    for t in targets {
        match f(t.clone()).await {
            Ok(v) => return Ok((v, t.clone())),
            Err(AppError::Cancelled) => return Err(AppError::Cancelled),
            Err(e) => {
                log::warn!("model {} failed: {e}", t.model);
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| AppError::Invalid("No model configured".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_sizes_from_names() {
        assert_eq!(param_billions("qwen2.5-coder:1.5b"), Some(1.5));
        assert_eq!(param_billions("llama3.1:8b"), Some(8.0));
        assert_eq!(param_billions("7.6B"), Some(7.6));
        assert_eq!(param_billions("smollm:135m"), Some(0.135));
        assert_eq!(param_billions("gpt-4o-mini"), None);
        assert_eq!(param_billions("qwen2.5:latest"), None);
    }

    #[test]
    fn extracts_json_from_messy_replies() {
        assert_eq!(extract_json("Sure! ```json\n{\"a\": {\"b\": \"}\"}}\n```"), Some("{\"a\": {\"b\": \"}\"}}"));
        assert_eq!(extract_json("<think>{not this}</think> {\"x\":1}"), Some("{\"x\":1}"));
        assert_eq!(extract_json("no json"), None);
        assert_eq!(extract_json("{\"unterminated\": "), None);
        assert!(is_runaway(&format!("{{\"a\":{}", " \n".repeat(150))));
        assert!(!is_runaway("{\"a\": 1}\n"));
    }

    #[test]
    fn presets_parse() {
        assert!(presets().iter().any(|p| p.provider_type == "ollama" && p.adapter == "ollama"));
        assert!(presets().iter().all(|p| p.adapter == "ollama" || p.adapter == "openai"));
    }
}
