---
id: cargo-fail
name: Cargo / test failure triage
description: Shrink cargo/rustc/test output and surface the failing assertion
tags: cargo, rustc, tests, compress_output
---

# Cargo fail playbook

1. `action=compress_output` with `output.domain=cargo` (or `rustc` / `pytest`).
2. `action=filter_relevant` with query like `error failed panic assertion`.
3. Optionally `action=rerank` on chunked sections if the log was `chunk`ed first.
4. Paste only the compressed/relevant slice into the model — not the full cargo dump.
