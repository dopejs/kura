//! Config file write-path contract fixture (Stage 7.1).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_config_file_write_schema_accepts_canonical_fixture() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/config-file-write.schema.json"##,
        r##"{"applied":true,"dryRun":false,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","restartRequired":true,"hotApplied":[],"restartRequiredFor":["llm","logLevel"],"warnings":["unknown key `llm.defaultModle` is ignored on load"]}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
