//! One-shot Ollama enablement for Compendium (`compendium setup-ollama`).
//!
//! Detects or installs Ollama, pulls a small chat (+ embed) model, probes the
//! loopback OpenAI endpoint, and optionally merges `COMPENDIUM_LOCAL_LLM_*`
//! into a Cursor MCP config. Non-interactive: every input is a flag.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::LocalLlmConfig;
use crate::pipeline::local_llm::{llm_status, LlmStatusResult};

/// Default Ollama OpenAI-compatible base (loopback only).
pub const DEFAULT_OLLAMA_OPENAI_URL: &str = "http://127.0.0.1:11434/v1";
/// Small chat model (~2 GiB). Override with `--chat-model`.
pub const DEFAULT_CHAT_MODEL: &str = "qwen2.5:3b";
/// Embedding model for hybrid `rerank` / `brief`.
pub const DEFAULT_EMBED_MODEL: &str = "nomic-embed-text";
/// Default `mcpServers` key written by `--write-mcp`.
pub const DEFAULT_MCP_SERVER_KEY: &str = "compendium";

const SERVE_WAIT: Duration = Duration::from_secs(20);
const SERVE_POLL: Duration = Duration::from_millis(400);

/// Parsed `setup-ollama` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOllamaArgs {
    pub url: String,
    pub chat_model: String,
    pub embed_model: Option<String>,
    pub install: bool,
    pub pull: bool,
    pub probe: bool,
    pub write_mcp: bool,
    pub mcp_path: Option<PathBuf>,
    pub server_key: String,
    pub dry_run: bool,
    pub json: bool,
    pub timeout_secs: u64,
}

impl Default for SetupOllamaArgs {
    fn default() -> Self {
        Self {
            url: DEFAULT_OLLAMA_OPENAI_URL.into(),
            chat_model: DEFAULT_CHAT_MODEL.into(),
            embed_model: Some(DEFAULT_EMBED_MODEL.into()),
            install: false,
            pull: true,
            probe: true,
            write_mcp: false,
            mcp_path: None,
            server_key: DEFAULT_MCP_SERVER_KEY.into(),
            dry_run: false,
            json: false,
            timeout_secs: 120,
        }
    }
}

/// Parse result: help vs run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSetup {
    Help,
    Run(SetupOllamaArgs),
}

