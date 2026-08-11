Architectural Blueprint for Compendium: High-Performance Local Context Optimization via MCP

> **Status (v0.6.0):** Sections 1–5 are design background. Section 6 (security) and section 7 (roadmap) are the live product truth — see [CHANGELOG.md](CHANGELOG.md) and [docs/architecture.md](docs/architecture.md). Compendium is a **context gateway** (single `compendium` tool), not a foreign-MCP schema proxy. B-roadmap shipped in 0.5.0; **C-roadmap** (eval depth / DX / retrieval polish / ops) shipped in 0.6.0; deferred items below remain out of scope.

1. Executive Summary: The Context Bloat Crisis and Local Mitigation Strategies

In production Model Context Protocol (MCP) environments, a systemic architectural bottleneck has emerged: "metadata token bloat." When an agent registers enterprise-grade MCP servers, the protocol mandates the upfront exposure of complete structural definitions, including nested JSON schemas and extensive parameter lists. Research indicates that the official Atlassian MCP server consumes approximately 10,000 tokens per request solely to advertise its interfaces, while the GitHub implementation, exposing 94 tools, imposes an immediate overhead of 17,600 tokens before a user prompt is processed.

To maintain operational efficiency, the architecture dictates a "Local-First" approach for the Compendium project, guided by the following 12 core findings:

* Upfront Token Tax: Combined enterprise MCP servers can exceed 30,000 tokens in initial schema registration, severely diluting model attention.
* Compression Utility: Proxy architectures reduce this overhead by 70–97% via dynamic schema expansion.
* Two-Tool Paradigm: Adopting a get_tool_schema and invoke_tool model reduces the registration footprint to as little as 500 tokens.
* Heuristic Efficiency: Deterministic heuristic compressors (e.g., regex/lexical tokenizers) operate with sub-millisecond overhead and zero GPU requirement.
* Neural Tradeoff: Small Language Models (SLMs) provide superior semantic synthesis but introduce multi-second latency and higher operational costs.
* Prompt Prefill Optimization: Token saving directly translates to reduced API costs and significantly faster real-time responsiveness.
* Deterministic Stability: For effective prefix caching, summarizers must be byte-identical; non-deterministic output destroys cache reuse.
* Local Latency Budgets: To maintain a fluid terminal-based UX, heuristic pipeline components must target a p99 latency of ≤ 87ms.
* Local Inference Speeds: Quantized local models can achieve over 300 tokens per second, bypassing cloud-based rate limits and KV cache quadratic growth.
* Tiered Fidelity: Compendium shall implement Adaptive Focus Memory (AFM) to manage context via Critical (Full), Thematic (Compressed), and Distant (Placeholder) tiers.
* Security Necessity: Local execution environments are high-value targets; configurations must be restricted to loopback-only (127.0.0.1).
* Auditability & IPI Protection: Every tool call requires mandatory sanitization hooks to mitigate Indirect Prompt Injection (IPI) and sensitive data exfiltration.

2. Comparative Landscape: Local Token Optimization and SLM Projects

The strategic selection of proxy architecture is the primary lever for minimizing prompt prefill cycles and API costs. A "Single Gateway" approach allows the agent to discover tools on-demand, preventing the "attention dilution" and high prefill costs associated with exposing 90+ tools simultaneously.

The following table evaluates current industry solutions for context and token optimization:

Project	Primary Mechanism	Local SLM Integration	Tool Surface Strategy	Distribution Format
mcp-compressor (Atlassian)	Deterministic (Heuristic)	None (Proxy-based)	Single Gateway (2 Tools)	Python / TypeScript
mcp-sophon (Rust/QuickTok)	Deterministic (QuickTok/BPE)	None (High-speed)	Compressed Multi-tool	Rust-backed Binary
token-optimizer-mcp	Deterministic (Hashing)	None (Local Graph)	Full Catalog	TypeScript
context-compress	Syntactic (Structural)	None (Rule-based)	Manual Application	Markdown / CLI
TokenSaver	Neural (Semantic)	Integrated SLM	API Gateway Proxy	Enterprise Platform

Compendium dictates that the "Action Set" (summarize, filter, rerank) constitutes the minimum viable capability required for a modern agentic gateway.

3. Heuristic Pipeline vs. Neural SLM: The Latency-Accuracy Tradeoff

The choice between deterministic heuristics and Small Language Models (SLMs) is the primary driver of User Experience (UX) in terminal-based agents. Heuristic pipelines offer immediate feedback but lack semantic flexibility, whereas SLMs provide depth at the cost of processing cycles.

SLM Reliability (1.5B–4B Range)

Models like Qwen, Phi, and Gemma are effective preprocessing engines but exhibit specific failure modes.

* Where they excel: Gisting, thematic summarization, and synthesizing overlapping information threads.
* Where they fail: Fact-lookup, preservation of specific metadata (IDs, version numbers, status codes), and Prefix Cache Destruction. Non-deterministic summarizers break byte-identity, forcing the provider to recompute the entire KV cache.

