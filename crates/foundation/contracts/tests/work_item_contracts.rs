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

/// Slice 4: an item whose round 2 a restart interrupted and the boot did
/// not rerun, and whose round 3 the next boot reran with its recovery note;
/// and the interrupted round_settled event.
const RECOVERED_ITEM: &str = r##"{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d5","tenantId":"ten_a","title":"Fix the flaky build","state":"open","stage":"working","threadId":"thr_wi_0193a1b2c3d4e5f6a7b8c9d5","seq":3,"mandate":{"basis":"api","strength":"explicit","policyHash":"","matchedRules":[],"sender":{"principalId":"prn_1"},"source":{"kind":"api"},"scopeText":"Fix CI on main.","createdAt":"2026-11-03T10:00:00Z"},"createdAt":"2026-11-03T10:00:00Z","updatedAt":"2026-11-03T12:00:00Z","rounds":[{"seq":2,"requestId":"wi_0193a1b2c3d4e5f6a7b8c9d5_r2","state":"failed","queryText":"This is round 2 of the work item.","outcome":"interrupted","errorCode":"interrupted","startedAt":"2026-11-03T10:05:00Z","finishedAt":"2026-11-03T11:00:00Z"},{"seq":3,"requestId":"wi_0193a1b2c3d4e5f6a7b8c9d5_r3","state":"running","queryText":"This is round 3 of the work item.","startedAt":"2026-11-03T12:00:00Z"}],"inputs":[{"seq":4,"kind":"recovery","authorIsPerson":false,"body":"The daemon restarted while you were working on this round.","state":"consumed","roundSeq":3,"createdAt":"2026-11-03T12:00:00Z"}]}"##;

#[test]
fn test_work_item_slice4_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let interrupted = r##"{"eventId":"evt_13","sequence":13,"tenantId":"ten_a","category":"work_item","name":"work_item.round_settled","occurredAt":"2026-11-03T11:00:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","roundSeq":2,"requestId":"wi_1_r2","state":"failed","outcome":"interrupted","stage":"person/delivery","reason":"recover_off"}}"##;
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/work-item-resource.schema.json"##,
            RECOVERED_ITEM,
        ),
        (
            r##"schemas/events/work-item-round-settled.event.schema.json"##,
            interrupted,
        ),
    ];
    validate_fixtures(&validator, fixtures);
    let unknown = interrupted.replace("recover_off", "because");
    assert!(
        validator
            .validate_relative(
                "schemas/events/work-item-round-settled.event.schema.json",
                unknown.as_bytes()
            )
            .is_err(),
        "accepted an unknown reason"
    );
}

/// Slice 5: an item waiting on the approval its round's tool call gave up
/// on, with the link and the approval input of an earlier, used approval;
/// the attention queue; the answer request; and the attention and woken
/// events for an approval.
const APPROVAL_ITEM: &str = r##"{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d6","tenantId":"ten_a","title":"Deploy","state":"open","stage":"person/approval","threadId":"thr_wi_0193a1b2c3d4e5f6a7b8c9d6","seq":2,"mandate":{"basis":"api","strength":"explicit","policyHash":"","matchedRules":[],"sender":{"principalId":"prn_1"},"source":{"kind":"api"},"scopeText":"Deploy main to prod.","createdAt":"2026-11-09T10:00:00Z"},"lastReport":{"status":"waiting","summary":"Waiting for a person to approve the deploy.","attention":{"question":"Deploy now?"}},"attention":{"kind":"approval","question":"run advance with {\"target\":\"prod\"}","approvalId":"approval_0a1b2c3d4e5f6a7b"},"createdAt":"2026-11-09T10:00:00Z","updatedAt":"2026-11-09T10:05:00Z","inputs":[{"seq":1,"kind":"approval","authorIsPerson":false,"body":"A person approved the tool call you asked about.","sourceRef":"approval_ffff","state":"consumed","roundSeq":2,"createdAt":"2026-11-09T10:04:00Z"}],"approvals":[{"approvalId":"approval_ffff","roundSeq":1,"state":"used","question":"run advance with {\"target\":\"staging\"}","createdAt":"2026-11-09T10:01:00Z","resolvedAt":"2026-11-09T10:03:00Z","usedAt":"2026-11-09T10:04:30Z"},{"approvalId":"approval_0a1b2c3d4e5f6a7b","roundSeq":2,"state":"pending","question":"run advance with {\"target\":\"prod\"}","createdAt":"2026-11-09T10:05:00Z"}]}"##;

