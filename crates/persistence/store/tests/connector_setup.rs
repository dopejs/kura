//! Round-trip integration tests for the per-connector setup + evidence DAOs
//! ported from `discord_setup.go`, `telegram_setup.go`, `slack_setup.go`, and
//! `matrix.go`. Ports of the corresponding Go behavioral tests, covering
//! tenant+connector filtering, nested destination/allowment/route-policy
//! round-trips, lifecycle updates, and retention expiry.

use std::collections::HashMap;

use chrono::{DateTime, Duration, TimeZone, Utc};
use kura_store::{
    ConnectorAccountBindingSummary, DiscordDestinationValidationRecord, DiscordHostedSetupRecord,
    DiscordSmokeEvidenceRecord, MatrixConversationRouteRecord, MatrixEventEvidenceRecord,
    MatrixHostedSetupRecord, MatrixRoutePolicyRecord, MatrixSmokeEvidenceRecord, SQLiteStore,
    SlackConversationRouteRecord, SlackEventEvidenceRecord, SlackHostedSetupRecord,
    SlackRoutePolicyRecord, SlackSmokeEvidenceRecord, SlackWorkspaceBinding,
    TelegramAllowmentRecord, TelegramHostedSetupRecord, TelegramSmokeEvidenceRecord,
    TelegramUpdateEvidenceRecord,
};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn evidence(kv: &[(&str, &str)]) -> HashMap<String, String> {
    kv.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn at(h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 10, h, 0, 0).unwrap()
}

#[test]
fn discord_setup_and_destinations_round_trip_tenant_safe() {
    let dir = temp_dir("discord_setup");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = at(10);

    store
        .save_discord_hosted_setup(&DiscordHostedSetupRecord {
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            connector_kind: "discord".to_string(),
            display_name: "Discord Main".to_string(),
            status: "degraded".to_string(),
            readiness_state: "degraded_needs_repair".to_string(),
            hosted_ready: false,
            credential_state: "valid".to_string(),
            respond_in_dm: true,
            require_mention: true,
            delivery_mode: "gateway".to_string(),
            reason_code: "destination_validation_failed".to_string(),
            redaction_status: "redacted".to_string(),
            created_at: now,
            updated_at: now,
            validated_at: Some(now),
            retention_expires_at: now + Duration::days(90),
            destinations: Vec::new(),
        })
        .unwrap();
    store
        .save_discord_destination_validation(&DiscordDestinationValidationRecord {
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            destination_id: "channel_redacted".to_string(),
            destination_type: "channel".to_string(),
            provider_label: String::new(),
            selected: true,
            validation_state: "missing_permission".to_string(),
            reason_code: "permission_missing".to_string(),
            validated_at: now,
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("permission", "send_messages")]),
        })
        .unwrap();

    let got = store
        .get_discord_hosted_setup("ten_discord", "discord-main")
        .unwrap()
        .expect("setup found");
    assert_eq!(got.readiness_state, "degraded_needs_repair");
    assert_eq!(got.credential_state, "valid");
    assert!(got.respond_in_dm && got.require_mention);
    assert_eq!(got.delivery_mode, "gateway");
    // Destinations are re-read from their own table (authoritative).
    assert_eq!(got.destinations.len(), 1);
    assert_eq!(
        got.destinations[0].safe_evidence.get("permission"),
        Some(&"send_messages".to_string())
    );

    // Cross-tenant lookup is empty.
    assert!(
        store
            .get_discord_hosted_setup("ten_other", "discord-main")
            .unwrap()
            .is_none()
    );

    // Upsert again through the ON CONFLICT path with a changed state.
    store
        .save_discord_hosted_setup(&DiscordHostedSetupRecord {
            readiness_state: "hosted_ready".to_string(),
            hosted_ready: true,
            updated_at: now + Duration::minutes(1),
            ..DiscordHostedSetupRecord {
                tenant_id: "ten_discord".to_string(),
                connector_id: "discord-main".to_string(),
                connector_kind: "discord".to_string(),
                display_name: "Discord Main".to_string(),
                status: "healthy".to_string(),
                readiness_state: String::new(),
                hosted_ready: false,
                credential_state: "valid".to_string(),
                respond_in_dm: true,
                require_mention: true,
                delivery_mode: "gateway".to_string(),
                reason_code: String::new(),
                redaction_status: "redacted".to_string(),
                created_at: now,
                updated_at: now,
                validated_at: Some(now),
                retention_expires_at: now + Duration::days(90),
                destinations: Vec::new(),
            }
        })
        .unwrap();
    let got = store
        .get_discord_hosted_setup("ten_discord", "discord-main")
        .unwrap()
        .unwrap();
    assert_eq!(got.readiness_state, "hosted_ready");
    assert!(got.hosted_ready);
}

