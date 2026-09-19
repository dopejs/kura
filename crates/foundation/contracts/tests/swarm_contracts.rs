//! Swarm run contract fixture (Stage 4).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_swarm_run_schema_accepts_canonical_fixture() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/swarm-run.schema.json"##,
        r##"{"runId":"swm_1","tenantId":"ten_a","requestedBy":"prn_1","provider":"echo","status":"partial_failed","children":[{"index":0,"goal":"summarise the inbox","threadId":"swarm:swm_1:0","status":"completed","dispatchId":"dsp_1","outputPreview":"done","startedAt":"2026-09-19T10:00:00Z","completedAt":"2026-09-19T10:00:02Z"},{"index":1,"goal":"draft the reply","threadId":"swarm:swm_1:1","status":"quota_denied","error":"run_launches_exhausted","completedAt":"2026-09-19T10:00:00Z"}],"createdAt":"2026-09-19T10:00:00Z","updatedAt":"2026-09-19T10:00:02Z","completedAt":"2026-09-19T10:00:02Z"}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
