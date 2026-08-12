//! MCP tool surface for Compendium — single gateway tool with `action` dispatch.

mod actions;

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::{
        ListResourcesResult, MetaObject, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RoleServer},
    tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

use crate::brand::{mcp_icons, WEBSITE_URL};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::pipeline::{
    brief::BriefOptions,
    cache::{CacheStore, CacheStoreOptions},
    catalog::{action_ads, catalog_json, help_markdown, parse_action_uri},
    chunk::{ChunkMap, ChunkOptions},
    compress::CompressOptions,
    filter::FilterOptions,
    output::CompressOutputOptions,
    pack::PackOptions,
    playbook::{
        get_playbook, list_playbooks, parse_playbook_uri, playbook_ads_json, skill_index_json,
    },
    prune::{HistoryMessage, PruneOptions},
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

impl CompendiumServer {
    pub fn new(config: Config) -> Self {
        let state = ServerState {
            cache: CacheStore::from_config(&config),
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
            state.stats.record(&format!("compendium:{action}"), metrics);
        }
    }

    fn record_call(&self, action: &str) {
        if let Ok(mut state) = self.state.lock() {
            state
                .stats
                .record_count_only(&format!("compendium:{action}"));
        }
    }

    /// Attach latency / bypass / backend flags after an action finishes (does not double-count calls).
    fn attach_telemetry(&self, action: &str, latency_ms: f64, result: Option<&Value>) {
        if let Ok(mut state) = self.state.lock() {
            let tool = format!("compendium:{action}");
            let bypassed = result
                .and_then(|v| v.get("bypassed"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let backend = result
                .and_then(|v| v.get("backend"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Session-level latency + flags (calls already counted by record*).
            SessionStats::attach_post_call(
                &mut state.stats,
                &tool,
                latency_ms,
                bypassed,
                backend.as_deref(),
            );
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
        let started = Instant::now();
        let discovery_id = params.id.clone().or_else(|| params.key.clone());
        let force = params.force.unwrap_or(false);

        let outcome = match action {
            CompendiumAction::Filter => self.act_filter(params),
            CompendiumAction::Compress => self.act_compress(params),
            CompendiumAction::CompressOutput => self.act_compress_output(params),
            CompendiumAction::Summarize => self.act_summarize(params),
            CompendiumAction::SummarizeSmart => self.act_summarize_smart(params),
            CompendiumAction::FilterRelevant => self.act_filter_relevant(params),
            CompendiumAction::PruneHistory => self.act_prune(params),
            CompendiumAction::Chunk => self.act_chunk(params),
            CompendiumAction::Resolve => self.act_resolve(params),
            CompendiumAction::CountTokens => self.act_count_tokens(params),
            CompendiumAction::Stats => self.act_stats(params),
            CompendiumAction::CacheStore => self.act_cache_store(params),
            CompendiumAction::CacheGet => self.act_cache_get(params),
            CompendiumAction::CacheInvalidate => self.act_cache_invalidate(params),
            CompendiumAction::Sanitize => self.act_sanitize(params),
            CompendiumAction::Rerank => self.act_rerank(params),
            CompendiumAction::Brief => self.act_brief(params),
            CompendiumAction::Catalog => self.act_catalog(params),
            CompendiumAction::Help => self.act_help(params),
            CompendiumAction::Playbooks => self.act_playbooks(params),
            CompendiumAction::Playbook => self.act_playbook(params),
            CompendiumAction::Pack => self.act_pack(params),
            CompendiumAction::Unpack => self.act_unpack(params),
            CompendiumAction::LlmStatus => self.act_llm_status(params),
        };

        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

        match outcome {
            Ok(result) => {
                self.attach_telemetry(action_name, latency_ms, Some(&result));
                self.note_lazy_telemetry(action, discovery_id.as_deref(), force);
                let backend = result.get("backend").and_then(|v| v.as_str());
                let metrics = result.get("metrics").and_then(|m| {
                    let o = m.get("original_tokens")?.as_u64()? as usize;
                    let r = m.get("result_tokens")?.as_u64()? as usize;
                    Some(TokenMetrics::new(o, r))
                });
                crate::pipeline::audit::record_action(
                    &self.config,
                    action_name,
                    true,
                    None,
                    metrics.as_ref(),
                    Some(latency_ms),
                    backend,
                );
                GatewayEnvelope {
                    ok: true,
                    action: action_name.into(),
                    result_json: result.to_string(),
                    error: None,
                }
            }
            Err(error) => {
                self.attach_telemetry(action_name, latency_ms, None);
                crate::pipeline::audit::record_action(
                    &self.config,
                    action_name,
                    false,
                    Some(&error),
                    None,
                    Some(latency_ms),
                    None,
                );
                GatewayEnvelope {
                    ok: false,
                    action: action_name.into(),
                    result_json: "{}".into(),
                    error: Some(error),
                }
            }
        }
    }

    fn note_lazy_telemetry(
        &self,
        action: CompendiumAction,
        discovery_id: Option<&str>,
        force: bool,
    ) {
        if let Ok(mut state) = self.state.lock() {
            match action {
                CompendiumAction::Catalog | CompendiumAction::Playbooks => {
                    state.stats.note_lazy_ad(None);
                }
                CompendiumAction::Help => {
                    let id =
                        discovery_id.map(|s| s.strip_prefix("cmp://skill/action/").unwrap_or(s));
                    if force {
                        state.stats.note_lazy_full(id);
                    } else {
                        state.stats.note_lazy_ad(id);
                    }
                }
                CompendiumAction::Playbook => {
                    let id =
                        discovery_id.map(|s| s.strip_prefix("cmp://skill/playbook/").unwrap_or(s));
                    state.stats.note_lazy_full(id);
                }
                CompendiumAction::Stats
                | CompendiumAction::CountTokens
                | CompendiumAction::CacheGet
                | CompendiumAction::CacheInvalidate
                | CompendiumAction::LlmStatus => {}
                other => {
                    state.stats.note_action_follow(other.as_str());
                }
            }
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

    /// Resolve MCP resource URI to (mime, body, content_hash).
    fn read_skill_resource(&self, uri: &str) -> Result<(String, String, String), McpError> {
        let uri = uri.trim();
        let (mime, body, discovered) = if uri == "cmp://skill/index" {
            let catalog = catalog_json(&playbook_ads_json(&self.config));
            let index = skill_index_json(&self.config, catalog);
            let body = serde_json::to_string_pretty(&index).unwrap_or_else(|_| "{}".into());
            ("application/json".into(), body, None)
        } else if let Some(id) = parse_action_uri(uri) {
            // Resources always serve full docs (on-demand load).
            let md = help_markdown(id, true).map_err(|e| McpError::resource_not_found(e, None))?;
            ("text/markdown".into(), md, Some(id.to_string()))
        } else if let Some(id) = parse_playbook_uri(uri) {
            let pb = get_playbook(id, &self.config)
                .map_err(|e| McpError::resource_not_found(e, None))?;
            let body = format!(
                "# {}\n\n{}\n\n---\n\n{}\n",
                pb.name, pb.description, pb.body
            );
            ("text/markdown".into(), body, Some(pb.id))
        } else {
            return Err(McpError::resource_not_found(
                format!("unknown resource uri: {uri}"),
                None,
            ));
        };

        if let Ok(mut state) = self.state.lock() {
            if let Some(id) = discovered.as_deref() {
                state.stats.note_lazy_full(Some(id));
            } else {
                state.stats.note_lazy_full(None);
            }
        }

        let hash = content_etag(&body);
        Ok((mime, body, hash))
    }

    fn list_skill_resources(&self) -> Vec<Resource> {
        let mut resources = Vec::new();
        resources.push(
            Resource::new("cmp://skill/index", "skill-index")
                .with_description("Compendium skill index (actions + playbooks)")
                .with_mime_type("application/json")
                .with_icons(mcp_icons()),
        );
        for ad in action_ads() {
            resources.push(
                Resource::new(ad.uri, ad.id)
                    .with_description(format!("{} — {}", ad.one_liner, ad.when_to_use))
                    .with_mime_type("text/markdown"),
            );
        }
        for pb in list_playbooks(&self.config) {
            resources.push(
                Resource::new(pb.uri, pb.id)
                    .with_description(pb.description)
                    .with_mime_type("text/markdown"),
            );
        }
        resources
    }
}

/// Gateway action selector.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompendiumAction {
    /// Strip ANSI/boilerplate; densify JSON; keep/drop regexes.
    Filter,
    /// Dense semantic compression of text/code/logs.
    Compress,
    /// Domain-aware stdout/stderr scrub (git/cargo/npm/docker/…).
    CompressOutput,
    /// Hierarchical summary (conversation / file tree / outline).
    Summarize,
    /// Local-SLM dense summary (heuristic fallback if LLM unset/fails).
    SummarizeSmart,
    /// Query-aware keep of relevant lines (local SLM + heuristic fallback).
    FilterRelevant,
    /// Drop filler and/or compress older chat turns.
    PruneHistory,
    /// Split corpus into `cmp://` chunks (also cached for resolve).
    Chunk,
    /// Resolve a chunk id to content.
    Resolve,
    /// Count tokens with the active backend.
    CountTokens,
    /// Session savings report (optional reset).
    Stats,
    /// Store bulky text outside the prompt; returns key + preview.
    CacheStore,
    /// Retrieve cached text / chunk by key.
    CacheGet,
    /// Drop one cache key or clear the session cache.
    CacheInvalidate,
    /// Redact secrets and neutralize Indirect Prompt Injection phrases.
    Sanitize,
    /// BM25-rank text chunks / candidates for a query.
    Rerank,
    /// Scan a workspace for task-relevant slices and pack a structured starter briefing.
    Brief,
    /// List short action (+ playbook) advertisements.
    Catalog,
    /// Full usage notes + example for one action.
    Help,
    /// List token-hygiene playbook advertisements.
    Playbooks,
    /// Load one playbook body by id.
    Playbook,
    /// Zip text/files into a bounded archive.
    Pack,
    /// Unpack zip with size caps into chunks (never runs scripts).
    Unpack,
    /// Probe configured local LLM (loopback) reachability / models.
    LlmStatus,
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
            Self::PruneHistory => "prune_history",
            Self::Chunk => "chunk",
            Self::Resolve => "resolve",
            Self::CountTokens => "count_tokens",
            Self::Stats => "stats",
            Self::CacheStore => "cache_store",
            Self::CacheGet => "cache_get",
            Self::CacheInvalidate => "cache_invalidate",
            Self::Sanitize => "sanitize",
            Self::Rerank => "rerank",
            Self::Brief => "brief",
            Self::Catalog => "catalog",
            Self::Help => "help",
            Self::Playbooks => "playbooks",
            Self::Playbook => "playbook",
            Self::Pack => "pack",
            Self::Unpack => "unpack",
            Self::LlmStatus => "llm_status",
        }
    }
}

/// Single-tool parameters. Set `action`, then only the fields that action needs.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GatewayParams {
    /// Which operation to run.
    pub action: CompendiumAction,

    /// Primary text for filter/compress/summarize/chunk/count_tokens/cache_store/compress_output,
    /// or a `user:/assistant:` transcript for prune_history, or raw corpus for resolve.
    pub text: Option<String>,

    /// Relevance query for `filter_relevant` / `rerank` / `brief`.
    pub query: Option<String>,

    /// Structured chat turns for `prune_history` (preferred over `text`).
    pub messages: Option<Vec<HistoryMessage>>,

    /// Cache key for `cache_get` / `cache_invalidate`, or explicit key inside `cache` options.
    pub key: Option<String>,

    /// Chunk id for `resolve`, or action/playbook id for `help` / `playbook`.
    pub id: Option<String>,

    /// Explicit chunk map JSON for `resolve` when not in session cache
    /// (pass the object previously returned by `action=chunk`).
    #[serde(default)]
    pub map: Option<serde_json::Map<String, Value>>,

    /// When `action=stats`, reset counters after snapshot.
    pub reset: Option<bool>,

    /// When true with `action=help`, return full docs (example+notes). Default compressed.
    /// Also used by compress/summarize signal bypass when nested under those option bags.
    pub force: Option<bool>,

    /// Options for `action=filter`.
    pub filter: Option<FilterOptions>,
    /// Options for `action=compress`.
    pub compress: Option<CompressOptions>,
    /// Options for `action=compress_output`.
    pub output: Option<CompressOutputOptions>,
    /// Options for `action=summarize` (also used as structure hints by `summarize_smart`).
    pub summarize: Option<SummarizeOptions>,
    /// Options for `action=summarize_smart` / `filter_relevant`.
    pub smart: Option<SmartOptions>,
    /// Options for `action=prune_history`.
    pub prune: Option<PruneOptions>,
    /// Options for `action=chunk` (and re-chunk fallback in `resolve`).
    pub chunk: Option<ChunkOptions>,
    /// Options for `action=cache_store`.
    pub cache: Option<CacheStoreOptions>,
    /// Options for `action=sanitize` (also used when `sanitize_input` is true).
    pub sanitize: Option<SanitizeOptions>,
    /// When true, scrub secrets/IPI from `text` before the chosen action runs.
    pub sanitize_input: Option<bool>,
    /// Candidates for `action=rerank` (preferred over parsing `text`), or files for `pack`.
    pub items: Option<Vec<RerankItem>>,
    /// Options for `action=rerank`.
    pub rerank: Option<RerankOptions>,
    /// Options for `action=brief` (workspace scan + pack).
    pub brief: Option<BriefOptions>,
    /// Options for `action=pack` / `unpack`.
    pub pack: Option<PackOptions>,
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
        description = "Shrink agent context safely (one tool). Set `action`. Unsure → catalog (then help+id). New repo task → brief. Logs → filter or compress_output. Bulky text/JSON → compress. Untrusted → sanitize. Recipes → playbooks. Details: cmp://skill/…",
        icons = mcp_icons()
    )]
    fn compendium(&self, Parameters(params): Parameters<GatewayParams>) -> Json<GatewayEnvelope> {
        Json(self.dispatch(params))
    }
}

#[tool_handler(
    name = "compendium",
    instructions = "Call `compendium` with `action`. Unsure → catalog then help+id (or cmp://skill/…). Quick map: new task→brief; ANSI/spinners→filter; cargo/npm/docker/git dumps→compress_output; bulky text/JSON→compress; untrusted→sanitize; question-known logs→filter_relevant; rank chunks→rerank; park blob→cache_store; multi-file zip→pack/unpack; long chat→prune_history(afm); measure→count_tokens/stats."
)]
impl ServerHandler for CompendiumServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(
            rmcp::model::Implementation::new("compendium", env!("CARGO_PKG_VERSION"))
                .with_title("Compendium")
                .with_description(
                    "Shrink agent context safely: filter/compress logs and blobs, sanitize untrusted text, brief a workspace — one MCP tool",
                )
                .with_icons(mcp_icons())
                .with_website_url(WEBSITE_URL),
        )
        .with_instructions(
            "Call `compendium` with `action`. Unsure → catalog then help+id (or cmp://skill/…). Quick map: new task→brief; ANSI/spinners→filter; cargo/npm/docker/git dumps→compress_output; bulky text/JSON→compress; untrusted→sanitize; question-known logs→filter_relevant; rank chunks→rerank; park blob→cache_store; multi-file zip→pack/unpack; long chat→prune_history(afm); measure→count_tokens/stats."
                .to_string(),
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        if let Ok(mut state) = self.state.lock() {
            state.stats.note_lazy_ad(None);
        }
        std::future::ready(Ok(ListResourcesResult::with_all_items(
            self.list_skill_resources(),
        )))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ReadResourceResponse, McpError>> + Send + '_ {
        let ttl = self.config.skill_resource_ttl_ms;
        let result = self
            .read_skill_resource(&request.uri)
            .map(|(mime, text, etag)| {
                let mut meta = MetaObject::new();
                meta.0.insert(
                    "etag".into(),
                    serde_json::Value::String(format!("\"{etag}\"")),
                );
                meta.0
                    .insert("contentHash".into(), serde_json::Value::String(etag));
                ReadResourceResult::new(vec![ResourceContents::text(text, request.uri.clone())
                    .with_mime_type(mime)
                    .with_meta(meta)])
                .with_ttl_ms(ttl)
                .into()
            });
        std::future::ready(result)
    }
}

fn content_etag(body: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    body.hash(&mut h);
    format!("{:x}", h.finish())
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
    fn new_server_uses_config_cache_dir() {
        let dir =
            std::env::temp_dir().join(format!("compendium-server-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = Config {
            cache_dir: Some(dir.clone()),
            cache_max_bytes: Some(1024 * 1024),
            ..Default::default()
        };
        let server = CompendiumServer::new(cfg);
        {
            let mut state = server.state.lock().unwrap();
            let stored = state.cache.store(
                "hello",
                &CacheStoreOptions {
                    key: Some("s1".into()),
                    ..Default::default()
                },
                &server.config,
            );
            assert_eq!(stored.backend, "disk");
        }
        let server2 = CompendiumServer::new(Config {
            cache_dir: Some(dir.clone()),
            cache_max_bytes: Some(1024 * 1024),
            ..Default::default()
        });
        let mut state = server2.state.lock().unwrap();
        let got = state.cache.get("s1", &server2.config);
        assert!(got.hit);
        assert_eq!(got.content, "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
