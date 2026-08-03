//! Streamable HTTP / SSE transport (MCP Streamable HTTP via rmcp + axum).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::server::CompendiumServer;

/// Serve Compendium over Streamable HTTP (SSE-capable) at `bind`.
///
/// Endpoint: `http://{bind}/mcp`
pub async fn serve_http(config: Config, bind: &str) -> Result<()> {
    let addr: SocketAddr = bind
        .parse()
        .with_context(|| format!("invalid HTTP bind address: {bind}"))?;

    let ct = CancellationToken::new();
    let http_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_cancellation_token(ct.clone())
        .with_allowed_hosts(vec![
            "localhost".into(),
            "127.0.0.1".into(),
            "0.0.0.0".into(),
            addr.ip().to_string(),
            format!("{}:{}", addr.ip(), addr.port()),
            format!("localhost:{}", addr.port()),
            format!("127.0.0.1:{}", addr.port()),
        ]);

    let pipeline_config = config.clone();
    let service: StreamableHttpService<CompendiumServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(CompendiumServer::new(pipeline_config.clone())),
            Arc::new(LocalSessionManager::default()),
            http_config,
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind HTTP listener on {addr}"))?;
    let local = listener.local_addr().unwrap_or(addr);

    tracing::info!(%local, "Compendium streamable HTTP listening at http://{local}/mcp");

    let shutdown_ct = ct.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutdown signal received; stopping HTTP server");
        shutdown_ct.cancel();
    });

    axum::serve(listener, router)
        .with_graceful_shutdown(async move { ct.cancelled_owned().await })
        .await
        .context("HTTP server error")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut stream) = signal(SignalKind::terminate()) {
            stream.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
