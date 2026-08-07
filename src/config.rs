//! Runtime configuration loaded from environment variables.

use std::env;
use std::path::PathBuf;

/// Default character threshold for signal-to-call bypass (compress/summarize).
pub const DEFAULT_SIGNAL_MIN_CHARS: usize = 1_000;

/// Default archive compressed size cap (2 MiB).
pub const DEFAULT_ARCHIVE_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Default archive uncompressed size cap (4 MiB).
pub const DEFAULT_ARCHIVE_MAX_UNCOMPRESSED: u64 = 4 * 1024 * 1024;
/// Default max files per archive.
pub const DEFAULT_ARCHIVE_MAX_FILES: usize = 50;
/// Default soft TTL for skill resource reads (5 minutes).
pub const DEFAULT_SKILL_RESOURCE_TTL_MS: u64 = 300_000;
/// Default disk cache size cap when `COMPENDIUM_CACHE_DIR` is set (64 MiB).
pub const DEFAULT_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Default BM25 weight in hybrid rerank (identifier-friendly).
pub const DEFAULT_HYBRID_ALPHA: f64 = 0.55;

/// Optional OpenAI-compatible local SLM endpoint.
#[derive(Debug, Clone)]
pub struct LocalLlmConfig {
    /// When true, smart actions may call the local endpoint.
    pub enabled: bool,
    /// Base URL including `/v1` or `/api/v1` (no trailing slash required).
    pub base_url: Option<String>,
    /// Model id accepted by the local server.
    pub model: String,
    /// Optional embeddings model (Ollama: e.g. `nomic-embed-text`). Defaults to [`Self::model`].
    pub embedding_model: Option<String>,
    /// Optional bearer token (Lemonade embed / locked loopback).
    pub api_key: Option<String>,
    /// HTTP timeout in seconds (first model load can be slow).
    pub timeout_secs: u64,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: None,
            model: "Qwen3-4B-GGUF".into(),
            embedding_model: None,
            api_key: None,
            timeout_secs: 120,
        }
    }
}

impl LocalLlmConfig {
    pub fn model_name(&self) -> Option<String> {
        if self.enabled {
            Some(self.model.clone())
        } else {
            None
        }
    }

    /// Embedding model id (falls back to chat model when unset).
    pub fn embedding_model_name(&self) -> &str {
        self.embedding_model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(self.model.as_str())
    }
}

/// Server and pipeline defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// Approximate characters per token (heuristic backend only).
    pub chars_per_token: f64,
    /// Default max output tokens for compress/summarize.
    pub default_max_tokens: usize,
    /// Collapse runs of blank lines to at most this many.
    pub max_blank_lines: usize,
    /// When deduplicating similar lines, Jaccard threshold (0.0–1.0).
    pub similarity_threshold: f64,
    /// Tiktoken encoding name when built with `real-tokens`
    /// (`cl100k_base` or `o200k_base`). Ignored by the heuristic backend.
    pub tokenizer: String,
    /// Bind address for streamable HTTP transport (`http` feature).
    pub http_bind: String,
    /// Optional local small-language-model endpoint for smart actions.
    pub local_llm: LocalLlmConfig,
    /// Character threshold for signal-to-call: compress/summarize bypass below this
    /// (0 disables bypass). Default 1000.
    pub signal_min_chars: usize,
    /// Optional directory of extra/override playbooks (`*.md`).
    pub playbooks_dir: Option<PathBuf>,
    /// Archive download/pack size cap (compressed).
    pub archive_max_bytes: u64,
    /// Archive uncompressed size cap.
    pub archive_max_uncompressed: u64,
    /// Max files per archive.
    pub archive_max_files: usize,
    /// Soft TTL (ms) advertised on skill resource reads (MCP `ttlMs`).
    pub skill_resource_ttl_ms: u64,
    /// Optional directory for persistent session cache (unset = memory only).
    pub cache_dir: Option<PathBuf>,
    /// Soft cap on total cached payload bytes (enforced when set).
    pub cache_max_bytes: Option<u64>,
    /// Optional JSONL audit log path (`COMPENDIUM_AUDIT_PATH`). Never writes raw secrets.
    pub audit_path: Option<PathBuf>,
    /// Default BM25 weight for hybrid rerank (0.0–1.0). Remainder is embedding cosine.
    pub hybrid_alpha: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chars_per_token: 4.0,
            default_max_tokens: 2_048,
            max_blank_lines: 1,
            similarity_threshold: 0.85,
            tokenizer: "cl100k_base".into(),
            http_bind: "127.0.0.1:8788".into(),
            local_llm: LocalLlmConfig::default(),
            signal_min_chars: DEFAULT_SIGNAL_MIN_CHARS,
            playbooks_dir: None,
            archive_max_bytes: DEFAULT_ARCHIVE_MAX_BYTES,
            archive_max_uncompressed: DEFAULT_ARCHIVE_MAX_UNCOMPRESSED,
            archive_max_files: DEFAULT_ARCHIVE_MAX_FILES,
            skill_resource_ttl_ms: DEFAULT_SKILL_RESOURCE_TTL_MS,
            cache_dir: None,
            cache_max_bytes: None,
            audit_path: None,
            hybrid_alpha: DEFAULT_HYBRID_ALPHA,
        }
    }
}