Latency Budget for stdio Agent UX

Compendium mandates adherence to the following performance thresholds:

* Heuristic Threshold: Target p99 ≤ 87ms for instantaneous lexical filtering.
* Neural Threshold: Local quantized models must maintain ≥ 300 t/s with sub-10ms time-to-first-token (TTFT).

Compendium shall move beyond Ollama dependencies. Citing the 13–80% performance gap between C++ implementations and Python-based alternatives, Ollama’s concurrency overhead remains the primary driver for native Rust integration.

4. Infrastructure Strategy: Decoupling Local Inference from Ollama

To ensure "zero-config" distribution, Compendium must mitigate the strategic risk of Ollama-dependency. Native Rust binaries provide the necessary performance and portability for integrated local inference.

Tradeoff Analysis: Integrated Inference vs. BYO Endpoint

Backend	Binary Weight	GPU/NPU Acceleration	Ease of Installation
llama.cpp server	Moderate	Metal / CUDA	Manual / Scripted
llama-cpp-rs/sys-4	Low	TurboQuant (2-bit V-cache)	Integrated (Prebuilt)
Lemonade	Moderate	NPU (Ryzen AI Strix/Halo)	Automated Discovery
Ollama	High	Standard GPU	External Installer

Distribution Feasibility

Native distribution via npx or Rust encounters significant friction due to the Clang/CMake build tool dependency on client machines. Compendium shall resolve this by utilizing a "prebuilt binary caching" strategy (similar to llama-cpp-sys-4), downloading precompiled static artifacts from GitHub releases to bypass local compilation failures. For Node.js integrations, the ASAR read-only limitation dictates that native bindings remain external to the archive for OS-level dynamic linking.

5. Agentic Orchestration: Gateway Patterns and Automated Navigation

A "Single Gateway" approach is superior for agentic accuracy, providing a +5.0 percentage point gain in selection precision. Compendium will implement a "Single Gateway + Action Enum" pattern to resolve the metadata tax.

Impact on Token Tax

By utilizing the Atlassian get_tool_schema + invoke_tool proxy pattern, the architecture eliminates the persistent token tax on multi-turn conversations. The model only ingests high-resolution metadata for the specific tool required for a given sub-task, rather than carrying the 17.6k token GitHub schema through every turn.

The "Inflation" Problem and Signal to Call

Compendium shall implement a threshold-based "Signal to Call" mechanism. Because wrappers like <general>...</general> can actually increase token counts on small payloads, the orchestrator must bypass compression for inputs <1000 characters. For larger contexts, the system triggers the compression pipeline to ensure efficient KV cache management.

6. Security and Operational Guardrails for Local Context

Local execution on developer filesystems makes MCP servers high-value targets for Indirect Prompt Injection (IPI) and "Cross-App Context Poisoning."

Identified Security Risks

1. systemPrompt Injection: Payloads delivered via JSON-RPC that override security guidelines with system-level priority.
2. isVisible Parameter Manipulation: Instructions executed silently without appearing in the user's chat UI.
3. PII Leakage: Persistence of sensitive identifiers in local caches or exfiltration via unauthorized outbound traffic.

Compendium Security Mandates (as implemented / planned)

* **Loopback allowlist for LLM:** `COMPENDIUM_LOCAL_LLM_URL` must resolve to loopback (`127.0.0.1` / `::1`). This is intentional — local SLMs run on loopback.
* **SSRF rejection:** Non-loopback hosts (including other private ranges such as `10.0.0.0/8` and `192.168.0.0/16`) are rejected. Do **not** block `127.0.0.0/8`; that would break BYO Ollama/llama.cpp server.
* **Sanitize hooks:** Secrets, IPI phrases, and cross-app poison params (`systemPrompt`, `isVisible`, …) via `action=sanitize` / `sanitize_input`.
* **Audit trails:** Opt-in JSONL forensic logging via `COMPENDIUM_AUDIT_PATH` (action metadata only; no request bodies). Session `stats` covers token/latency/cache telemetry.

7. Roadmap status (aligned with v0.2.0+)

### Shipped

| Item | Notes |
|------|--------|
| Single gateway + action enum | `src/server/` (`mod.rs` + `actions.rs`) — one tool `compendium` |
| Signal-to-call (<1000 chars bypass) | `pipeline/signal.rs`; `force` overrides |
| AFM `prune_history` | `strategy=afm` |
| Query-aware filter / BM25 + hybrid + opt-in CE rerank / brief | Lexical BM25; optional embeddings; opt-in CE (`/v1/rerank` or chat; partial resilience) |
| Persistent session cache | `COMPENDIUM_CACHE_DIR` / `_MAX_BYTES`; TTL + stats counters |
| `llm_status` | Probe local LLM `/models` (`force` = chat ping) |
| Opt-in audit JSONL | `COMPENDIUM_AUDIT_PATH` |
| `stats` telemetry | Session counters + latency/bypass/backend (incl. `cross_encoder`) |
| Sanitize middleware | Secrets / IPI / poison params |
| Progressive disclosure | `catalog`/`help`, `cmp://skill/…`, playbooks (incl. `hybrid-rerank`) |
| npm binary distribution | optionalDeps + GitHub Releases; platforms include musl + win32-arm64 |
| Brand icons (SEP-973) | data URIs; host rendering client-dependent |
| PR CI + docs | `ci.yml`, `examples/`, CHANGELOG, `docs/architecture.md` |
| Eval regression + latency smoke | `testdata/`, `tests/eval_regression.rs`, CI gate |
| Agent DX playbooks | `brief-then-rerank`, `sanitize-untrusted`, `stats-debug` |
| Embedding vector cache | Process-local + session/disk `cache://embed/…` |
| brief/server module split | `pipeline/brief/{walk,window,pack,synthesize}`, `server/actions` |

