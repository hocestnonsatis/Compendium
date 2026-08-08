//! Session-level token-savings and latency accounting.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::pipeline::tokens::{token_backend, TokenBackend};
use crate::pipeline::TokenMetrics;

/// Optional extras attached to a recorded call.
#[derive(Debug, Clone, Default)]
pub struct CallMeta {
    /// Wall-clock latency of the action in milliseconds.
    pub latency_ms: Option<f64>,
    /// Signal-to-call bypass (input left unchanged).
    pub bypassed: bool,
    /// Processing backend label: `heuristic` | `local_llm` | `bm25` | `hybrid` | `cross_encoder` | ….
    pub backend: Option<String>,
}

/// Per-tool counters accumulated during one server process lifetime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolStats {
    pub calls: usize,
    pub original_tokens: usize,
    pub result_tokens: usize,
    pub tokens_saved: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub bypass_calls: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p99_latency_ms: Option<f64>,
    /// Raw samples kept for percentile recompute (capped).
    #[serde(skip)]
    latency_samples_ms: Vec<f64>,
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

/// Aggregate session report.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionStats {
    pub total_calls: usize,
    pub original_tokens: usize,
    pub result_tokens: usize,
    pub tokens_saved: usize,
    pub reduction_ratio: f64,
    /// Alias of [`Self::reduction_ratio`] (CSR-style vocabulary).
    #[serde(default)]
    pub compression_ratio: f64,
    /// Calls that hit the signal-to-call short-input bypass.
    #[serde(default)]
    pub bypass_calls: usize,
    /// `bypass_calls / total_calls`.
    #[serde(default)]
    pub bypass_ratio: f64,
    /// Session-wide latency percentiles (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p99_latency_ms: Option<f64>,
    /// Wall-clock to resolve `action` enum dispatch (same samples as p50/p99).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_resolve_p50_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_resolve_p99_ms: Option<f64>,
    /// `catalog` / compressed `help` / `playbooks` / resources list ads.
    #[serde(default)]
    pub lazy_ad_calls: usize,
    /// Full `help` / `playbook` / resources/read bodies.
    #[serde(default)]
    pub lazy_full_loads: usize,
    /// Non-discovery action that matched the last discovered skill id.
    #[serde(default)]
    pub lazy_follow_through: usize,
    /// `lazy_follow_through / max(1, lazy_ad_calls + lazy_full_loads)`.
    #[serde(default)]
    pub lazy_hit_rate: f64,
    /// Build-time token counter: `heuristic` or `tiktoken`.
    #[serde(default)]
    pub token_backend: String,
    /// Counts by processing backend (`heuristic`, `local_llm`, `bm25`, …).
    #[serde(default)]
    pub by_backend: HashMap<String, usize>,
    pub by_tool: HashMap<String, ToolStats>,
    /// Cache hit counters (merged from session [`CacheStore`] on `stats`).
    #[serde(default)]
    pub cache_hits: usize,
    #[serde(default)]
    pub cache_misses: usize,
    #[serde(default)]
    pub cache_disk_loads: usize,
    #[serde(default)]
    pub cache_evictions: usize,
    #[serde(default)]
    pub cache_entries: usize,
    #[serde(default)]
    pub cache_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<String>,
    /// Process-local (+ session) embedding vector cache hits.
    #[serde(default)]
    pub embed_cache_hits: usize,
    /// Embedding lookups that required an HTTP `/embeddings` call.
    #[serde(default)]
    pub embed_cache_misses: usize,
    #[serde(skip)]
    latency_samples_ms: Vec<f64>,
    /// Last discovered action/playbook id awaiting follow-through.
    #[serde(skip)]
    pending_discovery: Option<String>,
}

const MAX_LATENCY_SAMPLES: usize = 4_096;

impl SessionStats {
    pub fn record(&mut self, tool: &str, metrics: &TokenMetrics) {
        self.record_with_meta(tool, metrics, &CallMeta::default());
    }

