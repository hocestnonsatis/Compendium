# Examples

Sample JSON argument objects for the MCP tool `compendium`.

In Cursor (or any MCP client), call the tool with the file contents as arguments.
Each file is a single JSON object: set `action` and the fields that action needs.

## First 10 minutes

1. Install / reload MCP ([`install-npx`](../playbooks/install-npx.md) playbook). Optional SLM: [`setup-ollama`](../playbooks/setup-ollama.md) (`npx -y compendium-mcp setup-ollama --write-mcp`).
2. Call [`catalog.json`](catalog.json) — pick an action from `when_to_use`.
3. Call `help` with that `id` (add `"force": true` for the full example).
4. Run one of the situation samples below.

| Situation | File |
|-----------|------|
| Unsure | [`catalog.json`](catalog.json) |
| Fresh workspace task | [`brief-workspace.json`](brief-workspace.json) |
| Noisy terminal log | [`filter-log.json`](filter-log.json) |
| Cargo / test dump | [`compress-output-cargo.json`](compress-output-cargo.json) |
| Untrusted paste | [`sanitize-untrusted.json`](sanitize-untrusted.json) |

## All samples

| File | Action | Purpose |
|------|--------|---------|
| [`catalog.json`](catalog.json) | `catalog` | List action advertisements |
| [`filter-log.json`](filter-log.json) | `filter` | Strip ANSI + keep error/info lines |
| [`compress-output-cargo.json`](compress-output-cargo.json) | `compress_output` | Domain scrub for cargo/test noise |
| [`sanitize-untrusted.json`](sanitize-untrusted.json) | `sanitize` | Redact secrets + neutralize IPI |
| [`brief-workspace.json`](brief-workspace.json) | `brief` | Pack a starter briefing for a task query |
| [`prune-afm.json`](prune-afm.json) | `prune_history` | AFM-tier history prune |
| [`llm-status.json`](llm-status.json) | `llm_status` | Why smart/hybrid fell back to heuristics |
| [`rerank-cross-encoder.json`](rerank-cross-encoder.json) | `rerank` | Opt-in SLM cross-encoder top-N rescore |

CLI smoke (after `cargo build --release`):

```bash
./target/release/compendium setup-ollama --help
# Not a full MCP client — use your host’s tool UI, or the e2e harness:
cargo test --test e2e_smoke -- --nocapture
```

Envelope reminder: responses look like `{ "ok", "action", "result_json" }`. Parse `result_json` as JSON.
