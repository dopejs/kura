//! Round-trip (wire format) and manager-behavior tests for `kura-triage`, mirroring the Go
//! `manager_test.go` / `persistence_test.go` coverage.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use kura_store::SQLiteStore;
use kura_triage::{
    Classification, Condition, ConditionField, ConditionOperator, Decision, Manager, Message,
    Outcome, Policy, Rule, Run, TriageError,
};
use uuid::Uuid;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kura_triage_{tag}_{}_{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Mirrors the Go `samplePolicy` helper.
fn sample_policy(m: &Manager) -> Policy {
    let policy = m
        .create_policy(
            "inbox",
            vec![
                Rule {
                    description: "block spam".to_string(),
                    conditions: vec![Condition {
                        field: ConditionField::Sender,
                        operator: ConditionOperator::Contains,
                        value: "spam@".to_string(),
                    }],
                    classification: Classification::Blocked,
                    outcome: Outcome::NoAction,
                    ..Rule::default()
                },
                Rule {
                    description: "newsletters".to_string(),
                    conditions: vec![Condition {
                        field: ConditionField::Sender,
                        operator: ConditionOperator::Contains,
                        value: "newsletter".to_string(),
                    }],
                    classification: Classification::Newsletter,
                    outcome: Outcome::DeliveryDigest,
                    ..Rule::default()
                },
                Rule {
                    description: "boss urgent".to_string(),
                    conditions: vec![
                        Condition {
                            field: ConditionField::Sender,
                            operator: ConditionOperator::Contains,
                            value: "boss@".to_string(),
                        },
                        Condition {
                            field: ConditionField::Subject,
                            operator: ConditionOperator::Contains,
                            value: "urgent".to_string(),
                        },
                    ],
                    classification: Classification::Urgent,
                    outcome: Outcome::Reminder,
                    ..Rule::default()
                },
                Rule {
                    description: "questions need reply".to_string(),
                    conditions: vec![Condition {
                        field: ConditionField::Subject,
                        operator: ConditionOperator::Contains,
                        value: "?".to_string(),
                    }],
                    classification: Classification::NeedsReply,
                    outcome: Outcome::DraftReply,
                    ..Rule::default()
                },
            ],
            Classification::Fyi,
        )
        .unwrap();
    policy
}

#[test]
fn policy_wire_round_trip() {
    let m = Manager::new("test");
    let policy = sample_policy(&m);
    let json = serde_json::to_string(&policy).unwrap();
    // camelCase keys + snake_case enum wire values.
    for key in [
        "\"policyId\"",
        "\"environmentScope\"",
        "\"defaultClassification\"",
        "\"createdAt\"",
        "\"conditions\"",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert!(
        json.contains("\"fyi\""),
        "default classification wire value"
    );
    assert!(
        json.contains("\"not_contains\"") == false,
        "no unexpected operator"
    );
    let decoded: Policy = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, policy);
}

#[test]
fn decision_wire_round_trip() {
    let now = Utc::now();
    let matched = Decision {
        message_id: "m1".to_string(),
        classification: Classification::Urgent,
        matched_rule_id: "triage_rule_x".to_string(),
        matched_evidence: vec![kura_triage::MatchedEvidence {
            field: ConditionField::Subject,
            operator: ConditionOperator::Contains,
            value: "urgent".to_string(),
        }],
        outcome: Outcome::Reminder,
        default_applied: false,
        replay_candidate: true,
        decided_at: now,
    };
    let json = serde_json::to_string(&matched).unwrap();
    assert!(json.contains("\"matchedRuleId\""));
    assert!(json.contains("\"matchedEvidence\""));
    assert!(json.contains("\"replayCandidate\":true"));
    assert!(
        !json.contains("defaultApplied"),
        "defaultApplied=false must be omitted"
    );
    let decoded: Decision = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, matched);

    // Default branch: empty rule id + evidence omitted; defaultApplied present when true.
    let defaulted = Decision {
        message_id: "m2".to_string(),
        classification: Classification::Fyi,
        matched_rule_id: String::new(),
        matched_evidence: Vec::new(),
        outcome: Outcome::NoAction,
        default_applied: true,
        replay_candidate: true,
        decided_at: now,
    };
    let json = serde_json::to_string(&defaulted).unwrap();
    assert!(json.contains("\"defaultApplied\":true"));
    assert!(
        !json.contains("matchedRuleId"),
        "empty matchedRuleId must be omitted"
    );
    let decoded: Decision = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, defaulted);
}

