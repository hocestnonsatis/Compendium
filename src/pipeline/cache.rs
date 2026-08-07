//! Session key/value cache for large payloads kept out of the prompt.
//!
//! Default: in-process only (lost on restart). When `COMPENDIUM_CACHE_DIR` is set,
//! entries are also written under that directory and reloaded on startup. Paths never
//! escape the cache root (blob names are content-addressed hashes).

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
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
    /// TTL in seconds; expired entries are removed on access and on disk load.
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
    /// `memory` or `disk` (disk means also persisted under cache dir).
    #[serde(default = "default_backend_memory")]
    pub backend: String,
}

fn default_backend_memory() -> String {
    "memory".into()
}

/// Result of retrieving a payload.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheGetResult {
    pub key: String,
    pub content: String,
    pub tokens: usize,
    pub bytes: usize,
    pub hit: bool,
    /// `memory`, `disk`, or empty on miss.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend: String,
}

/// Invalidate / clear response.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheInvalidateResult {
    pub removed: usize,
    pub keys: Vec<String>,
}

/// Snapshot of cache hit/miss/eviction counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheCounters {
    pub hits: usize,
    pub misses: usize,
    pub disk_loads: usize,
    pub evictions: usize,
    pub entries: usize,
    pub bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_dir: Option<String>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    content: String,
    tokens: usize,
    expires_unix: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskMeta {
    key: String,
    tokens: usize,
    expires_unix: Option<u64>,
    bytes: usize,
}

/// Process-local cache store with optional disk persistence.
#[derive(Debug, Default)]
pub struct CacheStore {
    entries: HashMap<String, CacheEntry>,
    disk_dir: Option<PathBuf>,
    max_bytes: Option<u64>,
    hits: usize,
    misses: usize,
    disk_loads: usize,
    evictions: usize,
}

impl CacheStore {
    /// Build from config: optional disk dir load + size cap.
    pub fn from_config(config: &Config) -> Self {
        let mut store = Self {
            entries: HashMap::new(),
            disk_dir: config.cache_dir.clone(),
            max_bytes: config.cache_max_bytes,
            hits: 0,
            misses: 0,
            disk_loads: 0,
            evictions: 0,
        };
        if let Some(dir) = store.disk_dir.clone() {
            if let Err(e) = fs::create_dir_all(&dir) {
                tracing::warn!(error = %e, path = %dir.display(), "cache dir create failed");
                store.disk_dir = None;
            } else {
                store.load_from_disk(&dir);
            }
        }
        store
    }