#[test]
fn test_work_item_slice5_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let queue = r##"{"items":[{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d6","title":"Deploy","stage":"person/approval","basis":"api","attention":{"kind":"approval","question":"run advance","approvalId":"approval_0a1b2c3d4e5f6a7b"},"summary":"Waiting for a person to approve the deploy.","updatedAt":"2026-11-09T10:05:00Z"},{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d7","title":"Fix CI","stage":"person/answer","basis":"im_mention","attention":{"kind":"question","question":"Pin or patch?","options":["pin","patch"]},"summary":"Two fixes.","updatedAt":"2026-11-09T10:06:00Z"},{"itemId":"wi_0193a1b2c3d4e5f6a7b8c9d8","title":"Mail","stage":"person/delivery","basis":"im_direct","updatedAt":"2026-11-09T10:07:00Z"}]}"##;
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/work-item-resource.schema.json"##,
            APPROVAL_ITEM,
        ),
        (
            r##"schemas/api/work-item-attention-list.response.schema.json"##,
            queue,
        ),
        (
            r##"schemas/api/work-item-answer.request.schema.json"##,
            r##"{"text":"pin"}"##,
        ),
        (
            r##"schemas/events/work-item-attention-raised.event.schema.json"##,
            r##"{"eventId":"evt_14","sequence":14,"tenantId":"ten_a","category":"work_item","name":"work_item.attention_raised","occurredAt":"2026-11-09T10:05:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","roundSeq":2,"kind":"approval","optionCount":0,"approvalId":"approval_0a1b2c3d4e5f6a7b"}}"##,
        ),
        (
            r##"schemas/events/work-item-attention-cleared.event.schema.json"##,
            r##"{"eventId":"evt_15","sequence":15,"tenantId":"ten_a","category":"work_item","name":"work_item.attention_cleared","occurredAt":"2026-11-09T10:06:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","by":"approval","approvalId":"approval_0a1b2c3d4e5f6a7b"}}"##,
        ),
        (
            r##"schemas/events/work-item-attention-cleared.event.schema.json"##,
            r##"{"eventId":"evt_16","sequence":16,"tenantId":"ten_a","category":"work_item","name":"work_item.attention_cleared","occurredAt":"2026-11-09T10:06:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","by":"answer"}}"##,
        ),
        (
            r##"schemas/events/work-item-woken.event.schema.json"##,
            r##"{"eventId":"evt_17","sequence":17,"tenantId":"ten_a","category":"work_item","name":"work_item.woken","occurredAt":"2026-11-09T10:06:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","inputSeq":2,"kind":"approval","path":"queued","resumed":false,"approvalId":"approval_0a1b2c3d4e5f6a7b"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);

    for (schema, bad) in [
        // The model's report cannot claim an approval: its question has no
        // kind and no approval id.
        (
            "schemas/api/work-item-report.schema.json",
            r##"{"status":"waiting","summary":"x","attention":{"question":"q","kind":"approval","approvalId":"approval_1"}}"##,
        ),
        (
            "schemas/api/work-item-answer.request.schema.json",
            r##"{"text":""}"##,
        ),
        (
            "schemas/api/work-item-answer.request.schema.json",
            r##"{"text":"pin","option":"pin"}"##,
        ),
        // A healthy item is never queued.
        (
            "schemas/api/work-item-attention-list.response.schema.json",
            r##"{"items":[{"itemId":"wi_1","title":"t","stage":"idle","basis":"api","updatedAt":"2026-11-09T10:00:00Z"}]}"##,
        ),
        // Events never carry what was said.
        (
            "schemas/events/work-item-attention-cleared.event.schema.json",
            r##"{"eventId":"evt_18","sequence":18,"tenantId":"ten_a","category":"work_item","name":"work_item.attention_cleared","occurredAt":"2026-11-09T10:06:00Z","scope":{},"resource":{"kind":"work_item","id":"wi_1"},"payload":{"itemId":"wi_1","basis":"api","by":"answer","text":"pin"}}"##,
        ),
    ] {
        assert!(
            validator.validate_relative(schema, bad.as_bytes()).is_err(),
            "{schema} accepted {bad}"
        );
    }
}
