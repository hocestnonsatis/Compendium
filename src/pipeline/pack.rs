//! Bounded archive pack/unpack for multi-file skill/corpus bundles.
//!
//! Scripts inside archives are never executed. Size and file-count caps guard
//! against decompression bombs.

use std::io::{Cursor, Read, Write};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::config::Config;
use crate::pipeline::chunk::{chunk_with_refs, ChunkMap, ChunkOptions};
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for `pack` / `unpack`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PackOptions {
    /// Cap compressed archive size in bytes.
    #[serde(default)]
    pub max_size_bytes: Option<u64>,
    /// Cap total uncompressed bytes across all members.
    #[serde(default)]
    pub max_uncompressed_bytes: Option<u64>,
    /// Cap number of files in the archive.
    #[serde(default)]
    pub max_files: Option<usize>,
    /// When packing: also store zip bytes in session cache (caller handles key).
    #[serde(default = "default_true")]
    pub store_in_cache: bool,
    /// When packing: include base64 of the zip in the result (can be large).
    #[serde(default)]
    pub include_base64: bool,
    /// Optional logical source label for resulting chunks after unpack.
    #[serde(default)]
    pub source: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            max_size_bytes: None,
            max_uncompressed_bytes: None,
            max_files: None,
            store_in_cache: true,
            include_base64: false,
            source: None,
        }
    }
}

impl PackOptions {
    fn resolved(&self, config: &Config) -> ResolvedCaps {
        ResolvedCaps {
            max_size_bytes: self.max_size_bytes.unwrap_or(config.archive_max_bytes),
            max_uncompressed_bytes: self
                .max_uncompressed_bytes
                .unwrap_or(config.archive_max_uncompressed),
            max_files: self.max_files.unwrap_or(config.archive_max_files),
        }
    }
}

struct ResolvedCaps {
    max_size_bytes: u64,
    max_uncompressed_bytes: u64,
    max_files: usize,
}

/// One logical file to pack (from `items` or a simple text convention).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PackItem {
    /// Path inside the archive (e.g. `notes.md`).
    pub path: String,
    /// File contents (UTF-8 text).
    pub text: String,
}

/// Result of packing.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PackResult {
    pub format: String,
    pub file_count: usize,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    /// Suggested cache key when `store_in_cache` (caller may store bytes).
    pub cache_key: String,
    /// Base64 zip when `include_base64`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
    pub metrics: TokenMetrics,
    /// Raw zip bytes for the server to cache (not serialized to client).
    #[serde(skip)]
    pub zip_bytes: Vec<u8>,
}

/// Result of unpacking.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UnpackResult {
    pub file_count: usize,
    pub uncompressed_bytes: u64,
    pub files: Vec<String>,
    /// Chunk map over concatenated sanitized file texts.
    pub chunks: ChunkMap,
    pub metrics: TokenMetrics,
}

/// Pack items into a zip. Rejects when caps would be exceeded.
pub fn pack_items(
    items: &[PackItem],
    options: &PackOptions,
    config: &Config,
) -> Result<PackResult, String> {
    let caps = options.resolved(config);
    if items.is_empty() {
        return Err("pack requires at least one file".into());
    }
    if items.len() > caps.max_files {
        return Err(format!(
            "pack file count {} exceeds max_files {}",
            items.len(),
            caps.max_files
        ));
    }

    let mut uncompressed: u64 = 0;
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for item in items {
            let path = sanitize_archive_path(&item.path)?;
            let data = item.text.as_bytes();
            uncompressed = uncompressed.saturating_add(data.len() as u64);
            if uncompressed > caps.max_uncompressed_bytes {
                return Err(format!(
                    "pack uncompressed size exceeds max_uncompressed_bytes ({})",
                    caps.max_uncompressed_bytes
                ));
            }
            zip.start_file(path, opts)
                .map_err(|e| format!("zip start_file: {e}"))?;
            zip.write_all(data).map_err(|e| format!("zip write: {e}"))?;
        }
        zip.finish().map_err(|e| format!("zip finish: {e}"))?;
    }
    let zip_bytes = cursor.into_inner();
    let compressed = zip_bytes.len() as u64;
    if compressed > caps.max_size_bytes {
        return Err(format!(
            "pack compressed size {compressed} exceeds max_size_bytes {}",
            caps.max_size_bytes
        ));
    }

    let cache_key = format!("cache://pack/{}", short_hash_bytes(&zip_bytes));
    let orig = estimate_tokens(
        &items
            .iter()
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        config,
    );
    let metrics = TokenMetrics::new(orig, estimate_tokens(&cache_key, config));

    Ok(PackResult {
        format: "zip".into(),
        file_count: items.len(),
        compressed_bytes: compressed,
        uncompressed_bytes: uncompressed,
        cache_key,
        base64: if options.include_base64 {
            Some(B64.encode(&zip_bytes))
        } else {
            None
        },
        metrics,
        zip_bytes,
    })
}

