# Examples

Sample JSON argument objects for the MCP tool `compendium`.

In Cursor (or any MCP client), call the tool with the file contents as arguments.
Each file is a single JSON object: set `action` and the fields that action needs.

| File | Action | Purpose |
|------|--------|---------|
| [`filter-log.json`](filter-log.json) | `filter` | Strip ANSI + keep error/info lines |
| [`compress-output-cargo.json`](compress-output-cargo.json) | `compress_output` | Domain scrub for cargo/test noise |
| [`brief-workspace.json`](brief-workspace.json) | `brief` | Pack a starter briefing for a task query |
| [`catalog.json`](catalog.json) | `catalog` | List action advertisements |
| [`prune-afm.json`](prune-afm.json) | `prune_history` | AFM-tier history prune |
| [`llm-status.json`](llm-status.json) | `llm_status` | Why smart/hybrid fell back to heuristics |
| [`rerank-cross-encoder.json`](rerank-cross-encoder.json) | `rerank` | Opt-in SLM cross-encoder top-N rescore |

CLI smoke (after `cargo build --release`):

```bash
# Not a full MCP client — use your host’s tool UI, or the e2e harness:
cargo test --test e2e_smoke -- --nocapture
```

Envelope reminder: responses look like `{ "ok", "action", "result_json" }`. Parse `result_json` as JSON.