    pub fn record_with_meta(&mut self, tool: &str, metrics: &TokenMetrics, meta: &CallMeta) {
        let saved = metrics
            .original_tokens
            .saturating_sub(metrics.result_tokens);
        self.total_calls += 1;
        self.original_tokens += metrics.original_tokens;
        self.result_tokens += metrics.result_tokens;
        self.tokens_saved += saved;
        self.recompute_ratios();

        if meta.bypassed {
            self.bypass_calls += 1;
        }
        self.recompute_bypass();

        if let Some(backend) = &meta.backend {
            *self.by_backend.entry(backend.clone()).or_insert(0) += 1;
        }

        if let Some(ms) = meta.latency_ms {
            push_sample(&mut self.latency_samples_ms, ms);
        }

        let entry = self.by_tool.entry(tool.to_string()).or_default();
        entry.calls += 1;
        entry.original_tokens += metrics.original_tokens;
        entry.result_tokens += metrics.result_tokens;
        entry.tokens_saved += saved;
        if meta.bypassed {
            entry.bypass_calls += 1;
        }
        if let Some(ms) = meta.latency_ms {
            push_sample(&mut entry.latency_samples_ms, ms);
            entry.p50_latency_ms = percentile(&entry.latency_samples_ms, 0.50);
            entry.p99_latency_ms = percentile(&entry.latency_samples_ms, 0.99);
        }
    }

    pub fn record_count_only(&mut self, tool: &str) {
        self.record_count_only_with_meta(tool, &CallMeta::default());
    }

    pub fn record_count_only_with_meta(&mut self, tool: &str, meta: &CallMeta) {
        self.total_calls += 1;
        if meta.bypassed {
            self.bypass_calls += 1;
        }
        self.recompute_bypass();
        if let Some(backend) = &meta.backend {
            *self.by_backend.entry(backend.clone()).or_insert(0) += 1;
        }
        if let Some(ms) = meta.latency_ms {
            push_sample(&mut self.latency_samples_ms, ms);
        }
        let entry = self.by_tool.entry(tool.to_string()).or_default();
        entry.calls += 1;
        if meta.bypassed {
            entry.bypass_calls += 1;
        }
        if let Some(ms) = meta.latency_ms {
            push_sample(&mut entry.latency_samples_ms, ms);
            entry.p50_latency_ms = percentile(&entry.latency_samples_ms, 0.50);
            entry.p99_latency_ms = percentile(&entry.latency_samples_ms, 0.99);
        }
    }

    /// Progressive-disclosure advertisement (catalog / compressed help / playbooks list).
    pub fn note_lazy_ad(&mut self, discovered_id: Option<&str>) {
        self.lazy_ad_calls += 1;
        if let Some(id) = discovered_id.map(str::trim).filter(|s| !s.is_empty()) {
            self.pending_discovery = Some(id.to_string());
        }
        self.recompute_lazy_hit();
    }

    /// Full skill body load (full help / playbook / resources/read).
    pub fn note_lazy_full(&mut self, discovered_id: Option<&str>) {
        self.lazy_full_loads += 1;
        if let Some(id) = discovered_id.map(str::trim).filter(|s| !s.is_empty()) {
            self.pending_discovery = Some(id.to_string());
        }
        self.recompute_lazy_hit();
    }

    /// After a non-discovery action: count follow-through if it matches pending discovery.
    pub fn note_action_follow(&mut self, action: &str) {
        let action = action.trim();
        if let Some(pending) = self.pending_discovery.take() {
            if pending.eq_ignore_ascii_case(action) {
                self.lazy_follow_through += 1;
            }
        }
        self.recompute_lazy_hit();
    }

    /// Attach latency / bypass / backend after `record` / `record_count_only` already ran.
    pub fn attach_post_call(
        &mut self,
        tool: &str,
        latency_ms: f64,
        bypassed: bool,
        backend: Option<&str>,
    ) {
        push_sample(&mut self.latency_samples_ms, latency_ms);
        if bypassed {
            self.bypass_calls += 1;
            self.recompute_bypass();
        }
        if let Some(backend) = backend {
            *self.by_backend.entry(backend.to_string()).or_insert(0) += 1;
        }
        if let Some(entry) = self.by_tool.get_mut(tool) {
            push_sample(&mut entry.latency_samples_ms, latency_ms);
            if bypassed {
                entry.bypass_calls += 1;
            }
            entry.p50_latency_ms = percentile(&entry.latency_samples_ms, 0.50);
            entry.p99_latency_ms = percentile(&entry.latency_samples_ms, 0.99);
        }
    }