/// Machine-readable summary (stdout when `--json`).
#[derive(Debug, Clone, Serialize)]
pub struct SetupReport {
    pub ok: bool,
    pub dry_run: bool,
    pub ollama_installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_version: Option<String>,
    pub ollama_running: bool,
    pub installed_now: bool,
    pub serve_started: bool,
    pub url: String,
    pub chat_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_model: Option<String>,
    pub pulled: Vec<String>,
    pub already_present: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<LlmStatusResult>,
    pub mcp_env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_path: Option<String>,
    pub mcp_written: bool,
    pub next: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Subcommand help (also used by `compendium setup-ollama --help`).
pub fn print_help(out: &mut dyn Write) -> io::Result<()> {
    writeln!(
        out,
        "\
Compendium — enable local Ollama for smart/hybrid actions

Usage:
  compendium setup-ollama [OPTIONS]
  compendium ollama [OPTIONS]

After adding the Compendium MCP server (heuristics work with no model),
run this once to pull small local models and set COMPENDIUM_LOCAL_LLM_*.

Options:
  --chat-model NAME     Chat model to pull (default: {chat})
  --embed-model NAME    Embedding model (default: {embed})
  --no-embed            Skip embedding model
  --url URL             OpenAI-compatible base (default: {url})
  --install             If Ollama is missing, run the official installer
  --no-pull             Do not pull models (probe + MCP env only)
  --skip-probe          Do not call GET /v1/models
  --write-mcp [PATH]    Merge env into Cursor mcp.json
                        (default PATH: ~/.cursor/mcp.json)
  --mcp-path PATH       Same as --write-mcp PATH
  --project             Write ./.cursor/mcp.json (cwd)
  --server-key NAME     mcpServers key (default: compendium)
  --timeout-secs N      Probe timeout (default: 120)
  --dry-run             Plan only: no install, pull, serve, or write
  --json                Print a JSON report on stdout (progress on stderr)
  -h, --help            Show this help

Examples:
  npx -y compendium-mcp setup-ollama
  npx -y compendium-mcp setup-ollama --write-mcp
  npx -y compendium-mcp setup-ollama --install --write-mcp --project
  npx -y compendium-mcp setup-ollama --chat-model qwen2.5:7b --dry-run --json
  ./target/release/compendium setup-ollama --no-pull --write-mcp ~/.cursor/mcp.json

Exit codes:
  0  ready (or dry-run plan printed)
  2  Ollama not installed (pass --install or install it yourself)
  3  Ollama not reachable / probe failed
  1  usage or other error

Then reload MCP and call action=llm_status (reachable: true → local_llm backend).
",
        chat = DEFAULT_CHAT_MODEL,
        embed = DEFAULT_EMBED_MODEL,
        url = DEFAULT_OLLAMA_OPENAI_URL,
    )
}

/// Parse argv **after** the `setup-ollama` / `ollama` verb.
pub fn parse_args(args: &[String]) -> Result<ParsedSetup, String> {
    let mut parsed = SetupOllamaArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if matches!(arg, "-h" | "--help" | "help") {
            return Ok(ParsedSetup::Help);
        }
        let (key, inline) = split_flag(arg);
        match key {
            "--chat-model" => {
                parsed.chat_model = require_value("--chat-model", inline, args, &mut i)?;
            }
            "--embed-model" => {
                parsed.embed_model = Some(require_value("--embed-model", inline, args, &mut i)?);
            }
            "--url" => {
                parsed.url = require_value("--url", inline, args, &mut i)?;
            }
            "--server-key" => {
                parsed.server_key = require_value("--server-key", inline, args, &mut i)?;
            }
            "--timeout-secs" => {
                let raw = require_value("--timeout-secs", inline, args, &mut i)?;
                let n: u64 = raw.parse().map_err(|_| {
                    format!("--timeout-secs expects a positive integer, got {raw:?}")
                })?;
                if n == 0 {
                    return Err("--timeout-secs must be > 0".into());
                }
                parsed.timeout_secs = n;
            }
            "--mcp-path" => {
                let path = require_value("--mcp-path", inline, args, &mut i)?;
                parsed.write_mcp = true;
                parsed.mcp_path = Some(PathBuf::from(path));
            }
            "--write-mcp" => {
                parsed.write_mcp = true;
                if let Some(v) = inline {
                    parsed.mcp_path = Some(PathBuf::from(v));
                } else if let Some(next) = args.get(i + 1) {
                    if !next.starts_with('-') {
                        i += 1;
                        parsed.mcp_path = Some(PathBuf::from(next));
                    }
                }
            }
            "--project" => {
                parsed.write_mcp = true;
                parsed.mcp_path = Some(PathBuf::from(".cursor/mcp.json"));
            }
            "--install" => parsed.install = true,
            "--no-pull" => parsed.pull = false,
            "--no-embed" => parsed.embed_model = None,
            "--skip-probe" => parsed.probe = false,
            "--dry-run" => parsed.dry_run = true,
            "--json" => parsed.json = true,
            other if other.starts_with('-') => {
                return Err(format!(
                    "unknown flag: {other}\n  compendium setup-ollama --help"
                ));
            }
            other => {
                return Err(format!(
                    "unexpected argument: {other}\n  compendium setup-ollama --help"
                ));
            }
        }
        i += 1;
    }
    if parsed.chat_model.trim().is_empty() {
        return Err("--chat-model must not be empty".into());
    }
    if parsed.url.trim().is_empty() {
        return Err("--url must not be empty".into());
    }
    if let Some(embed) = parsed.embed_model.as_deref() {
        if embed.trim().is_empty() {
            return Err("--embed-model must not be empty (or pass --no-embed)".into());
        }
    }
    Ok(ParsedSetup::Run(parsed))
}

fn split_flag(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((k, v)) => (k, Some(v)),
        None => (arg, None),
    }
}

fn require_value(
    flag: &str,
    inline: Option<&str>,
    args: &[String],
    i: &mut usize,
) -> Result<String, String> {
    if let Some(v) = inline {
        if v.is_empty() {
            return Err(format!(
                "{flag} needs a value\n  compendium setup-ollama --help"
            ));
        }
        return Ok(v.to_string());
    }
    let next = args.get(*i + 1).map(|s| s.as_str());
    match next {
        Some(v) if !v.starts_with('-') => {
            *i += 1;
            Ok(v.to_string())
        }
        _ => Err(format!(
            "{flag} needs a value\n  compendium setup-ollama --help"
        )),
    }
}

