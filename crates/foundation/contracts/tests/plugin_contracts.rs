//! Plugin plane contract fixtures (agent pluginization, phase 1).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_plugin_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/plugin/plugin-profile.schema.json"##,
            r##"{"disabled":["channel-discord"],"entries":{"memory":{"enabled":true,"config":{"tickSeconds":60}},"triage":{"enabled":false}}}"##,
        ),
        (r##"schemas/plugin/plugin-profile.schema.json"##, r##"{}"##),
        (
            r##"schemas/plugin/plugin-manifest.schema.json"##,
            r##"{"id":"session-strategy","version":"0.1.0","summary":"external session window","requires":["chat"],"hooks":[{"point":"chat/pre-dispatch","onError":"veto"}],"seams":["context.embedder"],"entry":{"kind":"process","command":"/bin/sh","args":["run.sh"],"timeoutMs":2000}}"##,
        ),
        (
            r##"schemas/plugin/plugin-manifest.schema.json"##,
            r##"{"id":"observer","entry":{"kind":"process","command":"./observer"}}"##,
        ),
        (
            r##"schemas/api/plugins-profile-update.response.schema.json"##,
            r##"{"profile":{"disabled":["channel-discord"],"entries":{"session-strategy":{"enabled":true,"config":{"personalBudgetChars":1000}}}},"restartRequired":true}"##,
        ),
        (
            r##"schemas/api/plugins-report.schema.json"##,
            r##"{"plugins":[{"id":"llm","summary":"LLM dispatcher","source":"builtin","enabled":true,"provides":["llm.dispatcher"],"requires":[]},{"id":"webhooks","summary":"Webhook ingress","source":"builtin","enabled":false,"reason":"requires disabled plugin `billing`","provides":["webhooks.manager"],"requires":["billing"]}],"warnings":["profile disables unknown plugin `ghost`"]}"##,
        ),
        (
            r##"schemas/api/plugins-report.schema.json"##,
            r##"{"plugins":[],"warnings":[],"hooks":[{"point":"chat/pre-dispatch","pluginId":"session-strategy"},{"point":"chat/turn-end","pluginId":"memory"}]}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

/// The report serialized by kura-plugin itself round-trips through the
/// schema — the wire contract and the Rust type cannot drift silently.
#[test]
fn test_resolved_report_matches_schema() {
    let report = kura_plugin::resolve(
        &[
            kura_plugin::PluginDescriptor {
                id: "alpha",
                summary: "base",
                provides: &["alpha.svc"],
                requires: &[],
            },
            kura_plugin::PluginDescriptor {
                id: "beta",
                summary: "dependent",
                provides: &[],
                requires: &["alpha"],
            },
        ],
        &kura_plugin::PluginProfile {
            disabled: vec!["alpha".to_string(), "ghost".to_string()],
            ..Default::default()
        },
    );
    let encoded = serde_json::to_vec(&report).expect("encode report");
    Validator::new(schema_root_dir())
        .validate_relative("schemas/api/plugins-report.schema.json", &encoded)
        .expect("resolved report validates against plugins-report schema");
}
