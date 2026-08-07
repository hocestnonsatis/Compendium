//! Conversation history pruning — filler drop, compress, and Adaptive Focus Memory (AFM).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pipeline::summarize::{summarize, SummarizeMode, SummarizeOptions};
use crate::pipeline::tokens::estimate_tokens;
use crate::pipeline::TokenMetrics;

/// One chat turn.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// Pruning strategy.
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PruneStrategy {
    /// Drop short acknowledgement / filler turns only.
    Remove,
    /// Summarize turns older than `keep_last_n` into one block.
    Compress,
    /// Remove filler, then compress older remaining turns.
    #[default]
    Hybrid,
    /// Adaptive Focus Memory: Critical (full) / Thematic (compressed) / Distant (placeholder+ref).
    Afm,
}

/// Options for [`prune_history`].
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PruneOptions {
    #[serde(default)]
    pub strategy: PruneStrategy,
    /// Keep the last N messages verbatim as the Critical tier (after filler removal).
    #[serde(default = "default_keep_last")]
    pub keep_last_n: usize,
    /// Soft cap on output tokens.
    pub max_output_tokens: Option<usize>,
    /// AFM: max messages folded into the Thematic (compressed) band. Default = `keep_last_n * 3`.
    pub thematic_n: Option<usize>,
}

fn default_keep_last() -> usize {
    4
}

impl Default for PruneOptions {
    fn default() -> Self {
        Self {
            strategy: PruneStrategy::Hybrid,
            keep_last_n: 4,
            max_output_tokens: None,
            thematic_n: None,
        }
    }
}

/// AFM tier summary for observability.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AfmTier {
    /// `critical` | `thematic` | `distant`
    pub name: String,
    pub message_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_key: Option<String>,
}

/// Result of pruning.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PruneResult {
    pub messages: Vec<HistoryMessage>,
    pub rendered: String,
    pub dropped: usize,
    pub compressed: bool,
    pub metrics: TokenMetrics,
    /// Present when `strategy=afm`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<AfmTier>,
    /// Cache key for distant tier blob (AFM). Server stores payload under this key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distant_key: Option<String>,
    /// Full distant transcript for session cache — omitted from agent-facing JSON.
    #[serde(skip)]
    pub distant_payload: Option<String>,
}

/// Prune a conversation history for a smaller context window.
pub fn prune_history(
    messages: &[HistoryMessage],
    options: &PruneOptions,
    config: &Config,
) -> PruneResult {
    let original = render_messages(messages);
    let original_tokens = estimate_tokens(&original, config);

    let (without_filler, dropped_filler) = match options.strategy {
        PruneStrategy::Remove | PruneStrategy::Hybrid | PruneStrategy::Afm => {
            remove_filler(messages)
        }
        PruneStrategy::Compress => (messages.to_vec(), 0),
    };

    let (mut final_msgs, compressed, tiers, distant_key, distant_payload) = match options.strategy {
        PruneStrategy::Remove => (without_filler, false, Vec::new(), None, None),
        PruneStrategy::Compress | PruneStrategy::Hybrid => {
            let (msgs, compressed) = compress_older(&without_filler, options.keep_last_n, config);
            (msgs, compressed, Vec::new(), None, None)
        }
        PruneStrategy::Afm => apply_afm(&without_filler, options, config),
    };

    let mut rendered = render_messages(&final_msgs);
    if let Some(max) = options.max_output_tokens {
        if estimate_tokens(&rendered, config) > max {
            while final_msgs.len() > 1 && estimate_tokens(&rendered, config) > max {
                final_msgs.remove(0);
                rendered = render_messages(&final_msgs);
            }
            if estimate_tokens(&rendered, config) > max {
                rendered = truncate_chars(&rendered, max, config);
            }
        }
    }

    let result_tokens = estimate_tokens(&rendered, config);
    PruneResult {
        messages: final_msgs,
        rendered,
        dropped: dropped_filler,
        compressed,
        metrics: TokenMetrics::new(original_tokens, result_tokens),
        tiers,
        distant_key,
        distant_payload,
    }
}

/// Parse either a JSON array of messages or a role-prefixed transcript.
pub fn parse_history_input(text: &str) -> Vec<HistoryMessage> {
    let trimmed = text.trim();
    if trimmed.starts_with('[') {
        if let Ok(msgs) = serde_json::from_str::<Vec<HistoryMessage>>(trimmed) {
            if !msgs.is_empty() {
                return msgs;
            }
        }
    }
    parse_transcript(trimmed)
}

