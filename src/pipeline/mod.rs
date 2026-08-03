//! Token-optimization pipeline primitives.

pub mod cache;
pub mod chunk;
pub mod compress;
pub mod filter;
pub mod local_llm;
pub mod output;
pub mod prune;
pub mod smart;
pub mod stats;
pub mod summarize;
pub mod tokens;

use crate::config::Config;

/// Shared context passed into pipeline stages.
#[derive(Debug, Clone)]
pub struct PipelineContext {
    pub config: Config,
}

impl PipelineContext {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn from_env() -> Self {
        Self::new(Config::from_env())
    }
}

impl Default for PipelineContext {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Metrics attached to every pipeline result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TokenMetrics {
    pub original_tokens: usize,
    pub result_tokens: usize,
    /// Fraction of tokens removed: `(original - result) / original`.
    pub reduction_ratio: f64,
}

impl TokenMetrics {
    pub fn new(original_tokens: usize, result_tokens: usize) -> Self {
        let reduction_ratio = if original_tokens == 0 {
            0.0
        } else {
            (original_tokens.saturating_sub(result_tokens) as f64) / original_tokens as f64
        };
        Self {
            original_tokens,
            result_tokens,
            reduction_ratio,
        }
    }
}
