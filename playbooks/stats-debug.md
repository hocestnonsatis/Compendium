---
id: stats-debug
name: Stats debug
description: Read session token/latency/backend telemetry when savings look wrong
tags: stats, telemetry, latency, debug
---

# Stats debug playbook

1. Call `action=stats` (no fields). Inspect `tokens_saved`, `reduction_ratio`, `p99_latency_ms`, `by_backend`, `cache_*`, and `embed_cache_hits` / `embed_cache_misses`.
2. Unexpected `heuristic` on smart paths, or `bm25` when you expected hybrid → `action=llm_status` (`force: true` for a chat ping). Read `fallback_reason` on the last `rerank` / smart result.
3. Hybrid needs `COMPENDIUM_LOCAL_LLM_URL` + working `/embeddings`; CE needs `use_cross_encoder` / `COMPENDIUM_RERANK_CROSS_ENCODER`. Partial CE → backend `cross_encoder_partial`.
4. Cross-encoder usage: look for `cross_encoder` / `cross_encoder_partial` in `by_backend`; per-call CE time is on `rerank` results as `cross_encoder_ms`.
5. High bypass_ratio → short inputs under `COMPENDIUM_SIGNAL_MIN_CHARS`; use `force: true` when compression is required.
6. Optional: set `reset: true` after diagnosing to clear session counters (also resets process embed-cache hit/miss counters).