/// Env map written into MCP config / printed for copy-paste.
pub fn mcp_env_map(url: &str, chat: &str, embed: Option<&str>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("COMPENDIUM_LOCAL_LLM_URL".into(), url.trim().to_string());
    env.insert("COMPENDIUM_LOCAL_LLM_MODEL".into(), chat.trim().to_string());
    if let Some(e) = embed.map(str::trim).filter(|s| !s.is_empty()) {
        env.insert("COMPENDIUM_LOCAL_EMBED_MODEL".into(), e.to_string());
    }
    env
}

/// Default Cursor user MCP config path (`~/.cursor/mcp.json`).
pub fn default_cursor_mcp_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".cursor").join("mcp.json"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Merge `COMPENDIUM_LOCAL_LLM_*` into an mcp.json document.
///
/// Creates `mcpServers.<server_key>` with `npx -y compendium-mcp` when missing.
/// Existing `command` / `args` are preserved.
pub fn merge_compendium_mcp_env(
    mut root: Value,
    env: &BTreeMap<String, String>,
    server_key: &str,
) -> Result<Value, String> {
    if root.is_null() {
        root = json!({});
    }
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "mcp.json root must be a JSON object".to_string())?;
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| "mcpServers must be a JSON object".to_string())?;
    let key = server_key.trim();
    if key.is_empty() {
        return Err("--server-key must not be empty".into());
    }
    let server = servers_obj.entry(key.to_string()).or_insert_with(|| {
        json!({
            "command": "npx",
            "args": ["-y", "compendium-mcp"]
        })
    });
    let server_obj = server
        .as_object_mut()
        .ok_or_else(|| format!("mcpServers.{key} must be a JSON object"))?;
    let env_val = server_obj.entry("env").or_insert_with(|| json!({}));
    let env_obj = env_val
        .as_object_mut()
        .ok_or_else(|| format!("mcpServers.{key}.env must be a JSON object"))?;
    for (k, v) in env {
        env_obj.insert(k.clone(), Value::String(v.clone()));
    }
    Ok(root)
}

/// Official install hints (when `--install` is not used).
pub fn install_hints() -> &'static str {
    "Install Ollama, then re-run:\n  \
     curl -fsSL https://ollama.com/install.sh | sh     # Linux / macOS\n  \
     winget install -e --id Ollama.Ollama              # Windows\n  \
     or: npx -y compendium-mcp setup-ollama --install"
}

/// Strip `/v1` or `/api/v1` so we can hit Ollama's native `/api/tags`.
pub fn ollama_native_base(openai_url: &str) -> String {
    let t = openai_url.trim().trim_end_matches('/');
    let t = t
        .strip_suffix("/api/v1")
        .or_else(|| t.strip_suffix("/v1"))
        .unwrap_or(t);
    t.trim_end_matches('/').to_string()
}

/// Normalize Ollama model ids (`qwen2.5:3b:latest` → `qwen2.5:3b`).
pub fn normalize_model_name(name: &str) -> String {
    name.trim().trim_end_matches(":latest").to_string()
}

/// Whether `want` is already in an `/api/tags` name list.
pub fn model_is_present(names: &[String], want: &str) -> bool {
    let want_n = normalize_model_name(want);
    names.iter().any(|m| {
        let n = normalize_model_name(m);
        n == want_n || n.starts_with(&format!("{want_n}:"))
    })
}

/// Entry point used by the binary. Returns a process exit code.
pub fn run(args: &[String]) -> i32 {
    match parse_args(args) {
        Ok(ParsedSetup::Help) => {
            let _ = print_help(&mut io::stdout());
            0
        }
        Ok(ParsedSetup::Run(opts)) => match execute(opts) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    }
}

