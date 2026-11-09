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

/// Slice 2: an IM-opened item with its owner subscription, one dropped
/// subscription, a steered input; and the woken, stopped, mandate_revoked
/// and attention_cleared events.
const IM_ITEM: &str = r##"{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d3","tenantId":"ten_a","title":"Fix the flaky build","state":"open","stage":"working","threadId":"thr_wi_0193a1b2c3d4e5f6a7b8c9d3","seq":2,"mandate":{"basis":"im_mention","strength":"explicit","policyHash":"","matchedRules":[],"sender":{"connectorId":"slack-main","authorId":"U1"},"source":{"kind":"im","ref":"delivery_1","conversation":"C1"},"receivingPolicyId":"route_policy_1","scopeText":"Fix CI on main.","createdAt":"2026-11-03T10:00:00Z"},"createdAt":"2026-11-03T10:00:00Z","updatedAt":"2026-11-03T10:05:00Z","inputs":[{"seq":1,"kind":"message","authorIsPerson":true,"body":"also bump the lockfile","sourceRef":"delivery_2","state":"steered","roundSeq":2,"createdAt":"2026-11-03T10:05:00Z"}],"subscriptions":[{"subscriptionId":"wis_0193a1b2c3d4","sourceKind":"im","sourceKey":"thr_src_0a1b2c3d4e5f6a7b8c9d0e1f","connectorId":"slack-main","conversationId":"C1","role":"owner","createdAt":"2026-11-03T10:00:00Z","lastEventAt":"2026-11-03T10:05:00Z"},{"subscriptionId":"wis_0193a1b2c3d5","sourceKind":"im","sourceKey":"thr_src_ffff","role":"owner","createdAt":"2026-10-01T10:00:00Z","lastEventAt":"2026-10-01T10:00:00Z","droppedAt":"2026-10-20T10:00:00Z","dropReason":"idle"}]}"##;

