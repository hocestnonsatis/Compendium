//! Workspace briefing: scan a local root for task-relevant slices and pack them.
//!
//! Packaging v2: structured starter pack (Status / Evidence / Caveats / Sources /
//! Read next), query-aware windows for large files, doc/code budget mix, and
//! optional local-SLM Status synthesis with heuristic fallback.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::bm25::score_documents;
use crate::pipeline::chunk::{chunk_with_refs, ChunkOptions};
use crate::pipeline::filter::{filter, FilterOptions};
use crate::pipeline::rerank::{rerank, RerankItem, RerankOptions};
use crate::pipeline::sanitize::{sanitize, SanitizeOptions};
use crate::pipeline::smart::{summarize_smart, SmartBackend, SmartOptions};
use crate::pipeline::summarize::SummarizeOptions;
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for [`brief`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct BriefOptions {
    /// Workspace root. When omitted, uses the process current directory.
    pub root: Option<String>,
    /// Max path candidates after path-level BM25 (before content read).
    #[serde(default = "default_max_files")]
    pub max_files: usize,
    /// Per-file read / window budget in bytes.
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: usize,
    /// Total bytes budget across selected file reads.
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: usize,
    /// Soft cap on packed briefing tokens.
    pub max_brief_tokens: Option<usize>,
    /// Keep top K content chunks after rerank (before per-file / token caps).
    #[serde(default = "default_top_k_chunks")]
    pub top_k_chunks: usize,
    /// Allowed file extensions (without dot). Empty / None → built-in allowlist.
    pub extensions: Option<Vec<String>>,
    /// Follow symlinks while walking (default false; still rejects escape outside root).
    #[serde(default)]
    pub follow_links: bool,
}

fn default_max_files() -> usize {
    40
}
fn default_max_file_bytes() -> usize {
    256 * 1024
}
fn default_max_total_bytes() -> usize {
    2 * 1024 * 1024
}
fn default_top_k_chunks() -> usize {
    12
}

impl Default for BriefOptions {
    fn default() -> Self {
        Self {
            root: None,
            max_files: default_max_files(),
            max_file_bytes: default_max_file_bytes(),
            max_total_bytes: default_max_total_bytes(),
            max_brief_tokens: None,
            top_k_chunks: default_top_k_chunks(),
            extensions: None,
            follow_links: false,
        }
    }
}

/// One selected source file in the briefing.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BriefSource {
    /// Path relative to the workspace root.
    pub path: String,
    pub score: f64,
    pub bytes: usize,
    /// True when the file exceeded the per-file budget and was windowed.
    #[serde(default)]
    pub truncated: bool,
    /// File mtime as unix seconds (if available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_secs: Option<u64>,
    /// `"code"` | `"doc"` | `"other"`.
    #[serde(default = "default_kind_other")]
    pub kind: String,
}

fn default_kind_other() -> String {
    "other".into()
}

/// Result of [`brief`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BriefResult {
    pub task: String,
    pub root: String,
    pub briefing: String,
    /// Suggested session-cache key (`cache://brief/…`); server stores the payload.
    pub cache_key: String,
    pub sources: Vec<BriefSource>,
    pub scanned_files: usize,
    pub selected_files: usize,
    pub metrics: TokenMetrics,
    /// `"local_llm"` when Status was synthesized by SLM; otherwise `"heuristic"`.
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

const HARD_SKIP_DIR_NAMES: &[&str] = &[".git", "node_modules", "target", "dist", "build", ".next"];
const HARD_SKIP_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "composer.lock",
    "Gemfile.lock",
];

const DEFAULT_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "md", "toml", "json", "yaml", "yml", "go",
    "java", "kt", "c", "h", "cpp", "hpp", "cc", "cs", "rb", "php", "sql", "sh", "bash", "zsh",
    "svelte", "vue", "css", "scss", "html", "txt", "xml", "gradle", "cmake", "swift", "m", "mm",
    "r", "lua", "pl", "pm", "ex", "exs", "erl", "hs", "scala", "dart", "proto", "graphql", "tf",
    "hcl", "nix", "zig", "gd", "wgsl",
];

