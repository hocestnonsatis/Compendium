---
id: setup-ollama
name: Enable Ollama (smart / hybrid)
description: After Compendium MCP is added, one CLI command installs/pulls Ollama models and writes COMPENDIUM_LOCAL_LLM_* 
tags: ollama, local-llm, install, dx
---

# Setup Ollama playbook

Heuristics work with **no** local model. Use this only when you want `summarize_smart` / `filter_relevant` / hybrid `rerank` on a loopback SLM.

1. Compendium MCP must already be on the client (`npx -y compendium-mcp` or a local binary). See playbook `install-npx`.
2. In a **terminal** (not inside the MCP stdio session), run:

```bash
npx -y compendium-mcp setup-ollama --write-mcp
```

   - Project `.cursor/mcp.json`: add `--project` (or `--write-mcp .cursor/mcp.json`).
   - Ollama missing: add `--install` (official Linux/macOS script, or `winget` on Windows).
   - Dry plan: `--dry-run --json`.
   - Local Cargo binary: `./target/release/compendium setup-ollama --write-mcp`.

3. Defaults: chat `qwen2.5:3b`, embed `nomic-embed-text`, URL `http://127.0.0.1:11434/v1` (loopback only). Override with `--chat-model` / `--embed-model` / `--url`.
4. Reload MCP. Call `action=llm_status`. Expect `configured: true` and `reachable: true`.
5. If `reachable: false`, read `fallback_reason`; re-run `setup-ollama --no-pull` or `ollama list`.
6. Then `summarize_smart` / `filter_relevant` should report `backend: "local_llm"`; `rerank` may report `hybrid`.
