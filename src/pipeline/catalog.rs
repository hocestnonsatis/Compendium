//! Action catalog: `help` without `id` lists every action; with `id` returns
//! the fields, an example call, and notes for one action.

use serde::Serialize;
use serde_json::{json, Value};

/// One-line action advertisement.
#[derive(Debug, Clone, Serialize)]
pub struct ActionAd {
    pub id: &'static str,
    pub one_liner: &'static str,
    pub when_to_use: &'static str,
    pub fields: &'static str,
}

/// Full help payload for one action.
#[derive(Debug, Clone, Serialize)]
pub struct ActionHelp {
    pub id: &'static str,
    pub one_liner: &'static str,
    pub when_to_use: &'static str,
    pub fields: &'static str,
    pub example: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<&'static str>,
}

const ACTIONS: &[ActionAd] = &[
    ActionAd {
        id: "filter",
        one_liner: "Strip ANSI/boilerplate; densify JSON; keep/drop regexes",
        when_to_use: "Noisy ANSI/spinner logs — not cargo/npm/docker/git dumps (use compress_output)",
        fields: "text, filter?, query?",
    },
    ActionAd {
        id: "compress",
        one_liner: "Dense representation of text/code/logs",
        when_to_use: "Bulky text/JSON/code — not domain CLI dumps (compress_output); small inputs need force=true",
        fields: "text, compress?",
    },
    ActionAd {
        id: "compress_output",
        one_liner: "Domain-aware stdout/stderr scrub (git/cargo/npm/docker/…)",
        when_to_use: "Known CLI tool dumps — prefer over filter; set output.domain or leave auto",
        fields: "text, output?",
    },
    ActionAd {
        id: "summarize",
        one_liner: "Hierarchical summary (conversation / file tree / outline)",
        when_to_use: "Long structured text that needs an outline",
        fields: "text, summarize?",
    },
    ActionAd {
        id: "summarize_smart",
        one_liner: "Local-SLM dense summary (heuristic fallback)",
        when_to_use: "When COMPENDIUM_LOCAL_LLM_URL is set and semantics matter",
        fields: "text, smart?, summarize?",
    },
    ActionAd {
        id: "filter_relevant",
        one_liner: "Query-aware keep of relevant lines",
        when_to_use: "You know the question the log must answer",
        fields: "text, query, smart?",
    },
    ActionAd {
        id: "sanitize",
        one_liner: "Redact secrets + neutralize prompt-injection phrases",
        when_to_use: "Untrusted tool/web output — or set sanitize_input on the next action",
        fields: "text, sanitize?",
    },
    ActionAd {
        id: "rerank",
        one_liner: "BM25 rank candidates for a query",
        when_to_use: "After chunk — pick the best candidates for a question",
        fields: "query, items|text|map, rerank?",
    },
    ActionAd {
        id: "chunk",
        one_liner: "Split corpus into cmp:// chunks",
        when_to_use: "Huge file/corpus you may revisit piece by piece",
        fields: "text, chunk?",
    },
    ActionAd {
        id: "resolve",
        one_liner: "Fetch chunk content by id",
        when_to_use: "After chunk — load only the cmp:// ids you need",
        fields: "id, map?, text?, chunk?",
    },
    ActionAd {
        id: "cache_store",
        one_liner: "Park a bulky payload outside the prompt",
        when_to_use: "Keep a blob available under a short cache:// key",
        fields: "text, cache?",
    },
    ActionAd {
        id: "cache_get",
        one_liner: "Retrieve cached text / chunk by key",
        when_to_use: "Expand a prior cache:// or cmp:// key",
        fields: "key",
    },
    ActionAd {
        id: "cache_invalidate",
        one_liner: "Drop one cache key or clear the session cache",
        when_to_use: "Evict stale parked blobs",
        fields: "key?",
    },
    ActionAd {
        id: "count_tokens",
        one_liner: "Count tokens with the active backend",
        when_to_use: "Measure size before/after a pipeline step",
        fields: "text",
    },
    ActionAd {
        id: "stats",
        one_liner: "Session token savings per action",
        when_to_use: "Check how much the session saved",
        fields: "reset?",
    },
    ActionAd {
        id: "llm_status",
        one_liner: "Probe the local LLM URL / models",
        when_to_use: "backend=heuristic unexpectedly — check URL/model before retrying",
        fields: "force? (true = also tiny chat probe)",
    },
    ActionAd {
        id: "help",
        one_liner: "List actions, or usage + example for one action",
        when_to_use: "Unsure which action? Call with no id, then with id",
        fields: "id?",
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

/// Compact catalog JSON.
pub fn catalog_json() -> Value {
    json!({
        "actions": ACTIONS,
        "hint": "Call help with `id` for fields, an example call, and notes.",
    })
}

/// Short markdown catalog (handy for prompts).
pub fn catalog_markdown() -> String {
    let mut out = String::from("# Compendium actions\n\n");
    for a in ACTIONS {
        out.push_str(&format!(
            "- **{}** — {} _(when: {})_\n",
            a.id, a.one_liner, a.when_to_use
        ));
    }
    out.push_str("\nDetails: `action=help` + `id`.\n");
    out
}

/// Help for one action id.
pub fn help_for(id: &str) -> Result<ActionHelp, String> {
    let ad = find_ad(id).ok_or_else(|| {
        format!("unknown action `{id}`; call action=help without id for the list")
    })?;
    Ok(ActionHelp {
        id: ad.id,
        one_liner: ad.one_liner,
        when_to_use: ad.when_to_use,
        fields: ad.fields,
        example: example_for(ad.id),
        notes: notes_for(ad.id),
    })
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
        "chunk" => json!({
            "action": "chunk",
            "text": "…huge corpus…",
            "chunk": { "chunk_tokens": 512 }
        }),
        "resolve" => json!({ "action": "resolve", "id": "cmp://abcd1234/0" }),
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
            "rerank": { "top_k": 3 }
        }),
        "llm_status" => json!({ "action": "llm_status", "force": false }),
        "help" => json!({ "action": "help", "id": "filter" }),
        _ => json!({ "action": id }),
    }
}

