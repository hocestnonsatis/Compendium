//! Compendium MCP server (stdio).

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
  compendium setup-ollama [OPTS]  Enable a local Ollama model for smart actions
  compendium --help               Show this help

Ollama setup:  compendium setup-ollama --help
  npx -y compendium-mcp setup-ollama --write-mcp
"
    );
}

fn init_tracing() -> anyhow::Result<()> {
    // Logs must go to stderr — stdout is reserved for MCP JSON-RPC.
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
        None => {}
        Some("stdio") if args.len() == 1 => {}
        _ => bail!(
            "unknown arguments: {:?}\n\nRun `compendium --help` for usage.",
            args
        ),
    }

    run_stdio()
}

#[tokio::main]
async fn run_stdio() -> anyhow::Result<()> {
    let config = compendium::Config::from_env();
    tracing::info!("starting Compendium MCP server on stdio");
    let server = CompendiumServer::new(config);
    let service = server
        .serve(stdio())
        .await
        .context("failed to initialize MCP stdio transport")?;
    service.waiting().await?;
    Ok(())
}
