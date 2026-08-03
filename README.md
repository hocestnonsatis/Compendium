# Compendium

MCP server that **minimizes LLM token usage** by compressing, summarizing, filtering, and chunk-referencing large context before it reaches the model.

Built in Rust with the official [`rmcp`](https://crates.io/crates/rmcp) SDK.

## Quick start (Cursor)

You need **Node.js 18+**. Compendium itself arrives via npm — no Rust install required.

### 1. Add the MCP server

Open Cursor MCP settings (`~/.cursor/mcp.json` or the project `.cursor/mcp.json`) and add:

```json
{
  "mcpServers": {
    "compendium": {
      "command": "npx",
      "args": ["-y", "compendium-mcp"]
    }
  }
}
```

Restart MCP / reload Cursor. You should see one tool named **`compendium`**.

That alone is enough: filter, compress, summarize, cache, and BM25 actions all work **without** a local model (fast heuristics).

### 2. (Optional) Smarter summaries with Ollama

Want better `summarize_smart` / `filter_relevant`? Run a small model on your machine and point Compendium at it.

1. Install [Ollama](https://ollama.com/) and start it (default: `http://127.0.0.1:11434`).
2. Pull a chat model, for example:

```bash
ollama pull qwen:latest
# or a smaller one: ollama pull qwen2.5:3b
```

3. Extend the MCP `env` block (URL must stay on **localhost** — Compendium blocks remote hosts on purpose):

```json
{
  "mcpServers": {
    "compendium": {
      "command": "npx",
      "args": ["-y", "compendium-mcp"],
      "env": {
        "COMPENDIUM_LOCAL_LLM_URL": "http://127.0.0.1:11434/v1",
        "COMPENDIUM_LOCAL_LLM_MODEL": "qwen:latest"
      }
    }
  }
}
```

4. Reload MCP, then ask the agent to call `compendium` with `action: "summarize_smart"`.  
   In the result, `"backend": "local_llm"` means Ollama answered; `"heuristic"` means it fell back (Ollama down, wrong model name, or URL missing).

**Notes**

- Package name on npm is **`compendium-mcp`** (`compendium` was already taken). The CLI binary name is still `compendium`.
- First Ollama reply can be slow while the model loads; later calls are faster.
- Other local OpenAI-compatible servers work the same way (e.g. Lemonade `http://127.0.0.1:13305/api/v1`). See [Environment](#environment).

Smoke-check from a terminal (any folder **except** this git repo root is fine):

```bash
npx -y compendium-mcp --help
```

Binary packaging details for maintainers: [npm/DISTRIBUTION.md](npm/DISTRIBUTION.md).

## Community

- [Contributing](CONTRIBUTING.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md)
- [Support](SUPPORT.md)

## Transports

| Mode | Command | Notes |
|------|---------|-------|
| **stdio** (default) | `compendium` / `compendium stdio` | Cursor / Claude Desktop |
| **Streamable HTTP/SSE** | `compendium http [BIND]` | Requires `--features http`. Endpoint: `http://{bind}/mcp` |

Default HTTP bind: `127.0.0.1:8788` (override with arg or `COMPENDIUM_HTTP_BIND`).

## Tools

Single MCP tool: **`compendium`**. Choose the operation with `action`:

| `action` | Purpose | Main fields |
|----------|---------|-------------|
| `filter` | Strip ANSI, boilerplate, whitespace; densify JSON; keep/drop regexes | `text`, `filter` |
| `compress` | Dense representation of text/code/logs | `text`, `compress` |
| `compress_output` | Domain-aware stdout/stderr scrub (git, cargo, npm, docker, …) | `text`, `output` |
| `summarize` | Hierarchical summary (conversation / file tree / outline) | `text`, `summarize` |
| `summarize_smart` | Local-SLM dense summary (heuristic fallback if unset/fails) | `text`, `smart?`, `summarize?` |
| `filter_relevant` | Query-aware keep of relevant lines (local SLM + heuristic fallback) | `text`, `query`, `smart?` |
| `prune_history` | Drop filler / compress older chat turns | `text` or `messages`, `prune` |
| `chunk` | Split into `cmp://` chunks (session-cached) | `text`, `chunk` |
| `resolve` | Fetch chunk content by id | `id` (+ optional `map` / `text`) |
| `count_tokens` | Measure tokens | `text` |
| `stats` | Session savings + latency/bypass/backend telemetry | `reset?` |
| `cache_store` | Park bulky payload outside the prompt | `text`, `cache` |
| `cache_get` | Retrieve by key | `key` |
| `cache_invalidate` | Drop one key or clear cache | `key?` |
| `sanitize` | Redact secrets + neutralize IPI phrases | `text`, `sanitize?` |
| `rerank` | BM25-rank candidates / chunks for a query | `query`, `items` or `text` or chunk `map`, `rerank?` |

Optional on most text actions: `sanitize_input: true` scrubs before processing. Soft payloads under `COMPENDIUM_SIGNAL_MIN_CHARS` (default 1000) bypass `compress` / `summarize` / `summarize_smart` unless `force: true`.

`filter` accepts optional `query` (top-level or `filter.query`) for BM25 line keep. `prune_history` supports `prune.strategy: "afm"` (Critical / Thematic / Distant tiers; distant blob cached for `cache_get`).

Example:

```json
{
  "action": "filter",
  "text": "…noisy log…",
  "filter": { "strip_ansi": true, "keep_patterns": ["ERROR|WARN"] }
}
```

Response envelope: `{ "ok": true, "action": "filter", "result_json": "{...}" }`. Parse `result_json` as JSON for the action-specific payload.

## Project layout

```
package.json / bin/run.js  # npm wrapper for npx compendium-mcp
npm/                       # platform packages + distribution docs
.github/workflows/         # release cross-compile + npm publish
src/
  main.rs              # CLI: stdio | http
  lib.rs
  config.rs            # COMPENDIUM_* env config
  server.rs            # MCP tool handlers (rmcp macros)
  http.rs              # Streamable HTTP/SSE (feature = "http")
  pipeline/
    tokens.rs          # heuristic or tiktoken BPE (feature = "real-tokens")
    filter.rs
    compress.rs
    summarize.rs
    smart.rs           # summarize_smart + filter_relevant
    local_llm.rs       # OpenAI-compatible local SLM client
    chunk.rs           # chunk + resolve
    cache.rs           # session key/value cache
    stats.rs           # session savings counters
    prune.rs           # conversation history pruning
    output.rs          # domain-aware compress_output
tests/
  integration.rs
  e2e_smoke.rs         # spawns binary, MCP handshake, all tools
```

## Build

```bash
# Default: heuristic tokens + stdio only
cargo build --release

# Exact BPE token counts (tiktoken-rs)
cargo build --release --features real-tokens

# Streamable HTTP transport
cargo build --release --features http

# Everything
cargo build --release --features real-tokens,http
```

Binary: `target/release/compendium`

## Configure (advanced)

The [Quick start](#quick-start-cursor) config is enough for most people. Extra options:

### Claude Desktop

Same `command` / `args` / `env` as Cursor, in Claude’s MCP config file.

### Optional tuning env

```json
"env": {
  "RUST_LOG": "compendium=info",
  "COMPENDIUM_DEFAULT_MAX_TOKENS": "2048",
  "COMPENDIUM_TOKENIZER": "cl100k_base",
  "COMPENDIUM_LOCAL_LLM_URL": "http://127.0.0.1:11434/v1",
  "COMPENDIUM_LOCAL_LLM_MODEL": "qwen:latest"
}
```

### Local Cargo binary (developers)

```json
{
  "mcpServers": {
    "compendium": {
      "command": "/absolute/path/to/Compendium/target/release/compendium",
      "env": {
        "RUST_LOG": "compendium=info",
        "COMPENDIUM_DEFAULT_MAX_TOKENS": "2048"
      }
    }
  }
}
```

### Remote / sidecar (HTTP)

```bash
cargo run --features http -- http 127.0.0.1:8788
# MCP endpoint: http://127.0.0.1:8788/mcp
```

Point an MCP streamable-HTTP client at that URL (e.g. `StreamableHttpClientTransport::from_uri`).

## Environment

| Variable | Default | Meaning |
|----------|---------|---------|
| `COMPENDIUM_CHARS_PER_TOKEN` | `4.0` | Heuristic chars÷tokens (ignored with `real-tokens`) |
| `COMPENDIUM_TOKENIZER` | `cl100k_base` | BPE encoding: `cl100k_base` or `o200k_base` (`real-tokens`) |
| `COMPENDIUM_DEFAULT_MAX_TOKENS` | `2048` | Soft cap for compress |
| `COMPENDIUM_MAX_BLANK_LINES` | `1` | Blank-line collapse limit |
| `COMPENDIUM_SIMILARITY_THRESHOLD` | `0.85` | Jaccard line-dedupe threshold |
| `COMPENDIUM_HTTP_BIND` | `127.0.0.1:8788` | Default HTTP listen address |
| `COMPENDIUM_LOCAL_LLM_URL` | _(unset)_ | OpenAI-compatible base URL (e.g. `http://127.0.0.1:11434/v1` or `http://127.0.0.1:13305/api/v1`). Enables smart actions. |
| `COMPENDIUM_LOCAL_LLM_MODEL` | `Qwen3-4B-GGUF` | Model id on that server (Ollama: e.g. `qwen:latest`) |
| `COMPENDIUM_LOCAL_LLM_API_KEY` | _(unset)_ | Optional bearer token for locked loopback servers |
| `COMPENDIUM_LOCAL_LLM_TIMEOUT_SECS` | `120` | HTTP timeout (first model load can be slow) |
| `COMPENDIUM_SIGNAL_MIN_CHARS` | `1000` | Bypass compress/summarize below this length (`0` disables) |
| `RUST_LOG` | `compendium=info` | Logs on **stderr** only |

## Example tool calls

**Filter noisy terminal output**

```json
{
  "name": "compendium_filter",
  "arguments": {
    "text": "\u001b[31mERROR\u001b[0m boom\n\n\nINFO ok",
    "options": {
      "strip_ansi": true,
      "keep_patterns": ["ERROR|WARN"]
    }
  }
}
```

**Compress a large log**

```json
{
  "name": "compendium_compress",
  "arguments": {
    "text": "...",
    "options": {
      "content_type": "log",
      "max_tokens": 512
    }
  }
}
```

**Chunk a document into references**

```json
{
  "name": "compendium_chunk",
  "arguments": {
    "text": "... huge file ...",
    "options": {
      "source": "file:///path/to/doc.md",
      "chunk_tokens": 400,
      "overlap_tokens": 40
    }
  }
}
```

Prefer the returned `index_text` in the model context; pull individual chunk contents by id only when needed.

**Query-aware filter (local SLM or heuristic fallback)**

```json
{
  "action": "filter_relevant",
  "text": "... noisy cargo/test log ...",
  "query": "why did the auth tests fail",
  "smart": { "max_tokens": 512, "fallback": true }
}
```

Without `COMPENDIUM_LOCAL_LLM_URL`, `summarize_smart` / `filter_relevant` automatically use heuristics and set `backend: "heuristic"` plus `fallback_reason` in the result.

## Local small language model

Follow [Quick start §2](#2-optional-smarter-summaries-with-ollama) for Ollama.

Rules of thumb:

- **Only loopback** URLs (`127.0.0.1`, `::1`, `localhost`) — no cloud endpoints.
- Without `COMPENDIUM_LOCAL_LLM_URL`, smart actions use heuristics and set `backend: "heuristic"`.
- Calls use `temperature=0` and `seed=0` for stable outputs.
- Lemonade example: `COMPENDIUM_LOCAL_LLM_URL=http://127.0.0.1:13305/api/v1` and `COMPENDIUM_LOCAL_LLM_MODEL=Qwen3-4B-GGUF`.
- llama.cpp OpenAI server: same pattern — set URL to its `/v1` base and the served model id.

## Develop / test

```bash
cargo test
cargo test --features real-tokens
cargo test --features http --test http_smoke
cargo test --test e2e_smoke
cargo run --features http -- http 127.0.0.1:8788
```

`e2e_smoke` spawns `CARGO_BIN_EXE_compendium`, completes the MCP initialize handshake over stdio, lists tools, then calls gateway actions. `http_smoke` (requires `--features http`) exercises streamable HTTP in-process.

## Design notes

- **Deterministic by default** — heuristic pipeline needs no network; smart actions only call a configured **local** OpenAI-compatible URL and fall back to heuristics when unset or failing.
- **Token backends** — fast heuristic by default; opt into exact BPE with `real-tokens`.
- **Zero stdout pollution** (stdio mode) — tracing goes to stderr so JSON-RPC framing stays clean.
- **Release profile** — LTO + stripped binary for low footprint.

## License

MIT
