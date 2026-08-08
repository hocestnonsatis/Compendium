//! Progressive-disclosure action catalog: short ads up front, full help on demand.
//!
//! URI scheme: `cmp://skill/action/{name}`, plus `cmp://skill/index` for the full index.

use serde::Serialize;
use serde_json::{json, Value};

/// One action advertisement (token-cheap).
#[derive(Debug, Clone, Serialize)]
pub struct ActionAd {
    pub id: &'static str,
    pub one_liner: &'static str,
    pub when_to_use: &'static str,
    pub fields: &'static str,
    pub uri: &'static str,
}

/// Full help payload for one action.
#[derive(Debug, Clone, Serialize)]
pub struct ActionHelp {
    pub id: &'static str,
    pub one_liner: &'static str,
    pub when_to_use: &'static str,
    pub fields: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<&'static str>,
    pub uri: String,
    /// `compressed` (default) or `full`.
    pub fidelity: &'static str,
}

const ACTIONS: &[ActionAd] = &[
    ActionAd {
        id: "filter",
        one_liner: "Strip ANSI/boilerplate; densify JSON; keep/drop regexes",
        when_to_use: "Noisy terminal logs — not for cargo/npm dumps (use compress_output)",
        fields: "text, filter?, query?",
        uri: "cmp://skill/action/filter",
    },
    ActionAd {
        id: "compress",
        one_liner: "Dense semantic compression of text/code/logs",
        when_to_use: "Bulky blobs >~1000 chars; small inputs bypass unless force=true",
        fields: "text, compress?",
        uri: "cmp://skill/action/compress",
    },
    ActionAd {
        id: "compress_output",
        one_liner: "Domain-aware stdout/stderr scrub (git/cargo/npm/docker/…)",
        when_to_use: "CLI tool dumps — prefer over filter when domain is known",
        fields: "text, output?",
        uri: "cmp://skill/action/compress_output",
    },
    ActionAd {
        id: "summarize",
        one_liner: "Hierarchical summary (conversation / file tree / outline)",
        when_to_use: "Long structured text that needs an outline",
        fields: "text, summarize?",
        uri: "cmp://skill/action/summarize",
    },
    ActionAd {
        id: "summarize_smart",
        one_liner: "Local-SLM dense summary (heuristic fallback)",
        when_to_use: "When COMPENDIUM_LOCAL_LLM_URL is set and semantics matter",
        fields: "text, smart?, summarize?",
        uri: "cmp://skill/action/summarize_smart",
    },
    ActionAd {
        id: "filter_relevant",
        one_liner: "Query-aware keep of relevant lines",
        when_to_use: "You know the question the log must answer",
        fields: "text, query, smart?",
        uri: "cmp://skill/action/filter_relevant",
    },
    ActionAd {
        id: "prune_history",
        one_liner: "Drop filler / compress older chat turns (AFM)",
        when_to_use: "Long chat transcripts eating context",
        fields: "text|messages, prune?",
        uri: "cmp://skill/action/prune_history",
    },
    ActionAd {
        id: "chunk",
        one_liner: "Split corpus into cmp:// chunks",
        when_to_use: "Huge file/corpus you may revisit piece by piece",
        fields: "text, chunk?",
        uri: "cmp://skill/action/chunk",
    },
    ActionAd {
        id: "resolve",
        one_liner: "Fetch chunk content by id",
        when_to_use: "After chunk — load only needed cmp:// ids",
        fields: "id, map?, text?, chunk?",
        uri: "cmp://skill/action/resolve",
    },
    ActionAd {
        id: "count_tokens",
        one_liner: "Count tokens with the active backend",
        when_to_use: "Measure size before/after a pipeline step",
        fields: "text",
        uri: "cmp://skill/action/count_tokens",
    },
    ActionAd {
        id: "stats",
        one_liner: "Session savings + latency/backend telemetry",
        when_to_use: "Savings look wrong, or CE/hybrid backend unclear — see playbook stats-debug",
        fields: "reset?",
        uri: "cmp://skill/action/stats",
    },
    ActionAd {
        id: "cache_store",
        one_liner: "Park bulky payload outside the prompt",
        when_to_use: "Keep only a cache:// key; set COMPENDIUM_CACHE_DIR to survive restarts",
        fields: "text, cache?",
        uri: "cmp://skill/action/cache_store",
    },
    ActionAd {
        id: "cache_get",
        one_liner: "Retrieve cached text / chunk by key",
        when_to_use: "Expand a prior cache:// or cmp:// key",
        fields: "key",
        uri: "cmp://skill/action/cache_get",
    },
    ActionAd {
        id: "cache_invalidate",
        one_liner: "Drop one cache key or clear the session cache",
        when_to_use: "Evict stale parked blobs",
        fields: "key?",
        uri: "cmp://skill/action/cache_invalidate",
    },
    ActionAd {
        id: "sanitize",
        one_liner: "Redact secrets + neutralize IPI phrases",
        when_to_use: "Untrusted tool/web output — or set sanitize_input on the next action",
        fields: "text, sanitize?",
        uri: "cmp://skill/action/sanitize",
    },
    ActionAd {
        id: "rerank",
        one_liner: "BM25 (+ embeddings + opt-in SLM cross-encoder) rank for a query",
        when_to_use: "After chunk/brief — pick best candidates; embeddings need loopback LLM",
        fields: "query, items|text|map, rerank?",
        uri: "cmp://skill/action/rerank",
    },
    ActionAd {
        id: "brief",
        one_liner: "Scan workspace; pack Task/Status/Evidence/Caveats/Sources/Read next",
        when_to_use: "Fresh agent turn — then follow Read next skill/playbook URIs",
        fields: "query, brief?, text?",
        uri: "cmp://skill/action/brief",
    },
    ActionAd {
        id: "llm_status",
        one_liner: "Probe local LLM URL / models (why heuristic fallback?)",
        when_to_use:
            "backend=heuristic unexpectedly — check URL/model before retrying smart/hybrid",
        fields: "force? (true = also tiny chat probe)",
        uri: "cmp://skill/action/llm_status",
    },
    ActionAd {
        id: "catalog",
        one_liner: "List short action (+ playbook) advertisements",
        when_to_use: "First call when unsure — do not invent action names",
        fields: "(none)",
        uri: "cmp://skill/action/catalog",
    },
    ActionAd {
        id: "help",
        one_liner: "Full usage notes + example for one action",
        when_to_use: "After catalog — load details for one id",
        fields: "id (or key)",
        uri: "cmp://skill/action/help",
    },
    ActionAd {
        id: "playbooks",
        one_liner: "List token-hygiene playbook advertisements",
        when_to_use: "Need a guided recipe (logs, cargo fail, AFM, …)",
        fields: "(none)",
        uri: "cmp://skill/action/playbooks",
    },
    ActionAd {
        id: "playbook",
        one_liner: "Load one playbook body by id",
        when_to_use: "After playbooks — follow the recipe",
        fields: "id (or key)",
        uri: "cmp://skill/action/playbook",
    },
    ActionAd {
        id: "pack",
        one_liner: "Zip text/files into a bounded archive (cache or base64)",
        when_to_use: "Bundle a multi-file corpus for later unpack",
        fields: "text|items, pack?",
        uri: "cmp://skill/action/pack",
    },
    ActionAd {
        id: "unpack",
        one_liner: "Unpack zip/tar.gz with size caps → chunks (never runs scripts)",
        when_to_use: "Expand a prior pack or trusted archive bytes",
        fields: "text|key, pack?",
        uri: "cmp://skill/action/unpack",
    },
];