fn execute(opts: SetupOllamaArgs) -> Result<i32, String> {
    let mcp_path = resolve_mcp_path(&opts)?;
    let env = mcp_env_map(&opts.url, &opts.chat_model, opts.embed_model.as_deref());

    let mut report = SetupReport {
        ok: false,
        dry_run: opts.dry_run,
        ollama_installed: false,
        ollama_version: None,
        ollama_running: false,
        installed_now: false,
        serve_started: false,
        url: opts.url.clone(),
        chat_model: opts.chat_model.clone(),
        embed_model: opts.embed_model.clone(),
        pulled: Vec::new(),
        already_present: Vec::new(),
        probe: None,
        mcp_env: env.clone(),
        mcp_path: mcp_path.as_ref().map(|p| p.display().to_string()),
        mcp_written: false,
        next: Vec::new(),
        error: None,
    };

    let mut version = ollama_version();
    report.ollama_installed = version.is_some();
    report.ollama_version = version.clone();

    if version.is_none() {
        if opts.install {
            if opts.dry_run {
                human(
                    opts.json,
                    &format!(
                        "dry-run: would install Ollama via {}\n",
                        install_command_label()
                    ),
                );
            } else {
                human(
                    opts.json,
                    "Ollama not found — running official installer…\n",
                );
                install_ollama()?;
                report.installed_now = true;
                version = ollama_version();
                report.ollama_installed = version.is_some();
                report.ollama_version = version.clone();
                if version.is_none() {
                    report.error = Some(
                        "installer finished but `ollama` is still not on PATH — open a new terminal and re-run setup-ollama".into(),
                    );
                    report.next = vec![
                        "Restart the shell so PATH includes ollama".into(),
                        "npx -y compendium-mcp setup-ollama --write-mcp".into(),
                    ];
                    return finish(opts.json, report, 2);
                }
            }
        } else if opts.dry_run {
            human(
                opts.json,
                &format!("dry-run: Ollama not installed.\n{}\n", install_hints()),
            );
            report.next = vec![
                "Install Ollama (see hints) or re-run with --install".into(),
                "npx -y compendium-mcp setup-ollama --install --write-mcp".into(),
            ];
            report.ok = true;
            return finish(opts.json, report, 0);
        } else {
            report.error = Some("Ollama is not installed".into());
            report.next = vec![
                install_hints().to_string(),
                "npx -y compendium-mcp setup-ollama --install --write-mcp".into(),
            ];
            human(
                opts.json,
                &format!("Ollama is not installed.\n\n{}\n", install_hints()),
            );
            return finish(opts.json, report, 2);
        }
    } else if let Some(v) = &version {
        human(opts.json, &format!("Ollama found: {v}\n"));
    }

    let native = ollama_native_base(&opts.url);
    let tags_url = format!("{native}/api/tags");
    report.ollama_running = tags_reachable(&tags_url);

    if !report.ollama_running {
        if opts.dry_run {
            human(
                opts.json,
                "dry-run: Ollama daemon not reachable — would try `ollama serve`\n",
            );
        } else {
            human(
                opts.json,
                "Ollama daemon not reachable — starting `ollama serve`…\n",
            );
            spawn_ollama_serve()?;
            report.serve_started = true;
            report.ollama_running = wait_for_tags(&tags_url, SERVE_WAIT);
            if !report.ollama_running {
                report.error = Some(format!(
                    "Ollama did not become reachable at {tags_url} within {}s",
                    SERVE_WAIT.as_secs()
                ));
                report.next =
                    vec!["Start Ollama (app or `ollama serve`) and re-run setup-ollama".into()];
                return finish(opts.json, report, 3);
            }
        }
    } else {
        human(opts.json, &format!("Ollama reachable at {native}\n"));
    }

    let mut wanted: Vec<String> = vec![opts.chat_model.clone()];
    if let Some(e) = &opts.embed_model {
        wanted.push(e.clone());
    }

    if report.ollama_running {
        let present = list_ollama_models(&tags_url).unwrap_or_default();
        for model in &wanted {
            if model_is_present(&present, model) {
                report.already_present.push(model.clone());
                human(opts.json, &format!("Already pulled: {model}\n"));
            } else if !opts.pull {
                human(
                    opts.json,
                    &format!("Missing model {model} (--no-pull, skipping)\n"),
                );
            } else if opts.dry_run {
                human(
                    opts.json,
                    &format!("dry-run: would `ollama pull {model}`\n"),
                );
            } else {
                human(opts.json, &format!("Pulling {model}…\n"));
                ollama_pull(model, opts.json)?;
                report.pulled.push(model.clone());
            }
        }
    } else if opts.dry_run && opts.pull {
        for model in &wanted {
            human(
                opts.json,
                &format!("dry-run: would `ollama pull {model}`\n"),
            );
        }
    }

    if opts.probe && report.ollama_running && !opts.dry_run {
        let cfg = LocalLlmConfig {
            enabled: true,
            base_url: Some(opts.url.clone()),
            model: opts.chat_model.clone(),
            embedding_model: opts.embed_model.clone(),
            api_key: None,
            timeout_secs: opts.timeout_secs,
        };
        let status = llm_status(&cfg, false);
        report.probe = Some(status.clone());
        if status.reachable {
            human(opts.json, "Probe OK (GET /v1/models)\n");
        } else {
            let why = status
                .fallback_reason
                .clone()
                .unwrap_or_else(|| "unreachable".into());
            report.error = Some(format!("probe failed: {why}"));
            report.next = vec![
                "Check `ollama list` and --chat-model / --url".into(),
                "npx -y compendium-mcp setup-ollama --no-pull".into(),
            ];
            return finish(opts.json, report, 3);
        }
    } else if opts.dry_run && opts.probe {
        human(opts.json, "dry-run: would probe GET /v1/models\n");
    }

    if opts.write_mcp {
        let path = mcp_path.ok_or_else(|| {
            "cannot resolve mcp.json path (set HOME/USERPROFILE or pass --mcp-path)".to_string()
        })?;
        if opts.dry_run {
            human(
                opts.json,
                &format!("dry-run: would merge env into {}\n", path.display()),
            );
        } else {
            write_mcp_env(&path, &env, &opts.server_key)?;
            report.mcp_written = true;
            human(
                opts.json,
                &format!("Wrote COMPENDIUM_LOCAL_LLM_* to {}\n", path.display()),
            );
        }
    } else {
        human(
            opts.json,
            &format!(
                "\nAdd this env to your Compendium MCP server, then reload MCP:\n\n{}\n\n\
                 Or write it automatically:\n  npx -y compendium-mcp setup-ollama --write-mcp\n  \
                 npx -y compendium-mcp setup-ollama --write-mcp .cursor/mcp.json\n",
                serde_json::to_string_pretty(&json!({ "env": env }))
                    .unwrap_or_else(|_| "{}".into())
            ),
        );
    }

    report.next = next_steps(&opts, report.mcp_written);
    report.ok = true;
    let code = 0;
    finish(opts.json, report, code)
}

