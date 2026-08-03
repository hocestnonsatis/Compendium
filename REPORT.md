Architectural Blueprint for Compendium: High-Performance Local Context Optimization via MCP

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

Compendium Security Mandates

Compendium shall implement "guardrail-hooks" designed to mitigate CVE-2025-59536 (insecure settings modification) and CVE-2026-21852 (base URL overrides).

* Loopback Enforcement: Mandate LLM URLs to 127.0.0.1.
* IP Blocking: The architecture shall block Private IP ranges including 127.0.0.0/8, 10.0.0.0/8, and 192.168.0.0/16 to prevent SSRF.
* Audit Trails: Implement a secure JSON-RPC bridge that logs every tool request and response for forensic analysis.

7. Compendium Development Roadmap: Prioritized 2-3 Sprint Plan

The following features represent the critical path for the production-ready Rust MCP server.

1. rerank_chunks
  * The Problem: Initial hybrid retrieval includes significant semantic noise.
  * The Approach: SLM-based (Cross-Encoder scoring).
  * Dependency: HTTP/Ollama required? No (Native Rust).
  * Estimated Difficulty: Medium.
2. smart_prune_history
  * The Problem: Quadratic token growth in conversation logs.
  * The Approach: Deterministic (Adaptive Focus Memory (AFM) Tiered Fidelity).
  * Dependency: HTTP/Ollama required? No.
  * Estimated Difficulty: Low.
3. query-aware_filter
  * The Problem: Standard compressors often drop specific metadata required for factual lookups.
  * The Approach: Deterministic (BM25 lexical scoring).
  * Dependency: HTTP/Ollama required? No.
  * Estimated Difficulty: Medium.
4. stats/telemetry
  * The Problem: No visibility into BPE token efficiency.
  * The Approach: Deterministic (QuickTok BPE trie match).
  * Dependency: HTTP/Ollama required? No.
  * Estimated Difficulty: Low.
5. sanitization_middleware
  * The Problem: Vulnerability to IPI payloads in tool outputs.
  * The Approach: Deterministic (Regex/Secret pattern matching).
  * Dependency: HTTP/Ollama required? No.
  * Estimated Difficulty: Medium.
6. binary_weight_manager
  * The Problem: Complex ML installation failures on client hardware.
  * The Approach: Deterministic (GitHub Artifact Pre-linking).
  * Dependency: HTTP/Ollama required? No.
  * Estimated Difficulty: High.
7. hybrid_search_bridge
  * The Problem: Semantic search misses exact technical identifiers/serial keys.
  * The Approach: Deterministic (Inverted BM25 index with 128x speedup).
  * Dependency: HTTP/Ollama required? No.
  * Estimated Difficulty: Medium.

8. Implementation Checklist: Do's and Don'ts

DO

* Prioritize Rust-native speed: Use llama-cpp-sys-4 for direct memory linking and NPU-acceleration on Ryzen AI hardware.
* Enforce local-first privacy: Mandate all traffic to the loopback interface.
* Implement Hybrid Retrieval: Combine BM25 for technical identifiers with semantic vectors.
* Apply Hadamard rotation for TurboQuant: Maintain accuracy during aggressive 2-bit V-cache compression.
* Use Deterministic Compression: Ensure byte-identical outputs to preserve downstream prefix caching.

DON'T

* Depend on cloud-only embeddings: This violates the local-first security model and introduces network latency.
* Use Candle for production: Avoid due to binary size issues and lack of Flash Attention parity.
* Allow Non-deterministic Summarizers: These break byte-identity and destroy prefix cache reuse.
* Ignore Multi-instance unified memory contention: Avoid running independent inference processes that starve the memory controller on Apple Silicon.

9. Reference Bibliography

* High-Performance Architectural Patterns for Model Context Protocol Environments: Semantic Metadata Compression, Edge Inference Topologies, and Local Security Guardrails.
* Atlassian Labs. mcp-compressor. https://github.com/atlassian-labs/mcp-compressor
* Shevchenko, G. Humanswith.ai MCP Token Savers. https://github.com/g-shevchenko/mcp-token-savers
* Upadhyay, A. Running Local AI: Mastering Llama.cpp from Zero to Production.
* TSCG: Deterministic Tool-Schema Compilation for Agentic LLM Deployments. arXiv:2605.04107v1.
* Adaptive Focus Memory for Language Models. arXiv:2511.12712v1.
* Confused ChatGPT: Cross-App Context Poisoning via First-Party APIs. arXiv:2606.00485v1.
