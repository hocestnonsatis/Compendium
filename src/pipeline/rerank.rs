//! BM25 (+ optional loopback embedding + optional SLM cross-encoder) rerank.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::bm25::score_documents;
use crate::pipeline::local_llm::{LocalLlmClient, LocalLlmError};
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// Cap characters sent to the embeddings API per candidate.
const EMBED_MAX_CHARS: usize = 4_000;
/// Max texts per embeddings HTTP request.
const EMBED_BATCH: usize = 32;
/// Cap document chars sent to the cross-encoder chat scorer.
const CE_DOC_MAX_CHARS: usize = 3_000;
/// Soft max tokens for a score-only chat reply.
const CE_MAX_TOKENS: u32 = 16;

const CE_SYSTEM: &str = "You score how relevant a document is to a query.\n\
Reply with ONLY a number between 0.0 and 1.0. No words, no markdown.";

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
    /// When set, overrides `COMPENDIUM_RERANK_CROSS_ENCODER` for SLM top-N rescore.
    pub use_cross_encoder: Option<bool>,
    /// Candidates passed to the cross-encoder (default: config / 16, clamped 4–64).
    pub cross_encoder_top_n: Option<usize>,
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
            use_cross_encoder: None,
            cross_encoder_top_n: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_encoder_score: Option<f64>,
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
    /// `bm25`, `hybrid`, `cross_encoder`, `cross_encoder_partial`, or prior backend with `fallback_reason`.
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
    /// Wall-clock for the cross-encoder stage only (ms), when attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_encoder_ms: Option<f64>,
    /// How CE ran: `rerank_api` | `chat` | absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_encoder_mode: Option<String>,
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

/// Blend min-max BM25 with cosine: `alpha * bm25 + (1 - alpha) * cosine`.
pub fn blend_hybrid(bm25_norm: &[f64], cosine: &[f64], alpha: f64) -> Vec<f64> {
    let a = alpha.clamp(0.0, 1.0);
    bm25_norm
        .iter()
        .zip(cosine.iter())
        .map(|(b, c)| a * b + (1.0 - a) * c)
        .collect()
}