fn parse_transcript(text: &str) -> Vec<HistoryMessage> {
    let mut msgs = Vec::new();
    let mut role = String::from("user");
    let mut buf = String::new();

    let flush = |role: &str, buf: &mut String, msgs: &mut Vec<HistoryMessage>| {
        let content = buf.trim().to_string();
        if !content.is_empty() {
            msgs.push(HistoryMessage {
                role: role.to_string(),
                content,
            });
        }
        buf.clear();
    };

    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let matched = ["user:", "assistant:", "system:", "human:", "ai:"]
            .iter()
            .find(|p| lower.starts_with(*p));
        if let Some(prefix) = matched {
            flush(&role, &mut buf, &mut msgs);
            role = match *prefix {
                "assistant:" | "ai:" => "assistant".into(),
                "system:" => "system".into(),
                _ => "user".into(),
            };
            let rest = line[prefix.len()..].trim_start();
            if !rest.is_empty() {
                buf.push_str(rest);
                buf.push('\n');
            }
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    flush(&role, &mut buf, &mut msgs);
    if msgs.is_empty() && !text.trim().is_empty() {
        msgs.push(HistoryMessage {
            role: "user".into(),
            content: text.to_string(),
        });
    }
    msgs
}

fn remove_filler(messages: &[HistoryMessage]) -> (Vec<HistoryMessage>, usize) {
    let mut out = Vec::with_capacity(messages.len());
    let mut dropped = 0usize;
    for m in messages {
        if is_filler(&m.content) {
            dropped += 1;
            continue;
        }
        out.push(m.clone());
    }
    (out, dropped)
}

fn is_filler(content: &str) -> bool {
    let t = content.trim().to_ascii_lowercase();
    if t.is_empty() {
        return true;
    }
    let fillers = [
        "ok",
        "okay",
        "sure",
        "thanks",
        "thank you",
        "thx",
        "got it",
        "sounds good",
        "cool",
        "great",
        "nice",
        "yep",
        "yeah",
        "yes",
        "no problem",
        "np",
        "👍",
        "lgtm",
        "done",
        "continue",
        "go ahead",
        "please continue",
    ];
    if fillers.iter().any(|f| t == *f) {
        return true;
    }
    t.chars().count() <= 12 && !t.contains('?') && fillers.iter().any(|f| t.starts_with(f))
}

fn compress_older(
    messages: &[HistoryMessage],
    keep_last_n: usize,
    config: &Config,
) -> (Vec<HistoryMessage>, bool) {
    if messages.len() <= keep_last_n {
        return (messages.to_vec(), false);
    }
    let split = messages.len().saturating_sub(keep_last_n);
    let older = &messages[..split];
    let recent = messages[split..].to_vec();

    let older_text = render_messages(older);
    let summary = summarize_band(&older_text, config);

    let mut out = Vec::with_capacity(1 + recent.len());
    out.push(HistoryMessage {
        role: "system".into(),
        content: format!("[earlier conversation summary]\n{}", summary.trim()),
    });
    out.extend(recent);
    (out, true)
}

/// Adaptive Focus Memory tiers.
fn apply_afm(
    messages: &[HistoryMessage],
    options: &PruneOptions,
    config: &Config,
) -> (
    Vec<HistoryMessage>,
    bool,
    Vec<AfmTier>,
    Option<String>,
    Option<String>,
) {
    let critical_n = options.keep_last_n.max(1);
    let thematic_n = options
        .thematic_n
        .unwrap_or(critical_n.saturating_mul(3).max(3));

    if messages.len() <= critical_n {
        return (
            messages.to_vec(),
            false,
            vec![AfmTier {
                name: "critical".into(),
                message_count: messages.len(),
                ref_key: None,
            }],
            None,
            None,
        );
    }

    let critical_start = messages.len() - critical_n;
    let critical = messages[critical_start..].to_vec();
    let before_critical = &messages[..critical_start];

    let (distant, thematic) = if before_critical.len() > thematic_n {
        let split = before_critical.len() - thematic_n;
        (&before_critical[..split], &before_critical[split..])
    } else {
        (&[][..], before_critical)
    };

    let mut out = Vec::new();
    let mut tiers = Vec::new();
    let mut distant_key = None;
    let mut distant_payload = None;
    let mut compressed = false;

    if !distant.is_empty() {
        let payload = render_messages(distant);
        let key = format!("cmp://afm/{}", short_hash(&payload));
        let preview: String = payload.chars().take(80).collect();
        out.push(HistoryMessage {
            role: "system".into(),
            content: format!(
                "[distant memory ref={key} — {} turns omitted; resolve via action=cache_get or resolve]\npreview: {preview}…",
                distant.len()
            ),
        });
        tiers.push(AfmTier {
            name: "distant".into(),
            message_count: distant.len(),
            ref_key: Some(key.clone()),
        });
        distant_key = Some(key);
        distant_payload = Some(payload);
        compressed = true;
    }

    if !thematic.is_empty() {
        let thematic_text = render_messages(thematic);
        let summary = summarize_band(&thematic_text, config);
        out.push(HistoryMessage {
            role: "system".into(),
            content: format!(
                "[thematic memory — {} turns compressed]\n{}",
                thematic.len(),
                summary.trim()
            ),
        });
        tiers.push(AfmTier {
            name: "thematic".into(),
            message_count: thematic.len(),
            ref_key: None,
        });
        compressed = true;
    }

    out.extend(critical);
    tiers.push(AfmTier {
        name: "critical".into(),
        message_count: critical_n,
        ref_key: None,
    });

    (out, compressed, tiers, distant_key, distant_payload)
}

fn summarize_band(text: &str, config: &Config) -> String {
    summarize(
        text,
        &SummarizeOptions {
            mode: SummarizeMode::Conversation,
            max_depth: 2,
            max_bullets_per_section: 6,
            max_tokens: Some(config.default_max_tokens.min(512)),
            force: true,
        },
        config,
    )
    .summary
}

fn short_hash(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn render_messages(messages: &[HistoryMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        out.push_str(&m.role);
        out.push_str(": ");
        out.push_str(m.content.trim());
        out.push('\n');
    }
    out
}

fn truncate_chars(text: &str, max_tokens: usize, config: &Config) -> String {
    let approx = (max_tokens as f64 * config.chars_per_token) as usize;
    let mut end = approx.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_filler_keeps_substance() {
        let msgs = vec![
            HistoryMessage {
                role: "user".into(),
                content: "How do I build?".into(),
            },
            HistoryMessage {
                role: "assistant".into(),
                content: "Run cargo build --release.".into(),
            },
            HistoryMessage {
                role: "user".into(),
                content: "ok".into(),
            },
            HistoryMessage {
                role: "user".into(),
                content: "thanks".into(),
            },
            HistoryMessage {
                role: "user".into(),
                content: "And how do I test?".into(),
            },
        ];
        let result = prune_history(
            &msgs,
            &PruneOptions {
                strategy: PruneStrategy::Remove,
                keep_last_n: 10,
                ..Default::default()
            },
            &Config::default(),
        );
        assert_eq!(result.dropped, 2);
        assert!(result.rendered.contains("build"));
        assert!(!result.rendered.to_ascii_lowercase().contains("\nok\n"));
    }

    #[test]
    fn hybrid_compresses_older() {
        let mut msgs = Vec::new();
        for i in 0..8 {
            msgs.push(HistoryMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("Turn {i}: discuss feature implementation details carefully."),
            });
        }
        let result = prune_history(
            &msgs,
            &PruneOptions {
                strategy: PruneStrategy::Hybrid,
                keep_last_n: 2,
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(result.compressed);
        assert!(result.messages.len() < msgs.len());
        assert!(result.messages[0].role == "system");
        assert!(result.rendered.contains("earlier conversation summary"));
    }

    #[test]
    fn afm_builds_three_tiers() {
        let mut msgs = Vec::new();
        for i in 0..20 {
            msgs.push(HistoryMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("Turn {i}: substantial conversation content about topic {i}."),
            });
        }
        let result = prune_history(
            &msgs,
            &PruneOptions {
                strategy: PruneStrategy::Afm,
                keep_last_n: 4,
                thematic_n: Some(6),
                ..Default::default()
            },
            &Config::default(),
        );
        assert!(result.compressed);
        assert!(result.distant_key.is_some());
        assert!(result.distant_payload.is_some());
        let names: Vec<_> = result.tiers.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"distant"));
        assert!(names.contains(&"thematic"));
        assert!(names.contains(&"critical"));
        assert!(result.rendered.contains("distant memory ref="));
        assert!(result.rendered.contains("thematic memory"));
        // Critical turns remain verbatim
        assert!(result.rendered.contains("Turn 19:"));
    }

    #[test]
    fn parses_transcript() {
        let msgs = parse_history_input("user: hi\nassistant: hello there\nuser: bye");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].role, "assistant");
    }
}
