//! File reads and query-aware window selection for large files.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

use crate::pipeline::bm25::score_documents;

use super::walk::path_under_or_eq;

/// How much of an oversized file we may load to score windows.
pub(crate) const SCORE_READ_CAP: usize = 2 * 1024 * 1024;
pub(crate) const WINDOW_TARGET_BYTES: usize = 80 * 1024;
pub(crate) const MAX_WINDOWS: usize = 3;

pub(crate) struct FileRead {
    pub(crate) content: String,
    pub(crate) bytes: usize,
    pub(crate) truncated: bool,
    pub(crate) mtime_secs: Option<u64>,
}

pub(crate) fn read_file_for_brief(
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
pub(crate) fn select_query_windows(text: &str, query: &str, budget: usize) -> Vec<String> {
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

    let docs: Vec<&str> = spans.iter().map(|(s, e)| &text[*s..*e]).collect();
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

pub(crate) fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
