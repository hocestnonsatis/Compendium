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
}

/// Dense summary via local LLM, with heuristic fallback.
pub fn summarize_smart(
    input: &str,
    options: &SmartOptions,
    summarize_opts: &SummarizeOptions,
    config: &Config,
) -> Result<SmartSummarizeResult, String> {
    let original_tokens = estimate_tokens(input, config);
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
    let max_tokens = options
        .max_tokens
        .unwrap_or(config.default_max_tokens);

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
    Ok(truncate_to_tokens(&out, max_tokens, config))
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
    Ok(truncate_to_tokens(&out, max_tokens, config))
}

fn format_llm_err(err: LocalLlmError) -> String {
    format!("local LLM: {err}")
}

/// Keyword / token-overlap line ranking used when no local LLM is available.
pub fn heuristic_filter_relevant(
    input: &str,
    query: &str,
    max_tokens: usize,
    config: &Config,
) -> String {
    let q_tokens = tokenize(query);
    if q_tokens.is_empty() {
        return truncate_to_tokens(input, max_tokens, config);
    }

    let mut scored: Vec<(usize, f64, &str)> = input
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(idx, line)| {
            let line_tokens = tokenize(line);
            let overlap = q_tokens
                .iter()
                .filter(|t| line_tokens.iter().any(|lt| lt == *t || lt.contains(t.as_str())))
                .count();
            let mut score = if line_tokens.is_empty() {
                0.0
            } else {
                overlap as f64 / q_tokens.len() as f64
            };
            // Boost high-signal lines slightly when they share any token.
            let upper = line.to_ascii_uppercase();
            if overlap > 0
                && (upper.contains("ERROR")
                    || upper.contains("WARN")
                    || upper.contains("FAIL")
                    || upper.contains("PANIC"))
            {
                score += 0.25;
            }
            (idx, score, line)
        })
        .collect();

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    // Keep lines with score > 0; if none, keep top few by length as a safety net.
    let positive: Vec<(usize, &str)> = scored
        .iter()
        .filter(|(_, score, _)| *score > 0.0)
        .map(|(idx, _, line)| (*idx, *line))
        .collect();

    let mut kept: Vec<(usize, &str)> = if positive.is_empty() {
        scored
            .iter()
            .take(8)
            .map(|(idx, _, line)| (*idx, *line))
            .collect()
    } else {
        positive
    };

    kept.sort_by_key(|(idx, _)| *idx);

    let mut out = String::new();
    for (_, line) in kept {
        let candidate = if out.is_empty() {
            line.to_string()
        } else {
            format!("{out}\n{line}")
        };
        if estimate_tokens(&candidate, config) > max_tokens {
            break;
        }
        out = candidate;
    }
    if out.is_empty() {
        truncate_to_tokens(input, max_tokens, config)
    } else {
        out
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() >= 2)
        .filter(|s| {
            !matches!(
                s.as_str(),
                "the" | "and" | "for" | "with" | "this" | "that" | "from" | "into" | "your"
            )
        })
        .collect()
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
            &SmartOptions::default(),
            &SummarizeOptions::default(),
            &Config::default(),
        )
        .expect("fallback ok");
        assert_eq!(result.backend, SmartBackend::Heuristic);
        assert!(result.fallback_reason.is_some());
        assert!(!result.summary.is_empty());
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
