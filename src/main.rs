//! Compendium MCP server — stdio (default) or streamable HTTP.

use anyhow::{bail, Context};
use compendium::CompendiumServer;
use rmcp::{transport::stdio, ServiceExt};
use tracing_subscriber::EnvFilter;

fn print_usage() {
    eprintln!(
        "\
Compendium — token-efficient MCP context server

Usage:
  compendium                      Serve MCP over stdio (default)
  compendium stdio                Same as default
  compendium http [BIND]          Streamable HTTP/SSE (requires --features http)
                                  Default bind: 127.0.0.1:8788 or COMPENDIUM_HTTP_BIND
  compendium setup-ollama [OPTS]  Enable local Ollama (pull models + MCP env)
  compendium ollama [OPTS]        Alias for setup-ollama
  compendium --help               Show this help

Ollama setup:  compendium setup-ollama --help
  npx -y compendium-mcp setup-ollama --write-mcp

Features (cargo build):
  --features real-tokens     Exact BPE counts via tiktoken-rs
  --features http            Enable streamable HTTP transport
  --features real-tokens,http
"
    );
}

fn init_tracing() -> anyhow::Result<()> {
    // Logs must go to stderr — stdout is reserved for MCP JSON-RPC in stdio mode.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("compendium=info".parse()?))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    Ok(())
}

fn main() -> anyhow::Result<()> {
    init_tracing()?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Blocking Ollama HTTP must not run inside the tokio runtime
    // (`reqwest::blocking` panics if a runtime is already active).
    match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") if args.len() == 1 => {
            print_usage();
            return Ok(());
        }
        Some("setup-ollama" | "ollama") => {
            std::process::exit(compendium::setup_ollama::run(&args[1..]));
        }
        _ => {}
    }

    async_main(args)
}

#[tokio::main]
async fn async_main(args: Vec<String>) -> anyhow::Result<()> {
    let config = compendium::Config::from_env();

    match args.first().map(String::as_str) {
        None => run_stdio(config).await,
        Some("stdio") if args.len() == 1 => run_stdio(config).await,
        Some("http") => {
            if args.len() > 2 {
                bail!(
                    "unknown arguments: {:?}\n\nRun `compendium --help` for usage.",
                    args
                );
            }
            run_http(config, args.get(1).cloned()).await
        }
        _ => {
            bail!(
                "unknown arguments: {:?}\n\nRun `compendium --help` for usage.",
                args
            );
        }
    }
}

async fn run_stdio(config: compendium::Config) -> anyhow::Result<()> {
    tracing::info!("starting Compendium MCP server on stdio");
    let server = CompendiumServer::new(config);
    let service = server
        .serve(stdio())
        .await
        .context("failed to initialize MCP stdio transport")?;
    service.waiting().await?;
    Ok(())
}

async fn run_http(config: compendium::Config, bind_override: Option<String>) -> anyhow::Result<()> {
    #[cfg(feature = "http")]
    {
        let bind = bind_override.unwrap_or_else(|| config.http_bind.clone());
        compendium::http::serve_http(config, &bind).await
    }
    #[cfg(not(feature = "http"))]
    {
        let _ = (config, bind_override);
        bail!(
            "HTTP transport not enabled in this build.\n\
             Rebuild with: cargo build --release --features http"
        );
    }
}
