//! Smart actions against a mock OpenAI-compatible local endpoint.

use compendium::{
    filter_relevant, summarize_smart, Config, LocalLlmConfig, SmartBackend, SmartOptions,
    SummarizeOptions,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn openai_chat_body(content: &str) -> serde_json::Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn summarize_smart_uses_local_llm_when_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai_chat_body("# Summary\n- local model wrote this\n")),
        )
        .mount(&server)
        .await;

    let config = Config {
        local_llm: LocalLlmConfig {
            enabled: true,
            base_url: Some(format!("{}/v1", server.uri())),
            model: "mock-model".into(),
            embedding_model: None,
            api_key: None,
            timeout_secs: 10,
        },
        ..Config::default()
    };

    let result = tokio::task::spawn_blocking(move || {
        summarize_smart(
            "long source document that should be summarized by the mock model",
            &SmartOptions {
                max_tokens: Some(128),
                fallback: false,
                force: true,
                ..Default::default()
            },
            &SummarizeOptions {
                force: true,
                ..Default::default()
            },
            &config,
        )
    })
    .await
    .expect("join")
    .expect("summarize_smart");

    assert_eq!(result.backend, SmartBackend::LocalLlm);
    assert_eq!(result.model.as_deref(), Some("mock-model"));
    assert!(result.deterministic);
    assert!(result.summary.contains("local model wrote this"));
}

#[tokio::test(flavor = "multi_thread")]
async fn filter_relevant_uses_local_llm_when_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai_chat_body("ERROR auth failed\nWARN auth retry")),
        )
        .mount(&server)
        .await;

    let config = Config {
        local_llm: LocalLlmConfig {
            enabled: true,
            base_url: Some(format!("{}/v1", server.uri())),
            model: "mock-model".into(),
            embedding_model: None,
            api_key: Some("secret".into()),
            timeout_secs: 10,
        },
        ..Config::default()
    };

    let result = tokio::task::spawn_blocking(move || {
        filter_relevant(
            "INFO boot\nERROR auth failed\nDEBUG noise\nWARN auth retry\n",
            "auth failures",
            &SmartOptions {
                max_tokens: Some(64),
                fallback: false,
                force: true,
                ..Default::default()
            },
            &config,
        )
    })
    .await
    .expect("join")
    .expect("filter_relevant");

    assert_eq!(result.backend, SmartBackend::LocalLlm);
    assert!(result.content.contains("auth"));
    assert!(!result.content.contains("DEBUG"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rerank_hybrid_uses_embeddings_when_configured() {
    use compendium::{rerank, RerankItem, RerankOptions};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "mock-embed" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "index": 0, "embedding": [1.0, 0.0, 0.0] },
                { "index": 1, "embedding": [0.1, 0.9, 0.0] },
                { "index": 2, "embedding": [0.95, 0.05, 0.0] },
                { "index": 3, "embedding": [0.0, 1.0, 0.0] }
            ]
        })))
        .mount(&server)
        .await;

    let config = Config {
        local_llm: LocalLlmConfig {
            enabled: true,
            base_url: Some(format!("{}/v1", server.uri())),
            model: "mock-chat".into(),
            embedding_model: Some("mock-embed".into()),
            api_key: None,
            timeout_secs: 10,
        },
        ..Config::default()
    };

    let items = vec![
        RerankItem {
            id: Some("noise".into()),
            text: "css layout sidebar".into(),
        },
        RerankItem {
            id: Some("auth".into()),
            text: "auth token refresh".into(),
        },
        RerankItem {
            id: Some("other".into()),
            text: "database inventory".into(),
        },
    ];

    let result = tokio::task::spawn_blocking(move || {
        rerank(
            "auth token",
            &items,
            &RerankOptions {
                top_k: Some(2),
                use_embeddings: true,
                alpha: Some(0.3),
                ..Default::default()
            },
            &config,
        )
    })
    .await
    .expect("join");

    assert_eq!(result.backend, "hybrid");
    assert!(result.fallback_reason.is_none());
    assert_eq!(result.hits[0].id.as_deref(), Some("auth"));
}
