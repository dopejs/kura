//! Port of daemon/internal/evaluation/tool_call_inspection_test.go and
//! tool_call_inspection_redaction_test.go.

mod common;

use chrono::DateTime;
use chrono::Utc;
use kura_evaluation::{
    EvaluationError, INSPECTION_DRIFTED, INSPECTION_FAILED, INSPECTION_LIVE_VALIDATION_ABORTED,
    INSPECTION_LIVE_VALIDATION_COMPLETED, INSPECTION_LIVE_VALIDATION_DENIED,
    INSPECTION_LIVE_VALIDATION_FAILED, INSPECTION_LIVE_VALIDATION_OPERATOR_ACTION,
    INSPECTION_MATCHED, INSPECTION_MISSING_ORIGINAL_EVIDENCE, INSPECTION_MISSING_REPLAY_EVIDENCE,
    INSPECTION_UNSUPPORTED, RedactionPolicy, RedactionStatus, ToolCallDiffInput,
    ToolCallInspectionInput, build_tool_call_inspection, classify_tool_call_inspection,
    redacted_tool_call_diff,
};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().expect("ts")
}

#[test]
fn tool_call_inspection_classification_states() {
    let base = ToolCallInspectionInput {
        tenant_id: "ten_eval".to_string(),
        campaign_id: "campaign_eval".to_string(),
        campaign_item_id: "campaign_item_eval".to_string(),
        tool_call_ref: "tool_call_eval".to_string(),
        original_evidence_ref: "original_evidence".to_string(),
        non_live_replay_evidence_ref: "replay_evidence".to_string(),
        ..Default::default()
    };
    let cases: Vec<(&str, ToolCallInspectionInput, &str)> = vec![
        ("matched", base.clone(), INSPECTION_MATCHED),
        (
            "drifted",
            ToolCallInspectionInput {
                drifted: true,
                ..base.clone()
            },
            INSPECTION_DRIFTED,
        ),
        (
            "failed",
            ToolCallInspectionInput {
                failed: true,
                ..base.clone()
            },
            INSPECTION_FAILED,
        ),
        (
            "unsupported",
            ToolCallInspectionInput {
                unsupported: true,
                ..base.clone()
            },
            INSPECTION_UNSUPPORTED,
        ),
        (
            "missing original",
            ToolCallInspectionInput {
                original_evidence_ref: String::new(),
                ..base.clone()
            },
            INSPECTION_MISSING_ORIGINAL_EVIDENCE,
        ),
        (
            "missing replay",
            ToolCallInspectionInput {
                non_live_replay_evidence_ref: String::new(),
                ..base.clone()
            },
            INSPECTION_MISSING_REPLAY_EVIDENCE,
        ),
        (
            "live denied",
            ToolCallInspectionInput {
                live_validation_outcome: "denied".to_string(),
                ..base.clone()
            },
            INSPECTION_LIVE_VALIDATION_DENIED,
        ),
        (
            "live aborted",
            ToolCallInspectionInput {
                live_validation_outcome: "aborted".to_string(),
                ..base.clone()
            },
            INSPECTION_LIVE_VALIDATION_ABORTED,
        ),
        (
            "live failed",
            ToolCallInspectionInput {
                live_validation_outcome: "failed".to_string(),
                ..base.clone()
            },
            INSPECTION_LIVE_VALIDATION_FAILED,
        ),
        (
            "live operator action",
            ToolCallInspectionInput {
                live_validation_outcome: "operator_action_needed".to_string(),
                ..base.clone()
            },
            INSPECTION_LIVE_VALIDATION_OPERATOR_ACTION,
        ),
        (
            "live completed",
            ToolCallInspectionInput {
                live_validation_outcome: "completed".to_string(),
                ..base.clone()
            },
            INSPECTION_LIVE_VALIDATION_COMPLETED,
        ),
    ];
    for (name, input, want) in cases {
        let got = build_tool_call_inspection(input, ts("2026-04-29T10:00:00Z"))
            .expect("BuildToolCallInspection");
        assert_eq!(got.classification, want, "{name}: classification mismatch");
    }
}

#[test]
fn build_tool_call_inspection_requires_evidence_coordinates() {
    let err = build_tool_call_inspection(
        ToolCallInspectionInput {
            tenant_id: "ten_eval".to_string(),
            ..Default::default()
        },
        Utc::now(),
    )
    .expect_err("evidence coordinates required");
    assert!(matches!(
        err,
        EvaluationError::ToolCallInspectionEvidenceRequired
    ));
}

#[test]
fn redacted_tool_call_diff_redacts_original_replay_and_custom_sensitive_fields() {
    let (summary, status) = redacted_tool_call_diff(ToolCallDiffInput {
        original: serde_json::json!({
            "access_token": "secret_original",
            "custom_secret": "tenant_secret",
            "result": { "value": "before" }
        })
        .as_object()
        .cloned()
        .expect("object"),
        replay: serde_json::json!({
            "access_token": "secret_replay",
            "custom_secret": "tenant_secret",
            "result": { "value": "after" }
        })
        .as_object()
        .cloned()
        .expect("object"),
        policy: RedactionPolicy {
            sensitive_field_rules: vec!["custom_secret".to_string()],
        },
    });
    assert_eq!(status, RedactionStatus::Redacted);
    assert!(summary.contains("drifted"), "summary={summary}");

    let (matched, status) = redacted_tool_call_diff(ToolCallDiffInput {
        original: serde_json::json!({ "access_token": "secret", "value": "same" })
            .as_object()
            .cloned()
            .expect("object"),
        replay: serde_json::json!({ "access_token": "other", "value": "same" })
            .as_object()
            .cloned()
            .expect("object"),
        policy: RedactionPolicy::default(),
    });
    assert_eq!(status, RedactionStatus::Redacted);
    assert!(matched.contains("matched"), "matched={matched}");
}

#[test]
fn classify_tool_call_inspection_prefers_unsupported_and_missing_evidence() {
    assert_eq!(
        classify_tool_call_inspection(&ToolCallInspectionInput {
            unsupported: true,
            original_evidence_ref: String::new(),
            ..Default::default()
        }),
        INSPECTION_UNSUPPORTED
    );
}
