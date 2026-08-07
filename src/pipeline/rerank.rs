//! BM25 (+ optional loopback embedding) rerank of text chunks / candidates.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::bm25::score_documents;
use crate::pipeline::local_llm::LocalLlmClient;
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Cap characters sent to the embeddings API per candidate.
const EMBED_MAX_CHARS: usize = 4_000;
/// Max texts per embeddings HTTP request.
const EMBED_BATCH: usize = 32;

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
    /// Drop hits below this **final** score (default: 0.0 keeps non-zero; negative keeps all).
    pub min_score: Option<f64>,
    /// Include full text in each hit (default false → preview only).
    #[serde(default)]
    pub include_text: bool,
    /// Preview character budget per hit.
    #[serde(default = "default_preview")]
    pub preview_chars: usize,
    /// When true (default), try loopback embeddings and blend with BM25.
    #[serde(default = "default_true")]
    pub use_embeddings: bool,
    /// BM25 weight in `[0, 1]`; remainder is cosine. Default: `COMPENDIUM_HYBRID_ALPHA` / 0.55.
    pub alpha: Option<f64>,
}

fn default_preview() -> usize {
    160
}

fn default_true() -> bool {
    true
}

impl Default for RerankOptions {
    fn default() -> Self {
        Self {
            top_k: None,
            min_score: None,
            include_text: false,
            preview_chars: 160,
            use_embeddings: true,
            alpha: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_score: Option<f64>,
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
    /// `bm25`, `hybrid`, or `bm25` with `fallback_reason` when embeddings failed.
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
    pub metrics: TokenMetrics,
}

/// Cosine similarity of two equal-length vectors (0 if either is empty / mismatched).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0)
}

/// Min-max normalize scores into `[0, 1]` (all-equal → 1.0 if positive else 0.0).
pub fn min_max_norm(scores: &[f64]) -> Vec<f64> {
    if scores.is_empty() {
        return Vec::new();
    }
    let min = scores.iter().copied().fold(f64::INFINITY, f64::min);
    let max = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if (max - min).abs() < 1e-12 {
        let v = if max > 0.0 { 1.0 } else { 0.0 };
        return vec![v; scores.len()];
    }
    scores.iter().map(|s| (s - min) / (max - min)).collect()
}

/// Blend normalized BM25 and cosine: `alpha * bm25 + (1 - alpha) * cosine`.
pub fn blend_hybrid(bm25_norm: &[f64], cosine: &[f64], alpha: f64) -> Vec<f64> {
    let alpha = alpha.clamp(0.0, 1.0);
    let beta = 1.0 - alpha;
    bm25_norm
        .iter()
        .zip(cosine.iter())
        .map(|(b, c)| alpha * b + beta * c)
        .collect()
}

fn truncate_for_embed(text: &str) -> String {
    text.chars().take(EMBED_MAX_CHARS).collect()
}

fn try_embeddings(config: &Config, query: &str, docs: &[&str]) -> Result<Vec<f64>, String> {
    let client = match LocalLlmClient::from_config(&config.local_llm) {
        None => return Err("local LLM unset".into()),
        Some(Err(e)) => return Err(e.to_string()),
        Some(Ok(c)) => c,
    };
    let model = config.local_llm.embedding_model_name().to_string();
    let mut inputs: Vec<String> = Vec::with_capacity(docs.len() + 1);
    inputs.push(truncate_for_embed(query));
    for d in docs {
        inputs.push(truncate_for_embed(d));
    }

    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(inputs.len());
    for chunk in inputs.chunks(EMBED_BATCH) {
        let batch = chunk.to_vec();
        let part = client.embed(&model, &batch).map_err(|e| e.to_string())?;
        vectors.extend(part);
    }
    if vectors.len() != inputs.len() {
        return Err(format!(
            "embedding count mismatch: {} vs {}",
            vectors.len(),
            inputs.len()
        ));
    }
    let q = &vectors[0];
    Ok(vectors[1..]
        .iter()
        .map(|v| {
            let c = cosine_similarity(q, v);
            // Map cosine [-1,1] → [0,1] for blending.
            ((c + 1.0) / 2.0).clamp(0.0, 1.0)
        })
        .collect())
}

/// Rank `items` by BM25 relevance to `query`, optionally blending loopback embeddings.
pub fn rerank(
    query: &str,
    items: &[RerankItem],
    options: &RerankOptions,
    config: &Config,
) -> RerankResult {
    let query = query.trim();
    let docs: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
    let scored = score_documents(query, &docs);

    // Dense BM25 vector aligned to item index.
    let mut bm25_raw = vec![0.0f64; items.len()];
    for (idx, score) in &scored {
        if *idx < bm25_raw.len() {
            bm25_raw[*idx] = *score;
        }
    }
    let bm25_norm = min_max_norm(&bm25_raw);

    let alpha = options.alpha.unwrap_or(config.hybrid_alpha).clamp(0.0, 1.0);

    let mut backend = "bm25".to_string();
    let mut fallback_reason = None;
    let mut embedding_scores: Option<Vec<f64>> = None;
    let mut final_scores = bm25_norm.clone();

    if options.use_embeddings && !items.is_empty() {
        match try_embeddings(config, query, &docs) {
            Ok(cos) => {
                embedding_scores = Some(cos.clone());
                final_scores = blend_hybrid(&bm25_norm, &cos, alpha);
                backend = "hybrid".into();
            }
            Err(e) => {
                fallback_reason = Some(e);
            }
        }
    }

    // Build ranked index list from final scores (stable: higher score first, then lower index).
    let mut ranked: Vec<(usize, f64)> = final_scores
        .iter()
        .enumerate()
        .map(|(i, s)| (i, *s))
        .collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let min_score = options.min_score.unwrap_or(f64::NEG_INFINITY);
    let mut filtered: Vec<(usize, f64)> = if options.min_score.is_some() {
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
            let preview: String = item.text.chars().take(preview_chars).collect();
            RerankHit {
                rank: rank + 1,
                index,
                id: item.id.clone(),
                score: (score * 1000.0).round() / 1000.0,
                bm25_score: Some((bm25_raw[index] * 1000.0).round() / 1000.0),
                embedding_score: embedding_scores
                    .as_ref()
                    .map(|v| (v[index] * 1000.0).round() / 1000.0),
                preview,
                text: if options.include_text {
                    Some(item.text.clone())
                } else {
                    None
                },
            }
        })
        .collect();

