//! Local-SLM-backed smart actions with deterministic heuristic fallbacks.
//!
//! - [`summarize_smart`]: abstractive / dense summary via local LLM
//! - [`filter_relevant`]: query-aware line keep via local LLM
//!
//! When `COMPENDIUM_LOCAL_LLM_URL` is unset (or the call fails and fallback is
//! enabled), these fall back to the existing heuristic pipeline so air-gapped
//! agents keep working.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::filter::{filter, FilterOptions};
use crate::pipeline::local_llm::{LocalLlmClient, LocalLlmError};
use crate::pipeline::sanitize::scrub_secrets;
use crate::pipeline::signal::{bypass_reason, should_bypass_signal};
use crate::pipeline::summarize::{summarize, SummarizeOptions};
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Which backend produced the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmartBackend {
    LocalLlm,
    Heuristic,
}

/// Shared options for smart gateway actions.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SmartOptions {
    /// Relevance query for `filter_relevant` (also accepted as top-level `query`).
    pub query: Option<String>,
    /// Soft cap on output tokens.
    pub max_tokens: Option<usize>,
    /// When true (default), fall back to heuristics if the local LLM is missing or fails.
    #[serde(default = "default_true")]
    pub fallback: bool,
    /// Optional system-prompt override (advanced).
    pub system_prompt: Option<String>,
    /// When true, skip the signal-to-call short-input bypass (`summarize_smart`).
    #[serde(default)]
    pub force: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SmartOptions {
    fn default() -> Self {
        Self {
            query: None,
            max_tokens: None,
            fallback: true,
            system_prompt: None,
            force: false,
        }
    }
}

/// Result of [`summarize_smart`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SmartSummarizeResult {
    pub summary: String,
    pub backend: SmartBackend,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub metrics: TokenMetrics,
    /// Prefer byte-stable outputs (temp=0 + seed on local LLM; heuristics always stable).
    #[serde(default = "default_true")]
    pub deterministic: bool,
    /// True when input was below the signal threshold and left unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypassed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_reason: Option<String>,
}

/// Result of [`filter_relevant`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SmartFilterResult {
    pub content: String,
    pub backend: SmartBackend,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub lines_kept: usize,
    pub lines_total: usize,
    pub metrics: TokenMetrics,
    /// Prefer byte-stable outputs (temp=0 + seed on local LLM; heuristics always stable).
    #[serde(default = "default_true")]
    pub deterministic: bool,
}

/// Dense summary via local LLM, with heuristic fallback.
pub fn summarize_smart(
    input: &str,
    options: &SmartOptions,
    summarize_opts: &SummarizeOptions,
    config: &Config,
) -> Result<SmartSummarizeResult, String> {
    let original_tokens = estimate_tokens(input, config);
    let force = options.force || summarize_opts.force;

    if should_bypass_signal(input, config, force) {
        return Ok(SmartSummarizeResult {
            summary: input.to_string(),
            backend: SmartBackend::Heuristic,
            model: None,
            fallback_reason: None,
            metrics: TokenMetrics::new(original_tokens, original_tokens),
            deterministic: true,
            bypassed: true,
            bypass_reason: Some(bypass_reason(config)),
        });
    }

    let max_tokens = options
        .max_tokens
        .or(summarize_opts.max_tokens)
        .unwrap_or(config.default_max_tokens);

    match try_llm_summarize(input, options, max_tokens, config) {
        Ok(summary) => {
            let result_tokens = estimate_tokens(&summary, config);
            Ok(SmartSummarizeResult {
                summary,
                backend: SmartBackend::LocalLlm,
                model: config.local_llm.model_name(),
                fallback_reason: None,
                metrics: TokenMetrics::new(original_tokens, result_tokens),
                deterministic: true,
                bypassed: false,
                bypass_reason: None,
            })
        }
        Err(err) => {
            if !options.fallback {
                return Err(err);
            }
            let heuristic = summarize(input, summarize_opts, config);
            let summary = if estimate_tokens(&heuristic.summary, config) > max_tokens {
                truncate_to_tokens(&heuristic.summary, max_tokens, config)
            } else {
                heuristic.summary
            };
            let result_tokens = estimate_tokens(&summary, config);
            Ok(SmartSummarizeResult {
                summary,
                backend: SmartBackend::Heuristic,
                model: None,
                fallback_reason: Some(err),
                metrics: TokenMetrics::new(original_tokens, result_tokens),
                deterministic: true,
                bypassed: false,
                bypass_reason: None,
            })
        }
    }
}

/// Keep only lines relevant to `query` via local LLM, with keyword fallback.
pub fn filter_relevant(
    input: &str,
    query: &str,
    options: &SmartOptions,
    config: &Config,
) -> Result<SmartFilterResult, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("`filter_relevant` requires a non-empty `query`".into());
    }

    let original_tokens = estimate_tokens(input, config);
    let lines_total = input.lines().count();
    let max_tokens = options.max_tokens.unwrap_or(config.default_max_tokens);

    // Pre-clean with cheap deterministic filter before scoring / LLM.
    let cleaned = filter(
        input,
        &FilterOptions {
            strip_ansi: true,
            collapse_whitespace: true,
            strip_boilerplate: true,
            densify_json: false,
            keep_patterns: Vec::new(),
            drop_patterns: Vec::new(),
            max_tokens: None,
            query: None,
        },
        config,
    );

    match try_llm_filter(&cleaned.content, query, options, max_tokens, config) {
        Ok(content) => {
            let lines_kept = content.lines().filter(|l| !l.trim().is_empty()).count();
            let result_tokens = estimate_tokens(&content, config);
            Ok(SmartFilterResult {
                content,
                backend: SmartBackend::LocalLlm,
                model: config.local_llm.model_name(),
                fallback_reason: None,
                lines_kept,
                lines_total,
                metrics: TokenMetrics::new(original_tokens, result_tokens),
                deterministic: true,
            })
        }
        Err(err) => {
            if !options.fallback {
                return Err(err);
            }
            let content = heuristic_filter_relevant(&cleaned.content, query, max_tokens, config);
            let lines_kept = content.lines().filter(|l| !l.trim().is_empty()).count();
            let result_tokens = estimate_tokens(&content, config);
            Ok(SmartFilterResult {
                content,
                backend: SmartBackend::Heuristic,
                model: None,
                fallback_reason: Some(err),
                lines_kept,
                lines_total,
                metrics: TokenMetrics::new(original_tokens, result_tokens),
                deterministic: true,
            })
        }
    }
}

