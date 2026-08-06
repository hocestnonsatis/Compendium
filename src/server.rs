//! MCP tool surface for Compendium — single gateway tool with `action` dispatch.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::{
        ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResult,
        Resource, ResourceContents, ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RoleServer},
    tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

use crate::brand::{mcp_icons, WEBSITE_URL};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::pipeline::{
    brief::{brief, BriefOptions},
    cache::{CacheStore, CacheStoreOptions},
    catalog::{
        catalog_json, catalog_markdown, help_for, help_markdown, parse_action_uri, action_ads,
    },
    chunk::{chunk_with_refs, resolve_ref, ChunkMap, ChunkOptions},
    compress::{compress, CompressOptions},
    filter::{filter, FilterOptions},
    output::{compress_output, CompressOutputOptions},
    pack::{
        decode_archive_bytes, pack_items, parse_pack_text, unpack_bytes, PackItem, PackOptions,
    },
    playbook::{
        get_playbook, list_playbooks, parse_playbook_uri, playbook_ads_json, playbook_catalog_lines,
        skill_index_json,
    },
    prune::{parse_history_input, prune_history, HistoryMessage, PruneOptions},
    rerank::{parse_rerank_items, rerank, RerankItem, RerankOptions},
    sanitize::{sanitize, SanitizeOptions},
    smart::{filter_relevant, summarize_smart, SmartOptions},
    stats::SessionStats,
    summarize::{summarize, SummarizeOptions},
    tokens::count_tokens_detailed,
    TokenMetrics,
};

