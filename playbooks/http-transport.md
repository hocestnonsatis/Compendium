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
5. Prefer stdio for Cursor Desktop; use HTTP for remote agents that must stay on loopback or a trusted tunnel.
