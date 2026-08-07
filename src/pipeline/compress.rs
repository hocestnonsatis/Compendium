//! Semantic compression: densify text while retaining entities, code shape,
//! and technical parameters.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::filter::{filter, FilterOptions};
use crate::pipeline::signal::{bypass_reason, should_bypass_signal};
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for [`compress`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CompressOptions {
    /// Soft cap on output tokens.
    pub max_tokens: Option<usize>,
    /// Deduplicate near-identical consecutive lines.
    #[serde(default = "default_true")]
    pub dedupe_lines: bool,
    /// Extract and prepend a short entity index (paths, URLs, errors, IDs).
    #[serde(default = "default_true")]
    pub extract_entities: bool,
    /// Run the smart filter pass first.
    #[serde(default = "default_true")]
    pub prefilter: bool,
    /// Hint about content kind for specialized heuristics.
    #[serde(default)]
    pub content_type: ContentType,
    /// When true, skip the signal-to-call short-input bypass.
    #[serde(default)]
    pub force: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            max_tokens: None,
            dedupe_lines: true,
            extract_entities: true,
            prefilter: true,
            content_type: ContentType::Auto,
            force: false,
        }
    }
}

/// Content kind hint for compression heuristics.
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    #[default]
    Auto,
    Text,
    Code,
    Log,
    Json,
    Conversation,
}

/// Result of compression.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CompressResult {
    pub content: String,
    pub metrics: TokenMetrics,
    pub entities: Vec<String>,
    /// True when input was below the signal threshold and left unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypassed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_reason: Option<String>,
}

/// Produce a token-optimized dense representation of `input`.
pub fn compress(input: &str, options: &CompressOptions, config: &Config) -> CompressResult {
    let original_tokens = estimate_tokens(input, config);

    if should_bypass_signal(input, config, options.force) {
        return CompressResult {
            content: input.to_string(),
            metrics: TokenMetrics::new(original_tokens, original_tokens),
            entities: Vec::new(),
            bypassed: true,
            bypass_reason: Some(bypass_reason(config)),
        };
    }

    let mut working = if options.prefilter {
        let filtered = filter(
            input,
            &FilterOptions {
                strip_ansi: true,
                collapse_whitespace: true,
                strip_boilerplate: true,
                densify_json: matches!(options.content_type, ContentType::Json | ContentType::Auto),
                max_tokens: None,
                ..Default::default()
            },
            config,
        );
        filtered.content
    } else {
        input.to_string()
    };

    let content_type = detect_type(&working, &options.content_type);

    let entities = if options.extract_entities {
        extract_entities(&working)
    } else {
        Vec::new()
    };

    if options.dedupe_lines {
        working = dedupe_similar_lines(&working, config.similarity_threshold);
    }

    working = match content_type {
        ContentType::Json => densify_json_structure(&working),
        ContentType::Log => compress_logs(&working),
        ContentType::Code => compress_code(&working),
        ContentType::Conversation => compress_conversation(&working),
        ContentType::Text | ContentType::Auto => working,
    };

    if !entities.is_empty() {
        let index = format!("[entities: {}]", entities.join(" · "));
        working = format!("{index}\n{working}");
    }

    if let Some(max) = options
        .max_tokens
        .or(Some(config.default_max_tokens))
        .filter(|&m| estimate_tokens(&working, config) > m)
    {
        working = hard_truncate(&working, max, config);
    }

    let result_tokens = estimate_tokens(&working, config);
    CompressResult {
        content: working,
        metrics: TokenMetrics::new(original_tokens, result_tokens),
        entities,
        bypassed: false,
        bypass_reason: None,
    }
}

fn detect_type(text: &str, hint: &ContentType) -> ContentType {
    if *hint != ContentType::Auto {
        return hint.clone();
    }
    let trimmed = text.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return ContentType::Json;
    }
    let log_hits = text
        .lines()
        .take(20)
        .filter(|l| {
            l.contains(" ERROR ")
                || l.contains(" WARN ")
                || l.contains(" INFO ")
                || l.starts_with('[')
                    && l.contains(']')
                    && (l.contains("error") || l.contains("warn") || l.contains("info"))
        })
        .count();
    if log_hits >= 3 {
        return ContentType::Log;
    }
    if text.lines().any(|l| {
        l.trim_start().starts_with("fn ")
            || l.trim_start().starts_with("def ")
            || l.trim_start().starts_with("class ")
            || l.trim_start().starts_with("import ")
            || l.trim_start().starts_with("pub ")
    }) {
        return ContentType::Code;
    }
    if text.contains("user:") || text.contains("assistant:") || text.contains("```") {
        return ContentType::Conversation;
    }
    ContentType::Text
}

fn extract_entities(text: &str) -> Vec<String> {
    let patterns: &[(&str, usize)] = &[
        // Absolute / relative paths
        (r"(?:[A-Za-z]:)?(?:/[\w.-]+)+(?:\.\w+)?", 12),
        // URLs
        (r#"https?://[^\s)>"']+"#, 8),
        // Error codes / HTTP
        (r"\b(?:E[A-Z]+\d*|[45]\d{2})\b", 6),
        // Semver / versions
        (r"\bv?\d+\.\d+\.\d+(?:[-+][\w.]+)?\b", 6),
        // UUID-ish
        (
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
            4,
        ),
    ];

    let mut seen = HashSet::new();
    let mut entities = Vec::new();

    for (pat, limit) in patterns {
        let Ok(re) = Regex::new(pat) else {
            continue;
        };
        let mut count = 0usize;
        for m in re.find_iter(text) {
            let s = m
                .as_str()
                .trim_matches(|c| matches!(c, ',' | ';' | '.' | ')'));
            if s.len() < 3 || !seen.insert(s.to_string()) {
                continue;
            }
            entities.push(s.to_string());
            count += 1;
            if count >= *limit {
                break;
            }
        }
    }

    entities.truncate(24);
    entities
}

fn dedupe_similar_lines(text: &str, threshold: f64) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut last_tokens: HashSet<String> = HashSet::new();

    for line in text.lines() {
        let tokens: HashSet<String> = line
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        if !last_tokens.is_empty() && !tokens.is_empty() {
            let inter = tokens.intersection(&last_tokens).count() as f64;
            let union = tokens.union(&last_tokens).count() as f64;
            let jaccard = if union == 0.0 { 0.0 } else { inter / union };
            if jaccard >= threshold {
                continue;
            }
        }
        // Exact consecutive duplicate
        if out.last().is_some_and(|prev| *prev == line) {
            continue;
        }
        last_tokens = tokens;
        out.push(line);
    }
    out.join("\n")
}

