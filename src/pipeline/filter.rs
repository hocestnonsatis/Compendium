//! Smart filtering: strip boilerplate, ANSI, low-entropy noise, and apply
//! optional relevance keep/drop rules.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for [`filter`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FilterOptions {
    /// Drop ANSI escape sequences (terminal colors/cursor codes).
    #[serde(default = "default_true")]
    pub strip_ansi: bool,
    /// Collapse excessive whitespace / blank lines.
    #[serde(default = "default_true")]
    pub collapse_whitespace: bool,
    /// Drop common log boilerplate (timestamps-only lines, spinner frames, etc.).
    #[serde(default = "default_true")]
    pub strip_boilerplate: bool,
    /// Pretty-print / densify JSON when the whole input parses as JSON.
    #[serde(default = "default_true")]
    pub densify_json: bool,
    /// Keep only lines matching at least one of these regexes (OR). Empty = keep all.
    #[serde(default)]
    pub keep_patterns: Vec<String>,
    /// Drop lines matching any of these regexes.
    #[serde(default)]
    pub drop_patterns: Vec<String>,
    /// Soft cap on output tokens (best-effort truncation after filtering).
    pub max_tokens: Option<usize>,
    /// Optional BM25 query: keep lines relevant to this query (after other filters).
    pub query: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for FilterOptions {
    fn default() -> Self {
        Self {
            strip_ansi: true,
            collapse_whitespace: true,
            strip_boilerplate: true,
            densify_json: true,
            keep_patterns: Vec::new(),
            drop_patterns: Vec::new(),
            max_tokens: None,
            query: None,
        }
    }
}

/// Result of a filter pass.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FilterResult {
    pub content: String,
    pub metrics: TokenMetrics,
    pub lines_removed: usize,
}

/// Apply token-saving filters to `input`.
pub fn filter(input: &str, options: &FilterOptions, config: &Config) -> FilterResult {
    let original_tokens = estimate_tokens(input, config);
    let original_lines = input.lines().count();

    let mut text = input.to_string();

    if options.strip_ansi {
        text = strip_ansi(&text);
    }

    if options.densify_json {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
            if let Ok(compact) = serde_json::to_string(&value) {
                text = compact;
            }
        }
    }

    let keep_res = compile_patterns(&options.keep_patterns);
    let drop_res = compile_patterns(&options.drop_patterns);

    let mut out_lines: Vec<String> = Vec::with_capacity(original_lines);
    let mut blank_run = 0usize;

    for line in text.lines() {
        let trimmed = line.trim_end();

        if options.strip_boilerplate && is_boilerplate(trimmed) {
            continue;
        }

        if !drop_res.is_empty() && drop_res.iter().any(|r| r.is_match(trimmed)) {
            continue;
        }

        if !keep_res.is_empty() && !keep_res.iter().any(|r| r.is_match(trimmed)) {
            continue;
        }

        if options.collapse_whitespace {
            if trimmed.is_empty() {
                blank_run += 1;
                if blank_run > config.max_blank_lines {
                    continue;
                }
                out_lines.push(String::new());
                continue;
            }
            blank_run = 0;
            out_lines.push(collapse_internal_ws(trimmed));
        } else {
            out_lines.push(trimmed.to_string());
        }
    }

    let mut content = out_lines.join("\n");

    if let Some(query) = options
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let max = options.max_tokens.unwrap_or(config.default_max_tokens);
        content = crate::pipeline::bm25::filter_lines_bm25(&content, query, max, |s| {
            estimate_tokens(s, config)
        });
    } else if let Some(max) = options.max_tokens {
        content = truncate_to_tokens(&content, max, config);
    }

    let result_tokens = estimate_tokens(&content, config);
    FilterResult {
        lines_removed: original_lines.saturating_sub(content.lines().count()),
        content,
        metrics: TokenMetrics::new(original_tokens, result_tokens),
    }
}

fn compile_patterns(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
}

fn ansi_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1B(?:\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1B]*(?:\x07|\x1B\\)|[()][AB012])")
            .expect("ANSI regex")
    })
}

fn strip_ansi(s: &str) -> String {
    ansi_regex().replace_all(s, "").into_owned()
}

fn npm_noise_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^(npm (warn|notice)|deprecated|using cache|added \d+ packages)")
            .expect("npm noise regex")
    })
}

fn is_boilerplate(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    if matches!(
        line.trim(),
        "|" | "/" | "-" | "\\" | "⠋" | "⠙" | "⠹" | "⠸" | "⠼" | "⠴" | "⠦" | "⠧" | "⠇" | "⠏"
    ) {
        return true;
    }
    if line
        .chars()
        .all(|c| matches!(c, '-' | '=' | '*' | '#' | '_' | ' ' | '─' | '═'))
        && line.chars().filter(|c| !c.is_whitespace()).count() >= 8
    {
        return true;
    }
    npm_noise_regex().is_match(line)
}

fn collapse_internal_ws(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut prev_ws = false;
    for ch in line.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

fn truncate_to_tokens(text: &str, max_tokens: usize, config: &Config) -> String {
    if estimate_tokens(text, config) <= max_tokens {
        return text.to_string();
    }
    let approx_chars = (max_tokens as f64 * config.chars_per_token) as usize;
    let mut end = approx_chars.min(text.len());
    while !text.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    let mut truncated = text[..end].to_string();
    truncated.push_str("\n…[truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_and_blank_runs() {
        let input = "\x1b[31merror\x1b[0m\n\n\n\nok";
        let result = filter(
            input,
            &FilterOptions {
                strip_ansi: true,
                collapse_whitespace: true,
                densify_json: false,
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(!result.content.contains('\u{1b}'));
        assert!(!result.content.contains("\n\n\n"));
        assert!(result.content.contains("error"));
    }

    #[test]
    fn densifies_json() {
        let input = r#"{
  "a": 1,
  "b": [1, 2, 3]
}"#;
        let result = filter(input, &FilterOptions::default(), &Config::default());
        assert!(!result.content.contains('\n'));
        assert!(result.metrics.result_tokens < result.metrics.original_tokens);
    }

    #[test]
    fn keep_and_drop_patterns() {
        let input = "keep this ERROR\nnoise INFO\ndrop DEBUG noise";
        let result = filter(
            input,
            &FilterOptions {
                keep_patterns: vec!["ERROR|INFO".into()],
                drop_patterns: vec!["DEBUG".into()],
                densify_json: false,
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(result.content.contains("ERROR"));
        assert!(result.content.contains("INFO"));
        assert!(!result.content.contains("DEBUG"));
    }
}
