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
- Güncelleme: 2026-08-03 — local SLM: `summarize_smart` + `filter_relevant` (OpenAI-compatible loopback; heuristic fallback).
- Güncelleme: 2026-08-03 — NotebookLM araştırması `REPORT.md` tamamlandı; sprint planı memories’de.
- Güncelleme: 2026-08-03 — Sprint 1 tamamlandı: loopback/SSRF, sanitize, signal-to-call, SLM temp=0/seed.
- Güncelleme: 2026-08-03 — Sprint 2 tamamlandı: AFM prune, BM25 filter/filter_relevant, action=rerank.
- Güncelleme: 2026-08-03 — Sprint 3 tamamlandı: stats telemetry, OWNER hizalama, HTTP smoke; release NPM_TOKEN bekliyor.

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
- Local SLM: `COMPENDIUM_LOCAL_LLM_URL` + model/api_key/timeout; reqwest blocking + `block_in_place`; cloud yok, sadece loopback; `fallback: true` varsayılan.
- Candle in-process LLM: değerlendirildi, vazgeçildi (binary/CI/GPU matrisi); HTTP (Ollama/Lemonade/llama.cpp) + heuristic fallback kalır.
- Güncelleme: 2026-08-03 — NotebookLM `REPORT.md` kararları (aşağıda).

### NotebookLM REPORT.md → Kararlar (2026-08-03)
| Öneri | Karar | Not |
|---|---|---|
| Single gateway + action enum | **Kabul / yapıldı** | Araştırma +5pp seçim doğruluğu; metadata tax azaltır |
| Heuristic-first, p99 ≤ 87ms | **Kabul** | SLM yalnızca smart path; varsayılan deterministik |
| BYO OpenAI-compat loopback | **Kabul** | Ollama zorunlu değil; llama.cpp/Lemonade tercih edilebilir |
| llama-cpp-sys / in-process LLM | **Red** | Candle ile aynı dağıtım riski; binary_weight_manager yok |
| Candle / cloud embeddings | **Red** | Rapor da uyarıyor |
| AFM tiered prune (Critical/Thematic/Distant) | **Kabul** | Mevcut `prune_history` üzerine |
| BM25 query-aware filter | **Kabul** | `filter` / `filter_relevant` heuristic güçlendirme |
| Signal-to-call (<1000 char bypass) | **Kabul** | Küçük payload’da inflation önleme |
| Loopback + SSRF guard | **Kabul / yapıldı** | Sprint 1: URL allowlist |
| Sanitization / IPI hooks | **Kabul** | Secret regex + outbound audit |
| Deterministic SLM (byte-identical) | **Kabul** | temp→0, seed; prefix cache koruma |
| rerank_chunks | **Kabul (Sprint 2)** | Önce lexical/BM25; SLM opsiyonel |
| hybrid_search_bridge | **Adapt** | Ayrı tool değil; BM25 + chunk/resolve içine |
| QuickTok telemetry | **Adapt** | Mevcut `stats` + `real-tokens` (tiktoken) yeterli |
| TurboQuant / Hadamard | **Ertele** | In-process inference yokken anlamsız |

## Rakip MCP Araç Haritası (2026-08-03)
Üç kamp: (A) tool-schema proxy, (B) içerik sıkıştırma/filtre, (C) cache+memory+smart wrappers.
- A: Atlassian mcp-compressor / mcp-compress-router → `get_tool_schema`, `invoke_tool`
- B: mcp-sophon, tokensaver — Compendium bu kampta (gateway yüzeyi)
- C: token-optimizer, context-compress, context-mem (bilinçli olarak kopyalanmadı)
- Rapor karşılaştırması: Compendium = B kampı + opsiyonel local SLM (TokenSaver benzeri, yerel)

## Sprint Planı (REPORT.md, 2026-08-03)
### Sprint 1 — Güvenlik + deterministik kalite ✅
1. Loopback/SSRF enforce: `LocalLlmClient` yalnızca `127.0.0.1` / `::1` / `localhost`.
2. `action=sanitize` + `sanitize_input` hook; LLM çıktılarında secret scrub.
3. Signal-to-call: `compress`/`summarize`/`summarize_smart` için `<signal_min_chars` bypass (`force` ile override); env `COMPENDIUM_SIGNAL_MIN_CHARS`.
4. SLM determinism: temperature `0`, seed `0`; result’ta `deterministic: true`.

### Sprint 2 — Retrieval / AFM ✅
5. AFM `prune_history`: `strategy=afm` → Critical / Thematic / Distant; distant `cmp://afm/…` session cache.
6. BM25: `filter` (+`query`) ve `filter_relevant` heuristic; teknik ID/versiyon/status koruma.
7. `action=rerank`: BM25 skorlu sıra (`items` / `text` / chunk `map`).

### Sprint 3 — Telemetri + dağıtım ✅ (release hariç)
8. Stats: `p50_latency_ms` / `p99_latency_ms`, `bypass_calls` / `bypass_ratio`, `token_backend`, `by_backend`.
9. npm/docs: `hocestnonsatis/Compendium` (`package.json`, `platform.js`, `DISTRIBUTION.md`).
10. HTTP e2e: `cargo test --features http --test http_smoke`.
- `v0.1.0` GitHub Release + npm publish: **`NPM_TOKEN` secret + commit/tag** gerekir (henüz yok / yapılmadı).

## Sıradaki Adımlar
- Commit + `v0.1.0` tag/release (önce `NPM_TOKEN` secret ekle).
- İsteğe bağlı: SLM cross-encoder rerank.

## Repo
- GitHub: `hocestnonsatis/Compendium` (private), default branch `master`.
