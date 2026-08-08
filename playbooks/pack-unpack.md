---
id: pack-unpack
name: Pack and unpack archives
description: Bundle multi-file text into a size-capped zip, then expand to chunks without running scripts
tags: pack, unpack, archive, cache
---

# Pack / unpack playbook

1. Prefer `action=pack` with `items` (path + text) or multi-file `text` — not `cache_store` (single blob).
2. Keep `pack.store_in_cache: true` (default) and retain the returned key; set `include_base64` only when you must pass bytes inline.
3. Respect caps (`COMPENDIUM_ARCHIVE_MAX_*` / `pack` options). Archives never execute scripts.
4. Later: `action=unpack` with `key` (or base64 `text`) → chunk map; `resolve` only the ids you need.
5. Untrusted archive source → `sanitize_input: true` on unpack or sanitize after resolve.
