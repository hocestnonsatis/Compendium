//! OpenAI-compatible local small-language-model client.
//!
//! Talks to loopback servers such as Ollama (`http://127.0.0.1:11434/v1`),
//! Lemonade (`http://127.0.0.1:13305/api/v1`), or llama.cpp's OpenAI server.
//! No cloud calls — the base URL must resolve to loopback (`127.0.0.1`, `::1`, or
//! `localhost`) to prevent SSRF / data exfiltration.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::LocalLlmConfig;
use crate::pipeline::cache::CacheStore;

/// Fixed seed for OpenAI-compatible servers that honor `seed` (prefix-cache friendly).
pub const DETERMINISTIC_SEED: i64 = 0;

/// Process-local embedding vector cache (model+text → vector).
fn process_embed_cache() -> &'static Mutex<HashMap<String, Vec<f32>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<f32>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

static EMBED_CACHE_HITS: AtomicUsize = AtomicUsize::new(0);
static EMBED_CACHE_MISSES: AtomicUsize = AtomicUsize::new(0);

/// Process-wide embedding cache hit/miss counters (memory + session store lookups).
pub fn process_embed_cache_counters() -> (usize, usize) {
    (
        EMBED_CACHE_HITS.load(Ordering::Relaxed),
        EMBED_CACHE_MISSES.load(Ordering::Relaxed),
    )
}

/// Reset process-wide embedding cache hit/miss counters.
pub fn reset_process_embed_cache_counters() {
    EMBED_CACHE_HITS.store(0, Ordering::Relaxed);
    EMBED_CACHE_MISSES.store(0, Ordering::Relaxed);
}

/// Stable key for embedding cache entries.
pub fn embedding_cache_key(model: &str, text: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    model.hash(&mut h);
    text.hash(&mut h);
    format!("cache://embed/{:016x}", h.finish())
}

/// Clear the process-local embedding cache (tests / diagnostics).
pub fn clear_process_embed_cache() {
    if let Ok(mut g) = process_embed_cache().lock() {
        g.clear();
    }
    reset_process_embed_cache_counters();
}

/// Errors from a local LLM round-trip.
#[derive(Debug, Error)]
pub enum LocalLlmError {
    #[error("local LLM is not configured (set COMPENDIUM_LOCAL_LLM_URL)")]
    NotConfigured,
    #[error("local LLM URL rejected (loopback only): {0}")]
    NonLoopback(String),
    #[error("local LLM HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("local LLM returned HTTP {status}: {body}")]
    BadStatus { status: u16, body: String },
    #[error("local LLM response missing assistant content")]
    EmptyContent,
    #[error("local LLM response parse error: {0}")]
    Parse(String),
}

/// Thin blocking client for chat completions.
#[derive(Debug, Clone)]
pub struct LocalLlmClient {
    base_url: String,
    model: String,
    api_key: Option<String>,
    timeout: Duration,
    http: reqwest::blocking::Client,
}

impl LocalLlmClient {
    /// Build a client when `config.enabled`; otherwise `None`.
    pub fn from_config(config: &LocalLlmConfig) -> Option<Result<Self, LocalLlmError>> {
        if !config.enabled {
            return None;
        }
        Some(Self::new(config))
    }