fn next_steps(opts: &SetupOllamaArgs, mcp_written: bool) -> Vec<String> {
    let mut next = Vec::new();
    if mcp_written || opts.write_mcp {
        next.push("Reload MCP in Cursor / Claude Desktop".into());
    } else {
        next.push("Paste the env block into mcp.json (or re-run with --write-mcp)".into());
        next.push("Reload MCP".into());
    }
    next.push("Call compendium action=llm_status — expect reachable: true".into());
    next.push(
        "Then summarize_smart / filter_relevant / rerank use backend local_llm or hybrid".into(),
    );
    next
}

fn resolve_mcp_path(opts: &SetupOllamaArgs) -> Result<Option<PathBuf>, String> {
    if !opts.write_mcp {
        return Ok(opts.mcp_path.clone());
    }
    if let Some(p) = &opts.mcp_path {
        return Ok(Some(p.clone()));
    }
    default_cursor_mcp_path()
        .map(Some)
        .ok_or_else(|| "HOME/USERPROFILE unset — pass --mcp-path".to_string())
}

fn finish(json: bool, report: SetupReport, code: i32) -> Result<i32, String> {
    if json {
        let mut out = io::stdout();
        serde_json::to_writer_pretty(&mut out, &report).map_err(|e| e.to_string())?;
        writeln!(out).map_err(|e| e.to_string())?;
    } else if report.ok && !report.dry_run {
        println!("Next:");
        for step in &report.next {
            println!("  • {step}");
        }
    }
    Ok(code)
}

fn human(json_mode: bool, msg: &str) {
    if json_mode {
        let _ = io::stderr().write_all(msg.as_bytes());
    } else {
        let _ = io::stdout().write_all(msg.as_bytes());
    }
}

fn ollama_version() -> Option<String> {
    let out = Command::new("ollama").arg("--version").output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let text = if !stdout.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() && !out.status.success() {
        return None;
    }
    if line.is_empty() {
        Some("ollama".into())
    } else {
        Some(line.to_string())
    }
}

fn install_command_label() -> &'static str {
    if cfg!(windows) {
        "winget install -e --id Ollama.Ollama"
    } else {
        "curl -fsSL https://ollama.com/install.sh | sh"
    }
}

