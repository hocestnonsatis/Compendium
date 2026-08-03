//! Runtime configuration loaded from environment variables.

use std::env;

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
        cfg
    }
}
