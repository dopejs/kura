//! Session frame contract fixture (Stage 5.1).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_session_frame_schema_accepts_canonical_fixture() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/session-frame.schema.json"##,
        r##"{"threadId":"thr_1","tenantId":"ten_a","goal":"ship the Q4 report","constraints":["no external sends","cite sources"],"updatedAt":"2026-09-19T10:00:00Z"}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