    pub fn new(config: &LocalLlmConfig) -> Result<Self, LocalLlmError> {
        let base = config
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(LocalLlmError::NotConfigured)?
            .trim_end_matches('/')
            .to_string();

        assert_loopback_url(&base)?;

        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self {
            base_url: base,
            model: config.model.clone(),
            api_key: config.api_key.clone(),
            timeout: Duration::from_secs(config.timeout_secs),
            http,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Chat completion: one system + one user message → assistant text.
    pub fn chat(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
    ) -> Result<String, LocalLlmError> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatCompletionRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: system.into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user.into(),
                },
            ],
            // Temperature 0 + seed: prefer byte-stable outputs for prefix caching.
            temperature: 0.0,
            seed: Some(DETERMINISTIC_SEED),
            max_tokens,
        };

        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }

        tracing::debug!(
            url = %url,
            model = %self.model,
            timeout_secs = self.timeout.as_secs(),
            "local LLM chat request"
        );

        // Avoid stalling the Tokio scheduler when invoked from an async MCP handler.
        let response = run_blocking(move || req.send())?;
        let status = response.status();
        let text = run_blocking(move || response.text())?;
        if !status.is_success() {
            return Err(LocalLlmError::BadStatus {
                status: status.as_u16(),
                body: text.chars().take(512).collect(),
            });
        }

        let parsed: ChatCompletionResponse =
            serde_json::from_str(&text).map_err(|e| LocalLlmError::Parse(e.to_string()))?;
        let content = parsed
            .choices
            .into_iter()
            .find_map(|c| c.message.content)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or(LocalLlmError::EmptyContent)?;
        Ok(content)
    }

    /// OpenAI-compatible embeddings for one or more input strings.
    ///
    /// Checks the process-local cache first, then optional [`CacheStore`] (session/disk),
    /// then batches HTTP requests for misses only.
    pub fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, LocalLlmError> {
        self.embed_with_cache(model, inputs, None)
    }

    /// Like [`Self::embed`], optionally reading/writing vectors via session [`CacheStore`].
    pub fn embed_with_cache(
        &self,
        model: &str,
        inputs: &[String],
        mut store: Option<&mut CacheStore>,
    ) -> Result<Vec<Vec<f32>>, LocalLlmError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let mut out: Vec<Option<Vec<f32>>> = vec![None; inputs.len()];
        let mut miss_indices: Vec<usize> = Vec::new();

        {
            let mem = process_embed_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for (i, text) in inputs.iter().enumerate() {
                let key = embedding_cache_key(model, text);
                if let Some(v) = mem.get(&key) {
                    out[i] = Some(v.clone());
                    EMBED_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if let Some(store) = store.as_mut() {
                    if let Some(v) = store.get_embedding(&key) {
                        out[i] = Some(v);
                        EMBED_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                }
                miss_indices.push(i);
                EMBED_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
            }
        }

        if !miss_indices.is_empty() {
            let miss_texts: Vec<String> = miss_indices.iter().map(|&i| inputs[i].clone()).collect();
            let mut fetched: Vec<Vec<f32>> = Vec::with_capacity(miss_texts.len());
            for chunk in miss_texts.chunks(32) {
                fetched.extend(self.embed_http(model, chunk)?);
            }
            if fetched.len() != miss_indices.len() {
                return Err(LocalLlmError::Parse(format!(
                    "expected {} embedding(s), got {}",
                    miss_indices.len(),
                    fetched.len()
                )));
            }
            let mut mem = process_embed_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for (j, vec) in fetched.into_iter().enumerate() {
                let i = miss_indices[j];
                let key = embedding_cache_key(model, &inputs[i]);
                mem.insert(key.clone(), vec.clone());
                if let Some(store) = store.as_mut() {
                    store.put_embedding(&key, &vec);
                }
                out[i] = Some(vec);
            }
        }

        out.into_iter()
            .map(|v| v.ok_or_else(|| LocalLlmError::Parse("embedding cache hole".into())))
            .collect()
    }

    fn embed_http(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, LocalLlmError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url);
        let body = EmbeddingsRequest {
            model: model.to_string(),
            input: inputs.to_vec(),
        };
        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        tracing::debug!(
            url = %url,
            model = %model,
            n = inputs.len(),
            "local LLM embeddings request"
        );
        let response = run_blocking(move || req.send())?;
        let status = response.status();
        let text = run_blocking(move || response.text())?;
        if !status.is_success() {
            return Err(LocalLlmError::BadStatus {
                status: status.as_u16(),
                body: text.chars().take(512).collect(),
            });
        }
        let parsed: EmbeddingsResponse =
            serde_json::from_str(&text).map_err(|e| LocalLlmError::Parse(e.to_string()))?;
        let mut data = parsed.data;
        data.sort_by_key(|d| d.index.unwrap_or(0));
        if data.len() != inputs.len() {
            return Err(LocalLlmError::Parse(format!(
                "expected {} embedding(s), got {}",
                inputs.len(),
                data.len()
            )));
        }
        let mut out = Vec::with_capacity(data.len());
        for row in data {
            if row.embedding.is_empty() {
                return Err(LocalLlmError::Parse("empty embedding vector".into()));
            }
            out.push(row.embedding);
        }
        Ok(out)
    }

    /// Cohere-style `/rerank` when the local server exposes it (batched, faster than chat CE).
    /// Returns `(original_document_index, relevance_score)` sorted by score descending.
    pub fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: Option<usize>,
    ) -> Result<Vec<(usize, f64)>, LocalLlmError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/rerank", self.base_url);
        let body = RerankApiRequest {
            model: self.model.clone(),
            query: query.to_string(),
            documents: documents.to_vec(),
            top_n,
        };
        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        tracing::debug!(
            url = %url,
            model = %self.model,
            n = documents.len(),
            "local LLM rerank request"
        );
        let response = run_blocking(move || req.send())?;
        let status = response.status();
        let text = run_blocking(move || response.text())?;
        if !status.is_success() {
            return Err(LocalLlmError::BadStatus {
                status: status.as_u16(),
                body: text.chars().take(512).collect(),
            });
        }
        let parsed: RerankApiResponse =
            serde_json::from_str(&text).map_err(|e| LocalLlmError::Parse(e.to_string()))?;
        let mut out = Vec::with_capacity(parsed.results.len());
        for row in parsed.results {
            let score = row
                .relevance_score
                .or(row.score)
                .ok_or_else(|| LocalLlmError::Parse("rerank row missing score".into()))?;
            out.push((row.index, score.clamp(0.0, 1.0)));
        }
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        Ok(out)
    }

    /// Lightweight health probe: list models, then optional tiny chat.
    pub fn probe(&self, check_chat: bool) -> LlmProbeResult {
        let started = std::time::Instant::now();
        let models_url = format!("{}/models", self.base_url);
        let mut models_ok = false;
        let mut models_error = None;
        let mut model_ids = Vec::new();

        let mut req = self.http.get(&models_url);
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
        match run_blocking(move || req.send()) {
            Ok(response) => {
                let status = response.status();
                match run_blocking(move || response.text()) {
                    Ok(text) if status.is_success() => {
                        models_ok = true;
                        if let Ok(parsed) = serde_json::from_str::<ModelsResponse>(&text) {
                            model_ids = parsed
                                .data
                                .into_iter()
                                .filter_map(|m| m.id)
                                .take(32)
                                .collect();
                        }
                    }
                    Ok(text) => {
                        models_error = Some(format!(
                            "HTTP {}: {}",
                            status.as_u16(),
                            text.chars().take(160).collect::<String>()
                        ));
                    }
                    Err(e) => models_error = Some(e.to_string()),
                }
            }
            Err(e) => models_error = Some(e.to_string()),
        }

        let mut chat_ok = None;
        let mut chat_error = None;
        if check_chat {
            match self.chat("Reply with exactly: pong", "ping", Some(8)) {
                Ok(_) => chat_ok = Some(true),
                Err(e) => {
                    chat_ok = Some(false);
                    chat_error = Some(e.to_string());
                }
            }
        }

        LlmProbeResult {
            configured: true,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            models_ok,
            models_error,
            model_ids,
            chat_ok,
            chat_error,
            latency_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }
}