#[test]
fn create_policy_normalizes_rules() {
    let m = Manager::new("test");
    let policy = m
        .create_policy(
            "inbox",
            vec![Rule {
                description: String::new(),
                conditions: Vec::new(),
                classification: Classification::Fyi,
                outcome: Outcome::NoAction,
                rule_id: String::new(),
            }],
            Classification::Fyi,
        )
        .unwrap();
    // Empty rule ids are generated; env scope is the manager's.
    assert!(policy.rules[0].rule_id.starts_with("triage_rule_"));
    assert_eq!(policy.environment_scope, "test");
    assert_eq!(policy.name, "inbox");
    assert_eq!(policy.default_classification, Classification::Fyi);
}

#[test]
fn get_and_list_policies() {
    let m = Manager::new("test");
    let a = m
        .create_policy("a", Vec::new(), Classification::Fyi)
        .unwrap();
    let b = m
        .create_policy("b", Vec::new(), Classification::Urgent)
        .unwrap();
    let listed = m.list_policies();
    assert_eq!(listed.len(), 2);
    // Insertion order is preserved.
    assert_eq!(listed[0].policy_id.as_str(), a.policy_id.as_str());
    assert_eq!(listed[1].policy_id.as_str(), b.policy_id.as_str());
    assert_eq!(m.get_policy(&a.policy_id).unwrap(), a);
    assert!(m.get_policy(&a.policy_id.trim().to_uppercase()).is_none());
    assert!(m.get_policy("missing").is_none());
}

#[test]
fn run_classifies_with_transparency() {
    let m = Manager::new("test");
    let policy = sample_policy(&m);
    let messages = vec![
        Message {
            message_id: "m1".to_string(),
            sender: "spam@bad.example".to_string(),
            subject: "win money".to_string(),
            ..Message::default()
        },
        Message {
            message_id: "m2".to_string(),
            sender: "weekly-newsletter@news.example".to_string(),
            subject: "this week".to_string(),
            ..Message::default()
        },
        Message {
            message_id: "m3".to_string(),
            sender: "boss@corp.example".to_string(),
            subject: "URGENT: review".to_string(),
            ..Message::default()
        },
        Message {
            message_id: "m4".to_string(),
            sender: "alice@corp.example".to_string(),
            subject: "can you help?".to_string(),
            ..Message::default()
        },
        Message {
            message_id: "m5".to_string(),
            sender: "bob@corp.example".to_string(),
            subject: "fyi notes".to_string(),
            ..Message::default()
        },
    ];
    let run = m.run(&policy.policy_id, &messages).unwrap();
    assert_eq!(run.decisions.len(), 5);
    let want: [(Classification, Outcome); 5] = [
        (Classification::Blocked, Outcome::NoAction),
        (Classification::Newsletter, Outcome::DeliveryDigest),
        (Classification::Urgent, Outcome::Reminder),
        (Classification::NeedsReply, Outcome::DraftReply),
        (Classification::Fyi, Outcome::NoAction),
    ];
    for (i, (class, outcome)) in want.iter().enumerate() {
        assert_eq!(
            run.decisions[i].classification, *class,
            "decision {i} classification"
        );
        assert_eq!(run.decisions[i].outcome, *outcome, "decision {i} outcome");
        assert!(run.decisions[i].replay_candidate);
    }
    // Transparency: matched rule + evidence recorded for matched decisions; default flagged.
    assert!(!run.decisions[0].matched_rule_id.is_empty());
    assert!(!run.decisions[0].matched_evidence.is_empty());
    assert!(run.decisions[4].default_applied);
    assert_eq!(run.message_count, 5);
    assert!(run.run_id.starts_with("triage_run_"));
    assert_eq!(run.environment_scope, "test");
}

#[test]
fn run_is_deterministic() {
    let m = Manager::new("test");
    let policy = sample_policy(&m);
    let messages = vec![Message {
        message_id: "m1".to_string(),
        sender: "boss@corp.example".to_string(),
        subject: "urgent please".to_string(),
        ..Message::default()
    }];
    let a = m.run(&policy.policy_id, &messages).unwrap();
    let b = m.run(&policy.policy_id, &messages).unwrap();
    // DecidedAt is wall-clock; compare the deterministic fields only.
    let mut da = a.decisions[0].clone();
    let mut db = b.decisions[0].clone();
    da.decided_at = db.decided_at;
    assert_eq!(da, db);
}