    pub fn counters(&self) -> CacheCounters {
        CacheCounters {
            hits: self.hits,
            misses: self.misses,
            disk_loads: self.disk_loads,
            evictions: self.evictions,
            entries: self.entries.len(),
            bytes: self.total_bytes(),
            disk_dir: self.disk_dir.as_ref().map(|p| p.display().to_string()),
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
        let now = unix_now();
        let expires = options.ttl_secs.map(|t| now.saturating_add(t));
        self.insert_entry(
            key.clone(),
            CacheEntry {
                content: text.to_string(),
                tokens,
                expires_unix: expires,
            },
        );
        let backend = if self.disk_dir.is_some() {
            "disk"
        } else {
            "memory"
        };
        let preview = preview_text(text, options.preview_chars);
        let preview_tokens = estimate_tokens(&preview, config);
        CacheStoreResult {
            key,
            tokens_stored: tokens,
            bytes: text.len(),
            preview,
            metrics: TokenMetrics::new(tokens, preview_tokens),
            backend: backend.into(),
        }
    }

    pub fn get(&mut self, key: &str, config: &Config) -> CacheGetResult {
        self.evict_expired();
        if let Some(entry) = self.entries.get(key) {
            self.hits += 1;
            return CacheGetResult {
                key: key.to_string(),
                content: entry.content.clone(),
                tokens: entry.tokens,
                bytes: entry.content.len(),
                hit: true,
                backend: "memory".into(),
            };
        }
        // Lazy disk reload for keys written by another process (same dir).
        if let Some(dir) = self.disk_dir.clone() {
            if self.try_load_key(&dir, key) {
                self.disk_loads += 1;
                self.hits += 1;
                if let Some(entry) = self.entries.get(key) {
                    return CacheGetResult {
                        key: key.to_string(),
                        content: entry.content.clone(),
                        tokens: entry.tokens,
                        bytes: entry.content.len(),
                        hit: true,
                        backend: "disk".into(),
                    };
                }
            }
        }
        self.misses += 1;
        CacheGetResult {
            key: key.to_string(),
            content: String::new(),
            tokens: estimate_tokens("", config),
            bytes: 0,
            hit: false,
            backend: String::new(),
        }
    }

    pub fn invalidate(&mut self, key: Option<&str>) -> CacheInvalidateResult {
        self.evict_expired();
        if let Some(k) = key {
            let removed = self.remove_key(k);
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
        let removed = keys.len();
        for k in &keys {
            self.remove_key(k);
        }
        CacheInvalidateResult { removed, keys }
    }

    pub fn put_raw(&mut self, key: String, content: String, tokens: usize) {
        self.insert_entry(
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

    fn total_bytes(&self) -> usize {
        self.entries.values().map(|e| e.content.len()).sum()
    }

    fn insert_entry(&mut self, key: String, entry: CacheEntry) {
        self.persist_entry(&key, &entry);
        self.entries.insert(key, entry);
        self.enforce_max_bytes();
    }

    fn remove_key(&mut self, key: &str) -> bool {
        let existed = self.entries.remove(key).is_some();
        self.delete_disk(key);
        existed
    }

    fn evict_expired(&mut self) {
        let now = unix_now();
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.expires_unix.map(|exp| exp <= now).unwrap_or(false))
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired {
            self.remove_key(&k);
            self.evictions += 1;
        }
    }

    fn enforce_max_bytes(&mut self) {
        let Some(max) = self.max_bytes else {
            return;
        };
        if max == 0 {
            return;
        }
        while self.total_bytes() as u64 > max && !self.entries.is_empty() {
            // Drop an arbitrary entry (HashMap iteration order); prefer expired first.
            let now = unix_now();
            let victim = self
                .entries
                .iter()
                .find(|(_, e)| e.expires_unix.map(|exp| exp <= now).unwrap_or(false))
                .map(|(k, _)| k.clone())
                .or_else(|| self.entries.keys().next().cloned());
            if let Some(k) = victim {
                self.remove_key(&k);
                self.evictions += 1;
            } else {
                break;
            }
        }
    }

    fn persist_entry(&self, key: &str, entry: &CacheEntry) {
        let Some(dir) = &self.disk_dir else {
            return;
        };
        let stem = blob_stem(key);
        let meta = DiskMeta {
            key: key.to_string(),
            tokens: entry.tokens,
            expires_unix: entry.expires_unix,
            bytes: entry.content.len(),
        };
        let meta_path = dir.join(format!("{stem}.meta.json"));
        let blob_path = dir.join(format!("{stem}.blob"));
        if let Ok(bytes) = serde_json::to_vec_pretty(&meta) {
            if let Err(e) = fs::write(&meta_path, bytes) {
                tracing::warn!(error = %e, "cache meta write failed");
                return;
            }
        }
        if let Err(e) = fs::write(&blob_path, entry.content.as_bytes()) {
            tracing::warn!(error = %e, "cache blob write failed");
            let _ = fs::remove_file(&meta_path);
        }
    }

    fn delete_disk(&self, key: &str) {
        let Some(dir) = &self.disk_dir else {
            return;
        };
        let stem = blob_stem(key);
        let _ = fs::remove_file(dir.join(format!("{stem}.meta.json")));
        let _ = fs::remove_file(dir.join(format!("{stem}.blob")));
    }

    fn load_from_disk(&mut self, dir: &Path) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        let now = unix_now();
        for ent in rd.flatten() {
            let path = ent.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !name.ends_with(".meta.json") {
                continue;
            }
            let Ok(raw) = fs::read(&path) else {
                continue;
            };
            let Ok(meta) = serde_json::from_slice::<DiskMeta>(&raw) else {
                continue;
            };
            if meta.expires_unix.map(|exp| exp <= now).unwrap_or(false) {
                let stem = name.trim_end_matches(".meta.json");
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(dir.join(format!("{stem}.blob")));
                self.evictions += 1;
                continue;
            }
            let stem = name.trim_end_matches(".meta.json");
            let blob_path = dir.join(format!("{stem}.blob"));
            let Ok(bytes) = fs::read(&blob_path) else {
                continue;
            };
            let Ok(content) = String::from_utf8(bytes) else {
                continue;
            };
            self.disk_loads += 1;
            self.entries.insert(
                meta.key,
                CacheEntry {
                    content,
                    tokens: meta.tokens,
                    expires_unix: meta.expires_unix,
                },
            );
        }
        self.enforce_max_bytes();
    }

    fn try_load_key(&mut self, dir: &Path, key: &str) -> bool {
        let stem = blob_stem(key);
        let meta_path = dir.join(format!("{stem}.meta.json"));
        let blob_path = dir.join(format!("{stem}.blob"));
        let Ok(raw) = fs::read(&meta_path) else {
            return false;
        };
        let Ok(meta) = serde_json::from_slice::<DiskMeta>(&raw) else {
            return false;
        };
        if meta.key != key {
            return false;
        }
        let now = unix_now();
        if meta.expires_unix.map(|exp| exp <= now).unwrap_or(false) {
            let _ = fs::remove_file(&meta_path);
            let _ = fs::remove_file(&blob_path);
            self.evictions += 1;
            return false;
        }
        let Ok(bytes) = fs::read(&blob_path) else {
            return false;
        };
        let Ok(content) = String::from_utf8(bytes) else {
            return false;
        };
        self.entries.insert(
            key.to_string(),
            CacheEntry {
                content,
                tokens: meta.tokens,
                expires_unix: meta.expires_unix,
            },
        );
        true
    }
}

fn blob_stem(key: &str) -> String {
    short_hash(key)
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
    use std::time::Duration;

    #[test]
    fn store_get_roundtrip() {
        let mut store = CacheStore::default();
        let cfg = Config::default();
        let stored = store.store("hello cache world", &CacheStoreOptions::default(), &cfg);
        assert!(stored.key.starts_with("cache://"));
        assert_eq!(stored.backend, "memory");
        let got = store.get(&stored.key, &cfg);
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
        let _b = store.store(
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
        let got = store.get(&stored.key, &cfg);
        assert!(!got.hit);
        assert!(store.counters().evictions >= 1);
    }

    #[test]
    fn disk_persists_across_stores() {
        let dir = std::env::temp_dir().join(format!(
            "compendium-cache-test-{}",
            short_hash(&format!("{:?}", std::time::Instant::now()))
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let cfg = Config {
            cache_dir: Some(dir.clone()),
            cache_max_bytes: Some(1024 * 1024),
            ..Default::default()
        };

        {
            let mut store = CacheStore::from_config(&cfg);
            let stored = store.store(
                "persisted payload",
                &CacheStoreOptions {
                    key: Some("disk-key".into()),
                    ..Default::default()
                },
                &cfg,
            );
            assert_eq!(stored.backend, "disk");
        }

        let mut store2 = CacheStore::from_config(&cfg);
        let got = store2.get("disk-key", &cfg);
        assert!(got.hit);
        assert_eq!(got.content, "persisted payload");
        assert!(store2.counters().disk_loads >= 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn max_bytes_evicts() {
        let mut store = CacheStore {
            max_bytes: Some(20),
            ..Default::default()
        };
        let cfg = Config::default();
        store.store(
            "abcdefghij", // 10 bytes
            &CacheStoreOptions {
                key: Some("a".into()),
                ..Default::default()
            },
            &cfg,
        );
        store.store(
            "0123456789", // 10 bytes
            &CacheStoreOptions {
                key: Some("b".into()),
                ..Default::default()
            },
            &cfg,
        );
        store.store(
            "xxxxxxxxxx", // forces eviction
            &CacheStoreOptions {
                key: Some("c".into()),
                ..Default::default()
            },
            &cfg,
        );
        assert!(store.total_bytes() as u64 <= 20);
        assert!(store.counters().evictions >= 1);
    }
}
