//! Workspace briefing: scan a local root for task-relevant slices and pack them.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::bm25::score_documents;
use crate::pipeline::chunk::{chunk_with_refs, ChunkOptions};
use crate::pipeline::compress::{compress, CompressOptions, ContentType};
use crate::pipeline::filter::{filter, FilterOptions};
use crate::pipeline::rerank::{rerank, RerankItem, RerankOptions};
use crate::pipeline::sanitize::{sanitize, SanitizeOptions};
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
    /// Per-file read cap in bytes.
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: usize,
    /// Total bytes budget across selected file reads.
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: usize,
    /// Soft cap on packed briefing tokens (after compress).
    pub max_brief_tokens: Option<usize>,
    /// Keep top K content chunks after rerank.
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
    pub backend: String,
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

    let max_files = options.max_files.max(1);
    let mut path_hits: Vec<(usize, f64)> = path_ranked
        .into_iter()
        .filter(|(_, s)| *s > 0.0)
        .collect();
    if path_hits.is_empty() {
        // No lexical path hit: take a small prefix so brief still returns something useful.
        path_hits = (0..candidates.len().min(max_files.min(8)))
            .map(|i| (i, 0.0))
            .collect();
    } else {
        path_hits.truncate(max_files);
    }

    let mut total_bytes = 0usize;
    let mut selected: Vec<(PathBuf, String, f64, String, usize)> = Vec::new();
    // (abs path, rel, path_score, content, bytes)
    let mut raw_corpus_tokens = estimate_tokens(task, config);

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
        match read_text_capped(abs, &root, cap) {
            Ok(Some((content, bytes))) => {
                total_bytes = total_bytes.saturating_add(bytes);
                raw_corpus_tokens =
                    raw_corpus_tokens.saturating_add(estimate_tokens(&content, config));
                selected.push((abs.clone(), rel, path_score, content, bytes));
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

    // Content-level BM25 over whole-file texts, then keep files that score.
    let content_docs: Vec<&str> = selected.iter().map(|s| s.3.as_str()).collect();
    let content_ranked = score_documents(&score_query, &content_docs);
    let mut file_scores: Vec<(usize, f64)> = content_ranked;
    // Blend path score lightly so path-relevant files stay preferred on ties.
    for (i, score) in file_scores.iter_mut() {
        *score += selected[*i].2 * 0.15;
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

    let mut kept: Vec<(String, f64, String, usize)> = Vec::new();
    for (i, score) in ranked_files {
        let (_abs, rel, _ps, content, bytes) = &selected[i];
        let filtered = filter(
            content,
            &FilterOptions {
                query: Some(task.to_string()),
                max_tokens: Some(1_200),
                ..Default::default()
            },
            config,
        );
        let body = if filtered.content.trim().is_empty() {
            content.clone()
        } else {
            filtered.content
        };
        kept.push((rel.clone(), round3(score), body, *bytes));
    }

    // Chunk + rerank across selected slices.
    let mut rerank_items: Vec<RerankItem> = Vec::new();
    for (rel, _score, body, _bytes) in &kept {
        let map = chunk_with_refs(
            body,
            &ChunkOptions {
                chunk_tokens: 384,
                overlap_tokens: 48,
                source: Some(format!("file://{rel}")),
                semantic_splits: true,
            },
            config,
        );
        for chunk in map.chunks {
            rerank_items.push(RerankItem {
                id: Some(chunk.id),
                text: format!("// file: {rel}\n{}", chunk.content),
            });
        }
    }

    let top_k = options.top_k_chunks.max(1);
    let ranked = if rerank_items.is_empty() {
        Vec::new()
    } else {
        let result = rerank(
            &score_query,
            &rerank_items,
            &RerankOptions {
                top_k: Some(top_k),
                include_text: true,
                min_score: None,
                preview_chars: 160,
            },
            config,
        );
        result
            .hits
            .into_iter()
            .filter_map(|h| h.text)
            .collect::<Vec<_>>()
    };

    let context_raw = if ranked.is_empty() {
        kept.iter()
            .map(|(rel, _, body, _)| format!("### {rel}\n{body}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        ranked.join("\n\n---\n\n")
    };

    let max_brief_tokens = options
        .max_brief_tokens
        .unwrap_or(config.default_max_tokens)
        .max(64);

    let compressed = compress(
        &context_raw,
        &CompressOptions {
            max_tokens: Some(max_brief_tokens),
            content_type: ContentType::Code,
            force: true,
            ..Default::default()
        },
        config,
    );

    let sanitized = sanitize(&compressed.content, &SanitizeOptions::default(), config);

    let sources: Vec<BriefSource> = kept
        .iter()
        .map(|(path, score, _, bytes)| BriefSource {
            path: path.clone(),
            score: *score,
            bytes: *bytes,
        })
        .collect();

    let sources_block = sources
        .iter()
        .map(|s| format!("- {} (score {:.3}, {} bytes)", s.path, s.score, s.bytes))
        .collect::<Vec<_>>()
        .join("\n");

    let briefing = format!(
        "## Task\n{task}\n\n## Sources\n{sources_block}\n\n## Context\n{}\n",
        sanitized.content.trim()
    );

    let original_tokens = raw_corpus_tokens;
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
        metrics: TokenMetrics::new(original_tokens, result_tokens),
        backend: "heuristic".into(),
    })
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
            // Special no-extension text files
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

fn read_text_capped(
    path: &Path,
    root: &Path,
    max_bytes: usize,
) -> Result<Option<(String, usize)>, String> {
    // Re-check escape after potential symlink resolution when follow_links was true.
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

    let mut file = fs::File::open(&canon).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; max_bytes.min(meta.len() as usize).max(1)];
    let n = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);

    if buf.contains(&0) {
        return Ok(None);
    }

    let text = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };

    Ok(Some((text, n)))
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
    use std::sync::{Mutex, OnceLock};
    use std::sync::atomic::{AtomicU64, Ordering};
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

    #[test]
    fn brief_selects_relevant_files_and_respects_gitignore() {
        let _guard = env_lock();
        env::remove_var("COMPENDIUM_BRIEF_ROOT");

        let root = temp_workspace();
        // ignore crate only applies .gitignore inside a git work tree.
        fs::create_dir_all(root.join(".git")).unwrap();
        write(
            &root,
            ".gitignore",
            "ignored_secret.rs\nnoise/\n",
        );
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
                "pub const PRIMARY_COLOR: &str = \"#112233\";\n// theme tokens for sidebar layout only\n".repeat(20)
            ),
        );
        write(
            &root,
            "docs/auth.md",
            &format!(
                "{}\n",
                "# Auth\nToken refresh and login flow documentation for authentication.\n".repeat(30)
            ),
        );
        write(
            &root,
            "ignored_secret.rs",
            "pub fn steal_tokens() {}\n",
        );
        write(
            &root,
            "noise/big.rs",
            "fn noise() { /* auth auth auth */ }\n",
        );

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
        assert!(
            result.metrics.original_tokens >= result.metrics.result_tokens,
            "expected reduction: orig={} result={}",
            result.metrics.original_tokens,
            result.metrics.result_tokens
        );
        assert!(result.cache_key.starts_with("cache://brief/"));
        assert!(result.briefing.contains("## Task"));
        assert!(result.briefing.contains("## Context"));

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
        assert!(
            !result.briefing.contains("leaked_secret_auth_token"),
            "outside content must not leak"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn brief_requires_query() {
        let err = brief("", None, &BriefOptions::default(), &Config::default())
            .expect_err("empty query");
        assert!(err.contains("query"));
    }
}
