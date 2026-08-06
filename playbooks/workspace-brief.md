---
id: workspace-brief
name: Workspace starter brief
description: Pack a structured Task/Status/Evidence briefing for a fresh agent turn
tags: brief, workspace, onboarding
---

# Workspace brief playbook

1. Call `action=brief` with a concrete `query` (the sub-task) and optional `brief.root`.
2. Use the returned `briefing` (or `cache_key`) as starter context — do not re-scan the whole tree by hand.
3. Open **Read next** paths; load playbooks via `cmp://skill/playbook/…` or `action=playbook`.
4. For deep dives, `chunk` large sources and `resolve` only needed ids.