/// Shared mutable session state (cache + savings counters).
#[derive(Default)]
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
        Self {
            config,
            state: Arc::new(Mutex::new(ServerState::default())),
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
            state
                .stats
                .record(&format!("compendium:{action}"), metrics);
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
        };

        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

        match outcome {
            Ok(result) => {
                self.attach_telemetry(action_name, latency_ms, Some(&result));
                GatewayEnvelope {
                    ok: true,
                    action: action_name.into(),
                    result_json: result.to_string(),
                    error: None,
                }
            }
            Err(error) => {
                self.attach_telemetry(action_name, latency_ms, None);
                GatewayEnvelope {
                    ok: false,
                    action: action_name.into(),
                    result_json: "{}".into(),
                    error: Some(error),
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

    fn act_filter(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let mut options = params.filter.unwrap_or_default();
        if options.query.is_none() {
            if let Some(q) = params
                .query
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                options.query = Some(q.to_string());
            }
        }
        let result = filter(&text, &options, &self.config);
        self.record("filter", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_compress(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let options = params.compress.unwrap_or_default();
        let result = compress(&text, &options, &self.config);
        self.record("compress", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_compress_output(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let options = params.output.unwrap_or_default();
        let result = compress_output(&text, &options, &self.config);
        self.record("compress_output", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_summarize(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let options = params.summarize.unwrap_or_default();
        let result = summarize(&text, &options, &self.config);
        self.record("summarize", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_summarize_smart(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let smart = params.smart.unwrap_or_default();
        let summarize_opts = params.summarize.unwrap_or_default();
        let result = summarize_smart(&text, &smart, &summarize_opts, &self.config)?;
        self.record("summarize_smart", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_filter_relevant(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let mut smart = params.smart.unwrap_or_default();
        let query = params
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| smart.query.take())
            .ok_or_else(|| {
                "filter_relevant requires `query` (top-level or smart.query)".to_string()
            })?;
        let result = filter_relevant(&text, &query, &smart, &self.config)?;
        self.record("filter_relevant", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_sanitize(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let options = params.sanitize.unwrap_or_default();
        let result = sanitize(&text, &options, &self.config);
        self.record("sanitize", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_prune(&self, params: GatewayParams) -> Result<Value, String> {
        let messages = if let Some(msgs) = params.messages {
            msgs
        } else if let Some(text) = params.text.as_deref() {
            parse_history_input(text)
        } else {
            return Err(
                "prune_history requires `messages` or `text` (user:/assistant: transcript)".into(),
            );
        };
        let options = params.prune.unwrap_or_default();
        let result = prune_history(&messages, &options, &self.config);

        if let (Some(key), Some(payload)) = (&result.distant_key, &result.distant_payload) {
            let tokens = crate::pipeline::tokens::estimate_tokens(payload, &self.config);
            if let Ok(mut state) = self.state.lock() {
                state.cache.put_raw(key.clone(), payload.clone(), tokens);
            }
        }

        self.record("prune_history", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_rerank(&self, params: GatewayParams) -> Result<Value, String> {
        let query = params
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "rerank requires `query`".to_string())?;

        let items = if let Some(items) = params.items {
            if items.is_empty() {
                return Err("rerank `items` must be non-empty".into());
            }
            items
        } else if let Some(text) = params.text.as_deref() {
            parse_rerank_items(text)?
        } else if let Some(map) = params.map.as_ref() {
            // Accept a prior chunk map object: extract chunks[].{id,content}
            let chunks = map
                .get("chunks")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "rerank `map` must contain a `chunks` array".to_string())?;
            let mut out = Vec::new();
            for c in chunks {
                let text = c
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let id = c
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                out.push(RerankItem { id, text });
            }
            if out.is_empty() {
                return Err("rerank `map.chunks` had no content".into());
            }
            out
        } else {
            return Err("rerank requires `items`, `text`, or chunk `map`".into());
        };

        let options = params.rerank.unwrap_or_default();
        let result = rerank(query, &items, &options, &self.config);
        self.record("rerank", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_chunk(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let options = params.chunk.unwrap_or_default();
        let result = chunk_with_refs(&text, &options, &self.config);
        self.cache_chunks(&result);
        self.record("chunk", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_resolve(&self, params: GatewayParams) -> Result<Value, String> {
        let id = params
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "resolve requires `id` (chunk ref from action=chunk)".to_string())?;

        let cached = self
            .state
            .lock()
            .ok()
            .map(|mut state| state.cache.get(id, &self.config));

        if let Some(cached) = cached {
            if cached.hit {
                let preview: String = cached.content.chars().take(96).collect();
                self.record_call("resolve");
                return Ok(json!({
                    "found": true,
                    "id": id,
                    "chunk": {
                        "id": id,
                        "index": 0,
                        "start_offset": 0,
                        "end_offset": cached.bytes,
                        "tokens": cached.tokens,
                        "preview": preview,
                        "content": cached.content,
                    },
                    "source": Value::Null,
                    "content_hash": Value::Null,
                }));
            }
        }

        let options = params.chunk.unwrap_or_default();
        let parsed_map: Option<ChunkMap> = match params.map {
            None => None,
            Some(map) if map.is_empty() => None,
            Some(map) => Some(
                serde_json::from_value(Value::Object(map))
                    .map_err(|e| format!("invalid `map` for resolve: {e}"))?,
            ),
        };
        let result = resolve_ref(
            id,
            parsed_map.as_ref(),
            params.text.as_deref(),
            &options,
            &self.config,
        );
        self.record_call("resolve");
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_count_tokens(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let result = count_tokens_detailed(&text, &self.config);
        self.record_call("count_tokens");
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_stats(&self, params: GatewayParams) -> Result<Value, String> {
        let reset = params.reset.unwrap_or(false);
        let result = if let Ok(mut state) = self.state.lock() {
            let snap = state.stats.snapshot();
            if reset {
                state.stats.clear();
            }
            snap
        } else {
            SessionStats::default()
        };
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_cache_store(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let options = params.cache.unwrap_or_default();
        let result = if let Ok(mut state) = self.state.lock() {
            state.cache.store(&text, &options, &self.config)
        } else {
            CacheStore::default().store(&text, &options, &self.config)
        };
        self.record("cache_store", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_cache_get(&self, params: GatewayParams) -> Result<Value, String> {
        let key = params
            .key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "cache_get requires `key`".to_string())?;
        let result = if let Ok(mut state) = self.state.lock() {
            state.cache.get(key, &self.config)
        } else {
            return Ok(json!({
                "key": key,
                "content": "",
                "tokens": 0,
                "bytes": 0,
                "hit": false,
            }));
        };
        self.record_call("cache_get");
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_cache_invalidate(&self, params: GatewayParams) -> Result<Value, String> {
        let result = if let Ok(mut state) = self.state.lock() {
            state.cache.invalidate(params.key.as_deref())
        } else {
            return Ok(json!({ "removed": 0, "keys": [] }));
        };
        self.record_call("cache_invalidate");
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_brief(&self, params: GatewayParams) -> Result<Value, String> {
        let query = params
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "brief requires `query` (task description)".to_string())?;

        let options = params.brief.unwrap_or_default();
        let hint = params
            .text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let mut result = brief(query, hint, &options, &self.config)?;

        // Always sanitize briefing payload (plan default); extra pass if sanitize_input.
        if params.sanitize_input.unwrap_or(false) {
            let scrub = sanitize(
                &result.briefing,
                &params.sanitize.unwrap_or_default(),
                &self.config,
            );
            result.briefing = scrub.content;
        }

        let store_opts = CacheStoreOptions {
            key: Some(result.cache_key.clone()),
            ttl_secs: None,
            preview_chars: 160,
        };
        if let Ok(mut state) = self.state.lock() {
            let _ = state
                .cache
                .store(&result.briefing, &store_opts, &self.config);
        }

        self.record("brief", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_catalog(&self, _params: GatewayParams) -> Result<Value, String> {
        let playbooks = playbook_ads_json(&self.config);
        let catalog = catalog_json(&playbooks);
        let markdown = catalog_markdown(&playbook_catalog_lines(&self.config));
        self.record_call("catalog");
        Ok(json!({
            "catalog": catalog,
            "markdown": markdown,
            "index_uri": "cmp://skill/index",
        }))
    }

    fn act_help(&self, params: GatewayParams) -> Result<Value, String> {
        let id = params
            .id
            .as_deref()
            .or(params.key.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "help requires `id` (action name)".to_string())?;
        let id = id.strip_prefix("cmp://skill/action/").unwrap_or(id);
        let help = help_for(id)?;
        let markdown = help_markdown(id)?;
        self.record_call("help");
        Ok(json!({
            "help": help,
            "markdown": markdown,
        }))
    }

    fn act_playbooks(&self, _params: GatewayParams) -> Result<Value, String> {
        let playbooks = list_playbooks(&self.config);
        self.record_call("playbooks");
        Ok(json!({ "playbooks": playbooks }))
    }

    fn act_playbook(&self, params: GatewayParams) -> Result<Value, String> {
        let id = params
            .id
            .as_deref()
            .or(params.key.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "playbook requires `id`".to_string())?;
        let id = id.strip_prefix("cmp://skill/playbook/").unwrap_or(id);
        let pb = get_playbook(id, &self.config)?;
        self.record_call("playbook");
        Ok(serde_json::to_value(pb).map_err(|e| e.to_string())?)
    }

    fn act_pack(&self, params: GatewayParams) -> Result<Value, String> {
        let options = params.pack.clone().unwrap_or_default();
        let items = if let Some(items) = params.items.as_ref() {
            // Reuse rerank items as pack files when path-like ids present; else treat id as path.
            let mut out = Vec::new();
            for it in items {
                let path = it
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("item-{}.txt", out.len()));
                out.push(PackItem {
                    path,
                    text: it.text.clone(),
                });
            }
            out
        } else {
            let text = Self::require_text(&params)?;
            parse_pack_text(&text)?
        };
        let mut result = pack_items(&items, &options, &self.config)?;
        if options.store_in_cache {
            if let Ok(mut state) = self.state.lock() {
                let tokens = result.metrics.result_tokens;
                state.cache.put_raw(
                    result.cache_key.clone(),
                    // Store raw base64 so unpack can decode from cache_get text.
                    {
                        use base64::{engine::general_purpose::STANDARD as B64, Engine};
                        B64.encode(&result.zip_bytes)
                    },
                    tokens,
                );
            }
        }
        self.record("pack", &result.metrics);
        // Drop raw bytes from JSON payload.
        result.zip_bytes.clear();
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    fn act_unpack(&self, params: GatewayParams) -> Result<Value, String> {
        let options = params.pack.clone().unwrap_or_default();
        let zip_bytes = if let Some(key) = params
            .key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let content = if let Ok(mut state) = self.state.lock() {
                state.cache.get(key, &self.config).content
            } else {
                String::new()
            };
            if content.is_empty() {
                return Err(format!("cache miss for pack key `{key}`"));
            }
            decode_archive_bytes(&content)?
        } else {
            let text = Self::require_text(&params)?;
            decode_archive_bytes(&text)?
        };

        let result = unpack_bytes(&zip_bytes, &options, &self.config)?;
        self.cache_chunks(&result.chunks);
        self.record("unpack", &result.metrics);
        Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
    }

    /// Resolve MCP resource URI to markdown/JSON text.
    fn read_skill_resource(&self, uri: &str) -> Result<(String, String), McpError> {
        let uri = uri.trim();
        if uri == "cmp://skill/index" {
            let catalog = catalog_json(&playbook_ads_json(&self.config));
            let index = skill_index_json(&self.config, catalog);
            let body = serde_json::to_string_pretty(&index).unwrap_or_else(|_| "{}".into());
            return Ok(("application/json".into(), body));
        }
        if let Some(id) = parse_action_uri(uri) {
            let md = help_markdown(id).map_err(|e| McpError::resource_not_found(e, None))?;
            return Ok(("text/markdown".into(), md));
        }
        if let Some(id) = parse_playbook_uri(uri) {
            let pb = get_playbook(id, &self.config)
                .map_err(|e| McpError::resource_not_found(e, None))?;
            let body = format!(
                "# {}\n\n{}\n\n---\n\n{}\n",
                pb.name, pb.description, pb.body
            );
            return Ok(("text/markdown".into(), body));
        }
        Err(McpError::resource_not_found(
            format!("unknown resource uri: {uri}"),
            None,
        ))
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
                    .with_description(ad.one_liner)
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
    /// Token-optimization gateway — one tool, many actions via `action`.
    #[tool(
        name = "compendium",
        description = "Token-optimization gateway. Set `action` (filter, compress, brief, catalog, help, playbooks, pack/unpack, …). Prefer action=catalog or MCP resources (cmp://skill/…) for details; action=help+id for one action. Pass text/query/messages/key/id/items as required.",
        icons = mcp_icons()
    )]
    fn compendium(
        &self,
        Parameters(params): Parameters<GatewayParams>,
    ) -> Json<GatewayEnvelope> {
        Json(self.dispatch(params))
    }
}

#[tool_handler(
    name = "compendium",
    instructions = "Call `compendium` with `action`. Prefer catalog/help or MCP resources cmp://skill/… for details. Quick map: noisy logs→filter/compress_output; untrusted→sanitize; relevance→filter_relevant/rerank; workspace start→brief; bulky→compress/cache_store/chunk+resolve; long chat→prune_history(afm); recipes→playbooks/playbook; archives→pack/unpack (size-capped, never runs scripts); measure→count_tokens/stats."
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
                    "MCP gateway that compresses, filters, and packs context to cut LLM tokens",
                )
                .with_icons(mcp_icons())
                .with_website_url(WEBSITE_URL),
        )
        .with_instructions(
            "Call `compendium` with `action`. Prefer catalog/help or MCP resources cmp://skill/… for details. Quick map: noisy logs→filter/compress_output; untrusted→sanitize; relevance→filter_relevant/rerank; workspace start→brief; bulky→compress/cache_store/chunk+resolve; long chat→prune_history(afm); recipes→playbooks/playbook; archives→pack/unpack (size-capped, never runs scripts); measure→count_tokens/stats."
                .to_string(),
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(
            self.list_skill_resources(),
        )))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ReadResourceResponse, McpError>> + Send + '_ {
        let result = self.read_skill_resource(&request.uri).map(|(mime, text)| {
            ReadResourceResult::new(vec![
                ResourceContents::text(text, request.uri.clone()).with_mime_type(mime),
            ])
            .into()
        });
        std::future::ready(result)
    }
}
