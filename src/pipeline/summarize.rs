//! Hierarchical summarization for conversation histories and file trees.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::signal::{bypass_reason, should_bypass_signal};
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for [`summarize`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SummarizeOptions {
    /// Maximum depth of the hierarchy (1 = top-level only).
    #[serde(default = "default_depth")]
    pub max_depth: usize,
    /// Soft cap on output tokens.
    pub max_tokens: Option<usize>,
    /// How to interpret the input.
    #[serde(default)]
    pub mode: SummarizeMode,
    /// Max bullet points per section.
    #[serde(default = "default_bullets")]
    pub max_bullets_per_section: usize,
    /// When true, skip the signal-to-call short-input bypass.
    #[serde(default)]
    pub force: bool,
}

fn default_depth() -> usize {
    3
}
fn default_bullets() -> usize {
    8
}

impl Default for SummarizeOptions {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_tokens: None,
            mode: SummarizeMode::Auto,
            max_bullets_per_section: 8,
            force: false,
        }
    }
}

/// Summarization strategy.
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SummarizeMode {
    #[default]
    Auto,
    Conversation,
    FileTree,
    Outline,
}

/// Hierarchical summary result.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SummarizeResult {
    pub summary: String,
    pub sections: Vec<SummarySection>,
    pub metrics: TokenMetrics,
    /// True when input was below the signal threshold and left unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypassed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_reason: Option<String>,
}

/// One node in the summary hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SummarySection {
    pub title: String,
    pub level: usize,
    pub bullets: Vec<String>,
    pub children: Vec<SummarySection>,
}

/// Build a hierarchical summary of `input`.
pub fn summarize(input: &str, options: &SummarizeOptions, config: &Config) -> SummarizeResult {
    let original_tokens = estimate_tokens(input, config);

    if should_bypass_signal(input, config, options.force) {
        return SummarizeResult {
            summary: input.to_string(),
            sections: Vec::new(),
            metrics: TokenMetrics::new(original_tokens, original_tokens),
            bypassed: true,
            bypass_reason: Some(bypass_reason(config)),
        };
    }

    let mode = detect_mode(input, &options.mode);

    let sections = match mode {
        SummarizeMode::Conversation => summarize_conversation(input, options),
        SummarizeMode::FileTree => summarize_file_tree(input, options),
        SummarizeMode::Outline | SummarizeMode::Auto => summarize_outline(input, options),
    };

    let mut summary = render_sections(&sections, options.max_depth);
    if let Some(max) = options.max_tokens {
        if estimate_tokens(&summary, config) > max {
            summary = truncate_summary(&summary, max, config);
        }
    }

    let result_tokens = estimate_tokens(&summary, config);
    SummarizeResult {
        summary,
        sections,
        metrics: TokenMetrics::new(original_tokens, result_tokens),
        bypassed: false,
        bypass_reason: None,
    }
}

fn detect_mode(input: &str, hint: &SummarizeMode) -> SummarizeMode {
    if *hint != SummarizeMode::Auto {
        return hint.clone();
    }
    let path_lines = input
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.contains('/')
                && (t.ends_with('/')
                    || t.contains('.')
                    || t.starts_with("├")
                    || t.starts_with("│")
                    || t.starts_with("└")
                    || t.starts_with("- "))
        })
        .count();
    if path_lines >= 5 {
        return SummarizeMode::FileTree;
    }
    let roles = input
        .lines()
        .filter(|l| {
            let lower = l.to_ascii_lowercase();
            lower.starts_with("user:")
                || lower.starts_with("assistant:")
                || lower.starts_with("system:")
                || lower.starts_with("**user**")
                || lower.starts_with("**assistant**")
        })
        .count();
    if roles >= 2 {
        return SummarizeMode::Conversation;
    }
    SummarizeMode::Outline
}