    /// Finalize computed fields for a client-facing snapshot.
    pub fn snapshot(&self) -> Self {
        let mut out = self.clone();
        out.token_backend = match token_backend() {
            TokenBackend::Heuristic => "heuristic".into(),
            #[cfg(feature = "real-tokens")]
            TokenBackend::Tiktoken => "tiktoken".into(),
        };
        out.recompute_ratios();
        out.recompute_lazy_hit();
        out.p50_latency_ms = percentile(&out.latency_samples_ms, 0.50);
        out.p99_latency_ms = percentile(&out.latency_samples_ms, 0.99);
        out.action_resolve_p50_ms = out.p50_latency_ms;
        out.action_resolve_p99_ms = out.p99_latency_ms;
        for tool in out.by_tool.values_mut() {
            tool.p50_latency_ms = percentile(&tool.latency_samples_ms, 0.50);
            tool.p99_latency_ms = percentile(&tool.latency_samples_ms, 0.99);
        }
        out
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn recompute_ratios(&mut self) {
        self.reduction_ratio = if self.original_tokens == 0 {
            0.0
        } else {
            self.tokens_saved as f64 / self.original_tokens as f64
        };
        self.compression_ratio = self.reduction_ratio;
    }

    fn recompute_bypass(&mut self) {
        self.bypass_ratio = if self.total_calls == 0 {
            0.0
        } else {
            self.bypass_calls as f64 / self.total_calls as f64
        };
    }

    fn recompute_lazy_hit(&mut self) {
        let denom = self
            .lazy_ad_calls
            .saturating_add(self.lazy_full_loads)
            .max(1);
        self.lazy_hit_rate = self.lazy_follow_through as f64 / denom as f64;
    }
}

fn push_sample(samples: &mut Vec<f64>, ms: f64) {
    if !ms.is_finite() || ms < 0.0 {
        return;
    }
    if samples.len() >= MAX_LATENCY_SAMPLES {
        let keep = MAX_LATENCY_SAMPLES / 2;
        samples.drain(0..samples.len() - keep);
    }
    samples.push(ms);
}

fn percentile(samples: &[f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    let v = sorted[idx.min(sorted.len() - 1)];
    Some((v * 1000.0).round() / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_savings() {
        let mut s = SessionStats::default();
        s.record("compendium_filter", &TokenMetrics::new(100, 40));
        s.record("compendium_compress", &TokenMetrics::new(200, 50));
        assert_eq!(s.total_calls, 2);
        assert_eq!(s.tokens_saved, 210);
        assert!((s.reduction_ratio - 0.7).abs() < 0.01);
        assert!((s.compression_ratio - 0.7).abs() < 0.01);
        assert_eq!(s.by_tool["compendium_filter"].tokens_saved, 60);
    }

    #[test]
    fn tracks_latency_bypass_backend() {
        let mut s = SessionStats::default();
        for ms in [10.0, 20.0, 30.0, 40.0, 100.0] {
            s.record_with_meta(
                "compendium:compress",
                &TokenMetrics::new(50, 50),
                &CallMeta {
                    latency_ms: Some(ms),
                    bypassed: ms < 15.0,
                    backend: Some("heuristic".into()),
                },
            );
        }
        let snap = s.snapshot();
        assert_eq!(snap.bypass_calls, 1);
        assert!(snap.bypass_ratio > 0.0);
        assert!(snap.p50_latency_ms.unwrap() >= 20.0);
        assert!(snap.p99_latency_ms.unwrap() >= 40.0);
        assert_eq!(snap.action_resolve_p50_ms, snap.p50_latency_ms);
        assert_eq!(snap.by_backend.get("heuristic"), Some(&5));
        assert!(!snap.token_backend.is_empty());
    }

    #[test]
    fn tracks_lazy_follow_through() {
        let mut s = SessionStats::default();
        s.note_lazy_ad(Some("brief"));
        s.note_action_follow("brief");
        assert_eq!(s.lazy_ad_calls, 1);
        assert_eq!(s.lazy_follow_through, 1);
        assert!(s.lazy_hit_rate > 0.0);
        s.note_lazy_full(Some("filter"));
        s.note_action_follow("compress");
        assert_eq!(s.lazy_follow_through, 1);
        assert_eq!(s.lazy_full_loads, 1);
    }
}
