# Compendium

<p align="center">
  <img src="assets/logo.svg" alt="Compendium" width="420" />
</p>

MCP server that **shrinks noisy context before it reaches the model**: filter logs, compress bulky text/JSON, redact secrets, chunk huge files, and keep only what a question needs.

One tool (`compendium`), one `action` field, deterministic heuristics that need **no model**. An optional local Ollama model improves the two "smart" actions.

## Install (Cursor / Claude Desktop)

Requires Node.js 18+. No Rust toolchain needed — the launcher downloads a prebuilt binary once.

Add to `~/.cursor/mcp.json` (or the project's `.cursor/mcp.json`, or Claude Desktop's MCP config):

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

Reload MCP. You should see a single tool named `compendium`. That's it.

Prebuilt binaries: macOS (arm64, x64), Linux (x64, arm64), Windows (x64). Other platforms: `cargo build --release` and set `COMPENDIUM_BINARY=/path/to/compendium` in the server `env`.

### Optional: smarter summaries with Ollama

`summarize_smart` and `filter_relevant` use a small local model when one is configured; otherwise they fall back to heuristics. One command sets it up:

```bash
npx -y compendium-mcp setup-ollama --write-mcp
```

It detects (or `--install`s) [Ollama](https://ollama.com/), pulls `qwen2.5:3b`, probes `http://127.0.0.1:11434/v1`, and writes `COMPENDIUM_LOCAL_LLM_URL` / `COMPENDIUM_LOCAL_LLM_MODEL` into `~/.cursor/mcp.json` (`--project` for `.cursor/mcp.json`). Reload MCP and call `{"action":"llm_status"}` — `reachable: true` means smart actions will report `backend: "local_llm"`.

Any OpenAI-compatible **loopback** server works (llama.cpp, Lemonade, …): set the two env vars yourself. Remote hosts are rejected on purpose.

## Actions

| `action` | Use it for | Main fields |
|----------|------------|-------------|
| `filter` | Strip ANSI/boilerplate, collapse whitespace, keep/drop regex lines | `text`, `filter?`, `query?` |
| `compress_output` | Domain-aware scrub of cargo / npm / docker / git / kubectl output | `text`, `output?` |
| `compress` | Dense rewrite of bulky text, code, or JSON | `text`, `compress?` |
| `summarize` | Outline / conversation / file-tree summary | `text`, `summarize?` |
| `summarize_smart` | Local-model summary (heuristic fallback) | `text`, `smart?` |
| `filter_relevant` | Keep only lines relevant to a question (local model or BM25) | `text`, `query` |
| `sanitize` | Redact secrets, neutralize prompt-injection phrases | `text`, `sanitize?` |
| `rerank` | BM25-rank candidates for a query | `query`, `items` / `text` / chunk `map` |
| `chunk` / `resolve` | Split a huge corpus into `cmp://` chunks; fetch one by id | `text`, `chunk?` / `id` |
| `cache_store` / `cache_get` / `cache_invalidate` | Park a blob outside the prompt under a short key | `text`, `cache?` / `key` |
| `count_tokens` | Measure size | `text` |
| `stats` | Session savings per action | `reset?` |
| `llm_status` | Is the local model configured and reachable? | `force?` |
| `help` | List actions, or fields + example for one action | `id?` |

Every response is `{ "ok": true, "action": "filter", "result_json": "{…}" }` — parse `result_json` for the action payload. Add `"sanitize_input": true` to any text action to scrub secrets first. `compress` / `summarize` leave inputs under 1000 characters untouched unless `force: true`.

Example:

```json
{
  "action": "filter",
  "text": "…noisy log…",
  "filter": { "strip_ansi": true, "keep_patterns": ["ERROR|WARN"] }
}
```

More samples in [`examples/`](examples/). Unsure which action fits? Call `{"action":"help"}`.

## Environment

All optional.

| Variable | Default | Meaning |
|----------|---------|---------|
| `COMPENDIUM_LOCAL_LLM_URL` | _(unset)_ | OpenAI-compatible loopback base URL; enables smart actions |
| `COMPENDIUM_LOCAL_LLM_MODEL` | `qwen2.5:3b` | Model id on that server |
| `COMPENDIUM_LOCAL_LLM_API_KEY` | _(unset)_ | Bearer token for locked loopback servers |
| `COMPENDIUM_LOCAL_LLM_TIMEOUT_SECS` | `120` | HTTP timeout (first model load can be slow) |
| `COMPENDIUM_LOCAL_LLM_REASONING_EFFORT` | `low` | `low`/`medium`/`high`/`off`; `omit` drops the field for strict servers |
| `COMPENDIUM_DEFAULT_MAX_TOKENS` | `2048` | Soft output cap for compress/summarize |
| `COMPENDIUM_SIGNAL_MIN_CHARS` | `1000` | Bypass compress/summarize below this length (`0` disables) |
| `COMPENDIUM_CHARS_PER_TOKEN` | `4.0` | Heuristic token estimate |
| `COMPENDIUM_TOKENIZER` | `cl100k_base` | BPE encoding when built with `--features real-tokens` |
| `COMPENDIUM_BINARY` | _(unset)_ | npm launcher: use this binary instead of downloading |
| `RUST_LOG` | `compendium=info` | Logs go to stderr only |

## Build from source

```bash
cargo build --release                        # heuristic token counts
cargo build --release --features real-tokens # exact BPE counts via tiktoken
cargo test
```

Point your MCP config at `target/release/compendium` directly, or keep `npx` and set `COMPENDIUM_BINARY`.

## Design

- **Deterministic by default.** Heuristic paths need no network and produce byte-identical output, which keeps prompt prefix caches warm.
- **Local-first.** Smart actions only ever call a loopback URL; there is no cloud path.
- **stdout is JSON-RPC.** All logging goes to stderr.

## Community

[Contributing](CONTRIBUTING.md) · [Changelog](CHANGELOG.md) · [Architecture](docs/architecture.md) · [Security](SECURITY.md) · [Support](SUPPORT.md) · [Code of Conduct](CODE_OF_CONDUCT.md)

## License

MIT