const CODE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "c", "h", "cpp", "hpp",
    "cc", "cs", "rb", "php", "sql", "svelte", "vue", "swift", "scala", "dart", "ex", "exs", "zig",
];

/// Soft evidence caps.
const MAX_CHUNKS_PER_FILE: usize = 2;
const MAX_TOKENS_PER_EXCERPT: usize = 400;
const WINDOW_TARGET_BYTES: usize = 80 * 1024;
const MAX_WINDOWS: usize = 3;
/// How much of an oversized file we may load to score windows.
const SCORE_READ_CAP: usize = 2 * 1024 * 1024;
/// Docs older than code median by this many seconds → stale caveat.
const STALE_DOC_SECS: u64 = 7 * 24 * 3600;

struct SelectedFile {
    abs: PathBuf,
    rel: String,
    path_score: f64,
    content: String,
    bytes: usize,
    truncated: bool,
    mtime_secs: Option<u64>,
    kind: String,
}

struct EvidenceItem {
    rel: String,
    kind: String,
    score: f64,
    text: String,
}

/// Scan `options.root` (or cwd) for files relevant to `query` and pack a compact briefing.
///
/// Optional `hint` is folded into scoring only (not pasted wholesale into the briefing).
pub fn brief(
    query: &str,
    hint: Option<&str>,
    options: &BriefOptions,
    config: &Config,
) -> Result<BriefResult, String> {
    let task = query.trim();
    if task.is_empty() {
        return Err("brief requires a non-empty `query` (task description)".into());
    }

    let root = resolve_root(options.root.as_deref())?;
    let root_str = root.to_string_lossy().to_string();
    let allow_ext = extension_set(options.extensions.as_ref());

    let score_query = match hint.map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => format!("{task}\n{h}"),
        None => task.to_string(),
    };

    let (candidates, scanned_files) = walk_candidates(&root, options.follow_links, &allow_ext)?;
    if candidates.is_empty() {
        return Err(format!(
            "brief found no readable text files under `{root_str}`"
        ));
    }

    let path_docs: Vec<String> = candidates
        .iter()
        .map(|p| rel_path_str(&root, p))
        .collect();
    let path_refs: Vec<&str> = path_docs.iter().map(|s| s.as_str()).collect();
    let path_ranked = score_documents(&score_query, &path_refs);

    // Mild mtime boost so fresher paths win ties.
    let now = unix_now();
    let mut path_hits: Vec<(usize, f64)> = path_ranked
        .into_iter()
        .map(|(i, s)| {
            let boost = mtime_boost(candidates[i].as_path(), now);
            (i, s + boost)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();

    let max_files = options.max_files.max(1);
    if path_hits.is_empty() {
        path_hits = (0..candidates.len().min(max_files.min(8)))
            .map(|i| (i, 0.0))
            .collect();
    } else {
        path_hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        path_hits.truncate(max_files);
    }

    let mut total_bytes = 0usize;
    let mut selected: Vec<SelectedFile> = Vec::new();
    let mut raw_corpus_tokens = estimate_tokens(task, config);
    let mut caveats: Vec<String> = Vec::new();

    for (idx, path_score) in path_hits {
        if total_bytes >= options.max_total_bytes {
            break;
        }
        let abs = &candidates[idx];
        let rel = path_docs[idx].clone();
        let remaining = options.max_total_bytes.saturating_sub(total_bytes);
        let cap = options.max_file_bytes.min(remaining);
        if cap == 0 {
            break;
        }
        match read_file_for_brief(abs, &root, &score_query, cap) {
            Ok(Some(read)) => {
                total_bytes = total_bytes.saturating_add(read.bytes);
                raw_corpus_tokens =
                    raw_corpus_tokens.saturating_add(estimate_tokens(&read.content, config));
                if read.truncated {
                    caveats.push(format!(
                        "{} truncated; windows around query matches",
                        rel
                    ));
                }
                selected.push(SelectedFile {
                    abs: abs.clone(),
                    rel,
                    path_score,
                    content: read.content,
                    bytes: read.bytes,
                    truncated: read.truncated,
                    mtime_secs: read.mtime_secs,
                    kind: classify_kind(abs, &path_docs[idx]),
                });
            }
            Ok(None) => continue,
            Err(_) => continue,
        }
    }

    if selected.is_empty() {
        return Err(format!(
            "brief could not read any candidate files under `{root_str}`"
        ));
    }

    // Content-level BM25 + path blend.
    let content_docs: Vec<&str> = selected.iter().map(|s| s.content.as_str()).collect();
    let content_ranked = score_documents(&score_query, &content_docs);
    let mut file_scores: Vec<(usize, f64)> = content_ranked;
    for (i, score) in file_scores.iter_mut() {
        *score += selected[*i].path_score * 0.15;
        *score += mtime_boost(selected[*i].abs.as_path(), now);
    }
    file_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let keep_n = selected.len().min(max_files.max(1));
    let positive: Vec<(usize, f64)> = file_scores
        .iter()
        .copied()
        .filter(|(_, s)| *s > 0.0)
        .collect();
    let ranked_files: Vec<(usize, f64)> = if positive.is_empty() {
        file_scores.into_iter().take(keep_n.min(8)).collect()
    } else {
        positive.into_iter().take(keep_n).collect()
    };

    let mut kept: Vec<(SelectedFile, f64)> = Vec::new();
    for (i, score) in ranked_files {
        let file = &selected[i];
        let filtered = filter(
            &file.content,
            &FilterOptions {
                query: Some(task.to_string()),
                max_tokens: Some(1_600),
                ..Default::default()
            },
            config,
        );
        let body = if filtered.content.trim().is_empty() {
            file.content.clone()
        } else {
            filtered.content
        };
        kept.push((
            SelectedFile {
                abs: file.abs.clone(),
                rel: file.rel.clone(),
                path_score: file.path_score,
                content: body,
                bytes: file.bytes,
                truncated: file.truncated,
                mtime_secs: file.mtime_secs,
                kind: file.kind.clone(),
            },
            round3(score),
        ));
    }

    // Stale doc caveats vs code median mtime.
    let code_mtimes: Vec<u64> = kept
        .iter()
        .filter(|(f, _)| f.kind == "code")
        .filter_map(|(f, _)| f.mtime_secs)
        .collect();
    if !code_mtimes.is_empty() {
        let mut sorted = code_mtimes;
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        for (f, _) in &kept {
            if f.kind == "doc" {
                if let Some(mt) = f.mtime_secs {
                    if median.saturating_sub(mt) >= STALE_DOC_SECS {
                        caveats.push(format!(
                            "possibly stale: {} (older than selected code)",
                            f.rel
                        ));
                    }
                }
            }
        }
    }

    // Chunk + rerank → evidence items with per-file and token caps.
    let mut rerank_items: Vec<RerankItem> = Vec::new();
    let mut chunk_meta: Vec<(String, String)> = Vec::new(); // (rel, kind)
    for (file, _score) in &kept {
        let map = chunk_with_refs(
            &file.content,
            &ChunkOptions {
                chunk_tokens: 320,
                overlap_tokens: 40,
                source: Some(format!("file://{}", file.rel)),
                semantic_splits: true,
            },
            config,
        );
        for chunk in map.chunks {
            chunk_meta.push((file.rel.clone(), file.kind.clone()));
            rerank_items.push(RerankItem {
                id: Some(chunk.id),
                text: chunk.content,
            });
        }
    }

    let top_k = options.top_k_chunks.max(1);
    let mut evidence: Vec<EvidenceItem> = Vec::new();
    if !rerank_items.is_empty() {
        let ranked = rerank(
            &score_query,
            &rerank_items,
            &RerankOptions {
                top_k: Some(top_k.saturating_mul(2)), // over-fetch then budget-mix
                include_text: true,
                min_score: None,
                preview_chars: 120,
            },
            config,
        );
        let mut per_file: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for hit in ranked.hits {
            let Some(text) = hit.text else { continue };
            let (rel, kind) = chunk_meta
                .get(hit.index)
                .cloned()
                .unwrap_or_else(|| ("unknown".into(), "other".into()));
            let count = per_file.entry(rel.clone()).or_insert(0);
            if *count >= MAX_CHUNKS_PER_FILE {
                continue;
            }
            *count += 1;
            let excerpt = truncate_tokens(&text, MAX_TOKENS_PER_EXCERPT, config);
            evidence.push(EvidenceItem {
                rel,
                kind,
                score: hit.score,
                text: excerpt,
            });
        }
    }

    if evidence.is_empty() {
        for (file, score) in &kept {
            evidence.push(EvidenceItem {
                rel: file.rel.clone(),
                kind: file.kind.clone(),
                score: *score,
                text: truncate_tokens(&file.content, MAX_TOKENS_PER_EXCERPT, config),
            });
        }
    }

    let max_brief_tokens = options
        .max_brief_tokens
        .unwrap_or(config.default_max_tokens)
        .max(128);
    // Reserve room for Status / headers (~25%).
    let evidence_budget = (max_brief_tokens * 75 / 100).max(64);
    let evidence_block = pack_evidence_budget(&evidence, evidence_budget, config);

    let sources: Vec<BriefSource> = kept
        .iter()
        .map(|(f, score)| BriefSource {
            path: f.rel.clone(),
            score: *score,
            bytes: f.bytes,
            truncated: f.truncated,
            mtime_secs: f.mtime_secs,
            kind: f.kind.clone(),
        })
        .collect();

    let sources_block = sources
        .iter()
        .map(|s| {
            let trunc = if s.truncated { ", truncated" } else { "" };
            format!(
                "- {} (score {:.3}, {} bytes, {}{trunc})",
                s.path, s.score, s.bytes, s.kind
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let read_next: Vec<String> = sources.iter().take(4).map(|s| s.path.clone()).collect();
    let suggestions = crate::pipeline::playbook::suggest_playbooks(task, config, 2);
    let mut read_next_lines: Vec<String> = read_next.iter().map(|p| format!("- {p}")).collect();
    for ad in &suggestions {
        read_next_lines.push(format!(
            "- playbook `{}` → `{}` (or action=playbook id={})",
            ad.id, ad.uri, ad.id
        ));
    }
    if !suggestions.iter().any(|a| a.id == "workspace-brief") && !task.is_empty() {
        read_next_lines.push(
            "- skill `brief` → `cmp://skill/action/brief` (or action=help id=brief)".into(),
        );
    }
    let read_next_block = if read_next_lines.is_empty() {
        "- (none)".into()
    } else {
        read_next_lines.join("\n")
    };

    let heuristic_status = build_heuristic_status(task, &kept, &evidence);
    let (status_block, backend, model, fallback_reason) =
        synthesize_status(task, &sources_block, &evidence_block, &heuristic_status, config);

    let caveats_block = if caveats.is_empty() {
        "- none".into()
    } else {
        // Dedup preserve order
        let mut seen = HashSet::new();
        caveats
            .into_iter()
            .filter(|c| seen.insert(c.clone()))
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let briefing_raw = format!(
        "## Task\n{task}\n\n## Status\n{status_block}\n\n## Evidence\n{evidence_block}\n\n## Caveats\n{caveats_block}\n\n## Sources\n{sources_block}\n\n## Read next\n{read_next_block}\n"
    );
    let sanitized = sanitize(&briefing_raw, &SanitizeOptions::default(), config);
    let briefing = sanitized.content;

    let result_tokens = estimate_tokens(&briefing, config);
    let cache_key = format!("cache://brief/{}", short_hash(&briefing));

    Ok(BriefResult {
        task: task.to_string(),
        root: root_str,
        briefing,
        cache_key,
        sources,
        scanned_files,
        selected_files: kept.len(),
        metrics: TokenMetrics::new(raw_corpus_tokens, result_tokens),
        backend,
        model,
        fallback_reason,
    })
}

fn synthesize_status(
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
        Ok(result) if result.backend == SmartBackend::LocalLlm && !result.summary.trim().is_empty() => {
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

fn build_heuristic_status(
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

fn first_useful_line(text: &str) -> String {
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

fn pack_evidence_budget(items: &[EvidenceItem], budget_tokens: usize, config: &Config) -> String {
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

struct FileRead {
    content: String,
    bytes: usize,
    truncated: bool,
    mtime_secs: Option<u64>,
}

fn read_file_for_brief(
    path: &Path,
    root: &Path,
    query: &str,
    max_bytes: usize,
) -> Result<Option<FileRead>, String> {
    let canon = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    if !path_under_or_eq(&canon, root) {
        return Ok(None);
    }

    let meta = fs::metadata(&canon).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Ok(None);
    }
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let file_len = meta.len() as usize;
    if file_len == 0 {
        return Ok(None);
    }

    if file_len <= max_bytes {
        let mut file = fs::File::open(&canon).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; file_len];
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        buf.truncate(n);
        if buf.contains(&0) {
            return Ok(None);
        }
        let text = match String::from_utf8(buf) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        return Ok(Some(FileRead {
            content: text,
            bytes: n,
            truncated: false,
            mtime_secs,
        }));
    }

    // Oversized: load up to SCORE_READ_CAP (or file size), pick best windows.
    let score_cap = file_len.min(SCORE_READ_CAP);
    let mut file = fs::File::open(&canon).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; score_cap];
    let n = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    if buf.contains(&0) {
        return Ok(None);
    }
    let text = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };

    let windows = select_query_windows(&text, query, max_bytes);
    let content = if windows.is_empty() {
        // Fallback: middle slice rather than head-only.
        let start = text.len().saturating_sub(max_bytes) / 2;
        let end = (start + max_bytes).min(text.len());
        // Align to char boundary
        let start = floor_char_boundary(&text, start);
        let end = floor_char_boundary(&text, end);
        text[start..end].to_string()
    } else {
        windows.join("\n\n/* --- window --- */\n\n")
    };

    Ok(Some(FileRead {
        bytes: content.len(),
        content,
        truncated: true,
        mtime_secs,
    }))
}

/// Split `text` into overlapping byte windows and keep top BM25 hits within `budget`.
fn select_query_windows(text: &str, query: &str, budget: usize) -> Vec<String> {
    let win = WINDOW_TARGET_BYTES.min(budget.max(1024));
    let step = (win * 3 / 4).max(1024);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let end = floor_char_boundary(text, (start + win).min(text.len()));
        let start_b = floor_char_boundary(text, start);
        if start_b >= end {
            break;
        }
        spans.push((start_b, end));
        if end >= text.len() {
            break;
        }
        start = start.saturating_add(step);
        if start >= text.len() {
            break;
        }
    }
    if spans.is_empty() {
        return Vec::new();
    }

    let docs: Vec<&str> = spans
        .iter()
        .map(|(s, e)| &text[*s..*e])
        .collect();
    let ranked = score_documents(query, &docs);
    let mut picked: Vec<(usize, f64)> = ranked.into_iter().filter(|(_, s)| *s > 0.0).collect();
    if picked.is_empty() {
        // Keep evenly spaced samples
        let n = spans.len().min(MAX_WINDOWS);
        let step_i = (spans.len() / n).max(1);
        picked = (0..n).map(|i| (i * step_i, 0.0)).collect();
    } else {
        picked.truncate(MAX_WINDOWS);
    }
    picked.sort_by_key(|(i, _)| *i); // document order

    let mut out = Vec::new();
    let mut used = 0usize;
    for (i, _) in picked {
        let (s, e) = spans[i];
        let slice = &text[s..e];
        if used + slice.len() > budget && !out.is_empty() {
            break;
        }
        if used + slice.len() > budget {
            let room = budget.saturating_sub(used);
            let end = floor_char_boundary(slice, room);
            if end > 0 {
                out.push(slice[..end].to_string());
            }
            break;
        }
        used += slice.len();
        out.push(slice.to_string());
    }
    out
}

fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn classify_kind(path: &Path, rel: &str) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let rel_l = rel.to_ascii_lowercase();
    if name.starts_with("roadmap")
        || name.starts_with("architecture")
        || name == "changelog.md"
        || name == "todo.md"
        || rel_l.contains("/docs/")
        || rel_l.starts_with("docs/")
    {
        return "doc".into();
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "md" || ext == "txt" || ext == "rst" || ext == "adoc" {
        return "doc".into();
    }
    if CODE_EXTS.contains(&ext.as_str()) {
        return "code".into();
    }
    "other".into()
}

fn mtime_boost(path: &Path, now: u64) -> f64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0.0;
    };
    let Ok(modified) = meta.modified() else {
        return 0.0;
    };
    let Ok(dur) = modified.duration_since(SystemTime::UNIX_EPOCH) else {
        return 0.0;
    };
    let age_days = now.saturating_sub(dur.as_secs()) as f64 / 86400.0;
    // Fresh files: up to +0.35; older than ~90d: ~0
    (0.35 * (1.0 - (age_days / 90.0).min(1.0))).max(0.0)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn truncate_tokens(text: &str, max_tokens: usize, config: &Config) -> String {
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

fn extension_set(custom: Option<&Vec<String>>) -> HashSet<String> {
    match custom {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect(),
        _ => DEFAULT_EXTENSIONS
            .iter()
            .map(|e| (*e).to_string())
            .collect(),
    }
}

fn resolve_root(explicit: Option<&str>) -> Result<PathBuf, String> {
    let raw = match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => env::current_dir().map_err(|e| format!("brief: cannot resolve cwd: {e}"))?,
    };

    let canonical = fs::canonicalize(&raw).map_err(|e| {
        format!(
            "brief: cannot canonicalize root `{}`: {e}",
            raw.display()
        )
    })?;

    if !canonical.is_dir() {
        return Err(format!(
            "brief: root `{}` is not a directory",
            canonical.display()
        ));
    }

    if let Ok(allow) = env::var("COMPENDIUM_BRIEF_ROOT") {
        let allow = allow.trim();
        if !allow.is_empty() {
            let allow_canon = fs::canonicalize(allow).map_err(|e| {
                format!("brief: COMPENDIUM_BRIEF_ROOT `{allow}` is invalid: {e}")
            })?;
            if !path_under_or_eq(&canonical, &allow_canon) {
                return Err(format!(
                    "brief: root `{}` is outside COMPENDIUM_BRIEF_ROOT `{}`",
                    canonical.display(),
                    allow_canon.display()
                ));
            }
        }
    }

    Ok(canonical)
}

fn walk_candidates(
    root: &Path,
    follow_links: bool,
    allow_ext: &HashSet<String>,
) -> Result<(Vec<PathBuf>, usize), String> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .follow_links(follow_links)
        .filter_entry(|entry| {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    if HARD_SKIP_DIR_NAMES.contains(&name) {
                        return false;
                    }
                }
            }
            true
        })
        .build();

    let mut out = Vec::new();
    let mut scanned = 0usize;

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        scanned += 1;

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if HARD_SKIP_FILE_NAMES.contains(&name) {
                continue;
            }
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "dockerfile" | "makefile" | "gemfile" | "procfile" | "cmakelists.txt"
            ) {
                if path_under_or_eq(path, root) {
                    out.push(path.to_path_buf());
                }
                continue;
            }
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if ext.is_empty() || !allow_ext.contains(&ext) {
            continue;
        }

        if !path_under_or_eq(path, root) {
            continue;
        }

        out.push(path.to_path_buf());
    }

    Ok((out, scanned))
}