fn install_ollama() -> Result<(), String> {
    let status = if cfg!(windows) {
        Command::new("winget")
            .args([
                "install",
                "-e",
                "--id",
                "Ollama.Ollama",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| format!("failed to run winget: {e}"))?
    } else {
        Command::new("sh")
            .args(["-c", "curl -fsSL https://ollama.com/install.sh | sh"])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| format!("failed to run Ollama install script: {e}"))?
    };
    if !status.success() {
        return Err(format!(
            "Ollama installer exited {status}. Install manually:\n{}",
            install_hints()
        ));
    }
    Ok(())
}

fn tags_reachable(tags_url: &str) -> bool {
    list_ollama_models(tags_url).is_ok()
}

fn wait_for_tags(tags_url: &str, budget: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < budget {
        if tags_reachable(tags_url) {
            return true;
        }
        thread::sleep(SERVE_POLL);
    }
    false
}

fn spawn_ollama_serve() -> Result<(), String> {
    Command::new("ollama")
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start `ollama serve`: {e}"))?;
    Ok(())
}

fn list_ollama_models(tags_url: &str) -> Result<Vec<String>, String> {
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http
        .get(tags_url)
        .send()
        .map_err(|e| format!("GET {tags_url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GET {tags_url} → HTTP {}", resp.status()));
    }
    let body: Value = resp.json().map_err(|e| format!("parse /api/tags: {e}"))?;
    let mut names = Vec::new();
    if let Some(models) = body.get("models").and_then(|v| v.as_array()) {
        for m in models {
            if let Some(n) = m
                .get("name")
                .or_else(|| m.get("model"))
                .and_then(|v| v.as_str())
            {
                names.push(n.to_string());
            }
        }
    }
    Ok(names)
}

