---
id: install-npx
name: First install / npx
description: Get a working Compendium binary via npm or a local Cargo build when Releases lag
tags: install, npm, npx, distribution
---

# Install / npx playbook

1. Cursor MCP: `npx -y compendium-mcp` (Node ≥18). Package name is **`compendium-mcp`**.
2. If spawn fails: check optionalDeps for your platform (`compendium-mcp-<os>-<arch>`), then GitHub Releases asset for the same tag as `package.json`.
3. Brand-new platforms (`linux-x64-musl`, `win32-arm64`) need npm Trusted Publisher or one interactive publish — see `npm/DISTRIBUTION.md`.
4. Override binary: `COMPENDIUM_BINARY=/path/to/compendium` or `COMPENDIUM_PLATFORM=linux-x64-musl`.
5. Developers: `cargo build --release --features real-tokens,http` and point MCP at `./target/release/compendium`, then reload MCP after schema changes.
6. Optional local SLM: `npx -y compendium-mcp setup-ollama --write-mcp` (playbook `setup-ollama`).
