//! OpenAI-compatible local small-language-model client.
//!
//! Talks to loopback servers such as Ollama (`http://127.0.0.1:11434/v1`),
//! Lemonade (`http://127.0.0.1:13305/api/v1`), or llama.cpp's OpenAI server.
//! No cloud calls — the base URL is always caller-provided via config.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::LocalLlmConfig;

/// Errors from a local LLM round-trip.
#[derive(Debug, Error)]
pub enum LocalLlmError {
    #[error("local LLM is not configured (set COMPENDIUM_LOCAL_LLM_URL)")]
    NotConfigured,
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
            temperature: 0.2,
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

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
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
}
