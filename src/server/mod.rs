//! MCP tool surface for Compendium — single gateway tool with `action` dispatch.

mod actions;

use std::sync::{Arc, Mutex};

use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ServerHandler,
};

use crate::brand::{mcp_icons, WEBSITE_URL};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::pipeline::{
    cache::{CacheStore, CacheStoreOptions},
    chunk::{ChunkMap, ChunkOptions},
    compress::CompressOptions,
    filter::FilterOptions,
    output::CompressOutputOptions,
    rerank::{RerankItem, RerankOptions},
    sanitize::{sanitize, SanitizeOptions},
    smart::SmartOptions,
    stats::SessionStats,
    summarize::SummarizeOptions,
    TokenMetrics,
};

/// Shared mutable session state (cache + savings counters).
struct ServerState {
    cache: CacheStore,
    stats: SessionStats,
}

/// MCP server holding shared configuration and session state.
#[derive(Clone)]
pub struct CompendiumServer {
    pub config: Config,
    state: Arc<Mutex<ServerState>>,
}

const INSTRUCTIONS: &str = "Call `compendium` with `action`. Unsure → help (then help+id). \
Noisy logs → filter; cargo/npm/docker/git dumps → compress_output; bulky text/JSON → compress; \
untrusted text → sanitize; known question → filter_relevant; rank candidates → rerank; \
huge corpus → chunk then resolve; park a blob → cache_store; measure → count_tokens/stats.";

impl CompendiumServer {
    pub fn new(config: Config) -> Self {
        let state = ServerState {
            cache: CacheStore::default(),
            stats: SessionStats::default(),
        };
        Self {
            config,
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn from_env() -> Self {
        Self::new(Config::from_env())
    }

    /// Public names of registered MCP tools (for tests / diagnostics).
    pub fn tool_names() -> Vec<String> {
        Self::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    fn record(&self, action: &str, metrics: &TokenMetrics) {
        if let Ok(mut state) = self.state.lock() {
            state.stats.record(action, metrics);
        }
    }

    fn record_call(&self, action: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.stats.record_count_only(action);
        }
    }

    fn cache_chunks(&self, map: &ChunkMap) {
        if let Ok(mut state) = self.state.lock() {
            for chunk in &map.chunks {
                state
                    .cache
                    .put_raw(chunk.id.clone(), chunk.content.clone(), chunk.tokens);
            }
            state.cache.put_raw(
                format!("refmap://{}", map.content_hash),
                map.index_text.clone(),
                map.metrics.result_tokens,
            );
        }
    }

    fn dispatch(&self, params: GatewayParams) -> GatewayEnvelope {
        let action = params.action;
        let action_name = action.as_str();

        let outcome = match action {
            CompendiumAction::Filter => self.act_filter(params),
            CompendiumAction::Compress => self.act_compress(params),
            CompendiumAction::CompressOutput => self.act_compress_output(params),
            CompendiumAction::Summarize => self.act_summarize(params),
            CompendiumAction::SummarizeSmart => self.act_summarize_smart(params),
            CompendiumAction::FilterRelevant => self.act_filter_relevant(params),
            CompendiumAction::Sanitize => self.act_sanitize(params),
            CompendiumAction::Rerank => self.act_rerank(params),
            CompendiumAction::Chunk => self.act_chunk(params),
            CompendiumAction::Resolve => self.act_resolve(params),
            CompendiumAction::CacheStore => self.act_cache_store(params),
            CompendiumAction::CacheGet => self.act_cache_get(params),
            CompendiumAction::CacheInvalidate => self.act_cache_invalidate(params),
            CompendiumAction::CountTokens => self.act_count_tokens(params),
            CompendiumAction::Stats => self.act_stats(params),
            CompendiumAction::LlmStatus => self.act_llm_status(params),
            CompendiumAction::Help => self.act_help(params),
        };

        match outcome {
            Ok(result) => {
                if let Some(backend) = result.get("backend").and_then(|v| v.as_str()) {
                    if let Ok(mut state) = self.state.lock() {
                        state.stats.note_backend(backend);
                    }
                }
                GatewayEnvelope {
                    ok: true,
                    action: action_name.into(),
                    result_json: result.to_string(),
                    error: None,
                }
            }
            Err(error) => GatewayEnvelope {
                ok: false,
                action: action_name.into(),
                result_json: "{}".into(),
                error: Some(error),
            },
        }
    }

    fn require_text(params: &GatewayParams) -> Result<String, String> {
        params
            .text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| "missing required field `text` for this action".into())
    }

    /// Optionally scrub secrets/IPI from text before the main action runs.
    fn maybe_sanitize_text(&self, text: String, params: &GatewayParams) -> String {
        if params.sanitize_input.unwrap_or(false) {
            let opts = params.sanitize.clone().unwrap_or_default();
            sanitize(&text, &opts, &self.config).content
        } else {
            text
        }
    }
}

/// Gateway action selector.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompendiumAction {
    /// Strip ANSI/boilerplate; densify JSON; keep/drop regexes.
    Filter,
    /// Dense compression of text/code/logs.
    Compress,
    /// Domain-aware stdout/stderr scrub (git/cargo/npm/docker/…).
    CompressOutput,
    /// Hierarchical summary (conversation / file tree / outline).
    Summarize,
    /// Local-SLM dense summary (heuristic fallback if LLM unset/fails).
    SummarizeSmart,
    /// Query-aware keep of relevant lines (local SLM + heuristic fallback).
    FilterRelevant,
    /// Redact secrets and neutralize prompt-injection phrases.
    Sanitize,
    /// BM25-rank candidates for a query.
    Rerank,
    /// Split corpus into `cmp://` chunks (cached for resolve).
    Chunk,
    /// Resolve a chunk id to content.
    Resolve,
    /// Store bulky text outside the prompt; returns key + preview.
    CacheStore,
    /// Retrieve cached text / chunk by key.
    CacheGet,
    /// Drop one cache key or clear the session cache.
    CacheInvalidate,
    /// Count tokens with the active backend.
    CountTokens,
    /// Session savings report (optional reset).
    Stats,
    /// Probe configured local LLM (loopback) reachability / models.
    LlmStatus,
    /// List actions (no `id`) or usage + example for one action (`id`).
    Help,
}