#[test]
fn discord_smoke_evidence_retention_expires() {
    let dir = temp_dir("discord_smoke");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = at(10);

    store
        .save_discord_smoke_evidence(&DiscordSmokeEvidenceRecord {
            smoke_evidence_id: "discord_smoke_1".to_string(),
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            status: "skipped".to_string(),
            credential_mode: "unavailable".to_string(),
            owner: "operator".to_string(),
            reason: "safe_credentials_unavailable".to_string(),
            remaining_risk: "live smoke skipped".to_string(),
            validated_at: now,
            retention_expires_at: now + Duration::days(90),
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("policy", "structured_skip")]),
        })
        .unwrap();

    let latest = store
        .latest_discord_smoke_evidence("ten_discord", "discord-main", now)
        .unwrap()
        .expect("smoke found");
    assert_eq!(latest.status, "skipped");
    assert_eq!(
        latest.safe_evidence.get("policy"),
        Some(&"structured_skip".to_string())
    );

    // After the retention horizon the evidence is no longer visible.
    assert!(
        store
            .latest_discord_smoke_evidence("ten_discord", "discord-main", now + Duration::days(91))
            .unwrap()
            .is_none()
    );
}

#[test]
fn telegram_setup_allowments_smoke_and_updates_round_trip_tenant_safe() {
    let dir = temp_dir("telegram_setup");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = at(10);

    store
        .save_telegram_hosted_setup(&TelegramHostedSetupRecord {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            connector_kind: "telegram".to_string(),
            display_name: "Telegram Main".to_string(),
            status: "healthy".to_string(),
            terminal_state: "ready".to_string(),
            hosted_ready: true,
            credential_state: "valid".to_string(),
            allowment_state: "valid".to_string(),
            group_behavior: "mention_or_command_required".to_string(),
            delivery_eligible: true,
            reason_code: "healthy".to_string(),
            redaction_status: "redacted".to_string(),
            created_at: now,
            updated_at: now,
            validated_at: Some(now),
            retention_expires_at: now + Duration::days(90),
            account_binding: Some(ConnectorAccountBindingSummary {
                tenant_id: "ten_telegram".to_string(),
                connector_id: "telegram-main".to_string(),
                connector_account_id: "telegram_bot_42".to_string(),
                display_name: "kura_test_bot".to_string(),
                provider_account_hint: "kura_test_bot".to_string(),
                redaction_status: "redacted".to_string(),
                updated_at: now,
            }),
            allowments: Vec::new(),
        })
        .unwrap();
    store
        .save_telegram_allowment(&TelegramAllowmentRecord {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            allowment_id: "allow_dm".to_string(),
            scope_type: "direct_chat".to_string(),
            scope_id: "chat_redacted".to_string(),
            provider_label: String::new(),
            enabled: true,
            group_gate: "not_applicable".to_string(),
            validation_state: "valid".to_string(),
            reason_code: "healthy".to_string(),
            validated_at: now,
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("scope", "direct_chat")]),
        })
        .unwrap();
    store
        .save_telegram_smoke_evidence(&TelegramSmokeEvidenceRecord {
            smoke_evidence_id: "telegram_smoke_1".to_string(),
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            status: "passed".to_string(),
            credential_mode: "fake".to_string(),
            owner: "operator".to_string(),
            reason: "healthy".to_string(),
            remaining_risk: String::new(),
            validated_at: now,
            retention_expires_at: now + Duration::days(90),
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("transport", "fake")]),
        })
        .unwrap();
    store
        .save_telegram_update_evidence(&TelegramUpdateEvidenceRecord {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            chat_id: "chat_redacted".to_string(),
            message_id: "message_redacted".to_string(),
            update_id: "update_redacted".to_string(),
            route_outcome: "accepted".to_string(),
            reason_code: "accepted".to_string(),
            received_at: now,
            retention_expires_at: now + Duration::days(90),
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("identityRule", "telegram_chat_message_id")]),
        })
        .unwrap();

    let got = store
        .get_telegram_hosted_setup("ten_telegram", "telegram-main")
        .unwrap()
        .expect("setup found");
    assert_eq!(got.terminal_state, "ready");
    assert!(got.hosted_ready && got.delivery_eligible);
    assert_eq!(got.allowments.len(), 1);
    assert_eq!(
        got.allowments[0].safe_evidence.get("scope"),
        Some(&"direct_chat".to_string())
    );
    let binding = got.account_binding.expect("account binding round-trips");
    assert_eq!(binding.connector_account_id, "telegram_bot_42");
    assert_eq!(binding.provider_account_hint, "kura_test_bot");

    // Cross-tenant lookups are empty.
    assert!(
        store
            .get_telegram_hosted_setup("ten_other", "telegram-main")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .list_telegram_allowments("ten_other", "telegram-main")
            .unwrap()
            .is_empty()
    );

    let smoke = store
        .latest_telegram_smoke_evidence("ten_telegram", "telegram-main", now)
        .unwrap()
        .expect("smoke found");
    assert_eq!(smoke.credential_mode, "fake");

    let updates = store
        .list_telegram_update_evidence("ten_telegram", "telegram-main", now, 10)
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].chat_id, "chat_redacted");
    assert_eq!(
        updates[0].safe_evidence.get("identityRule"),
        Some(&"telegram_chat_message_id".to_string())
    );
    assert!(
        store
            .list_telegram_update_evidence("ten_other", "telegram-main", now, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn slack_setup_route_policy_smoke_and_events_round_trip_tenant_safe() {
    let dir = temp_dir("slack_setup");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = at(10);

    store
        .save_slack_hosted_setup(&SlackHostedSetupRecord {
            tenant_id: "ten_slack".to_string(),
            connector_id: "slack-main".to_string(),
            connector_kind: "slack".to_string(),
            display_name: "Slack Main".to_string(),
            status: "degraded".to_string(),
            terminal_state: "action-required".to_string(),
            oauth_state: "grant_valid".to_string(),
            route_policy_state: "valid".to_string(),
            delivery_eligible: false,
            workspace_binding_id: "slack_workspace_binding_1".to_string(),
            reason_code: "slack_route_policy_missing".to_string(),
            redaction_status: "redacted".to_string(),
            created_at: now,
            updated_at: now,
            validated_at: Some(now),
            retention_expires_at: now + Duration::days(90),
            workspace_binding: Some(SlackWorkspaceBinding {
                tenant_id: "ten_slack".to_string(),
                connector_id: "slack-main".to_string(),
                workspace_binding_id: "slack_workspace_binding_1".to_string(),
                workspace_id: "workspace_redacted".to_string(),
                workspace_label: "Workspace Redacted".to_string(),
                installation_id: "installation_redacted".to_string(),
                oauth_grant_state: "valid".to_string(),
                required_scope_state: "valid".to_string(),
                validated_at: now,
                redaction_status: "redacted".to_string(),
                safe_evidence: evidence(&[("workspace", "redacted")]),
            }),
            route_policy: None,
        })
        .unwrap();
    store
        .save_slack_route_policy(&SlackRoutePolicyRecord {
            tenant_id: "ten_slack".to_string(),
            connector_id: "slack-main".to_string(),
            workspace_binding_id: "slack_workspace_binding_1".to_string(),
            selected_channels: vec![SlackConversationRouteRecord {
                conversation_id: "channel_redacted".to_string(),
                conversation_type: "channel".to_string(),
                selected_channel_state: "selected".to_string(),
                validation_state: "valid".to_string(),
                reason_code: String::new(),
                redaction_status: "redacted".to_string(),
                safe_evidence: evidence(&[("membership", "present")]),
            }],
            allowed_dm_users: vec!["user_hash_1".to_string()],
            allowed_dm_user_groups: vec!["group_hash_1".to_string()],
            mention_gate: "agent_mention_required".to_string(),
            thread_reply_mode: "channel_mentions_thread_rooted".to_string(),
            validation_state: "valid".to_string(),
            reason_code: "healthy".to_string(),
            validated_at: now,
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("route", "selected_channel_and_dm_allowment")]),
        })
        .unwrap();
    store
        .save_slack_smoke_evidence(&SlackSmokeEvidenceRecord {
            smoke_evidence_id: "slack_smoke_1".to_string(),
            tenant_id: "ten_slack".to_string(),
            connector_id: "slack-main".to_string(),
            workspace_binding_id: "slack_workspace_binding_1".to_string(),
            status: "skipped".to_string(),
            authorization_mode: "unavailable".to_string(),
            owner: "operator".to_string(),
            reason: "safe_slack_authorization_unavailable".to_string(),
            remaining_risk: String::new(),
            validated_at: now,
            retention_expires_at: now + Duration::days(90),
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("policy", "structured_skip")]),
        })
        .unwrap();
    store
        .save_slack_event_evidence(&SlackEventEvidenceRecord {
            tenant_id: "ten_slack".to_string(),
            connector_id: "slack-main".to_string(),
            workspace_id: "workspace_redacted".to_string(),
            conversation_id: "channel_redacted".to_string(),
            message_id: "message_redacted".to_string(),
            event_id: "event_redacted".to_string(),
            route_outcome: "accepted".to_string(),
            reason_code: "accepted".to_string(),
            received_at: now,
            retention_expires_at: now + Duration::days(90),
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("identityRule", "slack_workspace_conversation_message_id")]),
        })
        .unwrap();

    let got = store
        .get_slack_hosted_setup("ten_slack", "slack-main")
        .unwrap()
        .expect("setup found");
    assert_eq!(got.terminal_state, "action-required");
    let binding = got
        .workspace_binding
        .expect("workspace binding round-trips");
    assert_eq!(binding.workspace_id, "workspace_redacted");
    // The route policy is re-read from its own table.
    let policy = got.route_policy.expect("route policy attached");
    assert_eq!(policy.selected_channels.len(), 1);
    assert_eq!(
        policy.selected_channels[0].safe_evidence.get("membership"),
        Some(&"present".to_string())
    );
    assert_eq!(policy.allowed_dm_users, vec!["user_hash_1".to_string()]);
    assert_eq!(policy.mention_gate, "agent_mention_required");

    assert!(
        store
            .get_slack_hosted_setup("ten_other", "slack-main")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_slack_route_policy("ten_other", "slack-main")
            .unwrap()
            .is_none()
    );

    let smoke = store
        .latest_slack_smoke_evidence("ten_slack", "slack-main", now)
        .unwrap()
        .expect("smoke found");
    assert_eq!(smoke.authorization_mode, "unavailable");

    let events = store
        .list_slack_event_evidence("ten_slack", "slack-main", now, 10)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].safe_evidence.get("identityRule"),
        Some(&"slack_workspace_conversation_message_id".to_string())
    );
    assert!(
        store
            .list_slack_event_evidence("ten_other", "slack-main", now, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn slack_hosted_setup_lifecycle_repair_states() {
    let dir = temp_dir("slack_lifecycle");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = at(11);

    let base = SlackHostedSetupRecord {
        tenant_id: "ten_slack_lifecycle".to_string(),
        connector_id: "slack-main".to_string(),
        connector_kind: "slack".to_string(),
        display_name: "Slack Main".to_string(),
        status: "degraded".to_string(),
        terminal_state: "action-required".to_string(),
        oauth_state: "grant_missing".to_string(),
        route_policy_state: "none".to_string(),
        delivery_eligible: false,
        workspace_binding_id: "slack_workspace_binding_lifecycle".to_string(),
        reason_code: "auth_missing".to_string(),
        redaction_status: "redacted".to_string(),
        created_at: now,
        updated_at: now,
        validated_at: Some(now),
        retention_expires_at: now + Duration::days(90),
        workspace_binding: None,
        route_policy: None,
    };
    for (terminal, reason) in [
        ("action-required", "auth_missing"),
        ("action-required", "workspace_mismatch"),
        ("cancelled", "user_cancelled"),
        ("cancelled", "disabled_by_user"),
    ] {
        let record = SlackHostedSetupRecord {
            terminal_state: terminal.to_string(),
            reason_code: reason.to_string(),
            updated_at: now + Duration::minutes(1),
            ..base.clone()
        };
        store.save_slack_hosted_setup(&record).unwrap();
        let got = store
            .get_slack_hosted_setup("ten_slack_lifecycle", "slack-main")
            .unwrap()
            .unwrap();
        assert_eq!(got.terminal_state, terminal);
        assert_eq!(got.reason_code, reason);
        assert!(!got.retention_expires_at.to_rfc3339().is_empty());
    }
}

#[test]
fn matrix_setup_lifecycle_updates_terminal_state_and_route_policy() {
    let dir = temp_dir("matrix_setup");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = at(10);

    store
        .save_matrix_hosted_setup(&MatrixHostedSetupRecord {
            tenant_id: "ten_matrix_setup".to_string(),
            connector_id: "matrix-main".to_string(),
            connector_kind: "matrix".to_string(),
            display_name: "Matrix Main".to_string(),
            status: "degraded".to_string(),
            terminal_state: "action-required".to_string(),
            bot_credential_state: "submitted".to_string(),
            homeserver_state: "reachable".to_string(),
            route_policy_state: "none".to_string(),
            delivery_eligible: false,
            homeserver_binding_id: "matrix_hs_setup".to_string(),
            reason_code: String::new(),
            redaction_status: "redacted".to_string(),
            created_at: now,
            updated_at: now,
            validated_at: None,
            retention_expires_at: now + Duration::days(90),
            homeserver_binding: None,
            route_policy: None,
        })
        .unwrap();
    store
        .save_matrix_route_policy(&MatrixRoutePolicyRecord {
            tenant_id: "ten_matrix_setup".to_string(),
            connector_id: "matrix-main".to_string(),
            homeserver_binding_id: "matrix_hs_setup".to_string(),
            selected_rooms: vec![MatrixConversationRouteRecord {
                conversation_id: "room_redacted".to_string(),
                conversation_type: "room".to_string(),
                room_selection_state: "selected".to_string(),
                validation_state: "valid".to_string(),
                reason_code: String::new(),
                redaction_status: "redacted".to_string(),
                safe_evidence: evidence(&[("membership", "joined")]),
            }],
            allowed_direct_users: vec!["user_hash_1".to_string()],
            room_invocation_gate: "bot_mention_or_command_required".to_string(),
            configured_commands: Vec::new(),
            encrypted_room_policy: "unsupported".to_string(),
            validation_state: "valid".to_string(),
            reason_code: String::new(),
            validated_at: now,
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("route", "room_and_direct_allowment")]),
        })
        .unwrap();
    store
        .save_matrix_event_evidence(&MatrixEventEvidenceRecord {
            tenant_id: "ten_matrix_setup".to_string(),
            connector_id: "matrix-main".to_string(),
            homeserver_id: "hs_redacted".to_string(),
            conversation_id: "room_redacted".to_string(),
            matrix_event_id: "event_redacted".to_string(),
            sync_batch_id: "batch_redacted".to_string(),
            transaction_id: String::new(),
            route_outcome: "accepted".to_string(),
            reason_code: String::new(),
            received_at: now,
            retention_expires_at: now + Duration::days(90),
            redaction_status: "redacted".to_string(),
            safe_evidence: evidence(&[("identityRule", "matrix_hs_conversation_event_id")]),
        })
        .unwrap();

    // Lifecycle update through the ON CONFLICT path.
    store
        .save_matrix_hosted_setup(&MatrixHostedSetupRecord {
            tenant_id: "ten_matrix_setup".to_string(),
            connector_id: "matrix-main".to_string(),
            connector_kind: "matrix".to_string(),
            display_name: "Matrix Main".to_string(),
            status: "healthy".to_string(),
            terminal_state: "ready".to_string(),
            bot_credential_state: "valid".to_string(),
            homeserver_state: "reachable".to_string(),
            route_policy_state: "valid".to_string(),
            delivery_eligible: true,
            homeserver_binding_id: "matrix_hs_setup".to_string(),
            reason_code: String::new(),
            redaction_status: "redacted".to_string(),
            created_at: now,
            updated_at: now + Duration::minutes(1),
            validated_at: Some(now + Duration::minutes(1)),
            retention_expires_at: now + Duration::days(90),
            homeserver_binding: None,
            route_policy: None,
        })
        .unwrap();

    let got = store
        .get_matrix_hosted_setup("ten_matrix_setup", "matrix-main")
        .unwrap()
        .expect("setup found");
    assert_eq!(got.terminal_state, "ready");
    assert!(got.delivery_eligible);
    assert_eq!(got.bot_credential_state, "valid");
    let policy = got.route_policy.expect("route policy attached");
    assert_eq!(policy.selected_rooms.len(), 1);
    assert_eq!(
        policy.selected_rooms[0].safe_evidence.get("membership"),
        Some(&"joined".to_string())
    );

    assert!(
        store
            .get_matrix_hosted_setup("ten_other", "matrix-main")
            .unwrap()
            .is_none()
    );
    let events = store
        .list_matrix_event_evidence("ten_matrix_setup", "matrix-main", now, 10)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sync_batch_id, "batch_redacted");
    assert_eq!(
        events[0].safe_evidence.get("identityRule"),
        Some(&"matrix_hs_conversation_event_id".to_string())
    );

    let smoke = MatrixSmokeEvidenceRecord {
        smoke_evidence_id: "matrix_smoke_1".to_string(),
        tenant_id: "ten_matrix_setup".to_string(),
        connector_id: "matrix-main".to_string(),
        homeserver_binding_id: "matrix_hs_setup".to_string(),
        status: "skipped".to_string(),
        authorization_mode: "unavailable".to_string(),
        owner: "operator".to_string(),
        reason: "safe_matrix_authorization_unavailable".to_string(),
        remaining_risk: String::new(),
        validated_at: now,
        retention_expires_at: now + Duration::days(90),
        redaction_status: "redacted".to_string(),
        safe_evidence: evidence(&[("policy", "structured_skip")]),
    };
    store.save_matrix_smoke_evidence(&smoke).unwrap();
    let latest = store
        .latest_matrix_smoke_evidence("ten_matrix_setup", "matrix-main", now)
        .unwrap()
        .expect("smoke found");
    assert_eq!(latest.authorization_mode, "unavailable");
    assert!(
        store
            .latest_matrix_smoke_evidence(
                "ten_matrix_setup",
                "matrix-main",
                now + Duration::days(91)
            )
            .unwrap()
            .is_none()
    );
}