/// Parse a 0–1 relevance score from SLM output (first number; values >1 treated as percent).
pub fn parse_relevance_score(text: &str) -> Result<f64, String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(text.trim());
    if let Ok(v) = line.parse::<f64>() {
        return Ok(normalize_raw_score(v));
    }
    let mut num = String::new();
    let mut started = false;
    for ch in line.chars() {
        if ch.is_ascii_digit() || ch == '.' || (ch == '-' && !started) {
            num.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    if num.is_empty() {
        return Err(format!(
            "could not parse relevance score from: {}",
            text.chars().take(80).collect::<String>()
        ));
    }
    let v: f64 = num.parse().map_err(|e| format!("score parse: {e}"))?;
    Ok(normalize_raw_score(v))
}

fn normalize_raw_score(v: f64) -> f64 {
    if (0.0..=1.0).contains(&v) {
        v
    } else if (1.0..=100.0).contains(&v) {
        (v / 100.0).clamp(0.0, 1.0)
    } else {
        v.clamp(0.0, 1.0)
    }
}

fn try_embeddings(config: &Config, query: &str, docs: &[&str]) -> Result<Vec<f64>, String> {
    let Some(client_res) = LocalLlmClient::from_config(&config.local_llm) else {
        return Err("local LLM unset".into());
    };
    let client = client_res.map_err(|e| e.to_string())?;
    let model = config.local_llm.embedding_model_name().to_string();

    let mut inputs: Vec<String> = Vec::with_capacity(docs.len() + 1);
    inputs.push(query.chars().take(EMBED_MAX_CHARS).collect());
    for d in docs {
        inputs.push(d.chars().take(EMBED_MAX_CHARS).collect());
    }

    let mut all_vecs: Vec<Vec<f32>> = Vec::with_capacity(inputs.len());
    for chunk in inputs.chunks(EMBED_BATCH) {
        let batch = client.embed(&model, chunk).map_err(|e| e.to_string())?;
        all_vecs.extend(batch);
    }
    if all_vecs.len() != inputs.len() {
        return Err(format!(
            "embedding count mismatch: got {} want {}",
            all_vecs.len(),
            inputs.len()
        ));
    }
    let q = &all_vecs[0];
    Ok(all_vecs[1..]
        .iter()
        .map(|d| {
            let c = cosine_similarity(q, d);
            ((c + 1.0) / 2.0).clamp(0.0, 1.0)
        })
        .collect())
}

/// SLM / API scores for `candidate_indices`.
/// Prefer Cohere-style `/rerank`; fall back to pairwise chat. Per-pair chat parse
/// failures keep the prior (BM25/hybrid) score instead of aborting the whole stage.
fn try_cross_encoder(
    config: &Config,
    query: &str,
    items: &[RerankItem],
    candidate_indices: &[usize],
    prior_by_index: &[f64],
) -> Result<CrossEncoderOutcome, String> {
    let Some(client_res) = LocalLlmClient::from_config(&config.local_llm) else {
        return Err("local LLM unset".into());
    };
    let client = client_res.map_err(|e: LocalLlmError| e.to_string())?;
    let started = std::time::Instant::now();

    let docs: Vec<String> = candidate_indices
        .iter()
        .map(|&idx| items[idx].text.chars().take(CE_DOC_MAX_CHARS).collect())
        .collect();

    match client.rerank(query, &docs, Some(docs.len())) {
        Ok(api_hits) => {
            let mut by_local = vec![None; candidate_indices.len()];
            for (doc_i, score) in api_hits {
                if doc_i < by_local.len() {
                    by_local[doc_i] = Some(score);
                }
            }
            let mut scored = Vec::with_capacity(candidate_indices.len());
            let mut missing = 0usize;
            for (local_i, &item_idx) in candidate_indices.iter().enumerate() {
                if let Some(s) = by_local[local_i] {
                    scored.push((item_idx, s));
                } else {
                    missing += 1;
                    scored.push((item_idx, prior_by_index[item_idx]));
                }
            }
            let partial = missing > 0;
            return Ok(CrossEncoderOutcome {
                scored,
                mode: "rerank_api".into(),
                partial,
                pair_fallbacks: missing,
                latency_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
        }
        Err(LocalLlmError::BadStatus { status, .. }) if status == 404 || status == 405 => {
            // Fall through to chat pairwise.
        }
        Err(LocalLlmError::BadStatus { status, body }) if status == 501 || status == 400 => {
            // Unsupported / bad schema — try chat.
            tracing::debug!(status, %body, "rerank API rejected; using chat CE");
        }
        Err(e) => {
            // Network / other: still try chat before giving up.
            tracing::debug!(error = %e, "rerank API failed; using chat CE");
        }
    }

    let mut scored = Vec::with_capacity(candidate_indices.len());
    let mut pair_fallbacks = 0usize;
    let mut any_ce = false;
    for &idx in candidate_indices {
        let doc: String = items[idx].text.chars().take(CE_DOC_MAX_CHARS).collect();
        let user = format!("Query:\n{query}\n\nDocument:\n{doc}\n\nRelevance score:");
        match client
            .chat(CE_SYSTEM, &user, Some(CE_MAX_TOKENS))
            .map_err(|e| e.to_string())
            .and_then(|reply| parse_relevance_score(&reply))
        {
            Ok(score) => {
                any_ce = true;
                scored.push((idx, score));
            }
            Err(_) => {
                pair_fallbacks += 1;
                scored.push((idx, prior_by_index[idx]));
            }
        }
    }
    if !any_ce {
        return Err("all chat cross-encoder pairs failed to parse".into());
    }
    Ok(CrossEncoderOutcome {
        scored,
        mode: "chat".into(),
        partial: pair_fallbacks > 0,
        pair_fallbacks,
        latency_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

struct CrossEncoderOutcome {
    scored: Vec<(usize, f64)>,
    mode: String,
    partial: bool,
    pair_fallbacks: usize,
    latency_ms: f64,
}

fn sort_ranked(ranked: &mut [(usize, f64)]) {
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

/// Rank `items` by BM25 relevance to `query`, optionally blending embeddings and SLM CE.
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
    let bm25_norm = min_max_norm(&bm25_raw);

    let alpha = options.alpha.unwrap_or(config.hybrid_alpha).clamp(0.0, 1.0);

    let mut backend = "bm25".to_string();
    let mut fallback_reason = None;
    let mut embedding_scores: Option<Vec<f64>> = None;
    let mut final_scores = bm25_norm.clone();
    let mut had_hybrid = false;

    if options.use_embeddings && !items.is_empty() {
        match try_embeddings(config, query, &docs) {
            Ok(cos) => {
                embedding_scores = Some(cos.clone());
                final_scores = blend_hybrid(&bm25_norm, &cos, alpha);
                backend = "hybrid".into();
                had_hybrid = true;
            }
            Err(e) => {
                fallback_reason = Some(e);
            }
        }
    }

    let mut ranked: Vec<(usize, f64)> = final_scores
        .iter()
        .enumerate()
        .map(|(i, s)| (i, *s))
        .collect();
    sort_ranked(&mut ranked);

    let use_ce = options
        .use_cross_encoder
        .unwrap_or(config.rerank_cross_encoder);
    let mut cross_encoder_by_index: Option<Vec<Option<f64>>> = None;
    let mut cross_encoder_ms = None;
    let mut cross_encoder_mode = None;

    if use_ce && !items.is_empty() {
        let top_n = options
            .cross_encoder_top_n
            .unwrap_or(config.cross_encoder_top_n)
            .clamp(4, 64);
        let candidate_indices: Vec<usize> = ranked.iter().take(top_n).map(|(i, _)| *i).collect();
        let prior_by_index: Vec<f64> = final_scores.clone();
        match try_cross_encoder(config, query, items, &candidate_indices, &prior_by_index) {
            Ok(outcome) => {
                let mut by_idx = vec![None; items.len()];
                let mut ce_ranked: Vec<(usize, f64)> = Vec::with_capacity(outcome.scored.len());
                for (idx, score) in &outcome.scored {
                    by_idx[*idx] = Some(*score);
                    ce_ranked.push((*idx, *score));
                }
                sort_ranked(&mut ce_ranked);
                let mut seen = std::collections::HashSet::new();
                let mut new_ranked = Vec::with_capacity(ranked.len());
                for (idx, score) in ce_ranked {
                    seen.insert(idx);
                    new_ranked.push((idx, score));
                }
                for (idx, score) in &ranked {
                    if !seen.contains(idx) {
                        new_ranked.push((*idx, *score));
                    }
                }
                ranked = new_ranked;
                cross_encoder_by_index = Some(by_idx);
                cross_encoder_ms = Some(outcome.latency_ms);
                cross_encoder_mode = Some(outcome.mode);
                backend = if outcome.partial {
                    "cross_encoder_partial".into()
                } else {
                    "cross_encoder".into()
                };
                if outcome.pair_fallbacks > 0 {
                    let msg = format!(
                        "{} pair(s) kept prior BM25/hybrid score",
                        outcome.pair_fallbacks
                    );
                    fallback_reason = Some(match fallback_reason.take() {
                        Some(prev) => format!("{prev}; {msg}"),
                        None => msg,
                    });
                }
            }
            Err(e) => {
                let msg = format!("cross_encoder: {e}");
                fallback_reason = Some(match fallback_reason.take() {
                    Some(prev) => format!("{prev}; {msg}"),
                    None => msg,
                });
            }
        }
    }

    let mut filtered: Vec<(usize, f64)> = if options.min_score.is_some() {
        let min_score = options.min_score.unwrap_or(f64::NEG_INFINITY);
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
                cross_encoder_score: cross_encoder_by_index
                    .as_ref()
                    .and_then(|v| v[index])
                    .map(|s| (s * 1000.0).round() / 1000.0),
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
        alpha: if had_hybrid || (backend.starts_with("cross_encoder") && embedding_scores.is_some())
        {
            Some(alpha)
        } else {
            None
        },
        cross_encoder_ms,
        cross_encoder_mode,
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

    #[test]
    fn parse_relevance_score_accepts_plain_and_percent() {
        assert!((parse_relevance_score("0.85").unwrap() - 0.85).abs() < 1e-9);
        assert!((parse_relevance_score("Score: 90").unwrap() - 0.9).abs() < 1e-9);
        assert!((parse_relevance_score("1.0\n").unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cross_encoder_off_by_default_even_if_requested_without_llm() {
        let items = vec![
            RerankItem {
                id: Some("a".into()),
                text: "unrelated vegetables".into(),
            },
            RerankItem {
                id: Some("b".into()),
                text: "oauth refresh token path".into(),
            },
        ];
        let result = rerank(
            "oauth refresh",
            &items,
            &RerankOptions {
                use_embeddings: false,
                use_cross_encoder: Some(true),
                ..Default::default()
            },
            &Config::default(),
        );
        assert_eq!(result.backend, "bm25");
        assert!(result
            .fallback_reason
            .as_deref()
            .is_some_and(|s| s.contains("cross_encoder")));
    }
}
