---
id: long-chat-afm
name: Long chat / AFM prune
description: Tier chat history with Adaptive Focus Memory before continuing
tags: chat, prune, afm, memory
---

# Long chat AFM playbook

1. Prefer structured `messages` over a flat transcript when calling `prune_history`.
2. Use `prune.strategy=afm` with a sensible `keep_last_n` (default 4).
3. Distant-tier blobs are parked under `cmp://afm/…` — retrieve with `cache_get` only if needed.
4. After prune, continue the task with Critical + Thematic tiers only.
