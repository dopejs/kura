//! Skill lifecycle contract fixtures (Stage 3).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_skill_lifecycle_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/skill-usage.schema.json"##,
            r##"{"skillId":"deploy-check","invocations":12,"helpful":9,"corrected":1,"lastUsedAt":"2026-09-19T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/skill-search-hit.schema.json"##,
            r##"{"skillId":"deploy-check","name":"deploy-check","description":"pre-deploy checklist","rank":1,"usage":{"skillId":"deploy-check","invocations":12,"helpful":9,"corrected":1}}"##,
        ),
        (
            r##"schemas/api/skill-distill-result.schema.json"##,
            r##"{"distillThreadId":"skill-distill:thr_1","dispatchId":"dsp_1","parseError":"no JSON object in the model output"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
