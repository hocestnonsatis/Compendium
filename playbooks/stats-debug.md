---
id: stats-debug
name: Stats debug
description: Read session token/latency/backend telemetry when savings look wrong
tags: stats, telemetry, latency, debug
---

# Stats debug playbook

1. Call `action=stats` (no fields). Inspect `tokens_saved`, `reduction_ratio`, `p99_latency_ms`, `by_backend`, cache_* fields.
2. Unexpected `heuristic` on smart/hybrid paths → `action=llm_status` (`force: true` for a chat ping).
3. Cross-encoder usage: look for `cross_encoder` / `cross_encoder_partial` in `by_backend`; per-call CE time is on `rerank` results as `cross_encoder_ms`.
4. High bypass_ratio → short inputs under `COMPENDIUM_SIGNAL_MIN_CHARS`; use `force: true` when compression is required.
5. Optional: set `reset: true` after diagnosing to clear session counters.
