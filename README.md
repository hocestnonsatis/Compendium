# Compendium

MCP server that **minimizes LLM token usage** by compressing, summarizing, filtering, and chunk-referencing large context before it reaches the model.

Built in Rust with the official [`rmcp`](https://crates.io/crates/rmcp) SDK.

## Install via npm / npx

Package name: **`compendium-mcp`** (the npm name `compendium` is taken). CLI bin: `compendium`.

```bash
npx -y compendium-mcp --help
# or: npm install -g compendium-mcp
```

**Cursor / Claude Desktop**

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

Distribution uses **optional platform packages** (`compendium-mcp-darwin-arm64`, …) with a **GitHub Releases download fallback**. See [npm/DISTRIBUTION.md](npm/DISTRIBUTION.md) for local testing, CI release, and publishing.

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
| `prune_history` | Drop filler / compress older chat turns | `text` or `messages`, `prune` |
| `chunk` | Split into `cmp://` chunks (session-cached) | `text`, `chunk` |
| `resolve` | Fetch chunk content by id | `id` (+ optional `map` / `text`) |
| `count_tokens` | Measure tokens | `text` |
| `stats` | Session savings report | `reset?` |
| `cache_store` | Park bulky payload outside the prompt | `text`, `cache` |
| `cache_get` | Retrieve by key | `key` |
| `cache_invalidate` | Drop one key or clear cache | `key?` |

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

## Configure (Cursor / Claude Desktop)

### Cursor (`~/.cursor/mcp.json` or project `.cursor/mcp.json`)

```json
{
  "mcpServers": {
    "compendium": {
      "command": "npx",
      "args": ["-y", "compendium-mcp"],
      "env": {
        "RUST_LOG": "compendium=info",
        "COMPENDIUM_DEFAULT_MAX_TOKENS": "2048",
        "COMPENDIUM_TOKENIZER": "cl100k_base"
      }
    }
  }
}
```

Or point at a local release binary:

```json
{
  "mcpServers": {
    "compendium": {
      "command": "/absolute/path/to/Compendium/target/release/compendium",
      "env": {
        "RUST_LOG": "compendium=info",
        "COMPENDIUM_DEFAULT_MAX_TOKENS": "2048",
        "COMPENDIUM_CHARS_PER_TOKEN": "4.0",
        "COMPENDIUM_TOKENIZER": "cl100k_base"
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

## Develop / test

```bash
cargo test
cargo test --features real-tokens
cargo test --test e2e_smoke
cargo run --features http -- http 127.0.0.1:8788
```

`e2e_smoke` spawns `CARGO_BIN_EXE_compendium`, completes the MCP initialize handshake over stdio, lists tools, then calls filter → compress → summarize → chunk.

## Design notes

- **Deterministic pipeline** — no outbound LLM calls; safe for offline / air-gapped agents.
- **Token backends** — fast heuristic by default; opt into exact BPE with `real-tokens`.
- **Zero stdout pollution** (stdio mode) — tracing goes to stderr so JSON-RPC framing stays clean.
- **Release profile** — LTO + stripped binary for low footprint.

## License

MIT
