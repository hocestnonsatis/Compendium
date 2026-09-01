# Architecture

Compendium is a local-first MCP server that shrinks tool/log text before it enters the model context.

## Gateway

One MCP tool: **`compendium`**. The `action` enum selects the pipeline op. Every call returns:

```json
{ "ok": true, "action": "filter", "result_json": "{...}" }
```

Parse `result_json` as JSON for the action payload. Optional `sanitize_input: true` scrubs secrets/IPI before the chosen action. `action=help` lists actions; `help` + `id` returns fields, an example call, and notes.

## Pipeline

```
text / query
      │
      ▼
optional sanitize_input
      │
      ▼
action dispatch (src/server/)
      │
      ├── heuristic pipeline (filter, compress, compress_output, summarize, BM25 rerank, chunk)
      └── smart actions (summarize_smart, filter_relevant)
                │  loopback OpenAI-compatible model when COMPENDIUM_LOCAL_LLM_URL is set
                └── otherwise heuristic fallback (backend: "heuristic")
```

Session state (chunk ids, `cache://` keys, savings counters) lives in the server process and is lost on restart.

## Layout

```
src/
  main.rs            CLI: stdio server | setup-ollama
  setup_ollama.rs    detect/pull/probe Ollama, merge env into mcp.json
  config.rs          COMPENDIUM_* env
  server/            MCP tool + action handlers (rmcp)
  pipeline/          filter, compress, output, summarize, smart, local_llm,
                     bm25, rerank, chunk, cache, sanitize, signal, stats, tokens, catalog
bin/run.js           npm launcher (dev build → cached GitHub Release download)
npm/lib/platform.js  platform → release asset
tests/               integration, e2e stdio smoke, eval regression, local LLM mock, CLI
testdata/            eval fixtures
```

## Quality gates

- `cargo test --test eval_regression` — token-reduction floors, heuristic determinism, soft latency smoke (`COMPENDIUM_EVAL_LATENCY_MS`).
- `cargo test --test e2e_smoke` — spawns the binary, completes the MCP handshake over stdio, calls every action.
- Heuristic paths must stay deterministic. Smart paths must report `backend`.

## Security posture

- Local LLM URLs must resolve to **loopback**; private LAN and public hosts are rejected (SSRF guard).
- `sanitize` redacts common secret shapes and neutralizes prompt-injection / poison parameters.
- No cloud path. No archive handling. No filesystem scanning.

## Distribution

GitHub Release `vX.Y.Z` → `release.yml` builds five native binaries, attaches them to the Release, and publishes the single `compendium-mcp` npm package. The launcher downloads the matching asset to `~/.cache/compendium-mcp/<version>/` on first run.
