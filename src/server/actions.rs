//! Gateway `action` handlers (`act_*`).

use serde_json::{json, Value};

use crate::pipeline::{
    brief::brief,
    cache::CacheStoreOptions,
    catalog::{catalog_json, catalog_markdown, help_for, help_markdown},
    chunk::{chunk_with_refs, resolve_ref, ChunkMap},
    compress::compress,
    filter::filter,
    local_llm::llm_status,
    output::compress_output,
    pack::{decode_archive_bytes, pack_items, parse_pack_text, unpack_bytes, PackItem},
    playbook::{get_playbook, list_playbooks, playbook_ads_json, playbook_catalog_lines},
    prune::{parse_history_input, prune_history},
    rerank::{parse_rerank_items, RerankItem},
    sanitize::sanitize,
    smart::{filter_relevant, summarize_smart},
    stats::SessionStats,
    summarize::summarize,
    tokens::count_tokens_detailed,
};

use super::{CacheStore, CompendiumServer, GatewayParams};

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
        let options = params.compress.unwrap_or_default();
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
        let options = params.summarize.unwrap_or_default();
        let result = summarize(&text, &options, &self.config);
        self.record("summarize", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_summarize_smart(&self, params: GatewayParams) -> Result<Value, String> {
        let text = self.maybe_sanitize_text(Self::require_text(&params)?, &params);
        let smart = params.smart.unwrap_or_default();
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

    pub(super) fn act_prune(&self, params: GatewayParams) -> Result<Value, String> {
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
                let id = c.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
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
        let result = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "session state lock poisoned".to_string())?;
            crate::pipeline::rerank::rerank_with_cache(
                query,
                &items,
                &options,
                &self.config,
                Some(&mut state.cache),
            )
        };
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
            snap.cache_disk_loads = cache.disk_loads;
            snap.cache_evictions = cache.evictions;
            snap.cache_entries = cache.entries;
            snap.cache_bytes = cache.bytes;
            snap.cache_dir = cache.disk_dir;
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
        let result = if let Ok(mut state) = self.state.lock() {
            state.cache.store(&text, &options, &self.config)
        } else {
            CacheStore::default().store(&text, &options, &self.config)
        };
        self.record("cache_store", &result.metrics);
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_cache_get(&self, params: GatewayParams) -> Result<Value, String> {
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_cache_invalidate(&self, params: GatewayParams) -> Result<Value, String> {
        let result = if let Ok(mut state) = self.state.lock() {
            state.cache.invalidate(params.key.as_deref())
        } else {
            return Ok(json!({ "removed": 0, "keys": [] }));
        };
        self.record_call("cache_invalidate");
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_brief(&self, params: GatewayParams) -> Result<Value, String> {
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_catalog(&self, _params: GatewayParams) -> Result<Value, String> {
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

    pub(super) fn act_help(&self, params: GatewayParams) -> Result<Value, String> {
        let id = params
            .id
            .as_deref()
            .or(params.key.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "help requires `id` (action name)".to_string())?;
        let id = id.strip_prefix("cmp://skill/action/").unwrap_or(id);
        let full = params.force.unwrap_or(false);
        let help = help_for(id, full)?;
        let markdown = help_markdown(id, full)?;
        self.record_call("help");
        Ok(json!({
            "help": help,
            "markdown": markdown,
            "fidelity": if full { "full" } else { "compressed" },
        }))
    }

    pub(super) fn act_playbooks(&self, _params: GatewayParams) -> Result<Value, String> {
        let playbooks = list_playbooks(&self.config);
        self.record_call("playbooks");
        Ok(json!({ "playbooks": playbooks }))
    }

    pub(super) fn act_playbook(&self, params: GatewayParams) -> Result<Value, String> {
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
        serde_json::to_value(pb).map_err(|e| e.to_string())
    }

    pub(super) fn act_pack(&self, params: GatewayParams) -> Result<Value, String> {
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_unpack(&self, params: GatewayParams) -> Result<Value, String> {
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
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    pub(super) fn act_llm_status(&self, params: GatewayParams) -> Result<Value, String> {
        // `force: true` also probes a tiny chat completion (slower / loads model).
        let check_chat = params.force.unwrap_or(false);
        let result = llm_status(&self.config.local_llm, check_chat);
        self.record_call("llm_status");
        serde_json::to_value(result).map_err(|e| e.to_string())
    }
}
