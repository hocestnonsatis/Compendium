//! In-process key/value cache for large payloads kept out of the prompt.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for [`cache_store`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CacheStoreOptions {
    /// Explicit cache key. If omitted, a `cache://…` key is derived from content.
    pub key: Option<String>,
    /// Optional TTL hint in seconds (advisory; eviction is best-effort).
    pub ttl_secs: Option<u64>,
    /// Characters kept in the returned preview.
    #[serde(default = "default_preview")]
    pub preview_chars: usize,
}

fn default_preview() -> usize {
    120
}

impl Default for CacheStoreOptions {
    fn default() -> Self {
        Self {
            key: None,
            ttl_secs: None,
            preview_chars: 120,
        }
    }
}

/// Result of storing a payload.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheStoreResult {
    pub key: String,
    pub tokens_stored: usize,
    pub bytes: usize,
    pub preview: String,
    pub metrics: TokenMetrics,
}

/// Result of retrieving a payload.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheGetResult {
    pub key: String,
    pub content: String,
    pub tokens: usize,
    pub bytes: usize,
    pub hit: bool,
}

/// Invalidate / clear response.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheInvalidateResult {
    pub removed: usize,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    content: String,
    tokens: usize,
    expires_unix: Option<u64>,
}

/// Process-local cache store.
#[derive(Debug, Default)]
pub struct CacheStore {
    entries: HashMap<String, CacheEntry>,
}

impl CacheStore {
    pub fn store(
        &mut self,
        text: &str,
        options: &CacheStoreOptions,
        config: &Config,
    ) -> CacheStoreResult {
        self.evict_expired();
        let tokens = estimate_tokens(text, config);
        let key = options
            .key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| format!("cache://{}", short_hash(text)));
        let now = unix_now();
        let expires = options.ttl_secs.map(|t| now.saturating_add(t));
        self.entries.insert(
            key.clone(),
            CacheEntry {
                content: text.to_string(),
                tokens,
                expires_unix: expires,
            },
        );
        let preview = preview_text(text, options.preview_chars);
        // Storing removes text from the prompt → result tokens ≈ preview size.
        let preview_tokens = estimate_tokens(&preview, config);
        CacheStoreResult {
            key,
            tokens_stored: tokens,
            bytes: text.len(),
            preview,
            metrics: TokenMetrics::new(tokens, preview_tokens),
        }
    }

    pub fn get(&mut self, key: &str, config: &Config) -> CacheGetResult {
        self.evict_expired();
        match self.entries.get(key) {
            Some(entry) => CacheGetResult {
                key: key.to_string(),
                content: entry.content.clone(),
                tokens: entry.tokens,
                bytes: entry.content.len(),
                hit: true,
            },
            None => CacheGetResult {
                key: key.to_string(),
                content: String::new(),
                tokens: estimate_tokens("", config),
                bytes: 0,
                hit: false,
            },
        }
    }

    pub fn invalidate(&mut self, key: Option<&str>) -> CacheInvalidateResult {
        self.evict_expired();
        if let Some(k) = key {
            let removed = self.entries.remove(k).is_some() as usize;
            return CacheInvalidateResult {
                removed,
                keys: if removed > 0 {
                    vec![k.to_string()]
                } else {
                    Vec::new()
                },
            };
        }
        let keys: Vec<String> = self.entries.keys().cloned().collect();
        let removed = keys.len();
        self.entries.clear();
        CacheInvalidateResult { removed, keys }
    }

    pub fn put_raw(&mut self, key: String, content: String, tokens: usize) {
        self.entries.insert(
            key,
            CacheEntry {
                content,
                tokens,
                expires_unix: None,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn evict_expired(&mut self) {
        let now = unix_now();
        self.entries
            .retain(|_, e| e.expires_unix.map(|exp| exp > now).unwrap_or(true));
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn short_hash(s: &str) -> String {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let preview: String = text.chars().take(max_chars).collect();
    format!("{preview}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_get_roundtrip() {
        let mut store = CacheStore::default();
        let cfg = Config::default();
        let stored = store.store("hello cache world", &CacheStoreOptions::default(), &cfg);
        assert!(stored.key.starts_with("cache://"));
        let got = store.get(&stored.key, &cfg);
        assert!(got.hit);
        assert_eq!(got.content, "hello cache world");
    }

    #[test]
    fn invalidate_one_and_all() {
        let mut store = CacheStore::default();
        let cfg = Config::default();
        let a = store.store("a", &CacheStoreOptions {
            key: Some("k1".into()),
            ..Default::default()
        }, &cfg);
        let _b = store.store("b", &CacheStoreOptions {
            key: Some("k2".into()),
            ..Default::default()
        }, &cfg);
        assert_eq!(store.invalidate(Some(&a.key)).removed, 1);
        assert_eq!(store.invalidate(None).removed, 1);
        assert_eq!(store.len(), 0);
    }
}