/// All action advertisements.
pub fn action_ads() -> &'static [ActionAd] {
    ACTIONS
}

/// Look up an ad by id (case-insensitive).
pub fn find_ad(id: &str) -> Option<&'static ActionAd> {
    let needle = id.trim();
    ACTIONS.iter().find(|a| a.id.eq_ignore_ascii_case(needle))
}

/// Compact catalog JSON (ads only).
pub fn catalog_json(playbook_ads: &[Value]) -> Value {
    json!({
        "actions": ACTIONS,
        "playbooks": playbook_ads,
        "index_uri": "cmp://skill/index",
        "hint": "Default help is compressed (signature+fields). Use force=true or resources/read on cmp://skill/action/<id> for full example+notes."
    })
}

/// Render a short markdown catalog (handy for prompts).
pub fn catalog_markdown(playbook_lines: &[String]) -> String {
    let mut out = String::from("# Compendium action catalog\n\n");
    for a in ACTIONS {
        out.push_str(&format!(
            "- **{}** — {} _(when: {})_ [`{}`]\n",
            a.id, a.one_liner, a.when_to_use, a.uri
        ));
    }
    if !playbook_lines.is_empty() {
        out.push_str("\n# Playbooks\n\n");
        for line in playbook_lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("\nLoad details: `action=help` + `id`, or MCP `resources/read` on the uri.\n");
    out
}

/// Help for one action id.
///
/// Default is **compressed** (signature + fields only). Pass `full=true` for example+notes.
pub fn help_for(id: &str, full: bool) -> Result<ActionHelp, String> {
    let ad = find_ad(id)
        .ok_or_else(|| format!("unknown action `{id}`; call action=catalog for the list"))?;
    if full {
        Ok(ActionHelp {
            id: ad.id,
            one_liner: ad.one_liner,
            when_to_use: ad.when_to_use,
            fields: ad.fields,
            example: Some(example_for(ad.id)),
            notes: notes_for(ad.id),
            uri: ad.uri.to_string(),
            fidelity: "full",
        })
    } else {
        Ok(ActionHelp {
            id: ad.id,
            one_liner: ad.one_liner,
            when_to_use: ad.when_to_use,
            fields: ad.fields,
            example: None,
            notes: vec![],
            uri: ad.uri.to_string(),
            fidelity: "compressed",
        })
    }
}

/// Parse `cmp://skill/action/{name}` → action id.
pub fn parse_action_uri(uri: &str) -> Option<&str> {
    let uri = uri.trim();
    let prefix = "cmp://skill/action/";
    uri.strip_prefix(prefix)
        .filter(|s| !s.is_empty() && !s.contains('/'))
}

fn example_for(id: &str) -> Value {
    match id {
        "filter" => json!({
            "action": "filter",
            "text": "…noisy log…",
            "filter": { "strip_ansi": true, "keep_patterns": ["ERROR|WARN"] }
        }),
        "compress" => json!({
            "action": "compress",
            "text": "…large blob…",
            "compress": { "force": false, "max_tokens": 800 }
        }),
        "compress_output" => json!({
            "action": "compress_output",
            "text": "…cargo test stdout…",
            "output": { "domain": "cargo" }
        }),
        "summarize" => json!({
            "action": "summarize",
            "text": "…long doc…",
            "summarize": { "mode": "outline" }
        }),
        "summarize_smart" => json!({
            "action": "summarize_smart",
            "text": "…long doc…",
            "smart": { "fallback": true }
        }),
        "filter_relevant" => json!({
            "action": "filter_relevant",
            "query": "why did the build fail?",
            "text": "…full log…"
        }),
        "prune_history" => json!({
            "action": "prune_history",
            "messages": [
                {"role": "user", "content": "…"},
                {"role": "assistant", "content": "…"}
            ],
            "prune": { "strategy": "afm", "keep_last_n": 4 }
        }),
        "chunk" => json!({
            "action": "chunk",
            "text": "…huge corpus…",
            "chunk": { "chunk_tokens": 512 }
        }),
        "resolve" => json!({
            "action": "resolve",
            "id": "cmp://abcd1234/0"
        }),
        "count_tokens" => json!({ "action": "count_tokens", "text": "…" }),
        "stats" => json!({ "action": "stats", "reset": false }),
        "cache_store" => json!({
            "action": "cache_store",
            "text": "…bulky…",
            "cache": { "ttl_secs": 3600 }
        }),
        "cache_get" => json!({ "action": "cache_get", "key": "cache://…" }),
        "cache_invalidate" => json!({ "action": "cache_invalidate", "key": "cache://…" }),
        "sanitize" => json!({
            "action": "sanitize",
            "text": "…untrusted…",
            "sanitize": { "redact_secrets": true, "neutralize_ipi": true }
        }),
        "rerank" => json!({
            "action": "rerank",
            "query": "auth middleware",
            "items": [{ "id": "a", "text": "…" }, { "id": "b", "text": "…" }],
            "rerank": { "top_k": 3, "use_embeddings": true, "alpha": 0.55, "use_cross_encoder": true, "cross_encoder_top_n": 8 }
        }),
        "llm_status" => json!({ "action": "llm_status", "force": false }),
        "brief" => json!({
            "action": "brief",
            "query": "how does the MCP gateway dispatch actions?",
            "brief": { "root": "/path/to/repo", "max_files": 40 }
        }),
        "catalog" => json!({ "action": "catalog" }),
        "help" => json!({ "action": "help", "id": "brief" }),
        "playbooks" => json!({ "action": "playbooks" }),
        "playbook" => json!({ "action": "playbook", "id": "noisy-logs" }),
        "pack" => json!({
            "action": "pack",
            "text": "filename.md\n---\nbody…",
            "pack": { "store_in_cache": true }
        }),
        "unpack" => json!({
            "action": "unpack",
            "key": "cache://pack/…",
            "pack": { "max_files": 50 }
        }),
        _ => json!({ "action": id }),
    }
}

fn notes_for(id: &str) -> Vec<&'static str> {
    match id {
        "compress" | "summarize" | "summarize_smart" => vec![
            "Soft payloads under COMPENDIUM_SIGNAL_MIN_CHARS (default 1000) bypass unless force=true.",
        ],
        "brief" => vec![
            "Returns structured briefing + cache_key; sanitized by default.",
            "Optional COMPENDIUM_BRIEF_ROOT restricts allowed roots.",
            "Read next may include skill URIs (cmp://skill/…).",
            "Chunk rerank uses hybrid BM25+embeddings when COMPENDIUM_LOCAL_LLM_URL is set.",
        ],
        "rerank" => vec![
            "Default: try loopback embeddings (COMPENDIUM_LOCAL_EMBED_MODEL or chat model) and blend with BM25.",
            "use_embeddings=false forces pure BM25; alpha / COMPENDIUM_HYBRID_ALPHA sets BM25 weight (default 0.55).",
            "Opt-in SLM cross-encoder: use_cross_encoder=true or COMPENDIUM_RERANK_CROSS_ENCODER=1; top-N via cross_encoder_top_n / COMPENDIUM_CROSS_ENCODER_TOP_N (default 16).",
            "CE prefers POST /v1/rerank when available (cross_encoder_mode=rerank_api); else pairwise chat. Partial pair failures keep prior scores (backend cross_encoder_partial).",
            "Without a reachable local LLM, backend stays bm25/hybrid with fallback_reason.",
        ],
        "llm_status" => vec![
            "Reports whether COMPENDIUM_LOCAL_LLM_URL is set and reachable (GET /models).",
            "force=true also runs a tiny chat completion probe (may load the model).",
        ],
        "prune_history" => vec![
            "strategy=afm is preferred: Critical / Thematic / Distant tiers.",
        ],
        "unpack" | "pack" => vec![
            "Archives are untrusted: size caps apply; scripts are never executed.",
        ],
        "help" | "catalog" => vec![
            "Also available as MCP resources under cmp://skill/…",
        ],
        _ => vec![],
    }
}