fn ollama_pull(model: &str, json_mode: bool) -> Result<(), String> {
    let mut cmd = Command::new("ollama");
    cmd.arg("pull").arg(model).stdin(Stdio::null());
    if json_mode {
        // Keep stdout clean for the JSON report; pull progress stays on stderr.
        cmd.stdout(Stdio::null()).stderr(Stdio::inherit());
    } else {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run `ollama pull {model}`: {e}"))?;
    if !status.success() {
        return Err(format!(
            "`ollama pull {model}` failed ({status}). Try: ollama pull {model}"
        ));
    }
    Ok(())
}

fn write_mcp_env(
    path: &Path,
    env: &BTreeMap<String, String>,
    server_key: &str,
) -> Result<(), String> {
    let existing = if path.exists() {
        let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text)
                .map_err(|e| format!("invalid JSON in {}: {e}", path.display()))?
        }
    } else {
        json!({})
    };
    let merged = merge_compendium_mcp_env(existing, env, server_key)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
    }
    let pretty = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    fs::write(path, pretty + "\n").map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn parse_help() {
        assert_eq!(parse_args(&s(&["--help"])).unwrap(), ParsedSetup::Help);
        assert_eq!(parse_args(&s(&["-h"])).unwrap(), ParsedSetup::Help);
    }

    #[test]
    fn parse_defaults_and_flags() {
        let ParsedSetup::Run(a) = parse_args(&s(&[
            "--dry-run",
            "--json",
            "--no-pull",
            "--chat-model",
            "llama3.2:3b",
            "--write-mcp",
            "/tmp/mcp.json",
        ]))
        .unwrap() else {
            panic!("expected Run");
        };
        assert!(a.dry_run && a.json && !a.pull && a.write_mcp);
        assert_eq!(a.chat_model, "llama3.2:3b");
        assert_eq!(a.embed_model.as_deref(), Some(DEFAULT_EMBED_MODEL));
        assert_eq!(a.mcp_path.as_deref(), Some(Path::new("/tmp/mcp.json")));
    }

    #[test]
    fn parse_inline_equals_and_no_embed() {
        let ParsedSetup::Run(a) = parse_args(&s(&[
            "--chat-model=qwen2.5:7b",
            "--embed-model=all-minilm",
            "--url=http://127.0.0.1:11434/v1",
            "--no-embed",
            "--project",
        ]))
        .unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(a.chat_model, "qwen2.5:7b");
        assert!(
            a.embed_model.is_none(),
            "--no-embed should win after embed-model"
        );
        assert_eq!(a.mcp_path.as_deref(), Some(Path::new(".cursor/mcp.json")));
        assert!(a.write_mcp);
    }

    #[test]
    fn parse_unknown_flag_is_actionable() {
        let err = parse_args(&s(&["--please-install"])).unwrap_err();
        assert!(err.contains("unknown flag"), "{err}");
        assert!(err.contains("setup-ollama --help"), "{err}");
    }

    #[test]
    fn parse_missing_value() {
        let err = parse_args(&s(&["--chat-model"])).unwrap_err();
        assert!(err.contains("--chat-model needs a value"), "{err}");
    }

    #[test]
    fn mcp_env_includes_embed_unless_none() {
        let with = mcp_env_map(
            DEFAULT_OLLAMA_OPENAI_URL,
            DEFAULT_CHAT_MODEL,
            Some(DEFAULT_EMBED_MODEL),
        );
        assert_eq!(
            with.get("COMPENDIUM_LOCAL_LLM_URL").map(String::as_str),
            Some(DEFAULT_OLLAMA_OPENAI_URL)
        );
        assert_eq!(
            with.get("COMPENDIUM_LOCAL_EMBED_MODEL").map(String::as_str),
            Some(DEFAULT_EMBED_MODEL)
        );
        let without = mcp_env_map(DEFAULT_OLLAMA_OPENAI_URL, DEFAULT_CHAT_MODEL, None);
        assert!(!without.contains_key("COMPENDIUM_LOCAL_EMBED_MODEL"));
    }

    #[test]
    fn merge_creates_npx_server_and_preserves_existing_command() {
        let env = mcp_env_map(
            DEFAULT_OLLAMA_OPENAI_URL,
            "qwen2.5:3b",
            Some("nomic-embed-text"),
        );
        let created = merge_compendium_mcp_env(json!({}), &env, "compendium").unwrap();
        assert_eq!(created["mcpServers"]["compendium"]["command"], json!("npx"));
        assert_eq!(
            created["mcpServers"]["compendium"]["args"],
            json!(["-y", "compendium-mcp"])
        );
        assert_eq!(
            created["mcpServers"]["compendium"]["env"]["COMPENDIUM_LOCAL_LLM_MODEL"],
            json!("qwen2.5:3b")
        );

        let existing = json!({
            "mcpServers": {
                "other": { "command": "echo" },
                "compendium": {
                    "command": "/opt/compendium",
                    "env": { "RUST_LOG": "info" }
                }
            }
        });
        let merged = merge_compendium_mcp_env(existing, &env, "compendium").unwrap();
        assert_eq!(
            merged["mcpServers"]["compendium"]["command"],
            json!("/opt/compendium")
        );
        assert_eq!(
            merged["mcpServers"]["compendium"]["env"]["RUST_LOG"],
            json!("info")
        );
        assert_eq!(
            merged["mcpServers"]["compendium"]["env"]["COMPENDIUM_LOCAL_LLM_URL"],
            json!(DEFAULT_OLLAMA_OPENAI_URL)
        );
        assert_eq!(merged["mcpServers"]["other"]["command"], json!("echo"));
    }

    #[test]
    fn ollama_native_base_strips_openai_suffix() {
        assert_eq!(
            ollama_native_base("http://127.0.0.1:11434/v1"),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            ollama_native_base("http://localhost:11434/v1/"),
            "http://localhost:11434"
        );
        assert_eq!(
            ollama_native_base("http://127.0.0.1:13305/api/v1"),
            "http://127.0.0.1:13305"
        );
    }

    #[test]
    fn model_presence_normalizes_latest() {
        let names = vec!["qwen2.5:3b:latest".into(), "nomic-embed-text".into()];
        assert!(model_is_present(&names, "qwen2.5:3b"));
        assert!(model_is_present(&names, "nomic-embed-text:latest"));
        assert!(!model_is_present(&names, "llama3.2:3b"));
    }

    #[test]
    fn write_mcp_roundtrip_tmp() {
        let dir = std::env::temp_dir().join(format!(
            "compendium-setup-ollama-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp.json");
        let env = mcp_env_map(
            "http://127.0.0.1:11434/v1",
            "qwen2.5:3b",
            Some("nomic-embed-text"),
        );
        write_mcp_env(&path, &env, "compendium").unwrap();
        let again = mcp_env_map(
            "http://127.0.0.1:11434/v1",
            "qwen2.5:7b",
            Some("nomic-embed-text"),
        );
        write_mcp_env(&path, &again, "compendium").unwrap();
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            doc["mcpServers"]["compendium"]["env"]["COMPENDIUM_LOCAL_LLM_MODEL"],
            json!("qwen2.5:7b")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
