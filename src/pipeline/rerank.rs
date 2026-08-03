//! BM25 rerank of text chunks / candidates for a query.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::bm25::score_documents;
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// One candidate to rerank.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RerankItem {
    /// Optional stable id (e.g. `cmp://…` chunk ref).
    pub id: Option<String>,
    /// Full text of the candidate.
    pub text: String,
}

/// Options for [`rerank`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RerankOptions {
    /// Keep only the top K hits (default: all with score > 0, else all).
    pub top_k: Option<usize>,
    /// Drop hits below this BM25 score (default: 0.0 keeps non-zero; negative keeps all).
    pub min_score: Option<f64>,
    /// Include full text in each hit (default false → preview only).
    #[serde(default)]
    pub include_text: bool,
    /// Preview character budget per hit.
    #[serde(default = "default_preview")]
    pub preview_chars: usize,
}

fn default_preview() -> usize {
    160
}

impl Default for RerankOptions {
    fn default() -> Self {
        Self {
            top_k: None,
            min_score: None,
            include_text: false,
            preview_chars: 160,
        }
    }
}

/// One scored hit.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RerankHit {
    pub rank: usize,
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub score: f64,
    pub preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Result of [`rerank`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RerankResult {
    pub query: String,
    pub hits: Vec<RerankHit>,
    pub candidates: usize,
    pub backend: String,
    pub metrics: TokenMetrics,
}

/// Rank `items` by BM25 relevance to `query`.
pub fn rerank(query: &str, items: &[RerankItem], options: &RerankOptions, config: &Config) -> RerankResult {
    let query = query.trim();
    let docs: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
    let scored = score_documents(query, &docs);

    let min_score = options.min_score.unwrap_or(f64::NEG_INFINITY);
    let mut filtered: Vec<(usize, f64)> = if options.min_score.is_some() {
        scored
            .into_iter()
            .filter(|(_, s)| *s >= min_score)
            .collect()
    } else if options.top_k.is_some() {
        // top_k alone: keep ranking order including weak matches
        scored
    } else {
        // default: non-zero scores only; fall back to all if none matched
        let positive: Vec<_> = scored.iter().copied().filter(|(_, s)| *s > 0.0).collect();
        if positive.is_empty() {
            scored
        } else {
            positive
        }
    };

    if filtered.is_empty() && !items.is_empty() {
        filtered = items.iter().enumerate().map(|(i, _)| (i, 0.0)).collect();
    }

    if let Some(k) = options.top_k {
        filtered.truncate(k);
    }

    let preview_chars = options.preview_chars.max(32);
    let hits: Vec<RerankHit> = filtered
        .into_iter()
        .enumerate()
        .map(|(rank, (index, score))| {
            let item = &items[index];
            let preview: String = item.text.chars().take(preview_chars).collect();
            RerankHit {
                rank: rank + 1,
                index,
                id: item.id.clone(),
                score: (score * 1000.0).round() / 1000.0,
                preview,
                text: if options.include_text {
                    Some(item.text.clone())
                } else {
                    None
                },
            }
        })
        .collect();

    let original_tokens: usize = items
        .iter()
        .map(|i| estimate_tokens(&i.text, config))
        .sum();
    let result_tokens: usize = hits
        .iter()
        .map(|h| {
            let body = h.text.as_deref().unwrap_or(&h.preview);
            estimate_tokens(body, config)
        })
        .sum();

    RerankResult {
        query: query.to_string(),
        hits,
        candidates: items.len(),
        backend: "bm25".into(),
        metrics: TokenMetrics::new(original_tokens, result_tokens),
    }
}

/// Parse items from a JSON array string, newline-separated blocks, or object list.
pub fn parse_rerank_items(text: &str) -> Result<Vec<RerankItem>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("rerank requires non-empty `items` or `text`".into());
    }
    if trimmed.starts_with('[') {
        if let Ok(items) = serde_json::from_str::<Vec<RerankItem>>(trimmed) {
            if !items.is_empty() {
                return Ok(items);
            }
        }
        if let Ok(strings) = serde_json::from_str::<Vec<String>>(trimmed) {
            if !strings.is_empty() {
                return Ok(strings
                    .into_iter()
                    .map(|text| RerankItem { id: None, text })
                    .collect());
            }
        }
        return Err("rerank `text` JSON array could not be parsed as items or strings".into());
    }
    // Split on blank lines into paragraphs; else each non-empty line.
    let paras: Vec<&str> = trimmed
        .split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if paras.len() > 1 {
        return Ok(paras
            .into_iter()
            .map(|text| RerankItem {
                id: None,
                text: text.to_string(),
            })
            .collect());
    }
    Ok(trimmed
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|text| RerankItem {
            id: None,
            text: text.to_string(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reranks_auth_chunk_first() {
        let items = vec![
            RerankItem {
                id: Some("a".into()),
                text: "database migration notes for inventory".into(),
            },
            RerankItem {
                id: Some("b".into()),
                text: "auth token refresh failed with status 401".into(),
            },
            RerankItem {
                id: Some("c".into()),
                text: "css layout tweaks for sidebar".into(),
            },
        ];
        let result = rerank(
            "auth 401 token",
            &items,
            &RerankOptions {
                top_k: Some(2),
                ..Default::default()
            },
            &Config::default(),
        );
        assert_eq!(result.hits[0].id.as_deref(), Some("b"));
        assert_eq!(result.backend, "bm25");
        assert_eq!(result.hits.len(), 2);
    }
}
