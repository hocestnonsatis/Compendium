---
id: http-transport
name: HTTP / SSE transport
description: Run Compendium over streamable HTTP instead of stdio
tags: http, sse, transport
---

# HTTP transport playbook

1. Build with `--features http` (release binaries from npm already include http).
2. Start: `compendium http` or `compendium http 127.0.0.1:8788` (`COMPENDIUM_HTTP_BIND` overrides).
3. Client endpoint: `http://{bind}/mcp` (loopback by default — keep it local).
4. Smoke: point an MCP streamable-HTTP client at that URL and call `action=count_tokens`.
5. Prefer **stdio** for Cursor Desktop (dual-compat with legacy initialize and newer clients). Use HTTP for agents that speak Streamable HTTP on loopback or a trusted tunnel.

## Protocol notes (`2026-07-28` and dual-compat)

- HTTP is **sessionless**: no sticky `Mcp-Session-Id`, `NeverSessionManager`, `legacy_session_mode(false)`.
- Responses prefer a single `application/json` request/response; SSE only when the client needs it.
- rmcp advertises known protocol versions including MCP **`2026-07-28`** (stateless core). Do not assume every host client has upgraded — “supports MCP” is version-sensitive.
- **App cache ≠ MCP session.** `chunk` / `cache_*` / `stats` keys are application state. Across HTTP requests (or process restarts), set `COMPENDIUM_CACHE_DIR` so keys survive; without it, in-process state is lost when the handler instance ends.
