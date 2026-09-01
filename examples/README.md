# Examples

Sample JSON argument objects for the MCP tool `compendium`.

Each file is a single JSON object: set `action` and the fields that action needs.
In Cursor (or any MCP client), call the tool with the file contents as arguments.

| File | Action | Purpose |
|------|--------|---------|
| [`help.json`](help.json) | `help` | List every action (add `"id"` for one action's example) |
| [`filter-log.json`](filter-log.json) | `filter` | Strip ANSI + keep error/info lines |
| [`compress-output-cargo.json`](compress-output-cargo.json) | `compress_output` | Domain scrub for cargo/test noise |
| [`sanitize-untrusted.json`](sanitize-untrusted.json) | `sanitize` | Redact secrets + neutralize prompt injection |
| [`llm-status.json`](llm-status.json) | `llm_status` | Why smart actions fell back to heuristics |

Responses look like `{ "ok", "action", "result_json" }`. Parse `result_json` as JSON.
