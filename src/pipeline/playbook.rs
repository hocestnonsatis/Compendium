//! Bundled + directory-override token-hygiene playbooks (skill-md style).
//!
//! URIs: `cmp://skill/playbook/{id}`

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::pipeline::sanitize::{sanitize, SanitizeOptions};

const EMBEDDED: &[(&str, &str)] = &[
    ("noisy-logs", include_str!("../../playbooks/noisy-logs.md")),
    ("cargo-fail", include_str!("../../playbooks/cargo-fail.md")),
    ("e2e-triage", include_str!("../../playbooks/e2e-triage.md")),
    ("long-chat-afm", include_str!("../../playbooks/long-chat-afm.md")),
    ("workspace-brief", include_str!("../../playbooks/workspace-brief.md")),
];

/// Short playbook advertisement.
#[derive(Debug, Clone, Serialize)]
pub struct PlaybookAd {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub uri: String,
    pub source: String,
}

/// Full playbook after frontmatter parse.
#[derive(Debug, Clone, Serialize)]
pub struct Playbook {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub body: String,
    pub uri: String,
    pub source: String,
}

fn load_store(extra_dir: Option<&Path>) -> BTreeMap<String, Playbook> {
    let mut by_id = BTreeMap::new();
    for (fallback_id, raw) in EMBEDDED {
        if let Some(pb) = parse_playbook(raw, "embedded", Some(fallback_id)) {
            by_id.insert(pb.id.clone(), pb);
        }
    }
    if let Some(dir) = extra_dir {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(&path) {
                    let fallback = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("playbook");
                    if let Some(pb) = parse_playbook(&raw, "dir", Some(fallback)) {
                        by_id.insert(pb.id.clone(), pb);
                    }
                }
            }
        }
    }
    by_id
}

fn store(config: &Config) -> BTreeMap<String, Playbook> {
    load_store(config.playbooks_dir.as_deref())
}

/// List playbook advertisements.
pub fn list_playbooks(config: &Config) -> Vec<PlaybookAd> {
    store(config)
        .into_values()
        .map(|p| PlaybookAd {
            id: p.id,
            name: p.name,
            description: p.description,
            tags: p.tags,
            uri: p.uri,
            source: p.source,
        })
        .collect()
}

/// Ads as JSON values for catalog merge.
pub fn playbook_ads_json(config: &Config) -> Vec<Value> {
    list_playbooks(config)
        .into_iter()
        .filter_map(|a| serde_json::to_value(a).ok())
        .collect()
}

/// Markdown lines for catalog_markdown.
pub fn playbook_catalog_lines(config: &Config) -> Vec<String> {
    list_playbooks(config)
        .into_iter()
        .map(|a| format!("- **{}** — {} [`{}`]", a.id, a.description, a.uri))
        .collect()
}

/// Load one playbook by id (sanitized body).
pub fn get_playbook(id: &str, config: &Config) -> Result<Playbook, String> {
    let needle = id.trim();
    let by_id = store(config);
    let mut pb = by_id
        .get(needle)
        .cloned()
        .or_else(|| {
            by_id
                .values()
                .find(|p| p.id.eq_ignore_ascii_case(needle))
                .cloned()
        })
        .ok_or_else(|| format!("unknown playbook `{needle}`; call action=playbooks"))?;

    let scrub = sanitize(&pb.body, &SanitizeOptions::default(), config);
    pb.body = scrub.content;
    Ok(pb)
}

/// Parse `cmp://skill/playbook/{id}`.
pub fn parse_playbook_uri(uri: &str) -> Option<&str> {
    let uri = uri.trim();
    uri.strip_prefix("cmp://skill/playbook/")
        .filter(|s| !s.is_empty() && !s.contains('/'))
}

/// Rank playbooks for a query (simple token overlap); return top ids.
pub fn suggest_playbooks(query: &str, config: &Config, limit: usize) -> Vec<PlaybookAd> {
    let q = query.to_lowercase();
    let tokens: Vec<&str> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .collect();
    let mut scored: Vec<(i32, PlaybookAd)> = list_playbooks(config)
        .into_iter()
        .map(|ad| {
            let blob = format!(
                "{} {} {} {}",
                ad.id,
                ad.name,
                ad.description,
                ad.tags.join(" ")
            )
            .to_lowercase();
            let mut score = 0i32;
            for t in &tokens {
                if blob.contains(t) {
                    score += 1;
                }
            }
            (score, ad)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    scored.into_iter().take(limit).map(|(_, a)| a).collect()
}

/// Index document for `cmp://skill/index`.
pub fn skill_index_json(config: &Config, action_catalog: Value) -> Value {
    json!({
        "schema": "compendium.skill.index/v1",
        "actions": action_catalog.get("actions").cloned().unwrap_or(json!([])),
        "playbooks": playbook_ads_json(config),
        "uris": {
            "index": "cmp://skill/index",
            "action_template": "cmp://skill/action/{id}",
            "playbook_template": "cmp://skill/playbook/{id}"
        }
    })
}

fn parse_playbook(raw: &str, source: &str, fallback_id: Option<&str>) -> Option<Playbook> {
    let raw = raw.trim_start_matches('\u{feff}');
    let (meta, body) = if raw.starts_with("---") {
        let rest = raw.strip_prefix("---")?;
        let end = rest.find("\n---")?;
        let yaml = &rest[..end];
        let body = rest[end + 4..].trim_start_matches('\n').to_string();
        (parse_simple_frontmatter(yaml), body)
    } else {
        (Frontmatter::default(), raw.to_string())
    };

    let id = meta
        .id
        .or_else(|| fallback_id.map(|s| s.to_string()))
        .filter(|s| !s.is_empty())?;
    let name = meta.name.unwrap_or_else(|| id.clone());
    let description = meta
        .description
        .unwrap_or_else(|| format!("Playbook {id}"));
    let tags = meta.tags;
    let uri = format!("cmp://skill/playbook/{id}");
    Some(Playbook {
        id,
        name,
        description,
        tags,
        body,
        uri,
        source: source.to_string(),
    })
}

#[derive(Default)]
struct Frontmatter {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
}

/// Minimal YAML-ish frontmatter (key: value / key: a, b).
fn parse_simple_frontmatter(yaml: &str) -> Frontmatter {
    let mut fm = Frontmatter::default();
    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
        match key {
            "id" => fm.id = Some(val),
            "name" => fm.name = Some(val),
            "description" => fm.description = Some(val),
            "tags" => {
                fm.tags = val
                    .split(|c| c == ',' || c == ' ')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
            }
            _ => {}
        }
    }
    fm
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn embedded_playbooks_load() {
        let cfg = Config::default();
        let ads = list_playbooks(&cfg);
        assert!(ads.iter().any(|a| a.id == "noisy-logs"));
        let pb = get_playbook("noisy-logs", &cfg).unwrap();
        assert!(pb.body.contains("compress_output") || pb.body.contains("filter"));
    }

    #[test]
    fn suggest_matches_cargo() {
        let cfg = Config::default();
        let hits = suggest_playbooks("cargo test failed assertion", &cfg, 3);
        assert!(hits.iter().any(|h| h.id == "cargo-fail"));
    }
}
