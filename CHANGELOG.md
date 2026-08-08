# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-08-08

### Added

- Cross-encoder hardening: prefer Cohere-style `POST /v1/rerank` when available (`cross_encoder_mode: rerank_api`); pairwise chat fallback; per-pair parse failures keep prior BM25/hybrid scores (`backend: cross_encoder_partial`).
- `RerankResult.cross_encoder_ms` / `cross_encoder_mode` for telemetry (`stats.by_backend` already counts backends).
- Playbook `hybrid-rerank` (`cmp://skill/playbook/hybrid-rerank`).

## [0.3.1] - 2026-08-08

### Added

- Platform packages / release assets: `linux-x64-musl` (`x86_64-unknown-linux-musl` via cross) and `win32-arm64` (`windows-11-arm` runner).
- `COMPENDIUM_PLATFORM` override; musl auto-detect for Linux x64 in `npm/lib/platform.js`.

## [0.3.0] - 2026-08-08

### Added

- Opt-in SLM cross-encoder rescore for `rerank`: `rerank.use_cross_encoder` / `COMPENDIUM_RERANK_CROSS_ENCODER`, `cross_encoder_top_n` / `COMPENDIUM_CROSS_ENCODER_TOP_N` (default 16). Backend `cross_encoder`; hits expose `cross_encoder_score`. Falls back to BM25/hybrid with `fallback_reason` when the local LLM is unset or fails.
- Example payload `examples/rerank-cross-encoder.json`.

## [0.2.1] - 2026-08-08

### Changed

- Docs aligned with v0.2.0: REPORT banner/roadmap (audit + hybrid/cache/`llm_status` marked shipped), SECURITY support table (`0.2.x`), architecture notes for hybrid rerank and audit path.
- README example tool calls use single `compendium` tool + `action` (removed legacy `compendium_*` names).

## [0.2.0] - 2026-08-07

### Added

- GitHub Actions PR CI (`fmt`, `clippy`, `cargo test`, HTTP smoke).
- `examples/` sample MCP tool-call JSON payloads.
- `docs/architecture.md` gateway overview.
- `REPORT.md` roadmap rewritten as Shipped / Next / Deferred.
- Optional persistent session cache via `COMPENDIUM_CACHE_DIR` (+ `COMPENDIUM_CACHE_MAX_BYTES`); TTL eviction; `stats` cache hit/miss/disk/eviction fields.
- Hybrid `rerank` / `brief`: loopback OpenAI-compatible embeddings blended with BM25 (`use_embeddings`, `alpha` / `COMPENDIUM_HYBRID_ALPHA`, `COMPENDIUM_LOCAL_EMBED_MODEL`).
- `action=llm_status` — probe local LLM configuration and `/models` reachability (`force` = chat ping).
- Opt-in JSONL audit log via `COMPENDIUM_AUDIT_PATH` (action metadata only; no request bodies).

## [0.1.3] - 2026-08-07

### Added

- Progressive disclosure: `catalog` / `help`, MCP resources `cmp://skill/…`, playbooks, `pack` / `unpack`.
- Brand icons (SEP-973) on `serverInfo` and the `compendium` tool via data URIs.
- Sanitize poison-parameter strip; compressed/full help; skill resource etag + TTL.
- Stats: compression ratio, lazy-hit, resolve latency counters.

### Distribution

- Release workflow builds `real-tokens,http` natives and publishes `compendium-mcp` + platform optionalDeps via npm OIDC.

## [0.1.2] - 2026-08-06

### Added

- Session `stats`, sanitize (secrets / IPI), BM25 `rerank`, workspace `brief`.
- AFM strategy for `prune_history`.

## [0.1.1] - 2026-08-03

### Added

- Smart actions (`summarize_smart`, `filter_relevant`) with loopback OpenAI-compatible SLM and heuristic fallback.
- npm hybrid distribution (optionalDeps + GitHub Releases cache).

## [0.1.0] - 2026-08-01

### Added

- Initial MCP gateway: single `compendium` tool with filter / compress / summarize / chunk / cache / prune / compress_output.
- stdio transport; optional streamable HTTP (`--features http`).