### Shipped B-roadmap (0.5.0)

| Phase | Focus | Status |
|-------|--------|--------|
| **B0** | Release catch-up | Tagged/published `v0.5.0` (+ history tag `v0.4.0`); wrapper + core platforms on npm; GitHub Release assets include musl/win32-arm64. npm optionalDeps for `linux-x64-musl` / `win32-arm64` still need one Trusted Publisher / interactive create ([DISTRIBUTION.md](npm/DISTRIBUTION.md)); `npx` falls back to Releases |
| **B1** | Eval + perf | Shipped (`testdata/`, `eval_regression`, CI) |
| **B2** | Agent DX | Shipped (playbooks, catalog, brief Read next) |
| **B3** | Local retrieval | Shipped (embed cache + hybrid/CE docs/tests) |
| **B4** | Maintain | Shipped (`brief/` + `server/` split) |

### Shipped C-roadmap (0.6.0)

| Phase | Focus | Status |
|-------|--------|--------|
| **C0** | Doc truth | Done (architecture/REPORT/CONTRIBUTING/SECURITY) |
| **C1** | Eval floors | Done (brief/AFM/sanitize/hybrid fallback gates) |
| **C2** | Agent DX | Done (pack/install/http playbooks; catalog + stats-debug) |
| **C3** | Retrieval polish | Done (embed-cache stats; CE partial mock) |
| **C4** | Ops docs | Done (shared cache + binary freshness) |

### Next (ops / maintenance)

| Item | Notes |
|------|--------|
| npm Trusted Publisher for `linux-x64-musl` / `win32-arm64` | OptionalDeps still missing on registry; Release soft-fails those OIDC publishes (`residual_oidc`); Releases fallback works — see [DISTRIBUTION.md](npm/DISTRIBUTION.md). `npm run check-residual-npm` fails CI if soft-fail is stale (package exists), incomplete (missing package not listed), a still-missing residual lacks its GitHub Release asset for `v${version}`, a non-residual package lacks `versions[version]` after that tag exists, or main wrapper `compendium-mcp@${version}` is missing after that tag. |
| Keep eval floors green | `cargo test --test eval_regression`; avoid inventing a D product roadmap until needed |

### Deferred

| Item | Why |
|------|-----|
| Embedded llama.cpp / Candle | Binary size + maintenance; keep BYO OpenAI-compatible server |
| TurboQuant / Hadamard / NPU path | Research; not product path |
| Foreign-MCP schema proxy (`get_tool_schema` / `invoke_tool`) | Different product; Compendium compresses *context*, not third-party tool schemas |
| Cloud embeddings | Violates local-first |

8. Implementation Checklist: Do's and Don'ts

DO

* Keep heuristic pipelines fast and deterministic (prefix-cache friendly).
* Enforce local-first privacy: LLM and future embedding URLs on loopback only.
* Prefer hybrid BM25 + local vectors when improving retrieval (B3).
* Ship npm/GitHub prebuilt **Compendium** binaries; document BYO SLM separately.
* Use deterministic compressors by default; smart/SLM paths must report `backend`.

DON'T

* Depend on cloud-only embeddings or remote LLM bases.
* Embed Candle/llama.cpp in the default binary without a separate major feature + asset strategy.
* Build a foreign-tool schema proxy under this crate’s scope.
* Allow non-loopback SSRF via “helpful” URL rewriting.
* Block loopback IPs in the SSRF guard (breaks local SLM).

9. Reference Bibliography

* High-Performance Architectural Patterns for Model Context Protocol Environments: Semantic Metadata Compression, Edge Inference Topologies, and Local Security Guardrails.
* Atlassian Labs. mcp-compressor. https://github.com/atlassian-labs/mcp-compressor
* Shevchenko, G. Humanswith.ai MCP Token Savers. https://github.com/g-shevchenko/mcp-token-savers
* Upadhyay, A. Running Local AI: Mastering Llama.cpp from Zero to Production.
* TSCG: Deterministic Tool-Schema Compilation for Agentic LLM Deployments. arXiv:2605.04107v1.
* Adaptive Focus Memory for Language Models. arXiv:2511.12712v1.
* Confused ChatGPT: Cross-App Context Poisoning via First-Party APIs. arXiv:2606.00485v1.