fn run_blocking<T, F>(f: F) -> T
where
    F: FnOnce() -> T,
{
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(f),
        Err(_) => f(),
    }
}

/// Reject non-loopback LLM base URLs (SSRF / exfiltration guard).
pub fn assert_loopback_url(base: &str) -> Result<(), LocalLlmError> {
    let parsed = reqwest::Url::parse(base)
        .map_err(|e| LocalLlmError::NonLoopback(format!("invalid URL `{base}`: {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(LocalLlmError::NonLoopback(format!(
                "scheme `{other}` not allowed (use http/https on loopback)"
            )));
        }
    }

    let host_str = parsed
        .host_str()
        .ok_or_else(|| LocalLlmError::NonLoopback(format!("URL `{base}` has no host")))?;

    let lower = host_str.to_ascii_lowercase();
    // Some parsers leave brackets on IPv6 literals in edge cases.
    let host_for_ip = lower.trim_start_matches('[').trim_end_matches(']');

    let is_loopback = lower == "localhost"
        || lower == "localhost."
        || host_for_ip == "::1"
        || host_for_ip
            .parse::<IpAddr>()
            .map(|ip| match ip {
                IpAddr::V4(v4) => v4.is_loopback(),
                IpAddr::V6(v6) => v6.is_loopback(),
            })
            .unwrap_or(false);

    if !is_loopback {
        return Err(LocalLlmError::NonLoopback(format!(
            "`{base}` is not loopback; set COMPENDIUM_LOCAL_LLM_URL to http://127.0.0.1:…/v1 \
             (or ::1 / localhost). Private LAN and public hosts are blocked."
        )));
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: ChatMessageOut,
}

