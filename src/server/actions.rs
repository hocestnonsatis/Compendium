//! Gateway `action` handlers (`act_*`).

use serde_json::{json, Value};

use crate::pipeline::{
    catalog::{catalog_json, catalog_markdown, help_for},
    chunk::{chunk_with_refs, resolve_ref, ChunkMap},
    compress::compress,
    filter::filter,
    local_llm::llm_status,
    output::compress_output,
    rerank::{parse_rerank_items, rerank, RerankItem},
    sanitize::sanitize,
    smart::{filter_relevant, summarize_smart},
    stats::SessionStats,
    summarize::summarize,
    tokens::count_tokens_detailed,
};

use super::{CompendiumServer, GatewayParams};

impl CompendiumServer {
    pub(super) fn act_filter(&self, params: GatewayParams) -> Result<Value, String> {
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_compress(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let mut options = params.compress.unwrap_or_default();
        if params.force.unwrap_or(false) {
            options.force = true;
        }
        let result = compress(&text, &options, &self.config);
        self.record("compress", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_compress_output(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let options = params.output.unwrap_or_default();
        let result = compress_output(&text, &options, &self.config);
        self.record("compress_output", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_summarize(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let mut options = params.summarize.unwrap_or_default();
        if params.force.unwrap_or(false) {
            options.force = true;
        }
        let result = summarize(&text, &options, &self.config);
        self.record("summarize", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_summarize_smart(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let mut smart = params.smart.unwrap_or_default();
        if params.force.unwrap_or(false) {
            smart.force = true;
        }
        let summarize_opts = params.summarize.unwrap_or_default();
        let result = summarize_smart(&text, &smart, &summarize_opts, &self.config)?;
        self.record("summarize_smart", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_filter_relevant(&self, params: GatewayParams) -> Result<Value, String> {
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_sanitize(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let options = params.sanitize.unwrap_or_default();
        let result = sanitize(&text, &options, &self.config);
        self.record("sanitize", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_rerank(&self, params: GatewayParams) -> Result<Value, String> {
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
            let chunks = map
                .get("chunks")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "rerank `map` must contain a `chunks` array".to_string())?;
            let out: Vec<RerankItem> = chunks
                .iter()
                .filter_map(|c| {
                    let text = c.get("content")?.as_str()?.to_string();
                    if text.is_empty() {
                        return None;
                    }
                    let id = c.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                    Some(RerankItem { id, text })
                })
                .collect();
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_chunk(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let options = params.chunk.unwrap_or_default();
        let result = chunk_with_refs(&text, &options, &self.config);
        self.cache_chunks(&result);
        self.record("chunk", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_resolve(&self, params: GatewayParams) -> Result<Value, String> {
        let id = params
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "resolve requires `id` (chunk ref from action=chunk)".to_string())?;

        let cached = self.state.lock().ok().map(|mut state| state.cache.get(id));
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_count_tokens(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let result = count_tokens_detailed(&text, &self.config);
        self.record_call("count_tokens");
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_stats(&self, params: GatewayParams) -> Result<Value, String> {
        let reset = params.reset.unwrap_or(false);
        let result = if let Ok(mut state) = self.state.lock() {
            let mut snap = state.stats.snapshot();
            let cache = state.cache.counters();
            snap.cache_hits = cache.hits;
            snap.cache_misses = cache.misses;
            snap.cache_entries = cache.entries;
            snap.cache_bytes = cache.bytes;
            if reset {
                state.stats.clear();
            }
            snap
        } else {
            SessionStats::default()
        };
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_cache_store(&self, params: GatewayParams) -> Result<Value, String> {
        let text = Self::require_text(&params)?;
        let options = params.cache.unwrap_or_default();
        let mut state = self
            .state
            .lock()
            .map_err(|_| "session state lock poisoned".to_string())?;
        let result = state.cache.store(&text, &options, &self.config);
        state.stats.record("cache_store", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_cache_get(&self, params: GatewayParams) -> Result<Value, String> {
        let key = params
            .key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "cache_get requires `key`".to_string())?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "session state lock poisoned".to_string())?;
        let result = state.cache.get(key);
        state.stats.record_count_only("cache_get");
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_cache_invalidate(&self, params: GatewayParams) -> Result<Value, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "session state lock poisoned".to_string())?;
        let result = state.cache.invalidate(params.key.as_deref());
        state.stats.record_count_only("cache_invalidate");
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_llm_status(&self, params: GatewayParams) -> Result<Value, String> {
        // `force: true` also probes a tiny chat completion (slower / loads model).
        let check_chat = params.force.unwrap_or(false);
        let result = llm_status(&self.config.local_llm, check_chat);
        self.record_call("llm_status");
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_help(&self, params: GatewayParams) -> Result<Value, String> {
        self.record_call("help");
        let id = params
            .id
            .as_deref()
            .or(params.key.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match id {
            None => {
                let mut v = catalog_json();
                v["markdown"] = Value::String(catalog_markdown());
                Ok(v)
            }
            Some(id) => {
                let help = help_for(id)?;
                serde_json::to_value(help).map_err(|e| e.to_string())
            }
        }
    }
}
