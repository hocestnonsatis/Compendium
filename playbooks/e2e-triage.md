---
id: e2e-triage
name: E2E / MCP smoke triage
description: Diagnose failing MCP e2e or gateway smoke tests with minimal context
tags: e2e, mcp, tests
---

# E2E triage playbook

1. Capture failing test stdout → `compress_output` (domain `cargo` or `generic`).
2. `brief` with query describing the failure against the repo root.
3. Follow **Read next** paths / `cmp://skill/…` URIs before dumping more files.
4. Re-run the single failing test; if still opaque, `chunk` the log and `resolve` only top BM25 hits via `rerank`.
