//! Session-level token-savings accounting.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::pipeline::TokenMetrics;

/// Per-tool counters accumulated during one server process lifetime.
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
    pub reduction_ratio: f64,
    pub by_tool: HashMap<String, ToolStats>,
}

impl SessionStats {
    pub fn record(&mut self, tool: &str, metrics: &TokenMetrics) {
        let saved = metrics.original_tokens.saturating_sub(metrics.result_tokens);
        self.total_calls += 1;
        self.original_tokens += metrics.original_tokens;
        self.result_tokens += metrics.result_tokens;
        self.tokens_saved += saved;
        self.reduction_ratio = if self.original_tokens == 0 {
            0.0
        } else {
            self.tokens_saved as f64 / self.original_tokens as f64
        };

        let entry = self.by_tool.entry(tool.to_string()).or_default();
        entry.calls += 1;
        entry.original_tokens += metrics.original_tokens;
        entry.result_tokens += metrics.result_tokens;
        entry.tokens_saved += saved;
    }

    pub fn record_count_only(&mut self, tool: &str) {
        self.total_calls += 1;
        let entry = self.by_tool.entry(tool.to_string()).or_default();
        entry.calls += 1;
    }

    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_savings() {
        let mut s = SessionStats::default();
        s.record(
            "compendium_filter",
            &TokenMetrics::new(100, 40),
        );
        s.record(
            "compendium_compress",
            &TokenMetrics::new(200, 50),
        );
        assert_eq!(s.total_calls, 2);
        assert_eq!(s.tokens_saved, 210);
        assert!((s.reduction_ratio - 0.7).abs() < 0.01);
        assert_eq!(s.by_tool["compendium_filter"].tokens_saved, 60);
    }
}