fn summarize_conversation(input: &str, options: &SummarizeOptions) -> Vec<SummarySection> {
    let mut turns: Vec<(String, String)> = Vec::new();
    let mut current_role = String::from("narration");
    let mut buf = String::new();

    let push = |turns: &mut Vec<(String, String)>, role: &str, buf: &mut String| {
        let body = buf.trim().to_string();
        if !body.is_empty() {
            turns.push((role.to_string(), body));
        }
        buf.clear();
    };

    for line in input.lines() {
        let lower = line.trim().to_ascii_lowercase();
        let role = if lower.starts_with("user:") || lower.starts_with("**user**") {
            Some("user")
        } else if lower.starts_with("assistant:") || lower.starts_with("**assistant**") {
            Some("assistant")
        } else if lower.starts_with("system:") || lower.starts_with("**system**") {
            Some("system")
        } else {
            None
        };

        if let Some(r) = role {
            push(&mut turns, &current_role, &mut buf);
            current_role = r.to_string();
            if let Some((_, rest)) = line.split_once(':') {
                buf.push_str(rest.trim());
                buf.push('\n');
            }
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    push(&mut turns, &current_role, &mut buf);

    let mut root_bullets = Vec::new();
    let mut children = Vec::new();

    for (idx, (role, body)) in turns.iter().enumerate() {
        let gist = first_sentence(body, 160);
        root_bullets.push(format!("T{} [{role}]: {gist}", idx + 1));
        if options.max_depth > 1 {
            let bullets = key_phrases(body, options.max_bullets_per_section);
            children.push(SummarySection {
                title: format!("Turn {} ({role})", idx + 1),
                level: 2,
                bullets,
                children: Vec::new(),
            });
        }
    }

    root_bullets.truncate(options.max_bullets_per_section * 2);

    vec![SummarySection {
        title: format!("Conversation ({} turns)", turns.len()),
        level: 1,
        bullets: root_bullets,
        children,
    }]
}

fn summarize_file_tree(input: &str, options: &SummarizeOptions) -> Vec<SummarySection> {
    let mut dirs: Vec<(usize, String)> = Vec::new();
    let mut files_by_ext: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut file_count = 0usize;

    for line in input.lines() {
        let cleaned = line
            .trim()
            .trim_start_matches(|c: char| {
                matches!(c, '│' | '├' | '└' | '─' | '|' | '`' | '+' | '-' | ' ')
            })
            .trim();
        if cleaned.is_empty() {
            continue;
        }
        let depth = line.chars().take_while(|c| c.is_whitespace() || "│|".contains(*c)).count() / 2;
        if cleaned.ends_with('/') || !cleaned.contains('.') {
            dirs.push((depth, cleaned.trim_end_matches('/').to_string()));
        } else {
            file_count += 1;
            let ext = cleaned
                .rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .unwrap_or_else(|| "(none)".into());
            *files_by_ext.entry(ext).or_default() += 1;
        }
    }

    let mut bullets = vec![format!("{file_count} files across {} directories", dirs.len())];
    for (ext, n) in files_by_ext.iter().take(options.max_bullets_per_section) {
        bullets.push(format!(".{ext}: {n}"));
    }

    let top_dirs: Vec<_> = dirs
        .iter()
        .filter(|(d, _)| *d <= 1)
        .take(options.max_bullets_per_section)
        .map(|(_, name)| name.clone())
        .collect();

    let children = if options.max_depth > 1 {
        top_dirs
            .iter()
            .map(|name| SummarySection {
                title: name.clone(),
                level: 2,
                bullets: dirs
                    .iter()
                    .filter(|(d, child)| *d > 0 && child.starts_with(name.as_str()))
                    .take(4)
                    .map(|(_, c)| c.clone())
                    .collect(),
                children: Vec::new(),
            })
            .collect()
    } else {
        Vec::new()
    };

    vec![SummarySection {
        title: "File tree".into(),
        level: 1,
        bullets,
        children,
    }]
}

fn summarize_outline(input: &str, options: &SummarizeOptions) -> Vec<SummarySection> {
    let mut sections: Vec<SummarySection> = Vec::new();
    let mut current: Option<SummarySection> = None;

    let flush = |sections: &mut Vec<SummarySection>, current: &mut Option<SummarySection>| {
        if let Some(sec) = current.take() {
            sections.push(sec);
        }
    };

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let heading = heading_level(trimmed);
        if let Some(level) = heading {
            flush(&mut sections, &mut current);
            let title = trimmed
                .trim_start_matches('#')
                .trim()
                .trim_end_matches(':')
                .to_string();
            current = Some(SummarySection {
                title,
                level,
                bullets: Vec::new(),
                children: Vec::new(),
            });
            continue;
        }

        if let Some(sec) = current.as_mut() {
            if sec.bullets.len() < options.max_bullets_per_section {
                let bullet = first_sentence(trimmed.trim_start_matches(['-', '*', '•', ' ']), 140);
                if !bullet.is_empty() {
                    sec.bullets.push(bullet);
                }
            }
        }
    }
    flush(&mut sections, &mut current);

    if sections.is_empty() {
        // Paragraph fallback: chunk into pseudo-sections
        let paras: Vec<_> = input
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        for (i, para) in paras.iter().take(options.max_bullets_per_section).enumerate() {
            sections.push(SummarySection {
                title: format!("Section {}", i + 1),
                level: 1,
                bullets: key_phrases(para, options.max_bullets_per_section.min(4)),
                children: Vec::new(),
            });
        }
    }

    // Nest by level if depth allows
    if options.max_depth > 1 {
        nest_sections(sections)
    } else {
        sections
            .into_iter()
            .map(|mut s| {
                s.children.clear();
                s
            })
            .collect()
    }
}

fn nest_sections(flat: Vec<SummarySection>) -> Vec<SummarySection> {
    let mut roots: Vec<SummarySection> = Vec::new();
    let mut stack: Vec<SummarySection> = Vec::new();

    for section in flat {
        while stack.last().is_some_and(|s| s.level >= section.level) {
            let finished = stack.pop().expect("stack non-empty");
            if let Some(parent) = stack.last_mut() {
                parent.children.push(finished);
            } else {
                roots.push(finished);
            }
        }
        stack.push(section);
    }
    while let Some(finished) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(finished);
        } else {
            roots.push(finished);
        }
    }
    roots
}