    let original_tokens: usize = items.iter().map(|i| estimate_tokens(&i.text, config)).sum();
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
        backend: backend.clone(),
        fallback_reason,
        alpha: if backend == "hybrid" {
            Some(alpha)
        } else {
            None
        },
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
                use_embeddings: false,
                ..Default::default()
            },
            &Config::default(),
        );
        assert_eq!(result.hits[0].id.as_deref(), Some("b"));
        assert_eq!(result.backend, "bm25");
        assert_eq!(result.hits.len(), 2);
    }

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0f32, 0.0, 0.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hybrid_blend_prefers_high_cosine_when_alpha_low() {
        let bm25 = min_max_norm(&[10.0, 1.0]);
        let cos = vec![0.1, 0.9];
        let blended = blend_hybrid(&bm25, &cos, 0.2);
        assert!(blended[1] > blended[0]);
    }

    #[test]
    fn without_llm_embeddings_falls_back_to_bm25() {
        let items = vec![RerankItem {
            id: Some("x".into()),
            text: "hello world".into(),
        }];
        let result = rerank(
            "hello",
            &items,
            &RerankOptions {
                use_embeddings: true,
                ..Default::default()
            },
            &Config::default(),
        );
        assert_eq!(result.backend, "bm25");
        assert!(result.fallback_reason.is_some());
    }
}
