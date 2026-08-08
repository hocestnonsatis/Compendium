# Architecture

Compendium is a local-first MCP server that shrinks tool/log/workspace text before it enters the model context.

## Gateway

One MCP tool: **`compendium`**. The `action` enum selects the pipeline op. Every call returns:

```json
{ "ok": true, "action": "filter", "result_json": "{...}" }
```

Parse `result_json` as JSON for the action payload. Optional `sanitize_input: true` scrubs secrets/IPI before the chosen action.

Progressive disclosure:

| Surface | Role |
|---------|------|
| `action=catalog` / `help` | Short ads → full notes |
| `cmp://skill/index` | Skill index |
| `cmp://skill/action/{id}` | Full action markdown |
| `cmp://skill/playbook/{id}` | Playbook body |

## Pipeline

```
text / query / messages
        │
        ▼
 optional sanitize_input
        │
        ▼
  action dispatch (server/ + actions.rs)
        │
        ├── heuristic pipeline (filter, compress, BM25, AFM, …)
        ├── rerank / brief → BM25 (+ optional embeddings = hybrid)
        │                         └── opt-in SLM cross-encoder top-N
        └── smart actions → loopback OpenAI-compatible SLM
                    │ on unset/fail
                    └── heuristic fallback (backend: "heuristic")
```

Session state: in-process by default. Set `COMPENDIUM_CACHE_DIR` to persist cache/chunk keys across MCP restarts (size-capped; TTL enforced on access and load). Opt-in action audit: `COMPENDIUM_AUDIT_PATH` (JSONL metadata only).

**Next (C-roadmap):** deeper eval floors, agent DX playbooks (pack/install/http), retrieval telemetry polish, shared-cache ops docs. See [REPORT.md](../REPORT.md) §7. **Deferred:** embedded LLM, TurboQuant/NPU, foreign MCP proxy, cloud embeddings.

## Quality gates

- Fixtures: [`testdata/`](../testdata/) (noisy logs, cargo fail, long chat, bulky JSON, rerank candidates).
- CI: `cargo test --test eval_regression` — token-reduction floors, heuristic determinism, soft latency smoke.
- Latency budget: `COMPENDIUM_EVAL_LATENCY_MS` (default 2000 locally; CI uses a higher soft cap).
- Heuristic paths must stay deterministic (prefix-cache friendly). Smart/SLM paths report `backend`.

## Hybrid retrieval

- `rerank` / `brief` default `use_embeddings: true` when a loopback LLM is configured; blend weight `COMPENDIUM_HYBRID_ALPHA` (default 0.55 BM25).
- Opt-in CE: `COMPENDIUM_RERANK_CROSS_ENCODER` / `rerank.use_cross_encoder`; prefer `/v1/rerank`, else chat; partial pair failures keep prior scores (`cross_encoder_partial`).
- Embedding vectors are cached in-process and, when session cache is used, under `cache://embed/…` keys (also persisted if `COMPENDIUM_CACHE_DIR` is set). Hit/miss counters surface via `stats` / rerank telemetry.

## Shared cache ops

- Multiple MCP processes may share one `COMPENDIUM_CACHE_DIR`. There is **no** cross-process lock; TTL/eviction are best-effort on access and load.
- Prefer a dedicated dir per host/user; do not assume exclusive writers.
- Cursor/local development: after code changes, rebuild `target/release/compendium` and reload MCP so the live tool schema matches source (avoid stale npx/release binaries during development).

## Transports

| Mode | Binary | Notes |
|------|--------|-------|
| stdio | default | Cursor / Claude Desktop |
| HTTP/SSE | `--features http` → `compendium http [BIND]` | `/mcp` on loopback by default |

## Security posture

- Local LLM URLs must resolve to **loopback** (SSRF guard). Non-loopback private ranges are rejected.
- Archives never execute scripts; size/file caps apply.
- Cloud embeddings / remote LLM bases are out of scope.
- Hybrid `rerank` / `brief` may call loopback `/embeddings` when configured; never remote hosts.

## Related

- Product docs: [README.md](../README.md)
- Design essay + roadmap status: [REPORT.md](../REPORT.md)
- Releases: [CHANGELOG.md](../CHANGELOG.md)
