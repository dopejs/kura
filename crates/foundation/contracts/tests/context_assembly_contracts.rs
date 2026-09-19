//! Context assembly read-API contract fixture (Stage 1.6).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_context_assembly_schema_accepts_canonical_fixture() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/context-assembly.schema.json"##,
        r##"{"assemblyId":"01J...","tenantId":"ten_a","threadId":"thr_1","recordedAt":"2026-09-19T10:00:00Z","record":{"included":[{"assetId":"mem_1","layer":"l3","chars":120,"source":"bootstrap"},{"assetId":"mem_7","layer":"l2","chars":80,"source":"retrieval"}],"excluded":[{"assetId":"mem_2","layer":"l2","reason":"over_budget","source":"bootstrap"},{"assetId":"mem_1","layer":"l3","reason":"already_injected","source":"retrieval"}],"budgetChars":4000,"usedChars":120,"retrievalBudgetChars":2000,"retrievalUsedChars":80}}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