fn try_llm_summarize(
    input: &str,
    options: &SmartOptions,
    max_tokens: usize,
    config: &Config,
) -> Result<String, String> {
    let client = match LocalLlmClient::from_config(&config.local_llm) {
        None => return Err("local LLM not configured (set COMPENDIUM_LOCAL_LLM_URL)".into()),
        Some(Err(e)) => return Err(e.to_string()),
        Some(Ok(c)) => c,
    };

    let system = options.system_prompt.clone().unwrap_or_else(|| {
        format!(
            "You are a context compressor for coding agents. Produce a dense hierarchical \
             summary that preserves decisions, errors, file paths, commands, and API names. \
             Use short markdown headings and bullets. Target at most ~{max_tokens} tokens. \
             No preamble."
        )
    });

    let user = format!("Summarize the following content:\n\n{input}");
    let max_out = u32::try_from(max_tokens).unwrap_or(u32::MAX);
    let out = client
        .chat(&system, &user, Some(max_out))
        .map_err(format_llm_err)?;
    Ok(truncate_to_tokens(&scrub_secrets(&out), max_tokens, config))
}

fn try_llm_filter(
    input: &str,
    query: &str,
    options: &SmartOptions,
    max_tokens: usize,
    config: &Config,
) -> Result<String, String> {
    let client = match LocalLlmClient::from_config(&config.local_llm) {
        None => return Err("local LLM not configured (set COMPENDIUM_LOCAL_LLM_URL)".into()),
        Some(Err(e)) => return Err(e.to_string()),
        Some(Ok(c)) => c,
    };

    let system = options.system_prompt.clone().unwrap_or_else(|| {
        format!(
            "You filter noisy logs/tool output for a coding agent. Keep only lines relevant \
             to the user's query. Preserve original order and wording. Drop spinner frames, \
             ANSI noise, and unrelated chatter. Return plain text only (the kept lines). \
             Target at most ~{max_tokens} tokens."
        )
    });

    let user = format!("Query: {query}\n\n---\n{input}");
    let max_out = u32::try_from(max_tokens).unwrap_or(u32::MAX);
    let out = client
        .chat(&system, &user, Some(max_out))
        .map_err(format_llm_err)?;
    Ok(truncate_to_tokens(&scrub_secrets(&out), max_tokens, config))
}

fn format_llm_err(err: LocalLlmError) -> String {
    format!("local LLM: {err}")
}

/// Keyword / BM25 line ranking used when no local LLM is available.
pub fn heuristic_filter_relevant(
    input: &str,
    query: &str,
    max_tokens: usize,
    config: &Config,
) -> String {
    let out = crate::pipeline::bm25::filter_lines_bm25(input, query, max_tokens, |s| {
        estimate_tokens(s, config)
    });
    if out.is_empty() {
        truncate_to_tokens(input, max_tokens, config)
    } else {
        out
    }
}

fn truncate_to_tokens(text: &str, max_tokens: usize, config: &Config) -> String {
    if estimate_tokens(text, config) <= max_tokens {
        return text.to_string();
    }
    let budget = (max_tokens as f64 * config.chars_per_token) as usize;
    let mut end = budget.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(nl) = text[..end].rfind('\n') {
        end = nl;
    }
    format!("{}\n…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_keeps_query_matching_lines() {
        let input = "\
INFO starting worker
ERROR database connection refused on port 5432
DEBUG spinner ....
INFO unrelated weather update
WARN retrying database connection
";
        let out = heuristic_filter_relevant(input, "database connection", 256, &Config::default());
        assert!(out.contains("database"));
        assert!(!out.contains("weather"));
    }

    #[test]
    fn summarize_smart_falls_back_without_url() {
        let result = summarize_smart(
            "# Intro\nDetails about the system go here with enough length.\n",
            &SmartOptions {
                force: true,
                ..Default::default()
            },
            &SummarizeOptions {
                force: true,
                ..Default::default()
            },
            &Config::default(),
        )
        .expect("fallback ok");
        assert_eq!(result.backend, SmartBackend::Heuristic);
        assert!(result.fallback_reason.is_some());
        assert!(result.deterministic);
        assert!(!result.summary.is_empty());
        assert!(!result.bypassed);
    }

    #[test]
    fn filter_relevant_requires_query() {
        let err = filter_relevant("a\nb\n", "", &SmartOptions::default(), &Config::default())
            .expect_err("empty query");
        assert!(err.contains("query"));
    }

    #[test]
    fn filter_relevant_falls_back_without_url() {
        let input = "ERROR boom in auth\nINFO ok\nTRACE noise";
        let result = filter_relevant(
            input,
            "auth error",
            &SmartOptions {
                max_tokens: Some(128),
                ..Default::default()
            },
            &Config::default(),
        )
        .expect("fallback ok");
        assert_eq!(result.backend, SmartBackend::Heuristic);
        assert!(result.content.to_lowercase().contains("auth") || result.content.contains("ERROR"));
    }
}
