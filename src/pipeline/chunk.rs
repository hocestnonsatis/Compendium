//! Chunking and reference mapping — keep handles instead of raw heavy blobs.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Options for [`chunk_with_refs`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ChunkOptions {
    /// Target tokens per chunk.
    #[serde(default = "default_chunk_tokens")]
    pub chunk_tokens: usize,
    /// Overlap tokens between adjacent chunks.
    #[serde(default = "default_overlap")]
    pub overlap_tokens: usize,
    /// Optional logical source path/URI stored in references.
    pub source: Option<String>,
    /// Prefer splitting on blank lines / headings when possible.
    #[serde(default = "default_true")]
    pub semantic_splits: bool,
}

fn default_chunk_tokens() -> usize {
    512
}
fn default_overlap() -> usize {
    64
}
fn default_true() -> bool {
    true
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            chunk_tokens: 512,
            overlap_tokens: 64,
            source: None,
            semantic_splits: true,
        }
    }
}

/// A content chunk with a stable reference id.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Chunk {
    pub id: String,
    pub index: usize,
    pub start_offset: usize,
    pub end_offset: usize,
    pub tokens: usize,
    pub preview: String,
    pub content: String,
}

/// Reference map returned to clients so they can re-fetch slices by id.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChunkMap {
    pub source: String,
    pub content_hash: String,
    pub total_tokens: usize,
    pub chunks: Vec<Chunk>,
    pub index_text: String,
    pub metrics: TokenMetrics,
}

/// Split `input` into overlapping chunks and build a compact reference index.
pub fn chunk_with_refs(input: &str, options: &ChunkOptions, config: &Config) -> ChunkMap {
    let original_tokens = estimate_tokens(input, config);
    let source = options
        .source
        .clone()
        .unwrap_or_else(|| format!("mem://{}", short_hash(input)));
    let content_hash = format!("{:016x}", full_hash(input));

    let segments = if options.semantic_splits {
        semantic_segments(input)
    } else {
        vec![input.to_string()]
    };

    let chunk_char_budget = (options.chunk_tokens as f64 * config.chars_per_token) as usize;
    let overlap_char_budget = (options.overlap_tokens as f64 * config.chars_per_token) as usize;

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut buffer = String::new();
    let mut buffer_start = 0usize;
    let mut cursor = 0usize;

    let flush = |buffer: &mut String,
                 buffer_start: usize,
                 cursor: usize,
                 chunks: &mut Vec<Chunk>,
                 config: &Config,
                 source: &str| {
        if buffer.is_empty() {
            return;
        }
        let index = chunks.len();
        let id = format!("cmp://{}/{}", short_hash(source), index);
        let content = std::mem::take(buffer);
        let tokens = estimate_tokens(&content, config);
        let preview: String = content.chars().take(96).collect();
        chunks.push(Chunk {
            id,
            index,
            start_offset: buffer_start,
            end_offset: cursor,
            tokens,
            preview: if content.chars().count() > 96 {
                format!("{preview}…")
            } else {
                preview
            },
            content,
        });
    };

    for segment in segments {
        if estimate_tokens(&buffer, config) > 0
            && estimate_tokens(&buffer, config) + estimate_tokens(&segment, config)
                > options.chunk_tokens
            && buffer.chars().count() >= chunk_char_budget / 2
        {
            let overlap = take_overlap_suffix(&buffer, overlap_char_budget);
            flush(
                &mut buffer,
                buffer_start,
                cursor,
                &mut chunks,
                config,
                &source,
            );
            buffer_start = cursor.saturating_sub(overlap.len());
            buffer = overlap;
        }

        if buffer.is_empty() {
            buffer_start = cursor;
        }
        if !buffer.is_empty() {
            buffer.push('\n');
        }
        buffer.push_str(&segment);
        cursor += segment.len() + 1;

        // Hard-split oversized segments
        while estimate_tokens(&buffer, config) > options.chunk_tokens * 2 {
            let split_at = find_split_point(&buffer, chunk_char_budget);
            let (head, tail) = buffer.split_at(split_at);
            let mut head = head.to_string();
            let tail = tail.to_string();
            let head_end = buffer_start + head.len();
            flush(
                &mut head,
                buffer_start,
                head_end,
                &mut chunks,
                config,
                &source,
            );
            let overlap = take_overlap_suffix(
                chunks.last().map(|c| c.content.as_str()).unwrap_or(""),
                overlap_char_budget,
            );
            buffer_start = head_end.saturating_sub(overlap.len());
            buffer = overlap;
            if !buffer.is_empty() && !tail.is_empty() {
                buffer.push('\n');
            }
            buffer.push_str(&tail);
        }
    }

    flush(
        &mut buffer,
        buffer_start,
        input.len(),
        &mut chunks,
        config,
        &source,
    );

    let index_text = render_index(&source, &content_hash, &chunks);
    let index_tokens = estimate_tokens(&index_text, config);

    ChunkMap {
        source,
        content_hash,
        total_tokens: original_tokens,
        chunks,
        index_text,
        metrics: TokenMetrics::new(original_tokens, index_tokens),
    }
}

