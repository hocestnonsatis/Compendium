//! HTTP transport smoke: in-process Streamable HTTP server + reqwest MCP client.
//!
//! Run with: `cargo test --features http --test http_smoke`

#![cfg(feature = "http")]

use std::sync::Arc;

use compendium::{CompendiumServer, Config};
use rmcp::{
    model::CallToolRequestParams,
    transport::StreamableHttpClientTransport,
    ServiceExt,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

fn args_object(value: Value) -> rmcp::model::JsonObject {
    value
        .as_object()
        .expect("tool arguments must be a JSON object")
        .clone()
}

async fn spawn_http_server() -> (String, CancellationToken) {
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };

    let ct = CancellationToken::new();
    let http_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(ct.clone())
        .with_allowed_hosts(vec![
            "127.0.0.1".to_string(),
            "localhost".to_string(),
        ]);

    let config = Config::default();
    let service: StreamableHttpService<CompendiumServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(CompendiumServer::new(config.clone())),
            Arc::new(LocalSessionManager::default()),
            http_config,
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}/mcp");

    let serve_ct = ct.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { serve_ct.cancelled_owned().await })
            .await;
    });

    tokio::task::yield_now().await;
    (url, ct)
}

#[tokio::test(flavor = "multi_thread")]
async fn smoke_http_gateway_count_tokens() -> anyhow::Result<()> {
    let (url, ct) = spawn_http_server().await;

    let transport = StreamableHttpClientTransport::from_uri(url);
    let client = ().serve(transport).await?;

    let tools = client.list_all_tools().await?;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "compendium");

    let result = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "count_tokens",
                "text": "hello from http smoke"
            }))),
        )
        .await?;
    assert!(!result.is_error.unwrap_or(false));

    let payload = result
        .structured_content
        .clone()
        .unwrap_or_else(|| json!({}));
    assert_eq!(payload.get("ok"), Some(&json!(true)));
    let inner: Value = serde_json::from_str(
        payload
            .get("result_json")
            .and_then(|v| v.as_str())
            .expect("result_json"),
    )?;
    assert!(
        inner
            .get("tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 1
    );

    client.cancel().await?;
    ct.cancel();
    Ok(())
}
