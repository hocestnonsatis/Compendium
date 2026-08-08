//! Path walking, root resolution, and file classification helpers.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) const HARD_SKIP_DIR_NAMES: &[&str] =
    &[".git", "node_modules", "target", "dist", "build", ".next"];
pub(crate) const HARD_SKIP_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "composer.lock",
    "Gemfile.lock",
];

pub(crate) const DEFAULT_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "md", "toml", "json", "yaml", "yml", "go",
    "java", "kt", "c", "h", "cpp", "hpp", "cc", "cs", "rb", "php", "sql", "sh", "bash", "zsh",
    "svelte", "vue", "css", "scss", "html", "txt", "xml", "gradle", "cmake", "swift", "m", "mm",
    "r", "lua", "pl", "pm", "ex", "exs", "erl", "hs", "scala", "dart", "proto", "graphql", "tf",
    "hcl", "nix", "zig", "gd", "wgsl",
];

pub(crate) const CODE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "c", "h", "cpp", "hpp",
    "cc", "cs", "rb", "php", "sql", "svelte", "vue", "swift", "scala", "dart", "ex", "exs", "zig",
];

/// Docs older than code median by this many seconds → stale caveat.
pub(crate) const STALE_DOC_SECS: u64 = 7 * 24 * 3600;

pub(crate) struct SelectedFile {
    pub(crate) abs: PathBuf,
    pub(crate) rel: String,
    pub(crate) path_score: f64,
    pub(crate) content: String,
    pub(crate) bytes: usize,
    pub(crate) truncated: bool,
    pub(crate) mtime_secs: Option<u64>,
    pub(crate) kind: String,
}

pub(crate) fn classify_kind(path: &Path, rel: &str) -> String {
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

pub(crate) fn mtime_boost(path: &Path, now: u64) -> f64 {
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

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn extension_set(custom: Option<&Vec<String>>) -> HashSet<String> {
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

pub(crate) fn resolve_root(explicit: Option<&str>) -> Result<PathBuf, String> {
    let raw = match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => env::current_dir().map_err(|e| format!("brief: cannot resolve cwd: {e}"))?,
    };

    let canonical = fs::canonicalize(&raw)
        .map_err(|e| format!("brief: cannot canonicalize root `{}`: {e}", raw.display()))?;

    if !canonical.is_dir() {
        return Err(format!(
            "brief: root `{}` is not a directory",
            canonical.display()
        ));
    }

    if let Ok(allow) = env::var("COMPENDIUM_BRIEF_ROOT") {
        let allow = allow.trim();
        if !allow.is_empty() {
            let allow_canon = fs::canonicalize(allow)
                .map_err(|e| format!("brief: COMPENDIUM_BRIEF_ROOT `{allow}` is invalid: {e}"))?;
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

pub(crate) fn walk_candidates(
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

pub(crate) fn path_under_or_eq(child: &Path, root: &Path) -> bool {
    child.starts_with(root)
}

pub(crate) fn rel_path_str(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

pub(crate) fn short_hash(text: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}