fn notes_for(id: &str) -> Vec<&'static str> {
    match id {
        "filter" => vec![
            "Generic ANSI/boilerplate strip. For cargo/npm/docker/git/kubectl dumps prefer compress_output.",
            "Optional query (top-level or filter.query) keeps BM25-relevant lines after other filters.",
        ],
        "compress" | "summarize" | "summarize_smart" => vec![
            "Inputs under COMPENDIUM_SIGNAL_MIN_CHARS (default 1000) bypass unless force=true.",
            "compress densifies; summarize outlines. Prefer filter_relevant when you already know the question.",
        ],
        "compress_output" => vec![
            "Domain heuristics beat generic filter for tool dumps. Set output.domain or leave auto.",
        ],
        "rerank" => vec![
            "Pure BM25 — deterministic, no model needed.",
            "Pass a prior chunk `map` to rank chunks by id.",
        ],
        "llm_status" => vec![
            "Reports whether COMPENDIUM_LOCAL_LLM_URL is set and reachable (GET /models).",
            "force=true also runs a tiny chat completion probe (may load the model).",
            "If unset: npx -y compendium-mcp setup-ollama --write-mcp",
        ],
        "sanitize" => vec![
            "Prefer sanitize_input=true on the next action when you still need filter/compress afterward.",
        ],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_core_actions() {
        assert!(find_ad("FILTER").is_some());
        assert!(find_ad("brief").is_none());
        let help = help_for("chunk").unwrap();
        assert_eq!(help.example["action"], "chunk");
        assert!(help_for("nope").is_err());
        assert!(catalog_markdown().contains("compress_output"));
    }
}
