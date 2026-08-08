//! Evidence packing and Read-next suggestions.

use std::collections::HashSet;

use crate::config::Config;
use crate::pipeline::tokens::estimate_tokens;

use super::window::floor_char_boundary;
use super::BriefSource;

/// Soft evidence caps.
pub(crate) const MAX_CHUNKS_PER_FILE: usize = 2;
pub(crate) const MAX_TOKENS_PER_EXCERPT: usize = 400;

pub(crate) struct EvidenceItem {
    pub(crate) rel: String,
    pub(crate) kind: String,
    pub(crate) score: f64,
    pub(crate) text: String,
}

pub(crate) fn build_read_next(task: &str, sources: &[BriefSource], config: &Config) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut seen = HashSet::new();

    let push = |lines: &mut Vec<String>, seen: &mut HashSet<String>, line: String| {
        if seen.insert(line.clone()) {
            lines.push(line);
        }
    };

    for s in sources.iter().take(4) {
        push(&mut lines, &mut seen, format!("- {}", s.path));
    }

    let suggestions = crate::pipeline::playbook::suggest_playbooks(task, config, 3);
    for ad in &suggestions {
        push(
            &mut lines,
            &mut seen,
            format!(
                "- playbook `{}` → `{}` (or action=playbook id={})",
                ad.id, ad.uri, ad.id
            ),
        );
    }

    // Stable skill URIs so agents always get progressive-disclosure next steps.
    let q = task.to_lowercase();
    push(
        &mut lines,
        &mut seen,
        "- skill `brief` → `cmp://skill/action/brief` (or action=help id=brief)".into(),
    );
    if q.contains("rerank")
        || q.contains("rank")
        || q.contains("retriev")
        || q.contains("chunk")
        || suggestions.iter().any(|a| a.id.contains("rerank"))
    {
        push(
            &mut lines,
            &mut seen,
            "- skill `rerank` → `cmp://skill/action/rerank` (or action=help id=rerank)".into(),
        );
    }
    if q.contains("sanit") || q.contains("untrusted") || q.contains("secret") || q.contains("ipi") {
        push(
            &mut lines,
            &mut seen,
            "- skill `sanitize` → `cmp://skill/action/sanitize` (or action=help id=sanitize)"
                .into(),
        );
        if !suggestions.iter().any(|a| a.id == "sanitize-untrusted") {
            push(
                &mut lines,
                &mut seen,
                "- playbook `sanitize-untrusted` → `cmp://skill/playbook/sanitize-untrusted`"
                    .into(),
            );
        }
    }
    if q.contains("stat") || q.contains("telemetry") || q.contains("latency") {
        push(
            &mut lines,
            &mut seen,
            "- skill `stats` → `cmp://skill/action/stats` (or action=help id=stats)".into(),
        );
    }
    if !suggestions.iter().any(|a| a.id == "workspace-brief") {
        push(
            &mut lines,
            &mut seen,
            "- playbook `workspace-brief` → `cmp://skill/playbook/workspace-brief`".into(),
        );
    }
    if suggestions.iter().any(|a| a.id == "brief-then-rerank")
        || (q.contains("brief") && q.contains("rerank"))
    {
        push(
            &mut lines,
            &mut seen,
            "- playbook `brief-then-rerank` → `cmp://skill/playbook/brief-then-rerank`".into(),
        );
    }

    if lines.is_empty() {
        "- (none)".into()
    } else {
        lines.join("\n")
    }
}
pub(crate) fn pack_evidence_budget(
    items: &[EvidenceItem],
    budget_tokens: usize,
    config: &Config,
) -> String {
    if items.is_empty() {
        return "_no evidence_".into();
    }

    let code_budget = budget_tokens * 60 / 100;
    let doc_budget = budget_tokens * 25 / 100;
    let other_budget = budget_tokens.saturating_sub(code_budget + doc_budget);

    let mut used_code = 0usize;
    let mut used_doc = 0usize;
    let mut used_other = 0usize;
    let mut parts: Vec<String> = Vec::new();

    for item in items {
        let cost = estimate_tokens(&item.text, config).max(1);
        let remaining_global = budget_tokens.saturating_sub(used_code + used_doc + used_other);
        let (used_so_far, cap) = match item.kind.as_str() {
            "code" => (used_code, code_budget),
            "doc" => (used_doc, doc_budget),
            _ => (used_other, other_budget),
        };
        if used_so_far >= cap && remaining_global < cost {
            continue;
        }
        if used_so_far + cost > cap + remaining_global {
            let room = cap
                .saturating_add(remaining_global)
                .saturating_sub(used_so_far);
            if room < 40 {
                continue;
            }
            let clipped = truncate_tokens(&item.text, room, config);
            let clipped_cost = estimate_tokens(&clipped, config);
            match item.kind.as_str() {
                "code" => used_code += clipped_cost,
                "doc" => used_doc += clipped_cost,
                _ => used_other += clipped_cost,
            }
            parts.push(format!(
                "### {} (score {:.3})\n```\n{}\n```",
                item.rel, item.score, clipped
            ));
            continue;
        }
        match item.kind.as_str() {
            "code" => used_code += cost,
            "doc" => used_doc += cost,
            _ => used_other += cost,
        }
        parts.push(format!(
            "### {} (score {:.3})\n```\n{}\n```",
            item.rel, item.score, item.text
        ));
    }

    if parts.is_empty() {
        // Fallback: take first item truncated
        let item = &items[0];
        let clipped = truncate_tokens(&item.text, budget_tokens.min(400), config);
        format!(
            "### {} (score {:.3})\n```\n{}\n```",
            item.rel, item.score, clipped
        )
    } else {
        parts.join("\n\n")
    }
}
pub(crate) fn truncate_tokens(text: &str, max_tokens: usize, config: &Config) -> String {
    if estimate_tokens(text, config) <= max_tokens {
        return text.to_string();
    }
    // Approximate chars from heuristic chars_per_token.
    let approx_chars = (max_tokens as f64 * config.chars_per_token) as usize;
    let end = floor_char_boundary(text, approx_chars.min(text.len()));
    let mut out = text[..end].trim_end().to_string();
    // Prefer cutting at newline
    if let Some(pos) = out.rfind('\n') {
        if pos > out.len() / 2 {
            out.truncate(pos);
        }
    }
    out.push_str("\n…");
    out
}
