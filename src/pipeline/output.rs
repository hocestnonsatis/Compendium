//! Domain-aware compression of command / tool stdout and stderr.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::filter::{filter, FilterOptions};
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Output domain for specialized noise stripping.
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputDomain {
    #[default]
    Auto,
    Generic,
    Git,
    Cargo,
    Npm,
    Docker,
    Kubectl,
    Json,
    Rustc,
    Pytest,
    Gcc,
}

/// Options for [`compress_output`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CompressOutputOptions {
    #[serde(default)]
    pub domain: OutputDomain,
    /// Soft cap after domain filtering.
    pub max_tokens: Option<usize>,
    /// Keep head and tail when truncating long streams (like `head`+`tail`).
    #[serde(default = "default_true")]
    pub keep_head_tail: bool,
    #[serde(default = "default_head_lines")]
    pub head_lines: usize,
    #[serde(default = "default_tail_lines")]
    pub tail_lines: usize,
}

fn default_true() -> bool {
    true
}
fn default_head_lines() -> usize {
    40
}
fn default_tail_lines() -> usize {
    40
}

impl Default for CompressOutputOptions {
    fn default() -> Self {
        Self {
            domain: OutputDomain::Auto,
            max_tokens: None,
            keep_head_tail: true,
            head_lines: 40,
            tail_lines: 40,
        }
    }
}

/// Result of domain-aware output compression.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CompressOutputResult {
    pub content: String,
    pub domain: String,
    pub lines_in: usize,
    pub lines_out: usize,
    pub metrics: TokenMetrics,
}

/// Strip domain-specific noise from tool/command output.
pub fn compress_output(
    input: &str,
    options: &CompressOutputOptions,
    config: &Config,
) -> CompressOutputResult {
    let original_tokens = estimate_tokens(input, config);
    let lines_in = input.lines().count();
    let domain = detect_domain(input, &options.domain);

    let mut text = match domain {
        OutputDomain::Git => filter_git(input),
        OutputDomain::Cargo => filter_cargo(input),
        OutputDomain::Npm => filter_npm(input),
        OutputDomain::Docker => filter_docker(input),
        OutputDomain::Kubectl => filter_kubectl(input),
        OutputDomain::Json => filter_json_blob(input),
        OutputDomain::Rustc => filter_rustc(input),
        OutputDomain::Pytest => filter_pytest(input),
        OutputDomain::Gcc => filter_gcc(input),
        OutputDomain::Generic | OutputDomain::Auto => input.to_string(),
    };

    // Always apply generic scrub (ANSI, blank runs, boilerplate).
    let filtered = filter(
        &text,
        &FilterOptions {
            strip_ansi: true,
            collapse_whitespace: true,
            strip_boilerplate: true,
            densify_json: matches!(domain, OutputDomain::Json),
            keep_patterns: Vec::new(),
            drop_patterns: domain_drop_patterns(&domain),
            max_tokens: None,
            query: None,
        },
        config,
    );
    text = filtered.content;

    if options.keep_head_tail {
        text = head_tail(&text, options.head_lines, options.tail_lines);
    }

    if let Some(max) = options.max_tokens {
        if estimate_tokens(&text, config) > max {
            text = head_tail_tokens(&text, max, config);
        }
    }

    let result_tokens = estimate_tokens(&text, config);
    CompressOutputResult {
        lines_out: text.lines().count(),
        content: text,
        domain: domain_name(&domain).into(),
        lines_in,
        metrics: TokenMetrics::new(original_tokens, result_tokens),
    }
}

