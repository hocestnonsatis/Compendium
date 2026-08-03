//! End-to-end smoke test: spawn the compiled `compendium` binary, complete the
//! MCP handshake over JSON-RPC stdio, and exercise the gateway tool actions.

use rmcp::{
    model::CallToolRequestParams,
    transport::{ConfigureCommandExt, TokioChildProcess},
    ServiceExt,
};
use serde_json::{json, Value};

fn args_object(value: Value) -> rmcp::model::JsonObject {
    value
        .as_object()
        .expect("tool arguments must be a JSON object")
        .clone()
}

fn result_payload(result: &rmcp::model::CallToolResult) -> Value {
    if let Some(sc) = &result.structured_content {
        return sc.clone();
    }
    let text = result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    Value::String(text)
}

/// Decode gateway envelope: `{ ok, action, result_json, error? }`.
fn gateway_result(result: &rmcp::model::CallToolResult) -> Value {
    let payload = result_payload(result);
    assert_eq!(
        payload.get("ok"),
        Some(&json!(true)),
        "gateway not ok: {payload}"
    );
    let raw = payload
        .get("result_json")
        .and_then(|v| v.as_str())
        .expect("missing result_json");
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("bad result_json ({e}): {raw}"))
}

fn assert_ok(result: &rmcp::model::CallToolResult, label: &str) {
    assert!(
        !result.is_error.unwrap_or(false),
        "{label} returned is_error: {result:?}"
    );
    let payload = result_payload(result);
    assert_eq!(
        payload.get("ok"),
        Some(&json!(true)),
        "{label} envelope not ok: {payload}"
    );
}

#[tokio::test]
async fn smoke_stdio_gateway_actions() -> anyhow::Result<()> {
    let bin = env!("CARGO_BIN_EXE_compendium");

    let transport = TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
        cmd.env("RUST_LOG", "compendium=warn")
            .env("COMPENDIUM_DEFAULT_MAX_TOKENS", "1024");
    }))?;

    let client = ().serve(transport).await?;

    let tools = client.list_all_tools().await?;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, vec!["compendium"], "expected single gateway; have {names:?}");

    // filter
    let filter = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "filter",
                "text": "\u{1b}[31mERROR\u{1b}[0m boom\n\n\nINFO ok\nDEBUG noise",
                "filter": {
                    "strip_ansi": true,
                    "collapse_whitespace": true,
                    "densify_json": false,
                    "keep_patterns": ["ERROR|INFO"]
                }
            }))),
        )
        .await?;
    assert_ok(&filter, "filter");
    let filter_result = gateway_result(&filter);
    let filter_content = filter_result
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        filter_content.contains("ERROR") || filter_content.contains("boom"),
        "unexpected filter: {filter_result}"
    );

    // compress
    let compress = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "compress",
                "text": "2024-01-01T00:00:01Z INFO starting\n2024-01-01T00:00:02Z INFO starting\nsee https://example.com/docs",
                "compress": { "content_type": "log", "max_tokens": 256 }
            }))),
        )
        .await?;
    assert_ok(&compress, "compress");

    // summarize
    let summarize = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "summarize",
                "text": "user: How do I build?\nassistant: cargo build --release.\nuser: Thanks!",
                "summarize": { "mode": "conversation", "max_depth": 2 }
            }))),
        )
        .await?;
    assert_ok(&summarize, "summarize");

    // chunk + resolve
    let chunk = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "chunk",
                "text": format!("{}\n\n{}", "alpha word ".repeat(120), "beta word ".repeat(120)),
                "chunk": {
                    "source": "mem://e2e",
                    "chunk_tokens": 64,
                    "overlap_tokens": 8
                }
            }))),
        )
        .await?;
    assert_ok(&chunk, "chunk");
    let chunk_result = gateway_result(&chunk);
    let chunk_id = chunk_result
        .pointer("/chunks/0/id")
        .and_then(|v| v.as_str())
        .expect("chunk id")
        .to_string();

    let resolve = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "resolve",
                "id": chunk_id
            }))),
        )
        .await?;
    assert_ok(&resolve, "resolve");
    assert_eq!(gateway_result(&resolve).get("found"), Some(&json!(true)));

    // count_tokens
    let count = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "count_tokens",
                "text": "hello from e2e smoke"
            }))),
        )
        .await?;
    assert_ok(&count, "count_tokens");

    // cache store/get
    let store = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "cache_store",
                "text": "bulky payload outside the prompt",
                "cache": { "key": "e2e-cache" }
            }))),
        )
        .await?;
    assert_ok(&store, "cache_store");

    let get = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "cache_get",
                "key": "e2e-cache"
            }))),
        )
        .await?;
    assert_ok(&get, "cache_get");
    assert_eq!(gateway_result(&get).get("hit"), Some(&json!(true)));

    // prune + compress_output
    let prune = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "prune_history",
                "text": "user: How?\nassistant: Like this carefully.\nuser: ok\nuser: Next?"
            }))),
        )
        .await?;
    assert_ok(&prune, "prune_history");

    let cout = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "compress_output",
                "text": "   Compiling foo v0.1.0\n   Compiling bar v0.1.0\n    Finished `dev` profile\n"
            }))),
        )
        .await?;
    assert_ok(&cout, "compress_output");

    // stats
    let stats = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "stats",
                "reset": false
            }))),
        )
        .await?;
    assert_ok(&stats, "stats");
    assert!(
        gateway_result(&stats)
            .get("total_calls")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 1
    );

    let _ = client
        .call_tool(
            CallToolRequestParams::new("compendium").with_arguments(args_object(json!({
                "action": "cache_invalidate"
            }))),
        )
        .await?;

    client.cancel().await?;
    Ok(())
}
