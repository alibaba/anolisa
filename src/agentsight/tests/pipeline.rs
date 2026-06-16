//! Pipeline integration tests: feed synthetic SslEvents through the full
//! parser → aggregator → analyzer → genai builder → SQLite chain.
//!
//! No BPF probes needed — exercises the data path with in-memory components.

use std::collections::HashMap;

mod common;

use agentsight::aggregator::Aggregator;
use agentsight::analyzer::Analyzer;
use agentsight::event::Event;
use agentsight::genai::GenAIBuilder;
use agentsight::genai::semantic::GenAISemanticEvent;
use agentsight::parser::Parser;
use agentsight::response_map::ResponseSessionMapper;

/// Run a sequence of SslEvents through the full pipeline, return any GenAI events produced.
fn run_pipeline(ssl_events: Vec<(u32, u64, i32, Vec<u8>, &str)>) -> Vec<GenAISemanticEvent> {
    let parser = Parser::new();
    let mut aggregator = Aggregator::new();
    let analyzer = Analyzer::new();
    let builder = GenAIBuilder::new();
    let mapper = ResponseSessionMapper::new();
    let pid_cache: HashMap<u32, String> = HashMap::new();

    let mut all_genai_events = Vec::new();

    for (pid, ssl_ptr, rw, buf, comm) in ssl_events {
        let ssl_event = common::make_ssl_event(pid, ssl_ptr, rw, buf, comm);
        let event = Event::Ssl(ssl_event);

        let parse_result = parser.parse_event(event);
        let aggregated_results = aggregator.process_result(parse_result);

        for agg_result in &aggregated_results {
            let analysis_results = analyzer.analyze_aggregated(agg_result);
            let (output, _pending_info) =
                builder.build_with_pending(&analysis_results, &mapper, &pid_cache);

            all_genai_events.extend(output.events);
        }
    }

    all_genai_events
}

#[test]
fn test_openai_sse_pipeline() {
    let pid = 5000u32;
    let ssl_ptr = 0xA000u64;
    let comm = "node";

    let request_bytes = common::make_openai_request_bytes("gpt-4o", "Say hello in 3 words", true);
    let resp_headers = common::make_openai_sse_response_headers();
    let sse_chunk =
        common::make_openai_sse_chunk("chatcmpl-test-001", "gpt-4o", "Hello there friend!", 10, 5);
    let sse_done = common::make_sse_done();

    let events = vec![
        (pid, ssl_ptr, 1, request_bytes, comm),
        (pid, ssl_ptr, 0, resp_headers, comm),
        (pid, ssl_ptr, 0, sse_chunk, comm),
        (pid, ssl_ptr, 0, sse_done, comm),
    ];

    let genai_events = run_pipeline(events);

    assert!(
        !genai_events.is_empty(),
        "pipeline should produce at least one GenAI event"
    );

    let call = match &genai_events[0] {
        GenAISemanticEvent::LLMCall(call) => call,
        other => panic!("expected LLMCall, got {other:?}"),
    };

    assert_eq!(call.provider, "openai");
    assert_eq!(call.model, "gpt-4o");
    assert_eq!(call.pid, pid as i32);
    assert_eq!(call.process_name, comm);
}

#[test]
fn test_openai_non_streaming_pipeline() {
    let pid = 5001u32;
    let ssl_ptr = 0xB000u64;
    let comm = "python3";

    let request_bytes = common::make_openai_request_bytes("gpt-4o-mini", "Count to 3", false);
    let response_bytes = common::make_openai_json_response_bytes(
        "chatcmpl-test-002",
        "gpt-4o-mini",
        "1, 2, 3.",
        8,
        4,
    );

    let events = vec![
        (pid, ssl_ptr, 1, request_bytes, comm),
        (pid, ssl_ptr, 0, response_bytes, comm),
    ];

    let result = run_pipeline(events);

    assert!(
        !result.is_empty(),
        "non-streaming pipeline should produce GenAI events"
    );

    let call = match &result[0] {
        GenAISemanticEvent::LLMCall(call) => call,
        other => panic!("expected LLMCall, got {other:?}"),
    };

    assert_eq!(call.provider, "openai");
    assert_eq!(call.model, "gpt-4o-mini");

    // Token usage must be extracted from the non-streaming JSON `usage` field
    // (the fix in #789: extract_token_from_json_body on HttpComplete). Before
    // that fix, non-streaming responses reported zero/absent token usage.
    let usage = call
        .token_usage
        .as_ref()
        .expect("non-streaming call must carry token usage");
    assert_eq!(usage.input_tokens, 8, "input tokens from JSON usage");
    assert_eq!(usage.output_tokens, 4, "output tokens from JSON usage");
}

#[test]
fn test_anthropic_sse_pipeline() {
    let pid = 5002u32;
    let ssl_ptr = 0xC000u64;
    let comm = "claude";

    let request_bytes = common::make_anthropic_request_bytes("claude-sonnet-4-20250514", "Say hi");
    let chunks = common::make_anthropic_sse_chunks(
        "msg_test_003",
        "claude-sonnet-4-20250514",
        "Hi there!",
        12,
        3,
    );

    let mut events = vec![(pid, ssl_ptr, 1, request_bytes, comm)];
    for chunk in chunks {
        events.push((pid, ssl_ptr, 0, chunk, comm));
    }

    let result = run_pipeline(events);

    assert!(
        !result.is_empty(),
        "Anthropic SSE pipeline should produce GenAI events"
    );

    let call = match &result[0] {
        GenAISemanticEvent::LLMCall(call) => call,
        other => panic!("expected LLMCall, got {other:?}"),
    };

    assert_eq!(call.provider, "anthropic");
    assert!(
        call.model.contains("claude"),
        "model should contain 'claude', got: {}",
        call.model
    );
}