fn detect_domain(input: &str, hint: &OutputDomain) -> OutputDomain {
    if *hint != OutputDomain::Auto {
        return hint.clone();
    }
    let sample: String = input.chars().take(4_000).collect();
    let lower = sample.to_ascii_lowercase();

    if (sample.trim_start().starts_with('{') || sample.trim_start().starts_with('['))
        && serde_json::from_str::<serde_json::Value>(sample.trim()).is_ok()
    {
        return OutputDomain::Json;
    }
    if lower.contains("compiling ") && (lower.contains("cargo") || lower.contains("finished `")) {
        return OutputDomain::Cargo;
    }
    if lower.contains("error[e") || lower.contains(" --> src/") {
        return OutputDomain::Rustc;
    }
    if lower.contains("diff --git") || lower.contains("git status") || lower.starts_with("commit ")
    {
        return OutputDomain::Git;
    }
    if lower.contains("npm warn") || lower.contains("added ") && lower.contains("packages") {
        return OutputDomain::Npm;
    }
    if lower.contains("digest:") || lower.contains("pulling fs layer") || lower.contains("sha256:")
    {
        return OutputDomain::Docker;
    }
    if lower.contains("kubectl") || lower.contains("namespace/") || lower.contains("pod/") {
        return OutputDomain::Kubectl;
    }
    if lower.contains("===== test session") || lower.contains("passed in") {
        return OutputDomain::Pytest;
    }
    if lower.contains("gcc ") || lower.contains("collect2:") {
        return OutputDomain::Gcc;
    }
    OutputDomain::Generic
}

fn domain_name(d: &OutputDomain) -> &'static str {
    match d {
        OutputDomain::Auto => "auto",
        OutputDomain::Generic => "generic",
        OutputDomain::Git => "git",
        OutputDomain::Cargo => "cargo",
        OutputDomain::Npm => "npm",
        OutputDomain::Docker => "docker",
        OutputDomain::Kubectl => "kubectl",
        OutputDomain::Json => "json",
        OutputDomain::Rustc => "rustc",
        OutputDomain::Pytest => "pytest",
        OutputDomain::Gcc => "gcc",
    }
}

fn domain_drop_patterns(domain: &OutputDomain) -> Vec<String> {
    match domain {
        OutputDomain::Cargo => vec![
            r"(?i)^ {4,}Compiling ".into(),
            r"(?i)^ {4,}Downloading ".into(),
            r"(?i)^ {4,}Downloaded ".into(),
            r"(?i)^ {4,}Checking ".into(),
        ],
        OutputDomain::Npm => vec![r"(?i)^npm (warn|notice)".into(), r"(?i)^deprecated ".into()],
        OutputDomain::Docker => {
            vec![r"(?i)^[a-f0-9]{12}: (Waiting|Downloading|Extracting|Pull complete)".into()]
        }
        OutputDomain::Git => vec![r"(?i)^create mode ".into(), r"(?i)^delete mode ".into()],
        _ => Vec::new(),
    }
}

