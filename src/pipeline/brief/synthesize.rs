//! Status synthesis (local SLM with heuristic fallback).

use crate::config::Config;
use crate::pipeline::smart::{summarize_smart, SmartBackend, SmartOptions};
use crate::pipeline::summarize::SummarizeOptions;

use super::pack::EvidenceItem;
use super::walk::SelectedFile;

pub(crate) fn synthesize_status(
    task: &str,
    sources_block: &str,
    evidence_block: &str,
    heuristic_status: &str,
    config: &Config,
) -> (String, String, Option<String>, Option<String>) {
    let input = format!(
        "Task: {task}\n\nSources:\n{sources_block}\n\nEvidence excerpts:\n{evidence_block}\n"
    );
    let smart = summarize_smart(
        &input,
        &SmartOptions {
            max_tokens: Some(360),
            fallback: true,
            force: true,
            system_prompt: Some(
                "You write a compact project briefing Status for a coding agent. \
                 Output ONLY markdown with these headings and short bullets:\n\
                 ### Status\n### Gaps\n### Next\n\
                 Use only facts present in the provided Sources/Evidence. Do not invent. \
                 If uncertain, put a caveat under Gaps. No preamble."
                    .into(),
            ),
            ..Default::default()
        },
        &SummarizeOptions {
            force: true,
            max_tokens: Some(360),
            ..Default::default()
        },
        config,
    );

    match smart {
        Ok(result)
            if result.backend == SmartBackend::LocalLlm && !result.summary.trim().is_empty() =>
        {
            (
                result.summary.trim().to_string(),
                "local_llm".into(),
                result.model,
                None,
            )
        }
        Ok(result) => (
            heuristic_status.to_string(),
            "heuristic".into(),
            None,
            result.fallback_reason.or_else(|| {
                if result.backend == SmartBackend::Heuristic {
                    Some("status synthesized heuristically".into())
                } else {
                    None
                }
            }),
        ),
        Err(e) => (
            heuristic_status.to_string(),
            "heuristic".into(),
            None,
            Some(e),
        ),
    }
}

pub(crate) fn build_heuristic_status(
    task: &str,
    kept: &[(SelectedFile, f64)],
    evidence: &[EvidenceItem],
) -> String {
    let mut bullets: Vec<String> = Vec::new();
    bullets.push(format!("- Task focus: {task}"));
    for (file, score) in kept.iter().take(5) {
        let preview = evidence
            .iter()
            .find(|e| e.rel == file.rel)
            .map(|e| first_useful_line(&e.text))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| first_useful_line(&file.content));
        let preview = if preview.is_empty() {
            String::new()
        } else {
            format!(" — {preview}")
        };
        bullets.push(format!(
            "- {} ({}, score {:.2}){preview}",
            file.rel, file.kind, score
        ));
    }
    let gaps = if kept.iter().any(|(f, _)| f.truncated) {
        "- Some large files were window-truncated; open Read next paths for full context."
    } else {
        "- Confirm Status against primary sources before acting."
    };
    format!(
        "### Status\n{}\n### Gaps\n{gaps}\n### Next\n- Read the top Sources paths, then implement against code not docs alone.",
        bullets.join("\n")
    )
}

pub(crate) fn first_useful_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty()
                && !l.starts_with("//")
                && !l.starts_with("/*")
                && !l.starts_with('*')
                && *l != "---"
        })
        .map(|l| {
            let s: String = l.chars().take(120).collect();
            s
        })
        .unwrap_or_default()
}
