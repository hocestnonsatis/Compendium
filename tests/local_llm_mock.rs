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

#[tokio::test(flavor = "multi_thread")]
async fn rerank_cross_encoder_rescores_top_n() {
    use compendium::{rerank, RerankItem, RerankOptions};
    use wiremock::matchers::body_string_contains;

    let server = MockServer::start().await;

    // CE sees BM25-ordered top-N; flip so "css" outranks "oauth".
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("css layout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_chat_body("0.95")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("oauth refresh token path"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_chat_body("0.2")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_chat_body("0.4")))
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

    let items = vec![
        RerankItem {
            id: Some("oauth".into()),
            text: "oauth refresh token path implementation".into(),
        },
        RerankItem {
            id: Some("css".into()),
            text: "css layout sidebar widgets".into(),
        },
        RerankItem {
            id: Some("db".into()),
            text: "database migration inventory notes".into(),
        },
        RerankItem {
            id: Some("auth".into()),
            text: "session cookie auth middleware".into(),
        },
    ];

    let result = tokio::task::spawn_blocking(move || {
        rerank(
            "oauth refresh token",
            &items,
            &RerankOptions {
                top_k: Some(2),
                use_embeddings: false,
                use_cross_encoder: Some(true),
                cross_encoder_top_n: Some(4),
                ..Default::default()
            },
            &config,
        )
    })
    .await
    .expect("join");

    assert_eq!(result.backend, "cross_encoder");
    assert!(result.fallback_reason.is_none());
    assert!(result.hits[0].cross_encoder_score.is_some());
    assert_eq!(result.hits[0].id.as_deref(), Some("css"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rerank_cross_encoder_prefers_rerank_api() {
    use compendium::{rerank, RerankItem, RerankOptions};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/rerank"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                { "index": 1, "relevance_score": 0.99 },
                { "index": 0, "relevance_score": 0.1 },
                { "index": 2, "relevance_score": 0.05 },
                { "index": 3, "relevance_score": 0.02 }
            ]
        })))
        .mount(&server)
        .await;

    let config = Config {
        local_llm: LocalLlmConfig {
            enabled: true,
            base_url: Some(format!("{}/v1", server.uri())),
            model: "mock-rerank".into(),
            embedding_model: None,
            api_key: None,
            timeout_secs: 10,
        },
        ..Config::default()
    };

    let items = vec![
        RerankItem {
            id: Some("first".into()),
            text: "oauth refresh token path".into(),
        },
        RerankItem {
            id: Some("second".into()),
            text: "unrelated css widgets".into(),
        },
        RerankItem {
            id: Some("third".into()),
            text: "database notes".into(),
        },
        RerankItem {
            id: Some("fourth".into()),
            text: "inventory migration".into(),
        },
    ];

    let result = tokio::task::spawn_blocking(move || {
        rerank(
            "oauth refresh",
            &items,
            &RerankOptions {
                top_k: Some(2),
                use_embeddings: false,
                use_cross_encoder: Some(true),
                cross_encoder_top_n: Some(4),
                ..Default::default()
            },
            &config,
        )
    })
    .await
    .expect("join");

    assert_eq!(result.backend, "cross_encoder");
    assert_eq!(result.cross_encoder_mode.as_deref(), Some("rerank_api"));
    assert!(result.cross_encoder_ms.is_some());
    // API says local doc index 1 (second candidate in top-N list) wins.
    assert_eq!(result.hits[0].id.as_deref(), Some("second"));
}

#[tokio::test(flavor = "multi_thread")]
async fn embed_cache_avoids_second_http_roundtrip() {
    use compendium::pipeline::local_llm::{clear_process_embed_cache, LocalLlmClient};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "index": 0, "embedding": [1.0, 0.0, 0.0] },
                { "index": 1, "embedding": [0.9, 0.1, 0.0] }
            ],
            "model": "mock-embed",
            "object": "list"
        })))
        .mount(&server)
        .await;

    let base = format!("{}/v1", server.uri());
    let (a, b) = tokio::task::spawn_blocking(move || {
        clear_process_embed_cache();
        let config = LocalLlmConfig {
            enabled: true,
            base_url: Some(base),
            model: "mock-embed".into(),
            embedding_model: Some("mock-embed".into()),
            api_key: None,
            timeout_secs: 10,
        };
        let client = LocalLlmClient::from_config(&config)
            .expect("enabled")
            .expect("client");
        let inputs = vec!["query auth".into(), "doc about auth tokens".into()];
        let a = client.embed("mock-embed", &inputs).expect("embed1");
        let b = client.embed("mock-embed", &inputs).expect("embed2");
        (a, b)
    })
    .await
    .expect("join");

    assert_eq!(a, b);
    let reqs = server.received_requests().await.expect("received");
    let embeds = reqs
        .iter()
        .filter(|r| r.url.path().ends_with("/embeddings"))
        .count();
    assert_eq!(embeds, 1, "second embed must use process cache");
}
