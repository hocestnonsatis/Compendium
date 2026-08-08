---
id: hybrid-rerank
name: Hybrid + cross-encoder rerank
description: Rank chunks with BM25, optional embeddings, and opt-in SLM cross-encoder
tags: rerank, embeddings, cross-encoder, retrieval
---

# Hybrid / cross-encoder rerank playbook

1. Gather candidates (`chunk` map, `items`, or newline blocks) and a concrete `query`.
2. Call `action=rerank` (embeddings on by default when `COMPENDIUM_LOCAL_LLM_URL` is set).
3. For harder ranking, set `rerank.use_cross_encoder: true` or `COMPENDIUM_RERANK_CROSS_ENCODER=1`.
   - Prefer a local server that exposes Cohere-style `POST /v1/rerank` (batched).
   - Otherwise Compendium falls back to pairwise chat scoring (`cross_encoder_mode: chat`).
4. Check `backend`: `hybrid` | `cross_encoder` | `cross_encoder_partial` | `bm25`.
5. Use top hits only; keep `include_text: false` unless you need full bodies in context.
6. `action=stats` → `by_backend` counts CE usage (`cross_encoder` / `cross_encoder_partial`); result may include `cross_encoder_ms` and `cross_encoder_mode`.
7. Embedding vectors are cached in-process (and in session/disk cache when `COMPENDIUM_CACHE_DIR` is set) so repeated `rerank` calls avoid re-hitting `/embeddings`.
