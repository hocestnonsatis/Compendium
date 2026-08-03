# Contributing to Compendium

Thanks for helping improve Compendium — an MCP server that compresses, summarizes, and filters context to cut LLM token usage.

## Development setup

**Requirements:** Rust stable (edition 2021), Node.js ≥18 (npm wrapper / distribution).

```bash
git clone https://github.com/hocestnonsatis/Compendium.git
cd Compendium
cargo build --release --features real-tokens,http
cargo test
cargo test --features http --test http_smoke
```

Local MCP without npm:

```bash
./target/release/compendium
# or
COMPENDIUM_BINARY=./target/release/compendium node bin/run.js
```

Optional local SLM (Ollama / any OpenAI-compatible loopback server):

```bash
export COMPENDIUM_LOCAL_LLM_URL=http://127.0.0.1:11434/v1
export COMPENDIUM_LOCAL_LLM_MODEL=qwen:latest
```

npm packaging notes: [npm/DISTRIBUTION.md](npm/DISTRIBUTION.md).

## How we work

- Prefer small, focused PRs on `master` (or a short-lived branch).
- Match existing Rust style; keep changes scoped to the request.
- Add/adjust tests for pipeline and server behavior when you change logic.
- Do not commit secrets, local MCP credentials, or native binaries under `npm/platforms/*/bin/`.
- Logs for the MCP stdio server must go to **stderr** only.

## Pull requests

1. Describe **why** the change exists (not only what files moved).
2. Note how you tested (`cargo test`, e2e smoke, manual MCP call).
3. Update `README.md` only when user-facing behavior changes.
4. For releases: versions stay aligned across `Cargo.toml`, root `package.json`, and `npm/platforms/*/package.json`. CI publishes via Trusted Publishing (OIDC) on GitHub Release tags.

## Issues

Use the issue templates for bugs and feature requests. Include Compendium version (`compendium-mcp` / git SHA), OS/arch, and a minimal repro when reporting bugs.

## Code of conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

Please do **not** open public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).
