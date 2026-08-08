---
id: brief-then-rerank
name: Brief then rerank
description: Pack a workspace briefing, then BM25/hybrid-rank the evidence chunks
tags: brief, rerank, workspace, retrieval
---

# Brief → rerank playbook

1. Call `action=brief` with a concrete `query` (and optional `brief.root`).
2. Keep the returned `cache_key` / Sources paths; do not paste the whole corpus.
3. If you need ordered snippets, `action=chunk` the Evidence block (or file contents), then `action=rerank` with the same query (`use_embeddings` when a loopback SLM is set).
4. Optionally enable `rerank.use_cross_encoder` — see playbook `hybrid-rerank`.
5. Open only top hits via `resolve` / file paths listed under Read next.
