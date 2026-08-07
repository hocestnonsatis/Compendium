//! Opt-in JSONL audit trail for gateway actions (no raw secrets / payloads).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::Config;
use crate::pipeline::TokenMetrics;

#[derive(Debug, Serialize)]
struct AuditLine<'a> {
    ts_unix_ms: u64,
    action: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<&'a str>,
}

/// Append one audit record when `COMPENDIUM_AUDIT_PATH` is set.
///
/// Never writes request/response bodies — only action metadata and token counts.
pub fn record_action(
    config: &Config,
    action: &str,
    ok: bool,
    error: Option<&str>,
    metrics: Option<&TokenMetrics>,
    latency_ms: Option<f64>,
    backend: Option<&str>,
) {
    let Some(path) = config.audit_path.as_ref() else {
        return;
    };
    if let Err(e) = append_line(path, action, ok, error, metrics, latency_ms, backend) {
        tracing::warn!(error = %e, path = %path.display(), "audit write failed");
    }
}

fn append_line(
    path: &Path,
    action: &str,
    ok: bool,
    error: Option<&str>,
    metrics: Option<&TokenMetrics>,
    latency_ms: Option<f64>,
    backend: Option<&str>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let ts_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // Scrub error strings that might echo secrets (keep short).
    let err_short = error.map(|e| {
        let t = e.chars().take(200).collect::<String>();
        // Avoid dumping bearer-like tokens if present in error bodies.
        if t.to_ascii_lowercase().contains("bearer ") || t.contains("sk-") {
            "[redacted error]".to_string()
        } else {
            t
        }
    });
    let line = AuditLine {
        ts_unix_ms,
        action,
        ok,
        error: err_short.as_deref(),
        original_tokens: metrics.map(|m| m.original_tokens),
        result_tokens: metrics.map(|m| m.result_tokens),
        latency_ms,
        backend,
    };
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, &line)?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn writes_jsonl_without_bodies() {
        let dir = std::env::temp_dir().join(format!("compendium-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        let cfg = Config {
            audit_path: Some(path.clone()),
            ..Default::default()
        };
        let metrics = TokenMetrics::new(100, 40);
        record_action(
            &cfg,
            "filter",
            true,
            None,
            Some(&metrics),
            Some(1.5),
            Some("heuristic"),
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"action\":\"filter\""));
        assert!(text.contains("\"original_tokens\":100"));
        assert!(!text.contains("password"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