impl CompendiumAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Compress => "compress",
            Self::CompressOutput => "compress_output",
            Self::Summarize => "summarize",
            Self::SummarizeSmart => "summarize_smart",
            Self::FilterRelevant => "filter_relevant",
            Self::Sanitize => "sanitize",
            Self::Rerank => "rerank",
            Self::Chunk => "chunk",
            Self::Resolve => "resolve",
            Self::CacheStore => "cache_store",
            Self::CacheGet => "cache_get",
            Self::CacheInvalidate => "cache_invalidate",
            Self::CountTokens => "count_tokens",
            Self::Stats => "stats",
            Self::LlmStatus => "llm_status",
            Self::Help => "help",
        }
    }
}

/// Single-tool parameters. Set `action`, then only the fields that action needs.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GatewayParams {
    /// Which operation to run.
    pub action: CompendiumAction,

    /// Primary text for filter/compress/summarize/chunk/count_tokens/cache_store/compress_output,
    /// or raw corpus for resolve, or candidates for rerank.
    pub text: Option<String>,

    /// Relevance query for `filter_relevant` / `rerank` (also BM25 line keep in `filter`).
    pub query: Option<String>,

    /// Cache key for `cache_get` / `cache_invalidate`.
    pub key: Option<String>,

    /// Chunk id for `resolve`, or action name for `help`.
    pub id: Option<String>,

    /// Chunk map JSON for `resolve` when not in session cache, or chunks to `rerank`
    /// (pass the object previously returned by `action=chunk`).
    #[serde(default)]
    pub map: Option<serde_json::Map<String, Value>>,

    /// When `action=stats`, reset counters after snapshot.
    pub reset: Option<bool>,

    /// `llm_status`: also run a tiny chat probe. `compress`/`summarize`: skip the
    /// short-input bypass (also accepted inside those option bags).
    pub force: Option<bool>,

    /// Options for `action=filter`.
    pub filter: Option<FilterOptions>,
    /// Options for `action=compress`.
    pub compress: Option<CompressOptions>,
    /// Options for `action=compress_output`.
    pub output: Option<CompressOutputOptions>,
    /// Options for `action=summarize` (also structure hints for `summarize_smart`).
    pub summarize: Option<SummarizeOptions>,
    /// Options for `action=summarize_smart` / `filter_relevant`.
    pub smart: Option<SmartOptions>,
    /// Options for `action=chunk` (and re-chunk fallback in `resolve`).
    pub chunk: Option<ChunkOptions>,
    /// Options for `action=cache_store`.
    pub cache: Option<CacheStoreOptions>,
    /// Options for `action=sanitize` (also used when `sanitize_input` is true).
    pub sanitize: Option<SanitizeOptions>,
    /// When true, scrub secrets/IPI from `text` before the chosen action runs.
    pub sanitize_input: Option<bool>,
    /// Candidates for `action=rerank` (preferred over parsing `text`).
    pub items: Option<Vec<RerankItem>>,
    /// Options for `action=rerank`.
    pub rerank: Option<RerankOptions>,
}

/// Gateway response envelope.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GatewayEnvelope {
    pub ok: bool,
    pub action: String,
    /// Action payload serialized as a JSON string (shape depends on `action`).
    pub result_json: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[tool_router]
impl CompendiumServer {
    /// Shrink agent context safely — one tool, many actions via `action`.
    #[tool(
        name = "compendium",
        description = "Shrink agent context safely (one tool). Set `action`. Unsure → help. Logs → filter or compress_output. Bulky text/JSON → compress. Untrusted → sanitize. Huge corpus → chunk/resolve.",
        icons = mcp_icons()
    )]
    fn compendium(&self, Parameters(params): Parameters<GatewayParams>) -> Json<GatewayEnvelope> {
        Json(self.dispatch(params))
    }
}

#[tool_handler]
impl ServerHandler for CompendiumServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                rmcp::model::Implementation::new("compendium", env!("CARGO_PKG_VERSION"))
                    .with_title("Compendium")
                    .with_description(
                        "Shrink agent context safely: filter/compress logs and blobs, sanitize untrusted text — one MCP tool",
                    )
                    .with_icons(mcp_icons())
                    .with_website_url(WEBSITE_URL),
            )
            .with_instructions(INSTRUCTIONS.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_gateway_tool_registered() {
        let names = CompendiumServer::tool_names();
        assert_eq!(names, vec!["compendium".to_string()]);
    }

    #[test]
    fn help_without_id_lists_actions() {
        let server = CompendiumServer::new(Config::default());
        let env = server.dispatch(GatewayParams {
            action: CompendiumAction::Help,
            text: None,
            query: None,
            key: None,
            id: None,
            map: None,
            reset: None,
            force: None,
            filter: None,
            compress: None,
            output: None,
            summarize: None,
            smart: None,
            chunk: None,
            cache: None,
            sanitize: None,
            sanitize_input: None,
            items: None,
            rerank: None,
        });
        assert!(env.ok);
        let v: Value = serde_json::from_str(&env.result_json).unwrap();
        assert!(v["actions"].as_array().is_some_and(|a| a.len() >= 15));
    }
}
