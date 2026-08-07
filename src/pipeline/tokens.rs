//! Token counting — heuristic by default, exact BPE with `--features real-tokens`.

#[cfg(not(feature = "real-tokens"))]
use unicode_segmentation::UnicodeSegmentation;

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Structured result for the `compendium_count_tokens` tool.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CountTokensResult {
    pub tokens: usize,
    pub backend: String,
    pub tokenizer: String,
    pub chars: usize,
    pub bytes: usize,
}

/// Count tokens and return a diagnostics-friendly payload.
pub fn count_tokens_detailed(text: &str, config: &Config) -> CountTokensResult {
    CountTokensResult {
        tokens: estimate_tokens(text, config),
        backend: match token_backend() {
            TokenBackend::Heuristic => "heuristic".into(),
            #[cfg(feature = "real-tokens")]
            TokenBackend::Tiktoken => "tiktoken".into(),
        },
        tokenizer: config.tokenizer.clone(),
        chars: text.chars().count(),
        bytes: text.len(),
    }
}

/// Which backend produced a count (useful in diagnostics / tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenBackend {
    Heuristic,
    #[cfg(feature = "real-tokens")]
    Tiktoken,
}

/// Active token-counting backend for this build.
pub fn token_backend() -> TokenBackend {
    #[cfg(feature = "real-tokens")]
    {
        TokenBackend::Tiktoken
    }
    #[cfg(not(feature = "real-tokens"))]
    {
        TokenBackend::Heuristic
    }
}

/// Estimate (or exactly count) tokens in `text`.
///
/// - Default build: chars÷`chars_per_token` ∪ ~1.3×Unicode-words heuristic.
/// - `--features real-tokens`: tiktoken-rs BPE (`COMPENDIUM_TOKENIZER`, default `cl100k_base`).
pub fn estimate_tokens(text: &str, config: &Config) -> usize {
    if text.is_empty() {
        return 0;
    }

    #[cfg(feature = "real-tokens")]
    {
        bpe::count(text, &config.tokenizer)
    }

    #[cfg(not(feature = "real-tokens"))]
    {
        heuristic_tokens(text, config)
    }
}

/// Estimate tokens with default config (convenient for tests / callers).
pub fn estimate_tokens_default(text: &str) -> usize {
    estimate_tokens(text, &Config::default())
}

#[cfg(not(feature = "real-tokens"))]
fn heuristic_tokens(text: &str, config: &Config) -> usize {
    let char_estimate = (text.chars().count() as f64 / config.chars_per_token).ceil() as usize;

    let word_estimate = text
        .unicode_words()
        .count()
        .saturating_mul(13)
        .saturating_add(9)
        / 10; // ~1.3 tokens per word

    char_estimate.max(word_estimate).max(1)
}

#[cfg(feature = "real-tokens")]
mod bpe {
    use tiktoken_rs::{cl100k_base_singleton, o200k_base_singleton};

    pub fn count(text: &str, tokenizer: &str) -> usize {
        // `count_ordinary` ignores special-token syntax in arbitrary tool/log text.
        match normalize(tokenizer) {
            "o200k_base" | "o200k" => o200k_base_singleton().count_ordinary(text),
            _ => cl100k_base_singleton().count_ordinary(text),
        }
    }

    fn normalize(name: &str) -> &str {
        let t = name.trim();
        if t.is_empty() {
            "cl100k_base"
        } else {
            t
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate_tokens_default(""), 0);
    }

    #[test]
    fn short_text_at_least_one() {
        assert!(estimate_tokens_default("hi") >= 1);
    }

    #[test]
    fn longer_text_scales() {
        let short = estimate_tokens_default("hello world");
        let long = estimate_tokens_default(&"hello world ".repeat(100));
        assert!(long > short * 50);
    }

    #[cfg(feature = "real-tokens")]
    #[test]
    fn real_tokens_backend_active() {
        assert_eq!(token_backend(), TokenBackend::Tiktoken);
        // Known cl100k_base count for a short English phrase.
        let n = estimate_tokens("hello world", &Config::default());
        assert!((2..=4).contains(&n), "unexpected count {n}");
    }

    #[cfg(not(feature = "real-tokens"))]
    #[test]
    fn heuristic_backend_active() {
        assert_eq!(token_backend(), TokenBackend::Heuristic);
    }
}
