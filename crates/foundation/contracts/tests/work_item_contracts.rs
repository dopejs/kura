//! Work item contract fixtures (docs/harness/work-items.md, slice 1).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

const ITEM: &str = r##"{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d0","tenantId":"ten_a","title":"Fix the flaky build","state":"open","stage":"person/answer","threadId":"thr_wi_0193a1b2c3d4e5f6a7b8c9d0","provider":"echo","seq":1,"mandate":{"basis":"api","strength":"explicit","policyHash":"","matchedRules":[],"sender":{"principalId":"prn_1"},"source":{"kind":"api","ref":"req_1"},"scopeText":"Find out why CI fails on main and fix it.","createdAt":"2026-11-03T10:00:00Z"},"lastReport":{"status":"blocked","summary":"Two fixes are possible.","attention":{"question":"Pin the toolchain or patch the test?","options":["pin","patch"]}},"attention":{"question":"Pin the toolchain or patch the test?","options":["pin","patch"]},"createdAt":"2026-11-03T10:00:00Z","updatedAt":"2026-11-03T10:00:30Z","rounds":[{"seq":1,"requestId":"wi_0193a1b2c3d4e5f6a7b8c9d0_r1","state":"settled","queryText":"You are working on a work item.","outcome":"blocked","report":{"status":"blocked","summary":"Two fixes are possible."},"turnId":"trn_1","startedAt":"2026-11-03T10:00:00Z","finishedAt":"2026-11-03T10:00:30Z"}],"inputs":[{"seq":1,"kind":"text","authorIsPerson":true,"body":"pin it","state":"pending","createdAt":"2026-11-03T10:01:00Z"}]}"##;

const UNREADABLE_ITEM: &str = r##"{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d1","tenantId":"ten_a","title":"Summarise the inbox","state":"open","stage":"person/blocked","threadId":"thr_wi_0193a1b2c3d4e5f6a7b8c9d1","seq":1,"mandate":{"basis":"api","strength":"explicit","policyHash":"","matchedRules":[],"sender":{},"source":{"kind":"api"},"scopeText":"Summarise today's mail.","createdAt":"2026-11-03T10:00:00Z"},"lastReport":{"status":"blocked","summary":"The round's report could not be read.","reason":"report_unreadable"},"createdAt":"2026-11-03T10:00:00Z","updatedAt":"2026-11-03T10:00:30Z"}"##;

const ENDED_ITEM: &str = r##"{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d2","tenantId":"ten_a","title":"Done thing","state":"ended","endReason":"completed","threadId":"thr_wi_0193a1b2c3d4e5f6a7b8c9d2","seq":1,"mandate":{"basis":"api","strength":"explicit","policyHash":"","matchedRules":[],"sender":{"principalId":"prn_1"},"source":{"kind":"api"},"scopeText":"Do it.","createdAt":"2026-11-03T10:00:00Z"},"lastReport":{"status":"ready","summary":"Done."},"createdAt":"2026-11-03T10:00:00Z","updatedAt":"2026-11-03T10:00:30Z"}"##;

#[test]
fn test_work_item_api_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let list = format!(r#"{{"items":[{UNREADABLE_ITEM},{ENDED_ITEM}]}}"#);
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/create-work-item.request.schema.json"##,
            r##"{"title":"Fix the flaky build","scope":"Find out why CI fails on main and fix it.","provider":"echo"}"##,
        ),
        (r##"schemas/api/work-item-resource.schema.json"##, ITEM),
        (
            r##"schemas/api/work-item-resource.schema.json"##,
            UNREADABLE_ITEM,
        ),
        (
            r##"schemas/api/work-item-resource.schema.json"##,
            ENDED_ITEM,
        ),
        (
            r##"schemas/api/work-item-report.schema.json"##,
            r##"{"status":"waiting","summary":"Waiting for CI.","attention":{"question":"Merge when green?"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
    validator
        .validate_relative(
            "schemas/api/work-item-list.response.schema.json",
            list.as_bytes(),
        )
        .expect("work item list");
}

#[test]
fn test_work_item_report_schema_refuses_what_the_daemon_must_not_accept() {
    let validator = Validator::new(schema_root_dir());
    for bad in [
        r#"{"status":"done","summary":"x"}"#,
        r#"{"status":"ready"}"#,
        r#"{"status":"ready","summary":""}"#,
        r#"{"status":"ready","summary":"x","reason":"report_unreadable"}"#,
        r#"{"status":"blocked","summary":"x","attention":{"options":["a"]}}"#,
    ] {
        assert!(
            validator
                .validate_relative("schemas/api/work-item-report.schema.json", bad.as_bytes())
                .is_err(),
            "accepted {bad}"
        );
    }
}

#[test]
fn test_work_item_event_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/events/work-item-opened.event.schema.json"##,
            r##"{"eventId":"evt_1","sequence":1,"tenantId":"ten_a","category":"work_item","name":"work_item.opened","occurredAt":"2026-11-03T10:00:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","strength":"explicit","threadId":"thr_wi_1","roundSeq":1,"requestId":"wi_1_r1"}}"##,
        ),
        (
            r##"schemas/events/work-item-round-settled.event.schema.json"##,
            r##"{"eventId":"evt_2","sequence":2,"tenantId":"ten_a","category":"work_item","name":"work_item.round_settled","occurredAt":"2026-11-03T10:00:30Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","roundSeq":1,"requestId":"wi_1_r1","state":"settled","outcome":"report_unreadable","stage":"person/blocked","turnId":"trn_1"}}"##,
        ),
        (
            r##"schemas/events/work-item-attention-raised.event.schema.json"##,
            r##"{"eventId":"evt_3","sequence":3,"tenantId":"ten_a","category":"work_item","name":"work_item.attention_raised","occurredAt":"2026-11-03T10:00:30Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","roundSeq":1,"optionCount":2}}"##,
        ),
        (
            r##"schemas/events/work-item-ended.event.schema.json"##,
            r##"{"eventId":"evt_4","sequence":4,"tenantId":"ten_a","category":"work_item","name":"work_item.ended","occurredAt":"2026-11-03T10:00:30Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","endReason":"completed","roundSeq":1}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_work_item_events_carry_their_tenant() {
    let validator = Validator::new(schema_root_dir());
    let tenantless = r##"{"eventId":"evt_1","sequence":1,"category":"work_item","name":"work_item.opened","occurredAt":"2026-11-03T10:00:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","strength":"explicit","threadId":"thr_wi_1"}}"##;
    assert!(
        validator
            .validate_relative(
                "schemas/events/work-item-opened.event.schema.json",
                tenantless.as_bytes()
            )
            .is_err()
    );
}
