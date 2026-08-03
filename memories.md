# Compendium — session memories

## Proje Durumu
- Rust MCP sunucusu (rmcp 3.x).
- Transport: stdio (varsayılan) + streamable HTTP/SSE (`--features http`).
- Token: heuristic / `--features real-tokens` (tiktoken-rs).
- npm dağıtım: `compendium-mcp` + optional platform paketleri + GitHub Releases fallback.
- CI: `.github/workflows/release.yml` (cross-compile + npm publish).
- Tek MCP tool: `compendium` gateway (`action` enum ile dispatch).
- Cursor MCP: `target/release/compendium`, `COMPENDIUM_TOOLSET=gateway-v1`.
- Cursor rule: `.cursor/rules/compendium.mdc` (alwaysApply).
- `bin/run.js`: local `target/` optional platform paketinden önce; stderr’e binary yolu.
- Güncelleme: 2026-08-03 — 12 ayrı tool → tek gateway.

## Tercihler
- Dil: Rust; ana dalda çalış.
- npm paket adı: `compendium-mcp` (`compendium` npm’de dolu); bin: `compendium`.
- `npx -y compendium-mcp` ile Cursor/Claude.

## Mimari Kararlar
- Dağıtım: esbuild-style optionalDependencies + lazy GitHub download fallback.
- Release binary: `--features real-tokens,http`.
- Loglar stderr (stdio MCP).
- Session state: `Arc<Mutex<CacheStore + SessionStats>>`; chunk otomatik cache’lenir.
- Tool UX: tek gateway + `action`; eski `compendium_*` isimleri yok.

## Rakip MCP Araç Haritası (2026-08-03)
Üç kamp: (A) tool-schema proxy, (B) içerik sıkıştırma/filtre, (C) cache+memory+smart wrappers.
- A: Atlassian mcp-compressor / mcp-compress-router → `get_tool_schema`, `invoke_tool`
- B: mcp-sophon, tokensaver — Compendium bu kampta (gateway yüzeyi)
- C: token-optimizer, context-compress, context-mem (bilinçli olarak kopyalanmadı)

## Sıradaki Adımlar
- GitHub remote + `NPM_TOKEN` secret; `OWNER/Compendium` placeholder’ı gerçek repo ile değiştir.
- İlk `v0.1.0` release oluştur.
- İsteğe bağlı: HTTP e2e smoke.