#[derive(Debug, Default, Deserialize)]
struct ChatMessageOut {
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct EmbeddingsRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    #[serde(default)]
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    #[serde(default)]
    index: Option<usize>,
}

#[derive(Debug, Serialize)]
struct RerankApiRequest {
    model: String,
    query: String,
    documents: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_n: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RerankApiResponse {
    #[serde(default)]
    results: Vec<RerankApiHit>,
}

#[derive(Debug, Deserialize)]
struct RerankApiHit {
    index: usize,
    #[serde(default)]
    relevance_score: Option<f64>,
    #[serde(default)]
    score: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelRow>,
}

#[derive(Debug, Deserialize)]
struct ModelRow {
    id: Option<String>,
}

/// Result of [`LocalLlmClient::probe`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LlmProbeResult {
    pub configured: bool,
    pub base_url: String,
    pub model: String,
    pub models_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_error: Option<String>,
    #[serde(default)]
    pub model_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_error: Option<String>,
    pub latency_ms: f64,
}

/// Public status for `action=llm_status` (includes unconfigured case).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LlmStatusResult {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<LlmProbeResult>,
}

/// Build [`LlmStatusResult`] from config (no chat probe by default — fast).
pub fn llm_status(config: &LocalLlmConfig, check_chat: bool) -> LlmStatusResult {
    match LocalLlmClient::from_config(config) {
        None => LlmStatusResult {
            configured: false,
            base_url: None,
            model: None,
            embedding_model: None,
            reachable: false,
            fallback_reason: Some(
                "COMPENDIUM_LOCAL_LLM_URL unset — smart/hybrid actions use heuristics".into(),
            ),
            probe: None,
        },
        Some(Err(e)) => LlmStatusResult {
            configured: true,
            base_url: config.base_url.clone(),
            model: Some(config.model.clone()),
            embedding_model: Some(config.embedding_model_name().to_string()),
            reachable: false,
            fallback_reason: Some(e.to_string()),
            probe: None,
        },
        Some(Ok(client)) => {
            let probe = client.probe(check_chat);
            let reachable = probe.models_ok || probe.chat_ok == Some(true);
            let fallback_reason = if reachable {
                None
            } else {
                Some(
                    probe
                        .models_error
                        .clone()
                        .or_else(|| probe.chat_error.clone())
                        .unwrap_or_else(|| "local LLM unreachable".into()),
                )
            };
            LlmStatusResult {
                configured: true,
                base_url: Some(client.base_url().to_string()),
                model: Some(client.model().to_string()),
                embedding_model: Some(config.embedding_model_name().to_string()),
                reachable,
                fallback_reason,
                probe: Some(probe),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LocalLlmConfig;

    #[test]
    fn from_config_disabled_returns_none() {
        let cfg = LocalLlmConfig::default();
        assert!(LocalLlmClient::from_config(&cfg).is_none());
    }

    #[test]
    fn new_requires_url() {
        let cfg = LocalLlmConfig {
            enabled: true,
            base_url: None,
            ..Default::default()
        };
        assert!(matches!(
            LocalLlmClient::new(&cfg),
            Err(LocalLlmError::NotConfigured)
        ));
    }

    #[test]
    fn accepts_loopback_hosts() {
        for url in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:11434/v1",
            "http://[::1]:8080/v1",
        ] {
            assert!(assert_loopback_url(url).is_ok(), "expected ok for {url}");
        }
    }

    #[test]
    fn rejects_non_loopback_hosts() {
        for url in [
            "http://10.0.0.5:11434/v1",
            "http://192.168.1.1/v1",
            "http://example.com/v1",
            "https://api.openai.com/v1",
        ] {
            assert!(
                matches!(assert_loopback_url(url), Err(LocalLlmError::NonLoopback(_))),
                "expected reject for {url}"
            );
        }
    }

    #[test]
    fn new_rejects_public_url() {
        let cfg = LocalLlmConfig {
            enabled: true,
            base_url: Some("http://8.8.8.8:11434/v1".into()),
            ..Default::default()
        };
        assert!(matches!(
            LocalLlmClient::new(&cfg),
            Err(LocalLlmError::NonLoopback(_))
        ));
    }
}
