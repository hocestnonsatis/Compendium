//! Session-level token-savings accounting.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::pipeline::tokens::{token_backend, TokenBackend};
use crate::pipeline::TokenMetrics;

/// Per-action counters accumulated during one server process lifetime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolStats {
    pub calls: usize,
    pub original_tokens: usize,
    pub result_tokens: usize,
    pub tokens_saved: usize,
}

/// Aggregate session report.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionStats {
    pub total_calls: usize,
    pub original_tokens: usize,
    pub result_tokens: usize,
    pub tokens_saved: usize,
    /// `tokens_saved / original_tokens`.
    pub reduction_ratio: f64,
    /// Build-time token counter: `heuristic` or `tiktoken`.
    #[serde(default)]
    pub token_backend: String,
    /// Counts by processing backend (`heuristic`, `local_llm`, `bm25`, …).
    #[serde(default)]
    pub by_backend: HashMap<String, usize>,
    pub by_action: HashMap<String, ToolStats>,
    #[serde(default)]
    pub cache_hits: usize,
    #[serde(default)]
    pub cache_misses: usize,
    #[serde(default)]
    pub cache_entries: usize,
    #[serde(default)]
    pub cache_bytes: usize,
}

impl SessionStats {
    /// Record a call that transformed text (`metrics` carries before/after sizes).
    pub fn record(&mut self, action: &str, metrics: &TokenMetrics) {
        let saved = metrics
            .original_tokens
            .saturating_sub(metrics.result_tokens);
        self.total_calls += 1;
        self.original_tokens += metrics.original_tokens;
        self.result_tokens += metrics.result_tokens;
        self.tokens_saved += saved;
        self.recompute_ratio();

        let entry = self.by_action.entry(action.to_string()).or_default();
        entry.calls += 1;
        entry.original_tokens += metrics.original_tokens;
        entry.result_tokens += metrics.result_tokens;
        entry.tokens_saved += saved;
    }

    /// Record a call with no size change (lookups, help, status).
    pub fn record_count_only(&mut self, action: &str) {
        self.total_calls += 1;
        self.by_action.entry(action.to_string()).or_default().calls += 1;
    }

    pub fn note_backend(&mut self, backend: &str) {
        *self.by_backend.entry(backend.to_string()).or_insert(0) += 1;
    }

    /// Finalize computed fields for a client-facing snapshot.
    pub fn snapshot(&self) -> Self {
        let mut out = self.clone();
        out.token_backend = match token_backend() {
            TokenBackend::Heuristic => "heuristic".into(),
            #[cfg(feature = "real-tokens")]
            TokenBackend::Tiktoken => "tiktoken".into(),
        };
        out.recompute_ratio();
        out
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn recompute_ratio(&mut self) {
        self.reduction_ratio = if self.original_tokens == 0 {
            0.0
        } else {
            self.tokens_saved as f64 / self.original_tokens as f64
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_savings() {
        let mut s = SessionStats::default();
        s.record("filter", &TokenMetrics::new(100, 40));
        s.record("compress", &TokenMetrics::new(200, 50));
        s.record_count_only("help");
        s.note_backend("heuristic");
        let snap = s.snapshot();
        assert_eq!(snap.total_calls, 3);
        assert_eq!(snap.tokens_saved, 210);
        assert!((snap.reduction_ratio - 0.7).abs() < 0.01);
        assert_eq!(snap.by_action["filter"].tokens_saved, 60);
        assert_eq!(snap.by_action["help"].calls, 1);
        assert_eq!(snap.by_backend["heuristic"], 1);
        assert!(!snap.token_backend.is_empty());
    }
}