fn heading_level(line: &str) -> Option<usize> {
    if line.starts_with('#') {
        let n = line.chars().take_while(|c| *c == '#').count();
        if n >= 1 && n <= 6 {
            return Some(n);
        }
    }
    // ALL-CAPS short title
    if line.len() < 60
        && line
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(|c| c.is_uppercase())
        && line.chars().any(|c| c.is_alphabetic())
    {
        return Some(1);
    }
    None
}

fn first_sentence(text: &str, max_chars: usize) -> String {
    let flat = text.replace('\n', " ");
    let cut = flat
        .find(['.', '!', '?'])
        .map(|i| i + 1)
        .unwrap_or(flat.len())
        .min(max_chars)
        .min(flat.len());
    let mut end = cut;
    while end > 0 && !flat.is_char_boundary(end) {
        end -= 1;
    }
    let mut s = flat[..end].trim().to_string();
    if flat.len() > end {
        s.push('…');
    }
    s
}

fn key_phrases(text: &str, limit: usize) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.len() > 20)
        .take(limit)
        .map(|l| first_sentence(l, 120))
        .collect()
}

fn render_sections(sections: &[SummarySection], max_depth: usize) -> String {
    fn walk(sec: &SummarySection, max_depth: usize, out: &mut String) {
        if sec.level > max_depth {
            return;
        }
        let indent = "  ".repeat(sec.level.saturating_sub(1));
        out.push_str(&format!("{indent}{} {}\n", "#".repeat(sec.level.min(6)), sec.title));
        for b in &sec.bullets {
            out.push_str(&format!("{indent}- {b}\n"));
        }
        for child in &sec.children {
            walk(child, max_depth, out);
        }
    }
    let mut out = String::new();
    for s in sections {
        walk(s, max_depth, &mut out);
    }
    out
}

fn truncate_summary(text: &str, max_tokens: usize, config: &Config) -> String {
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
    fn summarizes_markdown_outline() {
        let input = r#"
# Intro
Welcome to the system overview document here.
## Setup
Install dependencies and configure the environment carefully.
## Usage
Run the binary with stdio transport enabled always.
"#;
        let result = summarize(input, &SummarizeOptions {
            force: true,
            ..Default::default()
        }, &Config::default());
        assert!(result.summary.contains("Intro"));
        assert!(!result.sections.is_empty());
        assert!(result.metrics.result_tokens > 0);
        assert!(!result.bypassed);
    }

    #[test]
    fn summarizes_conversation() {
        let input = "user: How do I build this?\nassistant: Run cargo build --release.\nuser: Thanks!";
        let result = summarize(
            input,
            &SummarizeOptions {
                mode: SummarizeMode::Conversation,
                force: true,
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(result.summary.to_lowercase().contains("conversation"));
    }

    #[test]
    fn bypasses_short_input_by_default() {
        let result = summarize("hi", &SummarizeOptions::default(), &Config::default());
        assert!(result.bypassed);
        assert_eq!(result.summary, "hi");
    }
}