/// Resolve a slice by reference id from a previously built map (by index suffix).
pub fn resolve_chunk<'a>(map: &'a ChunkMap, id: &str) -> Option<&'a Chunk> {
    map.chunks.iter().find(|c| c.id == id)
}

/// Result of resolving a chunk reference.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResolveResult {
    pub found: bool,
    pub id: String,
    pub chunk: Option<Chunk>,
    pub source: Option<String>,
    pub content_hash: Option<String>,
}

/// Resolve `id` from an explicit map, or by re-chunking `text` with `options`.
pub fn resolve_ref(
    id: &str,
    map: Option<&ChunkMap>,
    text: Option<&str>,
    options: &ChunkOptions,
    config: &Config,
) -> ResolveResult {
    if let Some(map) = map {
        if let Some(chunk) = resolve_chunk(map, id) {
            return ResolveResult {
                found: true,
                id: id.to_string(),
                chunk: Some(chunk.clone()),
                source: Some(map.source.clone()),
                content_hash: Some(map.content_hash.clone()),
            };
        }
    }
    if let Some(text) = text {
        let built = chunk_with_refs(text, options, config);
        if let Some(chunk) = resolve_chunk(&built, id) {
            return ResolveResult {
                found: true,
                id: id.to_string(),
                chunk: Some(chunk.clone()),
                source: Some(built.source),
                content_hash: Some(built.content_hash),
            };
        }
        // Also allow resolving by numeric suffix: cmp://hash/3 → index 3
        if let Some(idx) = id.rsplit('/').next().and_then(|s| s.parse::<usize>().ok()) {
            if let Some(chunk) = built.chunks.get(idx) {
                return ResolveResult {
                    found: true,
                    id: chunk.id.clone(),
                    chunk: Some(chunk.clone()),
                    source: Some(built.source),
                    content_hash: Some(built.content_hash),
                };
            }
        }
    }
    ResolveResult {
        found: false,
        id: id.to_string(),
        chunk: None,
        source: None,
        content_hash: None,
    }
}

fn semantic_segments(input: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut buf = String::new();

    for line in input.lines() {
        let is_break = line.trim().is_empty()
            || line.trim_start().starts_with('#')
            || line.trim_start().starts_with("```");
        if is_break && !buf.is_empty() {
            segments.push(std::mem::take(&mut buf));
            if !line.trim().is_empty() {
                buf.push_str(line);
                buf.push('\n');
            }
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if !buf.is_empty() {
        segments.push(buf);
    }
    if segments.is_empty() {
        segments.push(input.to_string());
    }
    segments
}

fn find_split_point(text: &str, budget: usize) -> usize {
    let mut end = budget.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(nl) = text[..end].rfind('\n') {
        if nl > end / 3 {
            return nl + 1;
        }
    }
    end.max(1)
}

fn take_overlap_suffix(text: &str, budget: usize) -> String {
    if budget == 0 || text.is_empty() {
        return String::new();
    }
    let start = text.len().saturating_sub(budget);
    let mut start = start;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    if let Some(nl) = text[start..].find('\n') {
        text[start + nl + 1..].to_string()
    } else {
        text[start..].to_string()
    }
}

fn render_index(source: &str, hash: &str, chunks: &[Chunk]) -> String {
    let mut out = format!("# refmap source={source} hash={hash} chunks={}\n", chunks.len());
    for c in chunks {
        out.push_str(&format!(
            "- {} [{}..{} · ~{} tok] {}\n",
            c.id, c.start_offset, c.end_offset, c.tokens, c.preview
        ));
    }
    out.push_str(
        "\n# usage: pass chunk id back to retrieve content; prefer index over raw corpus.\n",
    );
    out
}

fn short_hash(s: &str) -> String {
    format!("{:08x}", full_hash(s) as u32)
}

fn full_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_reference_map() {
        let input = format!("{}\n\n{}", "alpha ".repeat(200), "beta ".repeat(200));
        let map = chunk_with_refs(
            &input,
            &ChunkOptions {
                chunk_tokens: 80,
                overlap_tokens: 10,
                source: Some("file://demo.txt".into()),
                semantic_splits: true,
            },
            &Config::default(),
        );
        assert!(map.chunks.len() >= 2);
        assert!(map.index_text.contains("cmp://"));
        assert!(map.metrics.result_tokens < map.metrics.original_tokens);
        let id = map.chunks[0].id.clone();
        assert!(resolve_chunk(&map, &id).is_some());
    }
}