/// Markdown body for resources/read or help. `full` controls example/notes.
pub fn help_markdown(id: &str, full: bool) -> Result<String, String> {
    let h = help_for(id, full)?;
    if !full {
        return Ok(format!(
            "# Action: {id}\n\n{one}\n\n**When:** {when}\n\n**Fields:** `{fields}`\n\n**URI:** `{uri}`\n\n_fidelity: compressed — pass force=true or resources/read for full example_\n",
            id = h.id,
            one = h.one_liner,
            when = h.when_to_use,
            fields = h.fields,
            uri = h.uri,
        ));
    }
    let example = h
        .example
        .as_ref()
        .map(|e| serde_json::to_string_pretty(e).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "{}".into());
    let notes = if h.notes.is_empty() {
        String::new()
    } else {
        format!(
            "\n## Notes\n{}\n",
            h.notes
                .iter()
                .map(|n| format!("- {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    Ok(format!(
        "# Action: {id}\n\n{one}\n\n**When:** {when}\n\n**Fields:** `{fields}`\n\n**URI:** `{uri}`\n\n## Example\n```json\n{example}\n```\n{notes}",
        id = h.id,
        one = h.one_liner,
        when = h.when_to_use,
        fields = h.fields,
        uri = h.uri,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_core_actions() {
        assert!(find_ad("brief").is_some());
        assert!(find_ad("FILTER").is_some());
        assert!(help_for("chunk", false).is_ok());
        let compressed = help_for("brief", false).unwrap();
        assert_eq!(compressed.fidelity, "compressed");
        assert!(compressed.example.is_none());
        let full = help_for("brief", true).unwrap();
        assert_eq!(full.fidelity, "full");
        assert!(full.example.is_some());
        assert!(help_for("nope", false).is_err());
    }

    #[test]
    fn parse_action_uri_works() {
        assert_eq!(parse_action_uri("cmp://skill/action/brief"), Some("brief"));
        assert_eq!(parse_action_uri("cmp://skill/index"), None);
    }
}
