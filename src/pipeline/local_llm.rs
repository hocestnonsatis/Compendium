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
    pub fn chat(&self, system: &str, user: &str, max_tokens: Option<u32>) -> Result<String, LocalLlmError> {
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
    let parsed = reqwest::Url::parse(base).map_err(|e| {
        LocalLlmError::NonLoopback(format!("invalid URL `{base}`: {e}"))
    })?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(LocalLlmError::NonLoopback(format!(
                "scheme `{other}` not allowed (use http/https on loopback)"
            )));
        }
    }

    let host_str = parsed.host_str().ok_or_else(|| {
        LocalLlmError::NonLoopback(format!("URL `{base}` has no host"))
    })?;

    let lower = host_str.to_ascii_lowercase();
    // Some parsers leave brackets on IPv6 literals in edge cases.
    let host_for_ip = lower
        .trim_start_matches('[')
        .trim_end_matches(']');

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
            assert!(
                assert_loopback_url(url).is_ok(),
                "expected ok for {url}"
            );
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
