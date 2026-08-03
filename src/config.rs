//! Runtime configuration loaded from environment variables.

use std::env;

/// Optional OpenAI-compatible local SLM endpoint.
#[derive(Debug, Clone)]
pub struct LocalLlmConfig {
    /// When true, smart actions may call the local endpoint.
    pub enabled: bool,
    /// Base URL including `/v1` or `/api/v1` (no trailing slash required).
    pub base_url: Option<String>,
    /// Model id accepted by the local server.
    pub model: String,
    /// Optional bearer token (Lemonade embed / locked loopback).
    pub api_key: Option<String>,
    /// HTTP timeout in seconds (first model load can be slow).
    pub timeout_secs: u64,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: None,
            model: "Qwen3-4B-GGUF".into(),
            api_key: None,
            timeout_secs: 120,
        }
    }
}

impl LocalLlmConfig {
    pub fn model_name(&self) -> Option<String> {
        if self.enabled {
            Some(self.model.clone())
        } else {
            None
        }
    }
}

/// Server and pipeline defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// Approximate characters per token (heuristic backend only).
    pub chars_per_token: f64,
    /// Default max output tokens for compress/summarize.
    pub default_max_tokens: usize,
    /// Collapse runs of blank lines to at most this many.
    pub max_blank_lines: usize,
    /// When deduplicating similar lines, Jaccard threshold (0.0–1.0).
    pub similarity_threshold: f64,
    /// Tiktoken encoding name when built with `real-tokens`
    /// (`cl100k_base` or `o200k_base`). Ignored by the heuristic backend.
    pub tokenizer: String,
    /// Bind address for streamable HTTP transport (`http` feature).
    pub http_bind: String,
    /// Optional local small-language-model endpoint for smart actions.
    pub local_llm: LocalLlmConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chars_per_token: 4.0,
            default_max_tokens: 2_048,
            max_blank_lines: 1,
            similarity_threshold: 0.85,
            tokenizer: "cl100k_base".into(),
            http_bind: "127.0.0.1:8788".into(),
            local_llm: LocalLlmConfig::default(),
        }
    }
}

impl Config {
    /// Build config from `COMPENDIUM_*` environment variables.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = env::var("COMPENDIUM_CHARS_PER_TOKEN") {
            if let Ok(n) = v.parse() {
                cfg.chars_per_token = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_DEFAULT_MAX_TOKENS") {
            if let Ok(n) = v.parse() {
                cfg.default_max_tokens = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_MAX_BLANK_LINES") {
            if let Ok(n) = v.parse() {
                cfg.max_blank_lines = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_SIMILARITY_THRESHOLD") {
            if let Ok(n) = v.parse() {
                cfg.similarity_threshold = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_TOKENIZER") {
            if !v.trim().is_empty() {
                cfg.tokenizer = v;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_HTTP_BIND") {
            if !v.trim().is_empty() {
                cfg.http_bind = v;
            }
        }

        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_URL") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                cfg.local_llm.base_url = Some(trimmed);
                cfg.local_llm.enabled = true;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_MODEL") {
            if !v.trim().is_empty() {
                cfg.local_llm.model = v;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_API_KEY") {
            if !v.trim().is_empty() {
                cfg.local_llm.api_key = Some(v);
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_TIMEOUT_SECS") {
            if let Ok(n) = v.parse::<u64>() {
                if n > 0 {
                    cfg.local_llm.timeout_secs = n;
                }
            }
        }

        cfg
    }
}
