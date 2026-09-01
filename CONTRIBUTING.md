# Contributing to Compendium

Thanks for helping improve Compendium — an MCP server that compresses, summarizes, and filters context to cut LLM token usage.

## Development setup

**Requirements:** Rust stable (edition 2021), Node.js ≥18 (npm launcher).

```bash
git clone https://github.com/hocestnonsatis/Compendium.git
cd Compendium
cargo build --release
cargo test
```

Run the server locally without npm:

```bash
./target/release/compendium
# or through the npm launcher (picks up target/release automatically)
node bin/run.js
```

CI runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, the eval regression suite, and a Cargo.toml/package.json version check on every push/PR to `master`:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --features real-tokens -- -D warnings
cargo test
COMPENDIUM_EVAL_LATENCY_MS=5000 cargo test --test eval_regression
npm run check-version
```

Optional local model for the smart actions:

```bash
./target/release/compendium setup-ollama --write-mcp
# or manually
export COMPENDIUM_LOCAL_LLM_URL=http://127.0.0.1:11434/v1
export COMPENDIUM_LOCAL_LLM_MODEL=qwen2.5:3b
```

## How we work

- Small, focused changes on `master`.
- Match existing Rust style; keep changes scoped to the request.
- Add or adjust tests when you change pipeline or server behavior.
- Heuristic actions must stay deterministic; smart actions must report `backend`.
- Logs go to **stderr** only — stdout is MCP JSON-RPC.
- Never commit secrets or local MCP credentials.

## Releasing

1. Bump `version` in `Cargo.toml` and `package.json` (must match — `npm run check-version`).
2. Update `CHANGELOG.md`.
3. Commit, tag `vX.Y.Z`, push, and publish a GitHub Release for the tag.
4. `release.yml` builds the binaries, attaches them to the Release, and publishes `compendium-mcp` to npm via OIDC Trusted Publishing.

## Pull requests

1. Say **why** the change exists.
2. Note how you tested (`cargo test`, e2e smoke, manual MCP call).
3. Update `README.md` only when user-facing behavior changes.

## Issues

Use the issue templates. Include Compendium version, OS/arch, and a minimal repro.

## Code of conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

Do **not** open public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).