#[test]
fn test_work_item_slice2_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (r##"schemas/api/work-item-resource.schema.json"##, IM_ITEM),
        (
            r##"schemas/events/work-item-woken.event.schema.json"##,
            r##"{"eventId":"evt_5","sequence":5,"tenantId":"ten_a","category":"work_item","name":"work_item.woken","occurredAt":"2026-11-03T10:05:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"im_mention","inputSeq":1,"kind":"message","path":"steered","resumed":false,"roundSeq":2}}"##,
        ),
        (
            r##"schemas/events/work-item-stopped.event.schema.json"##,
            r##"{"eventId":"evt_6","sequence":6,"tenantId":"ten_a","category":"work_item","name":"work_item.stopped","occurredAt":"2026-11-03T10:06:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"im_mention","roundCancelled":true}}"##,
        ),
        (
            r##"schemas/events/work-item-mandate-revoked.event.schema.json"##,
            r##"{"eventId":"evt_7","sequence":7,"tenantId":"ten_a","category":"work_item","name":"work_item.mandate_revoked","occurredAt":"2026-11-03T10:07:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"im_direct","reason":"sender_not_allowed"}}"##,
        ),
        (
            r##"schemas/events/work-item-attention-cleared.event.schema.json"##,
            r##"{"eventId":"evt_8","sequence":8,"tenantId":"ten_a","category":"work_item","name":"work_item.attention_cleared","occurredAt":"2026-11-03T10:08:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","roundSeq":3}}"##,
        ),
        (
            r##"schemas/events/connector-route-outcome-recorded.event.schema.json"##,
            r##"{"eventId":"evt_9","sequence":9,"category":"connector","name":"connector.route_outcome_recorded","occurredAt":"2026-11-03T10:05:00Z","scope":{"connectorId":"slack-main"},"resource":{"kind":"connector","id":"slack-main"},"payload":{"tenantId":"ten_a","connectorId":"slack-main","outcome":"work_item","reasonCode":"work_item_round","surface":"room","messageDeliveryId":"delivery_2","replyMessageIds":["m1"],"redactionStatus":"redacted"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);

    // A woken event never carries what was said, and a revocation names
    // one of its known reasons.
    for (schema, bad) in [
        (
            "schemas/events/work-item-woken.event.schema.json",
            r##"{"eventId":"evt_5","sequence":5,"tenantId":"ten_a","category":"work_item","name":"work_item.woken","occurredAt":"2026-11-03T10:05:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"im_mention","inputSeq":1,"kind":"message","path":"queued","body":"secret"}}"##,
        ),
        (
            "schemas/events/work-item-mandate-revoked.event.schema.json",
            r##"{"eventId":"evt_7","sequence":7,"tenantId":"ten_a","category":"work_item","name":"work_item.mandate_revoked","occurredAt":"2026-11-03T10:07:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"im_direct","reason":"because"}}"##,
        ),
    ] {
        assert!(
            validator.validate_relative(schema, bad.as_bytes()).is_err(),
            "{schema} accepted {bad}"
        );
    }
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

/// Slice 3: an item whose outbox holds a sent reply, a failed one waiting
/// for its retry, one queued behind it and one dropped; and the
/// outbox_dropped event.
const OUTBOX_ITEM: &str = r##"{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d4","tenantId":"ten_a","title":"Fix the flaky build","state":"open","stage":"person/delivery","threadId":"thr_wi_0193a1b2c3d4e5f6a7b8c9d4","seq":2,"mandate":{"basis":"im_mention","strength":"explicit","policyHash":"","matchedRules":[],"sender":{"connectorId":"slack-main","authorId":"U1"},"source":{"kind":"im","ref":"delivery_1","conversation":"C1"},"scopeText":"Fix CI on main.","createdAt":"2026-11-03T10:00:00Z"},"createdAt":"2026-11-03T10:00:00Z","updatedAt":"2026-11-03T10:05:00Z","outbox":[{"seq":1,"conversationKey":"thr_src_0a1b2c3d4e5f6a7b8c9d0e1f","connectorId":"slack-main","conversationId":"C1","roundSeq":1,"body":"Found the flaky test.","state":"sent","attempts":0,"externalMessageId":"1730628000.000100","createdAt":"2026-11-03T10:01:00Z","sentAt":"2026-11-03T10:01:01Z"},{"seq":2,"conversationKey":"thr_src_0a1b2c3d4e5f6a7b8c9d0e1f","connectorId":"slack-main","conversationId":"C1","roundSeq":2,"body":"Pushed the fix.","state":"failed","attempts":3,"nextAttemptAt":"2026-11-03T10:06:00Z","lastError":"send_failed","createdAt":"2026-11-03T10:05:00Z"},{"seq":3,"conversationKey":"thr_src_0a1b2c3d4e5f6a7b8c9d0e1f","connectorId":"slack-main","body":"Added to the work item's running round.","state":"queued","attempts":0,"nextAttemptAt":"2026-11-03T10:05:10Z","createdAt":"2026-11-03T10:05:10Z"},{"seq":4,"conversationKey":"thr_src_ffff","connectorId":"slack-main","body":"Done.","state":"dropped","attempts":1,"lastError":"conversation_gone","dropReason":"conversation_gone","createdAt":"2026-11-03T10:02:00Z","droppedAt":"2026-11-03T10:02:05Z"}]}"##;

#[test]
fn test_work_item_slice3_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/work-item-resource.schema.json"##,
            OUTBOX_ITEM,
        ),
        (
            r##"schemas/events/work-item-outbox-dropped.event.schema.json"##,
            r##"{"eventId":"evt_10","sequence":10,"tenantId":"ten_a","category":"work_item","name":"work_item.outbox_dropped","occurredAt":"2026-11-03T10:02:05Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"im_mention","outboxSeq":4,"reason":"conversation_gone","attempts":1,"connectorId":"slack-main","roundSeq":2}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_work_item_outbox_dropped_event_refuses_a_body_or_an_unknown_reason() {
    let validator = Validator::new(schema_root_dir());
    for bad in [
        r##"{"eventId":"evt_11","sequence":11,"tenantId":"ten_a","category":"work_item","name":"work_item.outbox_dropped","occurredAt":"2026-11-03T10:02:05Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","outboxSeq":1,"reason":"withdrawn","attempts":0,"connectorId":"slack-main","body":"secret"}}"##,
        r##"{"eventId":"evt_12","sequence":12,"tenantId":"ten_a","category":"work_item","name":"work_item.outbox_dropped","occurredAt":"2026-11-03T10:02:05Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","outboxSeq":1,"reason":"timeout","attempts":0,"connectorId":"slack-main"}}"##,
    ] {
        assert!(
            validator
                .validate_relative(
                    "schemas/events/work-item-outbox-dropped.event.schema.json",
                    bad.as_bytes()
                )
                .is_err(),
            "accepted {bad}"
        );
    }
}
