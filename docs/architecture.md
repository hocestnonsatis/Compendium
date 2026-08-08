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
  action dispatch (server.rs)
        │
        ├── heuristic pipeline (filter, compress, BM25, AFM, …)
        ├── rerank / brief → BM25 (+ optional embeddings = hybrid)
        │                         └── opt-in SLM cross-encoder top-N
        └── smart actions → loopback OpenAI-compatible SLM
                    │ on unset/fail
                    └── heuristic fallback (backend: "heuristic")
```

Session state: in-process by default. Set `COMPENDIUM_CACHE_DIR` to persist cache/chunk keys across MCP restarts (size-capped; TTL enforced on access and load). Opt-in action audit: `COMPENDIUM_AUDIT_PATH` (JSONL metadata only).

**Next:** propose via issues — A-roadmap through 0.4 is shipped (see [REPORT.md](../REPORT.md) §7).

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