fn filter_git(input: &str) -> String {
    let mut out = Vec::new();
    let mut hunk_header = false;
    for line in input.lines() {
        if line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("@@")
            || line.starts_with("commit ")
            || line.starts_with("Author:")
            || line.starts_with("Date:")
        {
            out.push(line.to_string());
            hunk_header = line.starts_with("@@");
            continue;
        }
        // Keep changed lines and short context; skip pure context spam beyond 2 lines.
        if line.starts_with('+') || line.starts_with('-') || hunk_header {
            out.push(line.to_string());
            hunk_header = false;
            continue;
        }
        if line.starts_with(" ") {
            // drop excessive context
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

fn filter_cargo(input: &str) -> String {
    let mut out = Vec::new();
    let mut seen_compiling = 0usize;
    for line in input.lines() {
        let t = line.trim_start();
        if t.starts_with("Compiling ") {
            seen_compiling += 1;
            if seen_compiling <= 3 || t.contains("error") {
                out.push(line.to_string());
            }
            continue;
        }
        if t.starts_with("Downloading ")
            || t.starts_with("Downloaded ")
            || t.starts_with("Checking ")
        {
            continue;
        }
        out.push(line.to_string());
    }
    if seen_compiling > 3 {
        out.insert(0, format!("…[{seen_compiling} Compiling lines collapsed]"));
    }
    out.join("\n")
}

fn filter_npm(input: &str) -> String {
    input
        .lines()
        .filter(|l| {
            let lower = l.to_ascii_lowercase();
            !(lower.starts_with("npm warn")
                || lower.starts_with("npm notice")
                || lower.starts_with("deprecated "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_docker(input: &str) -> String {
    let progress = docker_progress_re();
    input
        .lines()
        .filter(|l| !progress.is_match(l))
        .collect::<Vec<_>>()
        .join("\n")
}

fn docker_progress_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^[a-f0-9]{8,}:\s+(Waiting|Downloading|Extracting|Pull complete|Verifying)")
            .expect("docker progress regex")
    })
}

fn filter_kubectl(input: &str) -> String {
    // Prefer name + status columns; drop wide AGE noise duplicates by collapsing spaces.
    input
        .lines()
        .map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() >= 3 && parts[0] != "NAME" {
                // NAME READY STATUS …
                format!(
                    "{} {} {}",
                    parts[0],
                    parts.get(1).unwrap_or(&""),
                    parts.get(2).unwrap_or(&"")
                )
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_json_blob(input: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(input.trim()) {
        Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| input.to_string()),
        Err(_) => input.to_string(),
    }
}

fn filter_rustc(input: &str) -> String {
    let mut out = Vec::new();
    for line in input.lines() {
        let t = line.trim_start();
        if t.starts_with("= note: `#[warn") || t.starts_with("= help: consider") {
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

fn filter_pytest(input: &str) -> String {
    let mut out = Vec::new();
    for line in input.lines() {
        let t = line.trim();
        // Keep failures / summary; drop long PASSED spam.
        if t.ends_with(" PASSED") && !t.contains("FAILED") {
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

fn filter_gcc(input: &str) -> String {
    input
        .lines()
        .filter(|l| {
            let lower = l.to_ascii_lowercase();
            lower.contains("error")
                || lower.contains("warning")
                || lower.contains("undefined")
                || !lower.contains("in function")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn head_tail(text: &str, head: usize, tail: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= head.saturating_add(tail) + 1 {
        return text.to_string();
    }
    let mut out = String::new();
    for l in &lines[..head] {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(&format!(
        "…[{} lines omitted]…\n",
        lines.len() - head - tail
    ));
    for l in &lines[lines.len() - tail..] {
        out.push_str(l);
        out.push('\n');
    }
    out
}

fn head_tail_tokens(text: &str, max_tokens: usize, config: &Config) -> String {
    if estimate_tokens(text, config) <= max_tokens {
        return text.to_string();
    }
    let budget_chars = (max_tokens as f64 * config.chars_per_token) as usize;
    let head = budget_chars / 2;
    let tail = budget_chars / 2;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= head + tail {
        return text.to_string();
    }
    let head_s: String = chars[..head].iter().collect();
    let tail_s: String = chars[chars.len() - tail..].iter().collect();
    format!("{head_s}\n…[truncated]…\n{tail_s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_and_collapses_cargo() {
        let input = "\
   Compiling foo v0.1.0
   Compiling bar v0.1.0
   Compiling baz v0.1.0
   Compiling qux v0.1.0
   Compiling zap v0.1.0
    Finished `dev` profile [unoptimized] target(s) in 1.2s
";
        let result = compress_output(input, &CompressOutputOptions::default(), &Config::default());
        assert_eq!(result.domain, "cargo");
        assert!(result.content.contains("Finished") || result.content.contains("collapsed"));
        assert!(result.metrics.result_tokens <= result.metrics.original_tokens);
    }

    #[test]
    fn densifies_json_domain() {
        let input = "{\n  \"a\": 1,\n  \"b\": [1, 2, 3]\n}";
        let result = compress_output(
            input,
            &CompressOutputOptions {
                domain: OutputDomain::Json,
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(!result.content.contains('\n') || result.content.len() < input.len());
    }
}
