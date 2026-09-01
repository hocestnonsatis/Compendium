//! OpenAI-compatible local small-language-model client.
//!
//! Talks to loopback servers such as Ollama (`http://127.0.0.1:11434/v1`),
//! Lemonade (`http://127.0.0.1:13305/api/v1`), or llama.cpp's OpenAI server.
//! No cloud calls — the base URL must resolve to loopback (`127.0.0.1`, `::1`, or
//! `localhost`) to prevent SSRF / data exfiltration.

use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::LocalLlmConfig;

/// Fixed seed for OpenAI-compatible servers that honor `seed` (prefix-cache friendly).
pub const DETERMINISTIC_SEED: i64 = 0;

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
    reasoning_effort: Option<String>,
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
            reasoning_effort: config.reasoning_effort.clone(),
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
        let (reasoning_effort, think) = chat_think_params(self.reasoning_effort.as_deref());
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
            reasoning_effort,
            think,
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
            // Thinking models may return empty `content` for very short replies;
            // a plain prompt with a roomier budget avoids false negatives.
            match self.chat("Return the word: pong", "ping", Some(64)) {
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
    let host_for_ip = lower.trim_start_matches('[').trim_end_matches(']');

    let is_loopback = lower == "localhost"
        || lower == "localhost."
        || host_for_ip == "::1"
        || host_for_ip
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);

    if !is_loopback {
        return Err(LocalLlmError::NonLoopback(format!(
            "`{base}` is not loopback; set COMPENDIUM_LOCAL_LLM_URL to http://127.0.0.1:…/v1 \
             (or ::1 / localhost). Private LAN and public hosts are blocked."
        )));
    }

    Ok(())
}

/// Map `COMPENDIUM_LOCAL_LLM_REASONING_EFFORT` onto OpenAI + Ollama chat fields.
///
/// Qwen 3.8 on Ollama ignores `think: false` (still fills `reasoning` and can
/// return empty `content`). `reasoning_effort: "low"` (and `think: "low"`)
/// returns assistant content. `off` still sends `think: false` for older models.
fn chat_think_params(effort: Option<&str>) -> (Option<String>, Option<ThinkParam>) {
    match effort.map(str::trim).filter(|s| !s.is_empty()) {
        None => (None, None),
        Some("off" | "false" | "no") => (None, Some(ThinkParam::Bool(false))),
        Some(s) => (Some(s.to_string()), Some(ThinkParam::Level(s.to_string()))),
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<ThinkParam>,
}

/// Ollama OpenAI-compat: `think` is a bool or a level string.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ThinkParam {
    Bool(bool),
    Level(String),
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
    pub reasoning_effort: Option<String>,
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
            reasoning_effort: None,
            reachable: false,
            fallback_reason: Some(
                "COMPENDIUM_LOCAL_LLM_URL unset — smart actions use heuristics. \
                 Enable Ollama: npx -y compendium-mcp setup-ollama"
                    .into(),
            ),
            probe: None,
        },
        Some(Err(e)) => LlmStatusResult {
            configured: true,
            base_url: config.base_url.clone(),
            model: Some(config.model.clone()),
            reasoning_effort: config.reasoning_effort.clone(),
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
                reasoning_effort: config.reasoning_effort.clone(),
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

    fn request_json(effort: Option<&str>) -> serde_json::Value {
        let (reasoning_effort, think) = chat_think_params(effort);
        serde_json::to_value(ChatCompletionRequest {
            model: "m".into(),
            messages: vec![],
            temperature: 0.0,
            seed: None,
            max_tokens: Some(16),
            reasoning_effort,
            think,
        })
        .unwrap()
    }

    #[test]
    fn chat_think_params_low_sends_effort_and_think_level() {
        let json = request_json(Some("low"));
        assert_eq!(json["reasoning_effort"], "low");
        assert_eq!(json["think"], "low");
    }

    #[test]
    fn chat_think_params_off_sends_think_false_only() {
        let json = request_json(Some("off"));
        assert!(json.get("reasoning_effort").is_none());
        assert_eq!(json["think"], false);
    }

    #[test]
    fn chat_think_params_omit_drops_both_fields() {
        let json = request_json(None);
        assert!(json.get("reasoning_effort").is_none());
        assert!(json.get("think").is_none());
    }
}
