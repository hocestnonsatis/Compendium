---
id: noisy-logs
name: Noisy logs
description: Strip ANSI/boilerplate and keep error-relevant lines from terminal dumps
tags: logs, filter, compress_output
---

# Noisy logs playbook

1. If the dump is from git/cargo/npm/docker/kubectl, call `action=compress_output` with matching `output.domain` (or `auto`).
2. Otherwise `action=filter` with `strip_ansi`, `strip_boilerplate`, `collapse_whitespace`.
3. If you already know the question, add `query` (filter) or use `action=filter_relevant`.
4. Still huge? `action=compress` then `cache_store` — keep only the key.
