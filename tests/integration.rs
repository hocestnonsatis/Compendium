//! Integration-style tests for the public pipeline API and MCP tool wiring.

use compendium::{
    chunk_with_refs, compress, compress_output, count_tokens_detailed, filter, parse_history_input,
    prune_history, summarize, ChunkOptions, CompressOptions, Config, FilterOptions, PruneOptions,
    SummarizeOptions,
};
use compendium::pipeline::compress::ContentType;
use compendium::pipeline::output::CompressOutputOptions;
use compendium::pipeline::prune::PruneStrategy;
use compendium::pipeline::summarize::SummarizeMode;
use compendium::CompendiumServer;

#[test]
fn end_to_end_pipeline_reduces_tokens() {
    let config = Config::default();
    let noisy = r#"
[32mINFO[0m starting worker
[32mINFO[0m starting worker
[32mINFO[0m starting worker

{
  "users": [
    {"id": 1, "name": "Ada", "bio": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"},
    {"id": 2, "name": "Grace", "bio": "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"}
  ],
  "meta": {"page": 1, "total": 2}
}
"#;

    let filtered = filter(noisy, &FilterOptions::default(), &config);
    assert!(
        filtered.metrics.result_tokens <= filtered.metrics.original_tokens,
        "filter should not inflate tokens"
    );

    let compressed = compress(
        noisy,
        &CompressOptions {
            content_type: ContentType::Auto,
            max_tokens: Some(400),
            ..Default::default()
        },
        &config,
    );
    assert!(compressed.metrics.reduction_ratio >= 0.0);
    assert!(!compressed.content.is_empty());

    let summary = summarize(
        "# Root\n## Child\nDetails about the child section go here with enough length.\n## Other\nMore details living in another section for outline mode.",
        &SummarizeOptions {
            mode: SummarizeMode::Outline,
            ..Default::default()
        },
        &config,
    );
    assert!(summary.summary.contains("Root") || !summary.sections.is_empty());

    let map = chunk_with_refs(
        &"lorem ipsum dolor sit amet. ".repeat(300),
        &ChunkOptions {
            chunk_tokens: 64,
            overlap_tokens: 8,
            source: Some("mem://test".into()),
            ..Default::default()
        },
        &config,
    );
    assert!(map.chunks.len() >= 2);
    assert!(map.index_text.contains("refmap"));

    let counted = count_tokens_detailed("hello world", &config);
    assert!(counted.tokens >= 1);

    let pruned = prune_history(
        &parse_history_input("user: How?\nassistant: Like this.\nuser: ok\nuser: And more?"),
        &PruneOptions {
            strategy: PruneStrategy::Remove,
            ..Default::default()
        },
        &config,
    );
    assert!(pruned.dropped >= 1);

    let out = compress_output(
        "   Compiling a v0.1.0\n   Compiling b v0.1.0\n    Finished `dev` profile\n",
        &CompressOutputOptions::default(),
        &config,
    );
    assert_eq!(out.domain, "cargo");
}

#[test]
fn server_exposes_single_gateway_tool() {
    let _server = CompendiumServer::new(Config::default());
    let names = CompendiumServer::tool_names();
    assert_eq!(
        names,
        vec!["compendium".to_string()],
        "expected a single gateway tool; have {names:?}"
    );
}