impl Config {
    /// Build config from `COMPENDIUM_*` environment variables.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = env::var("COMPENDIUM_CHARS_PER_TOKEN") {
            if let Ok(n) = v.parse() {
                cfg.chars_per_token = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_DEFAULT_MAX_TOKENS") {
            if let Ok(n) = v.parse() {
                cfg.default_max_tokens = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_MAX_BLANK_LINES") {
            if let Ok(n) = v.parse() {
                cfg.max_blank_lines = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_SIMILARITY_THRESHOLD") {
            if let Ok(n) = v.parse() {
                cfg.similarity_threshold = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_TOKENIZER") {
            if !v.trim().is_empty() {
                cfg.tokenizer = v;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_HTTP_BIND") {
            if !v.trim().is_empty() {
                cfg.http_bind = v;
            }
        }

        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_URL") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                cfg.local_llm.base_url = Some(trimmed);
                cfg.local_llm.enabled = true;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_MODEL") {
            if !v.trim().is_empty() {
                cfg.local_llm.model = v;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_EMBED_MODEL") {
            if !v.trim().is_empty() {
                cfg.local_llm.embedding_model = Some(v);
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_API_KEY") {
            if !v.trim().is_empty() {
                cfg.local_llm.api_key = Some(v);
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_TIMEOUT_SECS") {
            if let Ok(n) = v.parse::<u64>() {
                if n > 0 {
                    cfg.local_llm.timeout_secs = n;
                }
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_HYBRID_ALPHA") {
            if let Ok(n) = v.parse::<f64>() {
                if (0.0..=1.0).contains(&n) {
                    cfg.hybrid_alpha = n;
                }
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_AUDIT_PATH") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                cfg.audit_path = Some(PathBuf::from(trimmed));
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_SIGNAL_MIN_CHARS") {
            if let Ok(n) = v.parse::<usize>() {
                cfg.signal_min_chars = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_PLAYBOOKS_DIR") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                cfg.playbooks_dir = Some(PathBuf::from(trimmed));
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_ARCHIVE_MAX_BYTES") {
            if let Ok(n) = v.parse::<u64>() {
                if n > 0 {
                    cfg.archive_max_bytes = n;
                }
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_ARCHIVE_MAX_UNCOMPRESSED") {
            if let Ok(n) = v.parse::<u64>() {
                if n > 0 {
                    cfg.archive_max_uncompressed = n;
                }
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_ARCHIVE_MAX_FILES") {
            if let Ok(n) = v.parse::<usize>() {
                if n > 0 {
                    cfg.archive_max_files = n;
                }
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_SKILL_TTL_MS") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.skill_resource_ttl_ms = n;
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_CACHE_DIR") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                cfg.cache_dir = Some(PathBuf::from(trimmed));
                if cfg.cache_max_bytes.is_none() {
                    cfg.cache_max_bytes = Some(DEFAULT_CACHE_MAX_BYTES);
                }
            }
        }
        if let Ok(v) = env::var("COMPENDIUM_CACHE_MAX_BYTES") {
            if let Ok(n) = v.parse::<u64>() {
                if n > 0 {
                    cfg.cache_max_bytes = Some(n);
                }
            }
        }

        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn defaults_match_public_constants() {
        let cfg = Config::default();
        assert_eq!(cfg.signal_min_chars, DEFAULT_SIGNAL_MIN_CHARS);
        assert_eq!(cfg.archive_max_bytes, DEFAULT_ARCHIVE_MAX_BYTES);
        assert_eq!(
            cfg.archive_max_uncompressed,
            DEFAULT_ARCHIVE_MAX_UNCOMPRESSED
        );
        assert_eq!(cfg.archive_max_files, DEFAULT_ARCHIVE_MAX_FILES);
        assert_eq!(cfg.skill_resource_ttl_ms, DEFAULT_SKILL_RESOURCE_TTL_MS);
        assert_eq!(cfg.http_bind, "127.0.0.1:8788");
        assert!(!cfg.local_llm.enabled);
        assert!(cfg.local_llm.base_url.is_none());
        assert!(cfg.cache_dir.is_none());
        assert!(cfg.cache_max_bytes.is_none());
    }

    #[test]
    fn local_llm_model_name_respects_enabled() {
        let mut llm = LocalLlmConfig::default();
        assert!(llm.model_name().is_none());
        llm.enabled = true;
        assert_eq!(llm.model_name().as_deref(), Some("Qwen3-4B-GGUF"));
    }

    #[test]
    fn from_env_reads_signal_skill_and_cache() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("COMPENDIUM_SIGNAL_MIN_CHARS", "42");
        env::set_var("COMPENDIUM_SKILL_TTL_MS", "1234");
        env::set_var("COMPENDIUM_HTTP_BIND", "127.0.0.1:9999");
        env::set_var("COMPENDIUM_CACHE_DIR", "/tmp/compendium-cache-test-cfg");
        env::set_var("COMPENDIUM_CACHE_MAX_BYTES", "4096");
        let cfg = Config::from_env();
        env::remove_var("COMPENDIUM_SIGNAL_MIN_CHARS");
        env::remove_var("COMPENDIUM_SKILL_TTL_MS");
        env::remove_var("COMPENDIUM_HTTP_BIND");
        env::remove_var("COMPENDIUM_CACHE_DIR");
        env::remove_var("COMPENDIUM_CACHE_MAX_BYTES");
        assert_eq!(cfg.signal_min_chars, 42);
        assert_eq!(cfg.skill_resource_ttl_ms, 1234);
        assert_eq!(cfg.http_bind, "127.0.0.1:9999");
        assert_eq!(
            cfg.cache_dir,
            Some(PathBuf::from("/tmp/compendium-cache-test-cfg"))
        );
        assert_eq!(cfg.cache_max_bytes, Some(4096));
    }
}
