//! Prompt-cache token counts (agent-runtime-program.md 2.3): every usage
//! object accepts the cache split, which is omitted when zero.

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_usage_schemas_accept_prompt_cache_counts() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/chat-query.response.schema.json"##,
            r##"{"dispatchId":"dispatch_1","provider":"anthropic","model":"m","skills":[],"query":"hello","status":"completed","partial":false,"reply":"hi","finishReason":"stop","usage":{"inputTokens":125,"outputTokens":7,"totalTokens":132,"cacheReadInputTokens":100,"cacheCreationInputTokens":20}}"##,
        ),
        (
            r##"schemas/events/llm-dispatch-completed.event.schema.json"##,
            r##"{"eventId":"evt_1","sequence":9,"category":"llm","name":"llm.dispatch.completed","occurredAt":"2026-10-30T12:00:01Z","scope":{},"resource":{"kind":"llm_dispatch","id":"dispatch_1"},"payload":{"provider":"anthropic","model":"m","status":"completed","partial":false,"attemptCount":1,"finishReason":"stop","usage":{"inputTokens":125,"outputTokens":7,"totalTokens":132,"cacheReadInputTokens":100},"errorCode":"","error":"","skills":[],"skillContracts":[]}}"##,
        ),
        (
            r##"schemas/api/turn-trace-resource.schema.json"##,
            r##"{"turnId":"turn_1","status":"ok","startedAt":"2026-10-30T12:00:00Z","usage":{"inputTokens":250,"outputTokens":9,"totalTokens":259,"cacheReadInputTokens":100,"cacheCreationInputTokens":120},"truncated":false,"spans":[{"spanId":0,"kind":"turn","name":"chat.query","status":"ok","startedAt":"2026-10-30T12:00:00Z"}]}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