fn densify_json_structure(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text.trim()) {
        Ok(v) => {
            let compacted = compact_value(&v, 0);
            serde_json::to_string(&compacted).unwrap_or_else(|_| text.to_string())
        }
        Err(_) => text.to_string(),
    }
}

fn compact_value(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    use serde_json::{json, Value};
    match value {
        Value::Array(arr) if arr.len() > 8 && depth > 0 => {
            let head: Vec<Value> = arr
                .iter()
                .take(3)
                .map(|v| compact_value(v, depth + 1))
                .collect();
            let tail: Vec<Value> = arr
                .iter()
                .rev()
                .take(1)
                .map(|v| compact_value(v, depth + 1))
                .collect();
            let mut out = head;
            out.push(json!(format!("…(+{} items)", arr.len().saturating_sub(4))));
            out.extend(tail);
            Value::Array(out)
        }
        Value::Object(map) if map.len() > 20 => {
            let mut out = serde_json::Map::new();
            for (k, v) in map.iter().take(15) {
                out.insert(k.clone(), compact_value(v, depth + 1));
            }
            out.insert(
                "_truncated_keys".into(),
                json!(map.len().saturating_sub(15)),
            );
            Value::Object(out)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), compact_value(v, depth + 1));
            }
            Value::Object(out)
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| compact_value(v, depth + 1)).collect())
        }
        Value::String(s) if s.len() > 240 => {
            Value::String(format!("{}…(+{}c)", &s[..200], s.len() - 200))
        }
        other => other.clone(),
    }
}

fn compress_logs(text: &str) -> String {
    // Group identical message bodies after stripping leading timestamps.
    let ts = Regex::new(
        r"(?x)^
        (?:\d{4}-\d{2}-\d{2}[T\s]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?\s*)?
        (?:\[[^\]]+\]\s*)?
        ",
    )
    .expect("timestamp regex");

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for line in text.lines() {
        let body = ts.replace(line, "").trim().to_string();
        if body.is_empty() {
            continue;
        }
        let entry = counts.entry(body.clone()).or_insert_with(|| {
            order.push(body.clone());
            0
        });
        *entry += 1;
    }

    order
        .into_iter()
        .map(|body| {
            let n = counts[&body];
            if n > 1 {
                format!("×{n} {body}")
            } else {
                body
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn compress_code(text: &str) -> String {
    // Keep signatures / structural lines; collapse pure brace/indent noise and long comments.
    let mut out = Vec::new();
    let mut in_block_comment = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("/*") {
            in_block_comment = true;
        }
        if in_block_comment {
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with('#') && !trimmed.starts_with("#!") {
            // Keep doc-ish or TODO/FIXME
            if trimmed.contains("TODO")
                || trimmed.contains("FIXME")
                || trimmed.contains("NOTE")
                || trimmed.starts_with("///")
                || trimmed.starts_with("//!")
            {
                out.push(trimmed.to_string());
            }
            continue;
        }
        if matches!(trimmed, "{" | "}" | "};" | "}," | "(" | ")" | ");") {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out.join("\n")
}

fn compress_conversation(text: &str) -> String {
    // Collapse repeated assistant/user boilerplate; keep role headers + content densified.
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if out.last().is_some_and(|prev: &String| prev == t) {
            continue;
        }
        out.push(t.to_string());
    }
    out.join("\n")
}

fn hard_truncate(text: &str, max_tokens: usize, config: &Config) -> String {
    if estimate_tokens(text, config) <= max_tokens {
        return text.to_string();
    }
    let budget = ((max_tokens as f64) * config.chars_per_token * 0.95) as usize;
    let mut end = budget.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    // Prefer cutting at a newline
    if let Some(nl) = text[..end].rfind('\n') {
        if nl > end / 2 {
            end = nl;
        }
    }
    format!("{}\n…[truncated to ~{} tokens]", &text[..end], max_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_reduces_noisy_logs() {
        let input = r#"
2024-01-01T00:00:01Z INFO starting
2024-01-01T00:00:02Z INFO starting
2024-01-01T00:00:03Z INFO starting
2024-01-01T00:00:04Z ERROR boom path=/tmp/x
"#;
        let result = compress(
            input,
            &CompressOptions {
                content_type: ContentType::Log,
                max_tokens: Some(200),
                force: true,
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(result.content.contains("×3"));
        assert!(result.metrics.result_tokens < result.metrics.original_tokens);
        assert!(!result.bypassed);
    }

    #[test]
    fn extracts_urls_and_paths() {
        let input = "see https://example.com/docs and /home/user/proj/src/main.rs for details";
        let result = compress(
            input,
            &CompressOptions {
                force: true,
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(!result.entities.is_empty());
    }

    #[test]
    fn bypasses_short_input_by_default() {
        let result = compress("tiny", &CompressOptions::default(), &Config::default());
        assert!(result.bypassed);
        assert_eq!(result.content, "tiny");
    }
}
