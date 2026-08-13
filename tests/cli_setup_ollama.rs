//! CLI smoke for `compendium setup-ollama` (no Ollama daemon required).

use serde_json::Value;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_compendium")
}

#[test]
fn setup_ollama_help_exits_zero() {
    let out = Command::new(bin())
        .args(["setup-ollama", "--help"])
        .output()
        .expect("spawn setup-ollama --help");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Examples:"), "{stdout}");
    assert!(stdout.contains("--write-mcp"), "{stdout}");
    assert!(stdout.contains("qwen2.5:3b"), "{stdout}");
}

#[test]
fn ollama_alias_help_matches() {
    let out = Command::new(bin())
        .args(["ollama", "--help"])
        .output()
        .expect("spawn ollama --help");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("setup-ollama"));
}

#[test]
fn top_level_help_lists_setup_ollama() {
    let out = Command::new(bin())
        .arg("--help")
        .output()
        .expect("spawn --help");
    assert!(out.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("setup-ollama"), "{text}");
}

#[test]
fn dry_run_json_is_valid_and_idempotent_without_side_effects() {
    let out = Command::new(bin())
        .args([
            "setup-ollama",
            "--dry-run",
            "--json",
            "--no-pull",
            "--skip-probe",
        ])
        .output()
        .expect("spawn dry-run json");
    assert!(
        out.status.success(),
        "exit={:?} stderr={} stdout={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("expected JSON report ({e}): {stdout}");
    });
    assert_eq!(report["ok"], Value::Bool(true));
    assert_eq!(report["dry_run"], Value::Bool(true));
    assert_eq!(report["mcp_written"], Value::Bool(false));
    assert_eq!(
        report["mcp_env"]["COMPENDIUM_LOCAL_LLM_URL"],
        Value::String("http://127.0.0.1:11434/v1".into())
    );
    assert_eq!(report["chat_model"], Value::String("qwen2.5:3b".into()));
}

#[test]
fn unknown_flag_exits_one_with_help_hint() {
    let out = Command::new(bin())
        .args(["setup-ollama", "--please-install"])
        .output()
        .expect("spawn unknown flag");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown flag"), "{err}");
    assert!(err.contains("setup-ollama --help"), "{err}");
}