fn path_under_or_eq(child: &Path, root: &Path) -> bool {
    child.starts_with(root)
}

fn rel_path_str(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.to_string_lossy().replace('\\', "/"))
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn short_hash(text: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn temp_workspace() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = env::temp_dir().join(format!("compendium-brief-{nanos}-{n}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn touch_mtime(path: &Path, age_secs: u64) {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(age_secs);
        let _ = std::process::Command::new("touch")
            .arg("-d")
            .arg(format!("@{epoch}"))
            .arg(path)
            .status();
    }

    #[test]
    fn brief_selects_relevant_files_and_respects_gitignore() {
        let _guard = env_lock();
        env::remove_var("COMPENDIUM_BRIEF_ROOT");

        let root = temp_workspace();
        fs::create_dir_all(root.join(".git")).unwrap();
        write(&root, ".gitignore", "ignored_secret.rs\nnoise/\n");
        write(
            &root,
            "src/auth.rs",
            &format!(
                "{}\n",
                "pub fn login(user: &str, password: &str) -> Result<Token, AuthError> {\n    // authenticate user credentials against the auth provider\n    validate_credentials(user, password)\n}\n"
                    .repeat(40)
            ),
        );
        write(
            &root,
            "src/ui_theme.rs",
            &format!(
                "{}\n",
                "pub const PRIMARY_COLOR: &str = \"#112233\";\n// theme tokens for sidebar layout only\n"
                    .repeat(20)
            ),
        );
        write(
            &root,
            "docs/auth.md",
            &format!(
                "{}\n",
                "# Auth\nToken refresh and login flow documentation for authentication.\n"
                    .repeat(30)
            ),
        );
        write(&root, "ignored_secret.rs", "pub fn steal_tokens() {}\n");
        write(&root, "noise/big.rs", "fn noise() { /* auth auth auth */ }\n");

        let config = Config::default();
        let result = brief(
            "fix authentication login token",
            None,
            &BriefOptions {
                root: Some(root.to_string_lossy().to_string()),
                max_files: 10,
                top_k_chunks: 8,
                max_brief_tokens: Some(800),
                ..Default::default()
            },
            &config,
        )
        .expect("brief should succeed");

        assert!(!result.briefing.is_empty());
        assert!(result.selected_files >= 1);
        assert!(
            result.sources.iter().any(|s| s.path.contains("auth")),
            "expected auth source, got {:?}",
            result.sources
        );
        assert!(
            !result
                .sources
                .iter()
                .any(|s| s.path.contains("ignored_secret")),
            "gitignore should exclude ignored_secret.rs: {:?}",
            result.sources
        );
        assert!(
            !result.sources.iter().any(|s| s.path.starts_with("noise/")),
            "gitignore should exclude noise/: {:?}",
            result.sources
        );
        assert!(result.metrics.original_tokens >= result.metrics.result_tokens);
        assert!(result.cache_key.starts_with("cache://brief/"));
        assert!(result.briefing.contains("## Task"));
        assert!(result.briefing.contains("## Status"));
        assert!(result.briefing.contains("## Evidence"));
        assert!(result.briefing.contains("## Sources"));
        assert!(result.briefing.contains("## Read next"));
        assert_eq!(result.backend, "heuristic");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn brief_window_read_captures_middle_query_hit() {
        let _guard = env_lock();
        env::remove_var("COMPENDIUM_BRIEF_ROOT");

        let root = temp_workspace();
        fs::create_dir_all(root.join(".git")).unwrap();
        // ~400 KiB file: noise head/tail, unique marker in the middle.
        let mut body = String::new();
        body.push_str(&"fn filler_alpha() { let x = 1; }\n".repeat(4000));
        body.push_str(
            "pub fn unique_zebra_consensus_protocol() {\n    // zebra marker for window test\n}\n",
        );
        body.push_str(&"fn filler_omega() { let y = 2; }\n".repeat(4000));
        write(&root, "src/big.rs", &body);

        let result = brief(
            "unique_zebra_consensus_protocol",
            None,
            &BriefOptions {
                root: Some(root.to_string_lossy().to_string()),
                max_file_bytes: 64 * 1024,
                max_files: 5,
                top_k_chunks: 6,
                max_brief_tokens: Some(600),
                ..Default::default()
            },
            &Config::default(),
        )
        .expect("brief ok");

        assert!(
            result.sources.iter().any(|s| s.path.contains("big.rs") && s.truncated),
            "expected truncated big.rs: {:?}",
            result.sources
        );
        assert!(
            result.briefing.contains("unique_zebra_consensus_protocol")
                || result.briefing.contains("zebra"),
            "windowing should keep middle hit: {}",
            &result.briefing[..result.briefing.len().min(800)]
        );
        assert!(result.briefing.contains("truncated") || result.briefing.contains("Caveats"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn brief_marks_stale_doc_vs_fresh_code() {
        let _guard = env_lock();
        env::remove_var("COMPENDIUM_BRIEF_ROOT");

        let root = temp_workspace();
        fs::create_dir_all(root.join(".git")).unwrap();
        write(
            &root,
            "ROADMAP.md",
            "# Roadmap\nBaseline: 2PC test harness only. Auth not started.\n".repeat(20).as_str(),
        );
        write(
            &root,
            "src/auth.rs",
            "pub fn login() { /* auth complete B5 DONE */ }\n".repeat(30).as_str(),
        );
        touch_mtime(&root.join("ROADMAP.md"), 60 * 24 * 3600); // ~60 days old
        touch_mtime(&root.join("src/auth.rs"), 3600); // 1 hour old

        let result = brief(
            "auth login 2PC roadmap status",
            None,
            &BriefOptions {
                root: Some(root.to_string_lossy().to_string()),
                max_files: 10,
                max_brief_tokens: Some(700),
                ..Default::default()
            },
            &Config::default(),
        )
        .expect("brief ok");

        assert!(
            result.briefing.contains("possibly stale"),
            "expected stale caveat when ROADMAP is older than code: {}",
            result.briefing
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn brief_rejects_root_outside_allowlist_env() {
        let _guard = env_lock();
        let root = temp_workspace();
        write(&root, "a.rs", "fn a() {}\n");
        let outside = temp_workspace();
        write(&outside, "b.rs", "fn b() {}\n");

        env::set_var("COMPENDIUM_BRIEF_ROOT", root.to_string_lossy().as_ref());
        let err = brief(
            "anything",
            None,
            &BriefOptions {
                root: Some(outside.to_string_lossy().to_string()),
                ..Default::default()
            },
            &Config::default(),
        )
        .expect_err("should reject outside allow root");
        assert!(
            err.contains("COMPENDIUM_BRIEF_ROOT") || err.contains("outside"),
            "unexpected err: {err}"
        );
        env::remove_var("COMPENDIUM_BRIEF_ROOT");

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn brief_skips_symlink_escape_when_not_following() {
        let _guard = env_lock();
        env::remove_var("COMPENDIUM_BRIEF_ROOT");

        let root = temp_workspace();
        write(&root, "src/safe.rs", "pub fn safe_auth() {}\n");
        let outside = temp_workspace();
        write(
            &outside,
            "secret.rs",
            "pub fn leaked_secret_auth_token() {}\n",
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.join("escape.rs");
            let _ = symlink(outside.join("secret.rs"), &link);
        }

        let result = brief(
            "auth token secret",
            None,
            &BriefOptions {
                root: Some(root.to_string_lossy().to_string()),
                follow_links: false,
                max_files: 20,
                ..Default::default()
            },
            &Config::default(),
        )
        .expect("brief ok");

        assert!(
            !result.sources.iter().any(|s| s.path.contains("escape")),
            "symlink escape should not appear: {:?}",
            result.sources
        );
        assert!(!result.briefing.contains("leaked_secret_auth_token"));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn brief_requires_query() {
        let err = brief("", None, &BriefOptions::default(), &Config::default())
            .expect_err("empty query");
        assert!(err.contains("query"));
    }

    #[test]
    fn select_query_windows_prefers_matching_span() {
        let mut text = String::new();
        text.push_str(&"aaa noise line\n".repeat(5000));
        text.push_str("special_needle_token appears here\n");
        text.push_str(&"bbb noise line\n".repeat(5000));
        let wins = select_query_windows(&text, "special_needle_token", 32 * 1024);
        let joined = wins.join("\n");
        assert!(
            joined.contains("special_needle_token"),
            "expected needle in windows"
        );
    }
}
