//! BM25 rerank of candidate chunks / snippets for a query.

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
    /// Drop hits below this score (default: 0.0 keeps non-zero; negative keeps all).
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
    /// Min-max normalized BM25 score in `[0, 1]`.
    pub score: f64,
    /// Raw BM25 score.
    pub bm25_score: f64,
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
    /// Always `bm25`.
    pub backend: String,
    pub metrics: TokenMetrics,
}

/// Min-max normalize scores into `[0, 1]` (all-equal → 1.0 if positive else 0.0).
pub fn min_max_norm(scores: &[f64]) -> Vec<f64> {
    let Some((&min_v, &max_v)) = scores
        .iter()
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .zip(
            scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
        )
    else {
        return Vec::new();
    };
    if (max_v - min_v).abs() < f64::EPSILON {
        let fill = if max_v > 0.0 { 1.0 } else { 0.0 };
        return vec![fill; scores.len()];
    }
    scores
        .iter()
        .map(|s| ((s - min_v) / (max_v - min_v)).clamp(0.0, 1.0))
        .collect()
}

/// Rank `items` by BM25 relevance to `query`.
pub fn rerank(
    query: &str,
    items: &[RerankItem],
    options: &RerankOptions,
    config: &Config,
) -> RerankResult {
    let query = query.trim();
    let docs: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
    let scored = score_documents(query, &docs);

    let mut bm25_raw = vec![0.0f64; items.len()];
    for (idx, score) in &scored {
        if *idx < bm25_raw.len() {
            bm25_raw[*idx] = *score;
        }
    }
    let norm = min_max_norm(&bm25_raw);

    let mut ranked: Vec<(usize, f64)> = norm.iter().enumerate().map(|(i, s)| (i, *s)).collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut filtered: Vec<(usize, f64)> = if let Some(min_score) = options.min_score {
        ranked
            .into_iter()
            .filter(|(_, s)| *s >= min_score)
            .collect()
    } else if options.top_k.is_some() {
        ranked
    } else {
        let positive: Vec<_> = ranked.iter().copied().filter(|(_, s)| *s > 0.0).collect();
        if positive.is_empty() {
            ranked
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
            RerankHit {
                rank: rank + 1,
                index,
                id: item.id.clone(),
                score: round3(score),
                bm25_score: round3(bm25_raw[index]),
                preview: item.text.chars().take(preview_chars).collect(),
                text: options.include_text.then(|| item.text.clone()),
            }
        })
        .collect();

    let original_tokens: usize = items.iter().map(|i| estimate_tokens(&i.text, config)).sum();
    let result_tokens: usize = hits
        .iter()
        .map(|h| estimate_tokens(h.text.as_deref().unwrap_or(&h.preview), config))
        .sum();

    RerankResult {
        query: query.to_string(),
        hits,
        candidates: items.len(),
        backend: "bm25".into(),
        metrics: TokenMetrics::new(original_tokens, result_tokens),
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Parse items from a JSON array string, blank-line separated blocks, or one per line.
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

    #[test]
    fn parse_items_from_paragraphs_and_json() {
        let paras = parse_rerank_items("one two\n\nthree four").unwrap();
        assert_eq!(paras.len(), 2);
        let json = parse_rerank_items(r#"["a","b","c"]"#).unwrap();
        assert_eq!(json.len(), 3);
        assert!(parse_rerank_items("   ").is_err());
    }
}