#[test]
fn run_unknown_policy_errors() {
    let m = Manager::new("test");
    let err = m.run("nope", &[]).unwrap_err();
    assert_eq!(err, TriageError::PolicyNotFound);
}

#[test]
fn first_match_wins() {
    let m = Manager::new("test");
    let policy = m
        .create_policy(
            "ordered",
            vec![
                Rule {
                    conditions: vec![Condition {
                        field: ConditionField::Subject,
                        operator: ConditionOperator::Contains,
                        value: "report".to_string(),
                    }],
                    classification: Classification::NeedsReply,
                    outcome: Outcome::DraftReply,
                    ..Rule::default()
                },
                Rule {
                    conditions: vec![Condition {
                        field: ConditionField::Subject,
                        operator: ConditionOperator::Contains,
                        value: "report".to_string(),
                    }],
                    classification: Classification::Fyi,
                    outcome: Outcome::NoAction,
                    ..Rule::default()
                },
            ],
            Classification::Fyi,
        )
        .unwrap();
    let run = m
        .run(
            &policy.policy_id,
            &[Message {
                message_id: "m".to_string(),
                subject: "weekly report".to_string(),
                ..Message::default()
            }],
        )
        .unwrap();
    assert_eq!(run.decisions[0].classification, Classification::NeedsReply);
}

#[test]
fn operators_and_recipient_field() {
    let m = Manager::new("test");
    let policy = m
        .create_policy(
            "ops",
            vec![
                Rule {
                    conditions: vec![Condition {
                        field: ConditionField::Recipient,
                        operator: ConditionOperator::Equals,
                        value: "me@corp.example".to_string(),
                    }],
                    classification: Classification::NeedsReply,
                    outcome: Outcome::DraftReply,
                    ..Rule::default()
                },
                Rule {
                    conditions: vec![Condition {
                        field: ConditionField::Subject,
                        operator: ConditionOperator::NotContains,
                        value: "spam".to_string(),
                    }],
                    classification: Classification::Fyi,
                    outcome: Outcome::NoAction,
                    ..Rule::default()
                },
            ],
            Classification::Blocked,
        )
        .unwrap();
    let run = m
        .run(
            &policy.policy_id,
            &[Message {
                message_id: "m".to_string(),
                recipients: vec!["ME@corp.example".to_string()],
                subject: "hello world".to_string(),
                ..Message::default()
            }],
        )
        .unwrap();
    assert_eq!(run.decisions[0].classification, Classification::NeedsReply);
}

#[test]
fn restore_replaces_policies() {
    let m = Manager::new("test");
    let p1 = m
        .create_policy("one", Vec::new(), Classification::Fyi)
        .unwrap();
    let p2 = m
        .create_policy("two", Vec::new(), Classification::Urgent)
        .unwrap();
    m.restore(vec![p2.clone()]);
    assert!(m.get_policy(&p1.policy_id).is_none());
    assert_eq!(m.get_policy(&p2.policy_id).unwrap(), p2);
    assert_eq!(m.list_policies().len(), 1);
}

#[test]
fn persistence_round_trip() {
    let dir = temp_dir("persist");
    let policy_id;
    {
        let store = Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(&dir.to_string_lossy()).unwrap(),
        ));
        let mut m = Manager::new("test");
        m.with_store(Arc::clone(&store));
        let policy = m
            .create_policy("inbox", vec![], Classification::Fyi)
            .unwrap();
        policy_id = policy.policy_id.clone();
    }
    {
        let store = Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(&dir.to_string_lossy()).unwrap(),
        ));
        let mut m = Manager::new("test");
        m.with_store(Arc::clone(&store));
        m.load_from_store().unwrap();
        let reloaded = m.get_policy(&policy_id).expect("policy survived restart");
        assert_eq!(reloaded.environment_scope, "test");
        assert_eq!(reloaded.name, "inbox");
    }
}

#[test]
fn run_wire_round_trip() {
    let m = Manager::new("test");
    let policy = sample_policy(&m);
    let run = m
        .run(
            &policy.policy_id,
            &[Message {
                message_id: "m1".to_string(),
                sender: "spam@bad.example".to_string(),
                ..Message::default()
            }],
        )
        .unwrap();
    let json = serde_json::to_string(&run).unwrap();
    assert!(json.contains("\"messageCount\":1"));
    let decoded: Run = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, run);
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_triage::Manager>();
}
