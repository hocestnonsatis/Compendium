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
  compendium                 Serve MCP over stdio (default)
  compendium stdio           Same as default
  compendium http [BIND]     Streamable HTTP/SSE (requires --features http)
                             Default bind: 127.0.0.1:8788 or COMPENDIUM_HTTP_BIND
  compendium --help          Show this help

Features (cargo build):
  --features real-tokens     Exact BPE counts via tiktoken-rs
  --features http            Enable streamable HTTP transport
  --features real-tokens,http
"
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs must go to stderr — stdout is reserved for MCP JSON-RPC in stdio mode.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("compendium=info".parse()?))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = compendium::Config::from_env();

    match args.as_slice() {
        [] => run_stdio(config).await,
        [a] if a == "stdio" => run_stdio(config).await,
        [a] if matches!(a.as_str(), "-h" | "--help" | "help") => {
            print_usage();
            Ok(())
        }
        [a] if a == "http" => run_http(config, None).await,
        [a, bind] if a == "http" => run_http(config, Some(bind.clone())).await,
        other => {
            bail!(
                "unknown arguments: {:?}\n\nRun `compendium --help` for usage.",
                other
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
