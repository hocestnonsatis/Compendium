//! In-process session cache for large payloads kept out of the prompt.
//!
//! Entries live for the lifetime of the MCP server process.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for `cache_store`.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CacheStoreOptions {
    /// Explicit cache key. If omitted, a `cache://…` key is derived from content.
    pub key: Option<String>,
    /// TTL in seconds; expired entries are removed on access.
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

/// Snapshot of cache counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheCounters {
    pub hits: usize,
    pub misses: usize,
    pub entries: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    content: String,
    tokens: usize,
    expires_unix: Option<u64>,
}

/// Process-local key/value store.
#[derive(Debug, Default)]
pub struct CacheStore {
    entries: HashMap<String, CacheEntry>,
    hits: usize,
    misses: usize,
}

impl CacheStore {
    pub fn counters(&self) -> CacheCounters {
        CacheCounters {
            hits: self.hits,
            misses: self.misses,
            entries: self.entries.len(),
            bytes: self.entries.values().map(|e| e.content.len()).sum(),
        }
    }

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
        let expires = options.ttl_secs.map(|t| unix_now().saturating_add(t));
        self.entries.insert(
            key.clone(),
            CacheEntry {
                content: text.to_string(),
                tokens,
                expires_unix: expires,
            },
        );
        let preview = preview_text(text, options.preview_chars);
        let preview_tokens = estimate_tokens(&preview, config);
        CacheStoreResult {
            key,
            tokens_stored: tokens,
            bytes: text.len(),
            preview,
            metrics: TokenMetrics::new(tokens, preview_tokens),
        }
    }

    pub fn get(&mut self, key: &str) -> CacheGetResult {
        self.evict_expired();
        if let Some(entry) = self.entries.get(key) {
            self.hits += 1;
            return CacheGetResult {
                key: key.to_string(),
                content: entry.content.clone(),
                tokens: entry.tokens,
                bytes: entry.content.len(),
                hit: true,
            };
        }
        self.misses += 1;
        CacheGetResult {
            key: key.to_string(),
            content: String::new(),
            tokens: 0,
            bytes: 0,
            hit: false,
        }
    }

    pub fn invalidate(&mut self, key: Option<&str>) -> CacheInvalidateResult {
        self.evict_expired();
        if let Some(k) = key {
            let removed = self.entries.remove(k).is_some();
            return CacheInvalidateResult {
                removed: removed as usize,
                keys: if removed {
                    vec![k.to_string()]
                } else {
                    Vec::new()
                },
            };
        }
        let keys: Vec<String> = self.entries.keys().cloned().collect();
        self.entries.clear();
        CacheInvalidateResult {
            removed: keys.len(),
            keys,
        }
    }

    /// Insert without TTL (used for chunk maps).
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

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let preview: String = text.chars().take(max_chars).collect();
    format!("{preview}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn store_get_roundtrip() {
        let mut store = CacheStore::default();
        let cfg = Config::default();
        let stored = store.store("hello cache world", &CacheStoreOptions::default(), &cfg);
        assert!(stored.key.starts_with("cache://"));
        let got = store.get(&stored.key);
        assert!(got.hit);
        assert_eq!(got.content, "hello cache world");
        assert_eq!(store.counters().hits, 1);
    }

    #[test]
    fn invalidate_one_and_all() {
        let mut store = CacheStore::default();
        let cfg = Config::default();
        let a = store.store(
            "a",
            &CacheStoreOptions {
                key: Some("k1".into()),
                ..Default::default()
            },
            &cfg,
        );
        store.store(
            "b",
            &CacheStoreOptions {
                key: Some("k2".into()),
                ..Default::default()
            },
            &cfg,
        );
        assert_eq!(store.invalidate(Some(&a.key)).removed, 1);
        assert_eq!(store.invalidate(None).removed, 1);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn ttl_evicts_on_access() {
        let mut store = CacheStore::default();
        let cfg = Config::default();
        let stored = store.store(
            "temp",
            &CacheStoreOptions {
                key: Some("ttl-key".into()),
                ttl_secs: Some(1),
                ..Default::default()
            },
            &cfg,
        );
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!store.get(&stored.key).hit);
    }
}
