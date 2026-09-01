//! Runtime configuration loaded from environment variables.

use std::env;

/// Default character threshold for signal-to-call bypass (compress/summarize).
pub const DEFAULT_SIGNAL_MIN_CHARS: usize = 1_000;

/// Optional OpenAI-compatible local SLM endpoint.
#[derive(Debug, Clone)]
pub struct LocalLlmConfig {
    /// When true, smart actions may call the local endpoint.
    pub enabled: bool,
    /// Base URL including `/v1` or `/api/v1` (no trailing slash required).
    pub base_url: Option<String>,
    /// Model id accepted by the local server.
    pub model: String,
    /// Optional bearer token (locked loopback servers).
    pub api_key: Option<String>,
    /// HTTP timeout in seconds (first model load can be slow).
    pub timeout_secs: u64,
    /// OpenAI/Ollama reasoning depth (`low`/`medium`/`high`/`xhigh`).
    ///
    /// `Some("low")` by default so thinking models (Qwen 3.8+) return `content`
    /// instead of spending the token budget on hidden CoT. `None` omits the
    /// field. `Some("off")` sends Ollama `think: false` only.
    pub reasoning_effort: Option<String>,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: None,
            model: "qwen2.5:3b".into(),
            api_key: None,
            timeout_secs: 120,
            reasoning_effort: Some("low".into()),
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
}

/// Parse `COMPENDIUM_LOCAL_LLM_REASONING_EFFORT`.
///
/// Empty / `omit` / `none` / `unset` → omit the chat JSON field (Lemonade / strict
/// OpenAI servers). Other values are lowercased (`low`, `off`, `medium`, …).
pub fn parse_reasoning_effort(raw: &str) -> Option<String> {
    let t = raw.trim().to_ascii_lowercase();
    match t.as_str() {
        "" | "omit" | "none" | "unset" => None,
        other => Some(other.to_string()),
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
    /// Optional local small-language-model endpoint for smart actions.
    pub local_llm: LocalLlmConfig,
    /// Character threshold for signal-to-call: compress/summarize bypass below this
    /// (0 disables bypass). Default 1000.
    pub signal_min_chars: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chars_per_token: 4.0,
            default_max_tokens: 2_048,
            max_blank_lines: 1,
            similarity_threshold: 0.85,
            tokenizer: "cl100k_base".into(),
            local_llm: LocalLlmConfig::default(),
            signal_min_chars: DEFAULT_SIGNAL_MIN_CHARS,
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
        if let Ok(v) = env::var("COMPENDIUM_LOCAL_LLM_REASONING_EFFORT") {
            cfg.local_llm.reasoning_effort = parse_reasoning_effort(&v);
        }
        if let Ok(v) = env::var("COMPENDIUM_SIGNAL_MIN_CHARS") {
            if let Ok(n) = v.parse::<usize>() {
                cfg.signal_min_chars = n;
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
        assert!(!cfg.local_llm.enabled);
        assert!(cfg.local_llm.base_url.is_none());
        assert_eq!(cfg.local_llm.reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn local_llm_model_name_respects_enabled() {
        let mut llm = LocalLlmConfig::default();
        assert!(llm.model_name().is_none());
        llm.enabled = true;
        assert_eq!(llm.model_name().as_deref(), Some("qwen2.5:3b"));
    }

    #[test]
    fn from_env_reads_signal_and_llm() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("COMPENDIUM_SIGNAL_MIN_CHARS", "42");
        env::set_var("COMPENDIUM_LOCAL_LLM_URL", "http://127.0.0.1:11434/v1");
        env::set_var("COMPENDIUM_LOCAL_LLM_MODEL", "m1");
        let cfg = Config::from_env();
        env::remove_var("COMPENDIUM_SIGNAL_MIN_CHARS");
        env::remove_var("COMPENDIUM_LOCAL_LLM_URL");
        env::remove_var("COMPENDIUM_LOCAL_LLM_MODEL");
        assert_eq!(cfg.signal_min_chars, 42);
        assert!(cfg.local_llm.enabled);
        assert_eq!(cfg.local_llm.model, "m1");
    }

    #[test]
    fn parse_reasoning_effort_omits_empty_aliases() {
        assert_eq!(parse_reasoning_effort(""), None);
        assert_eq!(parse_reasoning_effort(" omit "), None);
        assert_eq!(parse_reasoning_effort("NONE"), None);
        assert_eq!(parse_reasoning_effort("low").as_deref(), Some("low"));
        assert_eq!(parse_reasoning_effort("OFF").as_deref(), Some("off"));
        assert_eq!(parse_reasoning_effort("xhigh").as_deref(), Some("xhigh"));
    }

    #[test]
    fn from_env_reads_reasoning_effort() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("COMPENDIUM_LOCAL_LLM_REASONING_EFFORT", "medium");
        let cfg = Config::from_env();
        env::remove_var("COMPENDIUM_LOCAL_LLM_REASONING_EFFORT");
        assert_eq!(cfg.local_llm.reasoning_effort.as_deref(), Some("medium"));

        env::set_var("COMPENDIUM_LOCAL_LLM_REASONING_EFFORT", "omit");
        let cfg = Config::from_env();
        env::remove_var("COMPENDIUM_LOCAL_LLM_REASONING_EFFORT");
        assert_eq!(cfg.local_llm.reasoning_effort, None);
    }
}