/// Parse `text` as either base64 zip, or a simple multi-file document:
/// ```text
/// path/one.md
/// ---
/// body
/// ===
/// path/two.md
/// ---
/// body2
/// ```
pub fn parse_pack_text(text: &str) -> Result<Vec<PackItem>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty pack text".into());
    }
    // Single file shortcut: no separators → pack as `input.txt`
    if !trimmed.contains("\n---\n") && !trimmed.contains("\n===\n") {
        return Ok(vec![PackItem {
            path: "input.txt".into(),
            text: trimmed.to_string(),
        }]);
    }

    let mut items = Vec::new();
    for part in trimmed.split("\n===\n") {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (path, body) = if let Some((p, b)) = part.split_once("\n---\n") {
            (p.trim(), b.to_string())
        } else {
            return Err(
                "each pack section needs `path\\n---\\nbody` (separate files with ===)".into(),
            );
        };
        if path.is_empty() {
            return Err("pack section missing path".into());
        }
        items.push(PackItem {
            path: path.to_string(),
            text: body,
        });
    }
    if items.is_empty() {
        return Err("no pack sections found".into());
    }
    Ok(items)
}

/// Decode zip from base64 or raw bytes hint (if text looks like base64).
pub fn decode_archive_bytes(text: &str) -> Result<Vec<u8>, String> {
    let trimmed = text.trim();
    B64.decode(trimmed)
        .map_err(|e| format!("unpack expects base64 zip (or pass cache key): {e}"))
}

/// Unpack zip bytes into a chunk map. Rejects bombs / oversize archives.
pub fn unpack_bytes(
    zip_bytes: &[u8],
    options: &PackOptions,
    config: &Config,
) -> Result<UnpackResult, String> {
    let caps = options.resolved(config);
    if zip_bytes.len() as u64 > caps.max_size_bytes {
        return Err(format!(
            "archive size {} exceeds max_size_bytes {}",
            zip_bytes.len(),
            caps.max_size_bytes
        ));
    }

    let cursor = Cursor::new(zip_bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|e| format!("invalid zip archive: {e}"))?;

    if archive.len() > caps.max_files {
        return Err(format!(
            "archive file count {} exceeds max_files {}",
            archive.len(),
            caps.max_files
        ));
    }

    let mut uncompressed_total: u64 = 0;
    let mut files = Vec::new();
    let mut corpus = String::new();

    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file
            .enclosed_name()
            .ok_or_else(|| format!("zip entry {i} has unsafe path"))?
            .to_string_lossy()
            .to_string();
        // Reject absolute / escape paths already handled by enclosed_name.
        let mut buf = Vec::new();
        let mut limited = file.take(
            caps.max_uncompressed_bytes
                .saturating_sub(uncompressed_total)
                + 1,
        );
        limited
            .read_to_end(&mut buf)
            .map_err(|e| format!("read zip entry {name}: {e}"))?;
        uncompressed_total = uncompressed_total.saturating_add(buf.len() as u64);
        if uncompressed_total > caps.max_uncompressed_bytes {
            return Err(format!(
                "archive uncompressed size exceeds max_uncompressed_bytes ({})",
                caps.max_uncompressed_bytes
            ));
        }
        // Never execute; treat as UTF-8 lossy text for chunking.
        let text = String::from_utf8_lossy(&buf);
        files.push(name.clone());
        corpus.push_str(&format!("# file: {name}\n{text}\n\n"));
    }

    if files.is_empty() {
        return Err("archive contained no files".into());
    }

    let mut chunk_opts = ChunkOptions::default();
    if let Some(src) = &options.source {
        chunk_opts.source = Some(src.clone());
    } else {
        chunk_opts.source = Some("archive://unpack".into());
    }
    let chunks = chunk_with_refs(&corpus, &chunk_opts, config);
    let metrics = TokenMetrics::new(
        estimate_tokens(&corpus, config),
        chunks.metrics.result_tokens,
    );

    Ok(UnpackResult {
        file_count: files.len(),
        uncompressed_bytes: uncompressed_total,
        files,
        chunks,
        metrics,
    })
}

fn sanitize_archive_path(path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    if path.is_empty() || path.starts_with('/') || path.contains("..") {
        return Err(format!("unsafe archive path: {path}"));
    }
    Ok(path)
}

fn short_hash_bytes(bytes: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn pack_unpack_roundtrip() {
        let cfg = Config::default();
        let items = vec![
            PackItem {
                path: "a.md".into(),
                text: "# hello\n".into(),
            },
            PackItem {
                path: "b.md".into(),
                text: "world".into(),
            },
        ];
        let packed = pack_items(&items, &PackOptions::default(), &cfg).unwrap();
        assert_eq!(packed.file_count, 2);
        let unpacked = unpack_bytes(&packed.zip_bytes, &PackOptions::default(), &cfg).unwrap();
        assert_eq!(unpacked.file_count, 2);
        assert!(!unpacked.chunks.chunks.is_empty());
    }

    #[test]
    fn rejects_too_many_files() {
        let cfg = Config {
            archive_max_files: 1,
            ..Config::default()
        };
        let items = vec![
            PackItem {
                path: "a.txt".into(),
                text: "a".into(),
            },
            PackItem {
                path: "b.txt".into(),
                text: "b".into(),
            },
        ];
        assert!(pack_items(&items, &PackOptions::default(), &cfg).is_err());
    }

    #[test]
    fn parse_multi_file_text() {
        let items = parse_pack_text("one.md\n---\nalpha\n===\ntwo.md\n---\nbeta").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].path, "one.md");
    }
}
