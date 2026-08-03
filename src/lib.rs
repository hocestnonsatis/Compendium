//! Compendium — token-efficient context processing for MCP clients.
//!
//! Core pipeline modules implement deterministic heuristics for compression,
//! summarization, filtering, and chunk/reference mapping. Optional local-SLM
//! smart actions call an OpenAI-compatible loopback endpoint when configured.
//! The MCP server exposes these as tools over JSON-RPC stdio.

pub mod config;
#[cfg(feature = "http")]
pub mod http;
pub mod pipeline;
pub mod server;

pub use config::{Config, LocalLlmConfig, DEFAULT_SIGNAL_MIN_CHARS};
pub use pipeline::{
    cache::{CacheGetResult, CacheInvalidateResult, CacheStoreOptions, CacheStoreResult},
    chunk::{chunk_with_refs, resolve_chunk, resolve_ref, ChunkMap, ChunkOptions, ResolveResult},
    compress::{compress, CompressOptions, CompressResult},
    filter::{filter, FilterOptions, FilterResult},
    output::{compress_output, CompressOutputOptions, CompressOutputResult},
    prune::{
        parse_history_input, prune_history, AfmTier, HistoryMessage, PruneOptions, PruneResult,
        PruneStrategy,
    },
    rerank::{parse_rerank_items, rerank, RerankHit, RerankItem, RerankOptions, RerankResult},
    sanitize::{sanitize, SanitizeFinding, SanitizeOptions, SanitizeResult},
    signal::{bypass_reason, should_bypass_signal},
    smart::{
        filter_relevant, summarize_smart, SmartBackend, SmartFilterResult, SmartOptions,
        SmartSummarizeResult,
    },
    stats::{CallMeta, SessionStats, ToolStats},
    summarize::{summarize, SummarizeOptions, SummarizeResult},
    tokens::{
        count_tokens_detailed, estimate_tokens, token_backend, CountTokensResult, TokenBackend,
    },
};
pub use server::CompendiumServer;
