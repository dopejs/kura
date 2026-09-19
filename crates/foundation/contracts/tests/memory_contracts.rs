//! Memory plane contract fixtures (Roadmap 78, spec 058).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_memory_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/create-memory-asset.request.schema.json"##,
            r##"{"kind":"chat_memory","layer":"l1","owner":{"kind":"operator","id":"op_1"},"atomType":"preference","title":"reply language","content":"The user prefers Chinese replies.","sourceLinks":[{"kind":"thread","id":"thr_1","excerpt":"用中文回复"}]}"##,
        ),
        (
            r##"schemas/api/memory-asset-resource.schema.json"##,
            r##"{"assetId":"mem_1","kind":"chat_memory","layer":"l1","tenantId":"ten_a","owner":{"kind":"system","id":"consolidator"},"visibility":"private","status":"ready","version":1,"atomType":"preference","title":"reply language","content":"The user prefers Chinese replies.","sourceLinks":[{"kind":"message","id":"msg_1"}],"createdAt":"2026-08-17T10:00:00Z","updatedAt":"2026-08-17T10:00:00Z","readyAt":"2026-08-17T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/memory-asset-resource.schema.json"##,
            r##"{"assetId":"mem_2","kind":"chat_memory","layer":"l3","owner":{"kind":"system","id":"consolidator"},"visibility":"private","status":"ready","version":2,"supersedesAssetId":"mem_prev","title":"persona","content":"Chinese-speaking operator.","memberAssetIds":["mem_l2_1"],"createdAt":"2026-08-17T10:00:00Z","updatedAt":"2026-08-17T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/memory-consolidation-run.schema.json"##,
            r##"{"runId":"memrun_1","tenantId":"ten_a","trigger":"turns","extractedL1":3,"aggregatedL2":1,"distilledL3":0,"pendingApproval":0,"startedAt":"2026-08-17T10:00:00Z","completedAt":"2026-08-17T10:00:02Z"}"##,
        ),
        (
            r##"schemas/api/memory-overview.schema.json"##,
            r##"{"tenantId":"ten_a","counts":[{"layer":"l1","status":"ready","count":12},{"layer":"l1","status":"revoked","count":3}],"derivedEmbeddings":12,"recentlyRemembered":[{"assetId":"mem_1","layer":"l1","status":"ready","title":"reply language","updatedAt":"2026-09-18T10:00:00Z"}],"recentlyForgotten":[{"assetId":"mem_2","layer":"l1","status":"revoked","title":"stale fact","updatedAt":"2026-09-18T10:01:00Z"}]}"##,
        ),
        (
            r##"schemas/api/memory-index-rebuild.schema.json"##,
            r##"{"tenantId":"ten_a","clearedEmbeddings":12}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
