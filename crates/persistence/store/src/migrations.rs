//! Schema migrations.
//!
//! First-release baseline (2026-08-17): the 55 development-era migrations
//! were collapsed into this single baseline — the exact schema they produced,
//! dumped from a fully migrated database. No public users existed to walk the
//! old chain; pre-release development databases at the legacy head (v55) are
//! re-stamped as baseline-equivalent by the runner, and anything older must
//! be re-initialized. Future migrations append as version 2, 3, ... on top of
//! this baseline.

use crate::SchemaMigration;

#[must_use]
pub fn schema_migrations() -> Vec<SchemaMigration> {
    vec![SchemaMigration {
        version: 1,
        name: "baseline_v1_first_release".to_string(),
        statements: vec![
            r#"CREATE TABLE IF NOT EXISTS activation_states (
                    activation_id TEXT PRIMARY KEY,
                    principal_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    status TEXT NOT NULL,
                    current_step_id TEXT NOT NULL,
                    completed_step_ids_json TEXT NOT NULL,
                    blocking_reason_codes_json TEXT NOT NULL,
                    readiness_items_json TEXT NOT NULL,
                    quota_baseline_json TEXT,
                    first_action_json TEXT NOT NULL,
                    test_chat_json TEXT,
                    failure_reason_json TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    first_action_completed_at TEXT,
                    last_evaluated_at TEXT NOT NULL,
                    last_transition_audit_event_id TEXT,
                    metadata_json TEXT,
                    UNIQUE(principal_id, tenant_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS agent_profile_active_selections (
                    selection_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    profile_id TEXT NOT NULL,
                    profile_version_id TEXT NOT NULL,
                    selection_scope TEXT NOT NULL,
                    selection_reason TEXT NOT NULL,
                    selected_by_principal_id TEXT,
                    selected_at TEXT NOT NULL,
                    audit_event_id TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(profile_id) REFERENCES agent_profiles(profile_id) ON DELETE RESTRICT,
                    FOREIGN KEY(profile_version_id) REFERENCES agent_profile_versions(profile_version_id) ON DELETE RESTRICT,
                    UNIQUE(tenant_id, selection_scope)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS agent_profile_audit_events (
                    audit_event_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    profile_id TEXT,
                    profile_version_id TEXT,
                    actor_principal_id TEXT,
                    event_kind TEXT NOT NULL,
                    outcome TEXT NOT NULL,
                    permission_gate TEXT,
                    reason_code TEXT NOT NULL,
                    safe_summary TEXT,
                    occurred_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS agent_profile_overlay_references (
                    overlay_reference_id TEXT PRIMARY KEY,
                    profile_id TEXT NOT NULL,
                    profile_version_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    reference_kind TEXT NOT NULL,
                    scope TEXT NOT NULL,
                    reference_uri TEXT NOT NULL,
                    safe_display_label TEXT NOT NULL,
                    validation_state TEXT NOT NULL,
                    failure_reason_code TEXT,
                    last_validated_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(profile_id) REFERENCES agent_profiles(profile_id) ON DELETE RESTRICT,
                    FOREIGN KEY(profile_version_id) REFERENCES agent_profile_versions(profile_version_id) ON DELETE RESTRICT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS agent_profile_runtime_projections (
                    runtime_profile_projection_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    profile_id TEXT NOT NULL,
                    profile_version_id TEXT NOT NULL,
                    selection_id TEXT NOT NULL,
                    resource_kind TEXT NOT NULL,
                    resource_id TEXT NOT NULL,
                    thread_id TEXT,
                    session_segment_id TEXT,
                    run_id TEXT,
                    workflow_id TEXT,
                    handoff_link_id TEXT,
                    selection_scope TEXT NOT NULL,
                    selection_reason TEXT NOT NULL,
                    safe_display_name TEXT NOT NULL,
                    safe_summary TEXT NOT NULL,
                    occurred_at TEXT NOT NULL,
                    retention_expires_at TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(profile_id) REFERENCES agent_profiles(profile_id) ON DELETE RESTRICT,
                    FOREIGN KEY(profile_version_id) REFERENCES agent_profile_versions(profile_version_id) ON DELETE RESTRICT,
                    FOREIGN KEY(selection_id) REFERENCES agent_profile_active_selections(selection_id) ON DELETE RESTRICT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS agent_profile_versions (
                    profile_version_id TEXT PRIMARY KEY,
                    profile_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    version_number INTEGER NOT NULL,
                    source_version_id TEXT,
                    change_kind TEXT NOT NULL,
                    change_summary TEXT NOT NULL,
                    rollback_eligibility TEXT NOT NULL,
                    actor_principal_id TEXT,
                    created_at TEXT NOT NULL,
                    audit_event_id TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(profile_id) REFERENCES agent_profiles(profile_id) ON DELETE RESTRICT,
                    UNIQUE(tenant_id, profile_id, version_number)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS agent_profiles (
                    profile_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    active_version_id TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    archived_at TEXT,
                    disabled_at TEXT,
                    created_by_principal_id TEXT,
                    updated_by_principal_id TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS approvals (
                    approval_id TEXT PRIMARY KEY,
                    action TEXT NOT NULL,
                    resource_kind TEXT,
                    resource_id TEXT,
                    reason TEXT NOT NULL,
                    requested_by TEXT,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    resolved_at TEXT,
                    resolution TEXT,
                    comment TEXT
                , integration_bindings_json TEXT, tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS auth_pairings (
                    pairing_id TEXT PRIMARY KEY,
                    mode TEXT NOT NULL,
                    label TEXT NOT NULL,
                    status TEXT NOT NULL,
                    code_hash TEXT NOT NULL,
                    code_preview TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    completed_at TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS auth_tokens (
                    token_id TEXT PRIMARY KEY,
                    label TEXT NOT NULL,
                    mode TEXT NOT NULL,
                    token_hash TEXT NOT NULL,
                    token_preview TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    last_used_at TEXT
                , principal_id TEXT, status TEXT NOT NULL DEFAULT 'active', default_tenant_id TEXT, expires_at TEXT, revoked_at TEXT, rotated_from_token_id TEXT, rotated_to_token_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_abuse_restrictions (
                    restriction_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    affected_category TEXT NOT NULL,
                    recovery_action TEXT NOT NULL,
                    visible_reason_code TEXT NOT NULL,
                    source_audit_ref TEXT,
                    support_contact_allowed INTEGER NOT NULL DEFAULT 0,
                    started_at TEXT NOT NULL,
                    expires_at TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_audit_retention_policies (
                    policy_id TEXT PRIMARY KEY,
                    tenant_id TEXT,
                    retention_mode TEXT NOT NULL,
                    retention_period TEXT,
                    created_by_principal_id TEXT,
                    reason TEXT,
                    created_at TEXT NOT NULL,
                    expires_at TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_manual_adjustments (
                    adjustment_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    category TEXT NOT NULL,
                    quota_period_id TEXT NOT NULL,
                    amount_delta INTEGER NOT NULL,
                    reason TEXT NOT NULL,
                    created_by_principal_id TEXT,
                    created_at TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_quota_definitions (
                    quota_definition_id TEXT PRIMARY KEY,
                    category TEXT NOT NULL UNIQUE,
                    unit TEXT NOT NULL,
                    period_kind TEXT NOT NULL,
                    period_anchor TEXT NOT NULL,
                    default_limit INTEGER NOT NULL,
                    carryover_enabled INTEGER NOT NULL,
                    carryover_max INTEGER NOT NULL,
                    reservation_rule TEXT NOT NULL,
                    commit_rule TEXT NOT NULL,
                    refund_rule TEXT NOT NULL,
                    denial_reason_code TEXT NOT NULL,
                    active INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_quota_denials (
                    denial_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    category TEXT,
                    quota_period_id TEXT,
                    operation_key TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    requested_amount INTEGER NOT NULL,
                    remaining_amount INTEGER NOT NULL,
                    guarded_entry_point TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_quota_overrides (
                    quota_override_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    category TEXT NOT NULL,
                    limit_amount INTEGER,
                    carryover_enabled INTEGER,
                    carryover_max INTEGER,
                    effective_at TEXT NOT NULL,
                    expires_at TEXT,
                    reason TEXT NOT NULL,
                    created_by_principal_id TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_quota_periods (
                    quota_period_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    category TEXT NOT NULL,
                    period_kind TEXT NOT NULL,
                    period_start TEXT NOT NULL,
                    period_end TEXT NOT NULL,
                    carryover_from_period_id TEXT,
                    status TEXT NOT NULL,
                    UNIQUE(tenant_id, category, period_start)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_tenant_plans (
                    plan_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    plan_key TEXT NOT NULL,
                    status TEXT NOT NULL,
                    enforcement_mode TEXT NOT NULL,
                    effective_at TEXT NOT NULL,
                    superseded_at TEXT,
                    assigned_by_principal_id TEXT,
                    assignment_reason TEXT,
                    document_json TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_usage_counters (
                    usage_counter_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    category TEXT NOT NULL,
                    quota_period_id TEXT NOT NULL,
                    committed_amount INTEGER NOT NULL,
                    reserved_amount INTEGER NOT NULL,
                    adjusted_amount INTEGER NOT NULL,
                    carryover_amount INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(tenant_id, category, quota_period_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_usage_events (
                    usage_event_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    category TEXT,
                    quota_period_id TEXT,
                    operation_key TEXT,
                    event_kind TEXT NOT NULL,
                    amount INTEGER NOT NULL,
                    reason_code TEXT NOT NULL,
                    reason TEXT,
                    actor_principal_id TEXT,
                    outcome TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    document_json TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS billing_usage_reservations (
                    reservation_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    category TEXT NOT NULL,
                    quota_period_id TEXT NOT NULL,
                    operation_key TEXT NOT NULL,
                    amount_reserved INTEGER NOT NULL,
                    amount_committed INTEGER NOT NULL,
                    amount_refunded INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    reservation_point TEXT,
                    commit_point TEXT,
                    refund_point TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    expires_at TEXT,
                    recovery_reason TEXT,
                    UNIQUE(tenant_id, category, operation_key)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS binding_audit_events (
                    audit_event_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    binding_id TEXT,
                    workspace_id TEXT,
                    actor_principal_id TEXT,
                    event_kind TEXT NOT NULL,
                    outcome TEXT NOT NULL,
                    permission_gate TEXT,
                    reason_code TEXT NOT NULL,
                    safe_summary TEXT,
                    occurred_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS binding_rules (
                    binding_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    scope_kind TEXT NOT NULL,
                    scope_ref TEXT NOT NULL,
                    selected_profile_id TEXT,
                    selected_profile_version_id TEXT,
                    selected_workspace_id TEXT,
                    status TEXT NOT NULL,
                    repair_status TEXT NOT NULL,
                    validation_status TEXT NOT NULL,
                    actor_principal_id TEXT,
                    audit_event_id TEXT,
                    previous_selection_summary TEXT,
                    resulting_selection_summary TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    disabled_at TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS binding_runtime_projections (
                    projection_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    resource_kind TEXT NOT NULL,
                    resource_id TEXT NOT NULL,
                    selected_profile_id TEXT,
                    selected_profile_version_id TEXT,
                    selected_workspace_id TEXT,
                    binding_scope TEXT NOT NULL,
                    binding_id TEXT,
                    classification TEXT NOT NULL,
                    selection_reason TEXT NOT NULL,
                    capability_visibility_summary TEXT,
                    occurred_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS calendar_accounts (
                    calendar_account_id TEXT PRIMARY KEY,
                    integration_id TEXT NOT NULL UNIQUE,
                    environment_scope TEXT NOT NULL,
                    account_key TEXT,
                    readiness_status TEXT NOT NULL,
                    canonical_default INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS calendar_artifacts (
                    artifact_id TEXT PRIMARY KEY,
                    operation_id TEXT NOT NULL,
                    integration_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    external_event_id TEXT,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS calendar_operations (
                    operation_id TEXT PRIMARY KEY,
                    integration_id TEXT NOT NULL,
                    calendar_account_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    operation_class TEXT NOT NULL,
                    status TEXT NOT NULL,
                    external_event_id TEXT,
                    run_id TEXT,
                    workflow_id TEXT,
                    schedule_id TEXT,
                    delivery_id TEXT,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS capabilities (
                    capability_id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    failure_count INTEGER NOT NULL,
                    restart_count INTEGER NOT NULL,
                    backoff_seconds INTEGER NOT NULL,
                    next_restart_at TEXT,
                    last_restart_at TEXT,
                    last_heartbeat_at TEXT,
                    last_failure_reason TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS capability_visibility_policies (
                    policy_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    scope_kind TEXT NOT NULL,
                    scope_ref TEXT NOT NULL,
                    capability_id TEXT NOT NULL,
                    visibility TEXT NOT NULL,
                    actor_principal_id TEXT,
                    validation_status TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    UNIQUE(tenant_id, scope_kind, scope_ref, capability_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_connector_enablement_states (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    state TEXT NOT NULL,
                    reason_code TEXT,
                    changed_by_principal_id TEXT,
                    changed_at TEXT NOT NULL,
                    validated_at TEXT,
                    audit_event_id TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_delivery_outcomes (
                    delivery_outcome_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    delivery_target_id TEXT,
                    status TEXT NOT NULL,
                    reason_code TEXT,
                    occurred_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_management_audit_records (
                    audit_event_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    principal_id TEXT,
                    action TEXT NOT NULL,
                    permission_gate TEXT NOT NULL,
                    outcome TEXT NOT NULL,
                    reason_code TEXT,
                    created_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_repair_actions (
                    repair_action_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    connector_kind TEXT NOT NULL,
                    actor_principal_id TEXT,
                    action_kind TEXT NOT NULL,
                    source_diagnostic_state_id TEXT,
                    setup_session_id TEXT,
                    status TEXT NOT NULL,
                    retry_safety TEXT,
                    remediation_owner TEXT,
                    started_at TEXT NOT NULL,
                    completed_at TEXT,
                    audit_event_id TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_reply_outcomes (
                    reply_outcome_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    routing_decision_id TEXT,
                    status TEXT NOT NULL,
                    reason_code TEXT,
                    occurred_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_route_policies (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    route_policy_id TEXT NOT NULL,
                    validation_state TEXT NOT NULL,
                    reason_code TEXT,
                    background_delivery_eligible INTEGER NOT NULL DEFAULT 0,
                    validated_at TEXT NOT NULL,
                    audit_event_id TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_route_policy_snapshots (
                    route_policy_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    validated_at TEXT NOT NULL,
                    audit_event_id TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_routing_decisions (
                    routing_decision_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    connector_kind TEXT NOT NULL,
                    outcome TEXT NOT NULL,
                    reason_code TEXT,
                    occurred_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS channel_support_evidence (
                    support_evidence_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    generated_by_principal_id TEXT,
                    generated_at TEXT NOT NULL,
                    current_state TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS checkpoints (
                    checkpoint_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    captured_at TEXT NOT NULL,
                    snapshot_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS computer_use_actions (
                    computer_use_action_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    computer_use_session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    step_id TEXT,
                    tool_call_id TEXT,
                    workflow_id TEXT,
                    workflow_step_id TEXT,
                    action_kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    risk_level TEXT NOT NULL,
                    approval_id TEXT,
                    target_match_context_json TEXT,
                    page_before_json TEXT,
                    page_after_json TEXT,
                    failure_class TEXT,
                    failure_reason TEXT,
                    requested_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    completed_at TEXT,
                    input_json TEXT,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(computer_use_session_id) REFERENCES computer_use_sessions(computer_use_session_id) ON DELETE CASCADE,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS computer_use_artifacts (
                    artifact_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    computer_use_session_id TEXT NOT NULL,
                    computer_use_action_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    mime_type TEXT,
                    file_name TEXT,
                    byte_size INTEGER NOT NULL,
                    storage_key TEXT,
                    sha256 TEXT,
                    capture_failure_reason TEXT,
                    created_at TEXT NOT NULL,
                    available_at TEXT,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(computer_use_session_id) REFERENCES computer_use_sessions(computer_use_session_id) ON DELETE CASCADE,
                    FOREIGN KEY(computer_use_action_id) REFERENCES computer_use_actions(computer_use_action_id) ON DELETE CASCADE,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS computer_use_sessions (
                    computer_use_session_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    workflow_id TEXT,
                    workflow_step_id TEXT,
                    status TEXT NOT NULL,
                    driver_kind TEXT NOT NULL,
                    trusted_page_scope_json TEXT,
                    current_page_json TEXT,
                    last_action_id TEXT,
                    started_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    closed_at TEXT,
                    interrupted_at TEXT,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS connector_conformance_results (
                    conformance_result_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_kind TEXT NOT NULL,
                    connector_id TEXT,
                    scenario_id TEXT NOT NULL,
                    area TEXT NOT NULL,
                    result TEXT NOT NULL,
                    reason_code TEXT,
                    redaction_status TEXT NOT NULL,
                    evidence_timestamp TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS connector_delivery_boundaries (
                    boundary_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    foreground_reply_outcome_id TEXT,
                    background_delivery_id TEXT,
                    transport_kind TEXT NOT NULL,
                    separation_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS connector_diagnostic_redaction_failures (
                    redaction_failure_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    diagnostic_state_id TEXT,
                    reason_code TEXT NOT NULL,
                    occurred_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS connector_diagnostic_states (
                    diagnostic_state_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    connector_account_id TEXT,
                    status TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    remediation_owner TEXT NOT NULL,
                    user_visible_severity TEXT NOT NULL,
                    retry_safety TEXT NOT NULL,
                    evidence_timestamp TEXT NOT NULL,
                    stale_after TEXT,
                    freshness_state TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_failure_id TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS connector_messages (
                    delivery_id TEXT PRIMARY KEY,
                    connector_id TEXT NOT NULL,
                    direction TEXT NOT NULL,
                    external_message_id TEXT,
                    session_id TEXT,
                    run_id TEXT,
                    channel_id TEXT NOT NULL,
                    peer_id TEXT,
                    thread_id TEXT,
                    author_id TEXT,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    error_text TEXT,
                    reply_to_external_message_id TEXT,
                    response_to_delivery_id TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL, tenant_id TEXT, connector_account_id TEXT, channel_or_conversation_id TEXT, provider_message_id TEXT, equivalent_rule_id TEXT, foreground_outcome_status TEXT, background_delivery_id TEXT, delivery_boundary_kind TEXT, thread_session_segment_id TEXT,
                    FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE SET NULL,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE SET NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS connectors (
                    connector_id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    failure_count INTEGER NOT NULL,
                    restart_count INTEGER NOT NULL,
                    backoff_seconds INTEGER NOT NULL,
                    next_restart_at TEXT,
                    last_restart_at TEXT,
                    last_heartbeat_at TEXT,
                    last_failure_reason TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                , tenant_id TEXT, disabled_reason TEXT, secret_refs_json TEXT NOT NULL DEFAULT '[]');;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS consumer_policy_records (
                    policy_record_id TEXT PRIMARY KEY,
                    consumer_kind TEXT NOT NULL,
                    consumer_id TEXT NOT NULL,
                    operation_kind TEXT NOT NULL,
                    declaration_id TEXT,
                    status TEXT NOT NULL,
                    decision TEXT NOT NULL,
                    approval_status TEXT NOT NULL,
                    secret_resolution TEXT NOT NULL,
                    requested_by TEXT,
                    sandbox_execution_id TEXT,
                    tool_call_id TEXT,
                    provider_operation_id TEXT,
                    started_at TEXT NOT NULL,
                    completed_at TEXT,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS decisions (
                    decision_id TEXT PRIMARY KEY,
                    action TEXT NOT NULL,
                    resource_kind TEXT,
                    resource_id TEXT,
                    outcome TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    approval_id TEXT,
                    created_at TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(approval_id) REFERENCES approvals(approval_id) ON DELETE SET NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS delivery_attempts (
                    attempt_id TEXT PRIMARY KEY,
                    delivery_id TEXT NOT NULL,
                    attempt_number INTEGER NOT NULL,
                    target_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    next_retry_at TEXT,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(delivery_id) REFERENCES delivery_outcomes(delivery_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS delivery_outcomes (
                    delivery_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    run_id TEXT,
                    workflow_id TEXT,
                    schedule_id TEXT,
                    integration_id TEXT,
                    status TEXT NOT NULL,
                    chosen_target_id TEXT,
                    preference_id TEXT,
                    summary_window_id TEXT,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS delivery_preferences (
                    preference_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    scope_kind TEXT NOT NULL,
                    integration_id TEXT,
                    active INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS delivery_summary_windows (
                    summary_window_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    preference_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    window_ends_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS delivery_targets (
                    target_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    target_kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS discord_destination_validations (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    destination_id TEXT NOT NULL,
                    destination_type TEXT NOT NULL,
                    provider_label TEXT,
                    selected INTEGER NOT NULL,
                    validation_state TEXT NOT NULL,
                    reason_code TEXT,
                    validated_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id, destination_type, destination_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS discord_hosted_setups (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    connector_kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    readiness_state TEXT NOT NULL,
                    credential_state TEXT NOT NULL,
                    respond_in_dm INTEGER NOT NULL,
                    require_mention INTEGER NOT NULL,
                    delivery_mode TEXT NOT NULL,
                    reason_code TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    validated_at TEXT,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS discord_smoke_evidence (
                    smoke_evidence_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    credential_mode TEXT NOT NULL,
                    owner TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    remaining_risk TEXT,
                    validated_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_campaign_attempt_groups (
                    attempt_group_id TEXT PRIMARY KEY,
                    campaign_id TEXT NOT NULL,
                    campaign_item_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    drift_count INTEGER NOT NULL,
                    failure_count INTEGER NOT NULL,
                    unsupported_count INTEGER NOT NULL,
                    operator_action_needed_count INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(campaign_id) REFERENCES evaluation_campaigns(campaign_id) ON DELETE CASCADE,
                    FOREIGN KEY(campaign_item_id) REFERENCES evaluation_campaign_items(campaign_item_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_campaign_items (
                    campaign_item_id TEXT PRIMARY KEY,
                    campaign_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    source_type TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    suppression_checked_at TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(campaign_id) REFERENCES evaluation_campaigns(campaign_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_campaigns (
                    campaign_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    started_at TEXT,
                    completed_at TEXT,
                    published_at TEXT,
                    retention_state TEXT NOT NULL,
                    idempotency_key TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_candidate_evidence (
                    evidence_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    discovered_candidate_id TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    materialization_allowed INTEGER NOT NULL,
                    retention_state TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    expires_at TEXT,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(discovered_candidate_id) REFERENCES evaluation_discovered_candidates(discovered_candidate_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_comparisons (
                    comparison_id TEXT PRIMARY KEY,
                    candidate_id TEXT NOT NULL,
                    attempt_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    terminal_status TEXT NOT NULL,
                    change_window_label TEXT,
                    generated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(candidate_id) REFERENCES evaluation_replay_candidates(candidate_id) ON DELETE CASCADE,
                    FOREIGN KEY(attempt_id) REFERENCES evaluation_replay_attempts(attempt_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_dashboard_projections (
                    projection_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    window_start TEXT NOT NULL,
                    window_end TEXT NOT NULL,
                    generated_at TEXT NOT NULL,
                    cursor TEXT,
                    retention_state TEXT NOT NULL DEFAULT 'active',
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_discovered_candidates (
                    discovered_candidate_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    discovery_run_id TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    score REAL NOT NULL,
                    score_band TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    evidence_ref TEXT,
                    readiness_status TEXT NOT NULL,
                    suppression_state TEXT NOT NULL,
                    retention_state TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    expires_at TEXT,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(discovery_run_id) REFERENCES evaluation_discovery_runs(discovery_run_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_discovery_policies (
                    policy_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    enabled INTEGER NOT NULL,
                    window_start TEXT NOT NULL,
                    window_end TEXT NOT NULL,
                    max_inspected_records INTEGER NOT NULL,
                    max_emitted_candidates INTEGER NOT NULL,
                    cost_budget INTEGER NOT NULL,
                    created_by TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_discovery_runs (
                    discovery_run_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    policy_id TEXT,
                    status TEXT NOT NULL,
                    cursor TEXT,
                    window_start TEXT NOT NULL,
                    window_end TEXT NOT NULL,
                    max_inspected_records INTEGER NOT NULL,
                    max_emitted_candidates INTEGER NOT NULL,
                    cost_budget INTEGER NOT NULL,
                    inspected_records INTEGER NOT NULL,
                    emitted_candidates INTEGER NOT NULL,
                    started_by TEXT,
                    started_at TEXT NOT NULL,
                    completed_at TEXT,
                    updated_at TEXT NOT NULL,
                    idempotency_key TEXT,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(policy_id) REFERENCES evaluation_discovery_policies(policy_id) ON DELETE SET NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_fixture_revisions (
                    revision_id TEXT PRIMARY KEY,
                    fixture_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    revision_number INTEGER NOT NULL,
                    redaction_status TEXT NOT NULL,
                    created_by TEXT,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(fixture_id) REFERENCES evaluation_product_fixtures(fixture_id) ON DELETE CASCADE,
                    UNIQUE(fixture_id, revision_number)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_product_fixtures (
                    fixture_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    domain_class TEXT NOT NULL,
                    source_kind TEXT,
                    source_candidate_id TEXT,
                    current_revision_id TEXT,
                    review_state TEXT NOT NULL,
                    suppression_state TEXT NOT NULL,
                    retention_state TEXT NOT NULL,
                    created_by TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_regression_fixtures (
                    fixture_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    domain_class TEXT NOT NULL,
                    candidate_id TEXT,
                    manifest_path TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_replay_attempts (
                    attempt_id TEXT PRIMARY KEY,
                    candidate_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    mode TEXT NOT NULL,
                    status TEXT NOT NULL,
                    change_window_label TEXT,
                    baseline_attempt_id TEXT,
                    started_at TEXT,
                    completed_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(candidate_id) REFERENCES evaluation_replay_candidates(candidate_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_replay_candidates (
                    candidate_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    candidate_kind TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    readiness_status TEXT NOT NULL,
                    latest_attempt_id TEXT,
                    latest_comparison_id TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_retention_applications (
                    application_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    resource_kind TEXT NOT NULL,
                    resource_id TEXT,
                    dry_run INTEGER NOT NULL,
                    outcome TEXT NOT NULL,
                    applied_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_suppressions (
                    suppression_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    target_kind TEXT NOT NULL,
                    target_id TEXT,
                    target_source_ref TEXT,
                    reason_code TEXT NOT NULL,
                    created_by TEXT,
                    active INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    expires_at TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS evaluation_tool_call_inspections (
                    inspection_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    campaign_id TEXT NOT NULL,
                    campaign_item_id TEXT NOT NULL,
                    tool_call_ref TEXT NOT NULL,
                    classification TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    retention_state TEXT NOT NULL DEFAULT 'active',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(campaign_id) REFERENCES evaluation_campaigns(campaign_id) ON DELETE CASCADE,
                    FOREIGN KEY(campaign_item_id) REFERENCES evaluation_campaign_items(campaign_item_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS events (
                    event_id TEXT PRIMARY KEY,
                    category TEXT NOT NULL,
                    name TEXT NOT NULL,
                    occurred_at TEXT NOT NULL,
                    session_id TEXT,
                    run_id TEXT,
                    step_id TEXT,
                    connector_id TEXT,
                    capability_id TEXT,
                    resource_kind TEXT NOT NULL,
                    resource_id TEXT NOT NULL,
                    payload_json TEXT
                , workflow_id TEXT, workflow_step_id TEXT, schedule_id TEXT, schedule_attempt_id TEXT, environment_scope TEXT NOT NULL DEFAULT '', tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS integration_diagnostic_results (
                    diagnostic_result_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    integration_id TEXT NOT NULL,
                    integration_account_id TEXT,
                    domain_kind TEXT NOT NULL,
                    provider_kind TEXT NOT NULL,
                    capability TEXT NOT NULL,
                    status TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    remediation_owner TEXT NOT NULL,
                    retry_safety TEXT NOT NULL,
                    checked_at TEXT NOT NULL,
                    stale_after TEXT NOT NULL,
                    freshness_state TEXT NOT NULL,
                    run_id TEXT,
                    redaction_status TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES integration_diagnostic_runs(diagnostic_run_id) ON DELETE SET NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS integration_diagnostic_retention (
                    retention_record_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    target_kind TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    policy_ref TEXT,
                    default_expires_at TEXT NOT NULL,
                    effective_expires_at TEXT NOT NULL,
                    retention_state TEXT NOT NULL,
                    applied_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS integration_diagnostic_runs (
                    diagnostic_run_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    integration_id TEXT NOT NULL,
                    integration_account_id TEXT,
                    domain_kind TEXT,
                    provider_kind TEXT,
                    requested_by TEXT NOT NULL,
                    trigger TEXT NOT NULL,
                    status TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    completed_at TEXT,
                    failure_reason_code TEXT,
                    redaction_status TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    idempotency_key TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS integration_provider_classifications (
                    classification_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    provider_kind TEXT NOT NULL,
                    domain_kind TEXT NOT NULL,
                    integration_id TEXT,
                    operation_class TEXT,
                    reason_code TEXT NOT NULL,
                    retry_safety TEXT NOT NULL,
                    remediation_owner TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS integration_smoke_probe_outcomes (
                    probe_outcome_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    smoke_report_id TEXT NOT NULL,
                    integration_id TEXT NOT NULL,
                    integration_account_id TEXT,
                    domain_kind TEXT NOT NULL,
                    provider_kind TEXT NOT NULL,
                    probe_action TEXT NOT NULL,
                    result TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    retry_safety TEXT NOT NULL,
                    checked_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(smoke_report_id) REFERENCES integration_smoke_reports(smoke_report_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS integration_smoke_reports (
                    smoke_report_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    report_kind TEXT NOT NULL,
                    requested_by TEXT NOT NULL,
                    status TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    completed_at TEXT,
                    published_at TEXT,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS integrations (
                    integration_id TEXT PRIMARY KEY,
                    domain_kind TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    account_key TEXT,
                    backend_kind TEXT NOT NULL,
                    readiness_status TEXT NOT NULL,
                    canonical_default INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_ambiguous_commits (
                    ambiguous_commit_id TEXT PRIMARY KEY,
                    ledger_entry_id TEXT NOT NULL,
                    validation_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    cause TEXT NOT NULL,
                    automatic_retry_stopped INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(ledger_entry_id) REFERENCES live_validation_ledger_entries(ledger_entry_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_approvals (
                    approval_id TEXT PRIMARY KEY,
                    validation_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    approval_target TEXT NOT NULL,
                    tool_class TEXT NOT NULL,
                    action_ref TEXT,
                    status TEXT NOT NULL,
                    requested_at TEXT NOT NULL,
                    resolved_at TEXT,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(validation_id) REFERENCES live_validation_attempts(validation_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_attempts (
                    validation_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    candidate_id TEXT NOT NULL,
                    source_attempt_id TEXT,
                    environment_scope TEXT NOT NULL,
                    status TEXT NOT NULL,
                    comparison_id TEXT,
                    created_at TEXT NOT NULL,
                    started_at TEXT,
                    completed_at TEXT,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_comparisons (
                    comparison_id TEXT PRIMARY KEY,
                    validation_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    candidate_id TEXT NOT NULL,
                    terminal_status TEXT NOT NULL,
                    generated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(validation_id) REFERENCES live_validation_attempts(validation_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_kill_switches (
                    kill_switch_id TEXT PRIMARY KEY,
                    scope TEXT NOT NULL,
                    tenant_id TEXT,
                    enabled INTEGER NOT NULL,
                    changed_at TEXT NOT NULL,
                    expires_at TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_ledger_entries (
                    ledger_entry_id TEXT PRIMARY KEY,
                    validation_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    candidate_id TEXT NOT NULL,
                    tool_class TEXT NOT NULL,
                    safety_class TEXT NOT NULL,
                    action_ref TEXT NOT NULL,
                    outcome TEXT NOT NULL,
                    attempted_at TEXT,
                    completed_at TEXT,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(validation_id) REFERENCES live_validation_attempts(validation_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_reconciliation_resolutions (
                    reconciliation_id TEXT PRIMARY KEY,
                    ambiguous_commit_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    resolved_by TEXT NOT NULL,
                    resolution TEXT NOT NULL,
                    resolved_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(ambiguous_commit_id) REFERENCES live_validation_ambiguous_commits(ambiguous_commit_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_retention_policies (
                    policy_id TEXT PRIMARY KEY,
                    tenant_id TEXT,
                    applies_to TEXT NOT NULL,
                    retention_mode TEXT NOT NULL,
                    created_by_principal_id TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    expires_at TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_scopes (
                    scope_id TEXT PRIMARY KEY,
                    validation_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    approval_mode TEXT NOT NULL,
                    declared_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(validation_id) REFERENCES live_validation_attempts(validation_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS live_validation_support_matrix_snapshots (
                    snapshot_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    validation_id TEXT,
                    version TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS llm_dispatches (
                    dispatch_id TEXT PRIMARY KEY,
                    provider TEXT NOT NULL,
                    model TEXT NOT NULL,
                    messages_json TEXT NOT NULL,
                    stream INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    output_text TEXT NOT NULL,
                    finish_reason TEXT,
                    usage_json TEXT NOT NULL,
                    error_code TEXT,
                    error_text TEXT,
                    timeout_ms INTEGER NOT NULL,
                    max_retries INTEGER NOT NULL,
                    attempt_count INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    started_at TEXT,
                    completed_at TEXT
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS mail_accounts (
                    mail_account_id TEXT PRIMARY KEY,
                    integration_id TEXT NOT NULL UNIQUE,
                    environment_scope TEXT NOT NULL,
                    account_key TEXT,
                    readiness_status TEXT NOT NULL,
                    canonical_default INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS mail_artifacts (
                    artifact_id TEXT PRIMARY KEY,
                    operation_id TEXT NOT NULL,
                    integration_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    thread_id TEXT,
                    message_id TEXT,
                    draft_id TEXT,
                    attachment_ref_id TEXT,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS mail_operations (
                    operation_id TEXT PRIMARY KEY,
                    integration_id TEXT NOT NULL,
                    mail_account_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    operation_class TEXT NOT NULL,
                    status TEXT NOT NULL,
                    result_mode TEXT NOT NULL,
                    thread_id TEXT,
                    message_id TEXT,
                    draft_id TEXT,
                    run_id TEXT,
                    workflow_id TEXT,
                    schedule_id TEXT,
                    delivery_id TEXT,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS manager_documents (
                    doc_kind TEXT NOT NULL,
                    doc_id TEXT NOT NULL,
                    environment_scope TEXT,
                    tenant_id TEXT,
                    document_json TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (doc_kind, doc_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS matrix_event_evidence (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    homeserver_id TEXT NOT NULL,
                    conversation_id TEXT NOT NULL,
                    matrix_event_id TEXT NOT NULL,
                    sync_batch_id TEXT,
                    transaction_id TEXT,
                    route_outcome TEXT NOT NULL,
                    reason_code TEXT,
                    received_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id, homeserver_id, conversation_id, matrix_event_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS matrix_hosted_setups (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    connector_kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    terminal_state TEXT NOT NULL,
                    bot_credential_state TEXT NOT NULL,
                    homeserver_state TEXT NOT NULL,
                    route_policy_state TEXT NOT NULL,
                    delivery_eligible INTEGER NOT NULL,
                    homeserver_binding_id TEXT NOT NULL,
                    reason_code TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    validated_at TEXT,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS matrix_route_policies (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    homeserver_binding_id TEXT NOT NULL,
                    validation_state TEXT NOT NULL,
                    reason_code TEXT,
                    validated_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS matrix_smoke_evidence (
                    smoke_evidence_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    homeserver_binding_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    authorization_mode TEXT NOT NULL,
                    owner TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    remaining_risk TEXT,
                    validated_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS mcp_server_states (
                    server_id TEXT PRIMARY KEY,
                    status TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(server_id) REFERENCES mcp_servers(server_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS mcp_servers (
                    server_id TEXT PRIMARY KEY,
                    enabled INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS mcp_tool_exposure_rules (
                    server_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    runtime_surface TEXT NOT NULL,
                    exposure_mode TEXT NOT NULL,
                    active INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    PRIMARY KEY (server_id, tool_name, runtime_surface),
                    FOREIGN KEY(server_id, tool_name) REFERENCES mcp_tools(server_id, tool_name) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS mcp_tools (
                    server_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    discovery_status TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    last_discovered_at TEXT,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    PRIMARY KEY (server_id, tool_name),
                    FOREIGN KEY(server_id) REFERENCES mcp_servers(server_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS memberships (
                    membership_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    principal_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    status TEXT NOT NULL,
                    invitation_id TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    accepted_at TEXT,
                    removed_at TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS principals (
                    principal_id TEXT PRIMARY KEY,
                    principal_kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    default_tenant_id TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    disabled_at TEXT,
                    removed_at TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS provider_auth_states (
                    provider_id TEXT PRIMARY KEY,
                    family TEXT NOT NULL,
                    auth_mode TEXT NOT NULL,
                    status TEXT NOT NULL,
                    cli_path TEXT,
                    cli_available INTEGER NOT NULL,
                    account_label TEXT,
                    account_id TEXT,
                    plan TEXT,
                    auth_method TEXT,
                    login_command_json TEXT NOT NULL,
                    logout_command_json TEXT NOT NULL,
                    last_checked_at TEXT NOT NULL,
                    last_authenticated_at TEXT,
                    last_error TEXT,
                    metadata_json TEXT NOT NULL
                , sandbox_json TEXT, tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS provider_checks (
                    check_id TEXT PRIMARY KEY,
                    provider_id TEXT NOT NULL,
                    family TEXT NOT NULL,
                    auth_mode TEXT NOT NULL,
                    status TEXT NOT NULL,
                    model TEXT NOT NULL,
                    endpoint TEXT,
                    error_class TEXT,
                    error_code TEXT,
                    error_message TEXT,
                    usage_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    completed_at TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS provider_models (
                    provider_id TEXT NOT NULL,
                    model_id TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    description TEXT,
                    default_flag INTEGER NOT NULL,
                    available_flag INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    chat INTEGER NOT NULL,
                    stream INTEGER NOT NULL,
                    coding INTEGER NOT NULL,
                    tool_use INTEGER NOT NULL,
                    reasoning_levels_json TEXT NOT NULL,
                    PRIMARY KEY (provider_id, model_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS provider_preferences (
                    provider_id TEXT PRIMARY KEY,
                    default_model TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS reminder_actions (
                    action_id TEXT PRIMARY KEY,
                    reminder_id TEXT NOT NULL,
                    occurrence_id TEXT,
                    action_kind TEXT NOT NULL,
                    new_state TEXT,
                    run_id TEXT,
                    workflow_id TEXT,
                    delivery_id TEXT,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS reminder_occurrences (
                    occurrence_id TEXT PRIMARY KEY,
                    reminder_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    state TEXT NOT NULL,
                    scheduled_for TEXT NOT NULL,
                    run_id TEXT,
                    workflow_id TEXT,
                    latest_delivery_id TEXT,
                    latest_delivery_status TEXT,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS reminders (
                    reminder_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    behavior_mode TEXT NOT NULL,
                    current_state TEXT NOT NULL,
                    next_due_at TEXT,
                    active_occurrence_id TEXT,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS runs (
                    run_id TEXT PRIMARY KEY,
                    session_id TEXT,
                    entrypoint TEXT NOT NULL,
                    status TEXT NOT NULL,
                    goal TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL, schedule_id TEXT, schedule_attempt_id TEXT, reminder_id TEXT, reminder_occurrence_id TEXT, tenant_id TEXT,
                    FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE SET NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS sandbox_executions (
                    execution_id TEXT PRIMARY KEY,
                    profile_id TEXT NOT NULL,
                    backend_kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    approval_id TEXT,
                    requested_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    started_at TEXT,
                    completed_at TEXT,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(approval_id) REFERENCES approvals(approval_id) ON DELETE SET NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS schedule_dispatch_attempts (
                    attempt_id TEXT PRIMARY KEY,
                    schedule_id TEXT NOT NULL,
                    due_at TEXT NOT NULL,
                    trigger_source TEXT NOT NULL,
                    dispatch_status TEXT NOT NULL,
                    failure_class TEXT,
                    failure_reason TEXT,
                    retry_count INTEGER NOT NULL,
                    retry_budget INTEGER NOT NULL,
                    next_retry_at TEXT,
                    resolved_target_revision INTEGER NOT NULL,
                    run_id TEXT,
                    workflow_id TEXT,
                    downstream_status TEXT NOT NULL,
                    skipped_reason TEXT,
                    missed_count INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(schedule_id) REFERENCES schedules(schedule_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS schedule_targets (
                    target_ref_id TEXT PRIMARY KEY,
                    schedule_id TEXT NOT NULL,
                    target_kind TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    active INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(schedule_id) REFERENCES schedules(schedule_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS schedules (
                    schedule_id TEXT PRIMARY KEY,
                    environment_scope TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    target_ref_id TEXT NOT NULL,
                    timezone TEXT,
                    next_due_at TEXT,
                    last_attempt_at TEXT,
                    last_outcome TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    paused_at TEXT,
                    cancelled_at TEXT,
                    completed_at TEXT,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS secret_scope_bindings (
                    binding_id TEXT PRIMARY KEY,
                    consumer_kind TEXT NOT NULL,
                    consumer_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    secret_ref TEXT NOT NULL,
                    default_source TEXT NOT NULL,
                    delivery_kind TEXT NOT NULL,
                    active INTEGER NOT NULL,
                    document_json TEXT NOT NULL
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS sessions (
                    session_id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    account_id TEXT,
                    peer_id TEXT NOT NULL,
                    thread_id TEXT,
                    routing_key TEXT NOT NULL UNIQUE,
                    generation INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    last_active_at TEXT NOT NULL,
                    last_reset_at TEXT
                , tenant_id TEXT);;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS setup_attempts (
                    attempt_id TEXT PRIMARY KEY,
                    setup_session_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    actor_principal_id TEXT,
                    operation TEXT NOT NULL,
                    from_state TEXT,
                    to_state TEXT NOT NULL,
                    reason_code TEXT,
                    diagnostic_result_id TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(setup_session_id) REFERENCES setup_sessions(setup_session_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS setup_sessions (
                    setup_session_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    actor_principal_id TEXT,
                    target_id TEXT NOT NULL,
                    target_kind TEXT NOT NULL,
                    setup_style TEXT NOT NULL,
                    state TEXT NOT NULL,
                    reason_code TEXT,
                    diagnostic_result_id TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    last_transition_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    UNIQUE(tenant_id, target_id, setup_style)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS slack_event_evidence (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    conversation_id TEXT NOT NULL,
                    message_id TEXT NOT NULL,
                    event_id TEXT NOT NULL,
                    route_outcome TEXT NOT NULL,
                    reason_code TEXT,
                    received_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id, workspace_id, conversation_id, message_id, event_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS slack_hosted_setups (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    connector_kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    terminal_state TEXT NOT NULL,
                    oauth_state TEXT NOT NULL,
                    route_policy_state TEXT NOT NULL,
                    delivery_eligible INTEGER NOT NULL,
                    workspace_binding_id TEXT NOT NULL,
                    reason_code TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    validated_at TEXT,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS slack_route_policies (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    workspace_binding_id TEXT NOT NULL,
                    validation_state TEXT NOT NULL,
                    reason_code TEXT,
                    validated_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS slack_smoke_evidence (
                    smoke_evidence_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    workspace_binding_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    authorization_mode TEXT NOT NULL,
                    owner TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    remaining_risk TEXT,
                    validated_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS steps (
                    step_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    input_json TEXT,
                    output_json TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL, workflow_id TEXT, workflow_step_id TEXT, attempt INTEGER NOT NULL DEFAULT 0, tenant_id TEXT,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS telegram_allowments (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    allowment_id TEXT NOT NULL,
                    scope_type TEXT NOT NULL,
                    scope_id TEXT NOT NULL,
                    provider_label TEXT,
                    enabled INTEGER NOT NULL,
                    group_gate TEXT NOT NULL,
                    validation_state TEXT NOT NULL,
                    reason_code TEXT,
                    validated_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id, allowment_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS telegram_hosted_setups (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    connector_kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    terminal_state TEXT NOT NULL,
                    credential_state TEXT NOT NULL,
                    allowment_state TEXT NOT NULL,
                    group_behavior TEXT NOT NULL,
                    delivery_eligible INTEGER NOT NULL,
                    reason_code TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    validated_at TEXT,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS telegram_smoke_evidence (
                    smoke_evidence_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    credential_mode TEXT NOT NULL,
                    owner TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    remaining_risk TEXT,
                    validated_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS telegram_update_evidence (
                    tenant_id TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    chat_id TEXT NOT NULL,
                    message_id TEXT NOT NULL,
                    update_id TEXT NOT NULL,
                    route_outcome TEXT NOT NULL,
                    reason_code TEXT,
                    received_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    PRIMARY KEY (tenant_id, connector_id, chat_id, message_id, update_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS tenant_audit_events (
                    audit_event_id TEXT PRIMARY KEY,
                    event_kind TEXT NOT NULL,
                    tenant_id TEXT,
                    principal_id TEXT,
                    target_principal_id TEXT,
                    token_id TEXT,
                    outcome TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    document_json TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS tenant_invitations (
                    invitation_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    invited_principal_id TEXT NOT NULL,
                    invited_by_principal_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    expires_at TEXT,
                    decided_at TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS tenant_migration_progress (
                    step_name           TEXT PRIMARY KEY,
                    status              TEXT NOT NULL,
                    started_at          INTEGER,
                    completed_at        INTEGER,
                    last_processed_key  TEXT,
                    error               TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS tenant_secret_versions (
                    secret_version_id TEXT PRIMARY KEY,
                    secret_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    secret_ref TEXT NOT NULL,
                    version_number INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    value_backend_ref TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    activated_at TEXT,
                    superseded_at TEXT,
                    FOREIGN KEY(secret_id) REFERENCES tenant_secrets(secret_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS tenant_secrets (
                    secret_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    secret_ref TEXT NOT NULL,
                    display_name TEXT,
                    status TEXT NOT NULL,
                    active_version_id TEXT,
                    disabled_reason TEXT,
                    remediation_reason TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    rotated_at TEXT,
                    disabled_at TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS tenants (
                    tenant_id TEXT PRIMARY KEY,
                    tenant_kind TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    created_by_principal_id TEXT,
                    default_owner_principal_id TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_continuity_preview_items (
                    preview_item_id TEXT PRIMARY KEY,
                    continuity_preview_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    item_kind TEXT NOT NULL,
                    continuity_turn_id TEXT,
                    artifact_ref TEXT,
                    artifact_excerpt_id TEXT,
                    decision TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    acceptance_sequence INTEGER,
                    source_timestamp TEXT,
                    safe_summary TEXT,
                    redaction_status TEXT NOT NULL,
                    item_order INTEGER NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(continuity_preview_id) REFERENCES thread_continuity_previews(continuity_preview_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_continuity_previews (
                    continuity_preview_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    session_segment_id TEXT NOT NULL,
                    dispatch_id TEXT,
                    request_turn_id TEXT,
                    response_turn_id TEXT,
                    window_policy_id TEXT NOT NULL,
                    max_prior_turns INTEGER NOT NULL,
                    active_window_days INTEGER NOT NULL,
                    included_count INTEGER NOT NULL,
                    excluded_count INTEGER NOT NULL,
                    continuity_applied INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    failure_class TEXT,
                    assembly_started_at TEXT NOT NULL,
                    assembly_completed_at TEXT NOT NULL,
                    assembly_duration_ms INTEGER NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_continuity_turns (
                    continuity_turn_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    session_segment_id TEXT NOT NULL,
                    acceptance_sequence INTEGER NOT NULL,
                    role TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source_linkage_id TEXT,
                    source_message_id TEXT,
                    source_timestamp TEXT,
                    dispatch_id TEXT,
                    response_to_turn_id TEXT,
                    safe_content TEXT,
                    content_redaction_status TEXT NOT NULL,
                    recorded_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    source_event_key TEXT,
                    document_json TEXT NOT NULL,
                    UNIQUE(tenant_id, thread_id, acceptance_sequence),
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_conversation_shapes (
                    conversation_shape_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    session_segment_id TEXT,
                    shape TEXT NOT NULL,
                    source_kind TEXT,
                    connector_id TEXT,
                    connector_kind TEXT,
                    source_account_id TEXT,
                    source_conversation_id TEXT,
                    shape_evidence_status TEXT NOT NULL,
                    recorded_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_handoff_links (
                    handoff_link_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    source_thread_id TEXT NOT NULL,
                    source_session_segment_id TEXT,
                    destination_thread_id TEXT NOT NULL,
                    destination_session_segment_id TEXT,
                    source_conversation_shape TEXT NOT NULL,
                    destination_conversation_shape TEXT NOT NULL,
                    source_kind TEXT,
                    destination_kind TEXT,
                    source_connector_id TEXT,
                    destination_connector_id TEXT,
                    source_conversation_id TEXT,
                    destination_conversation_id TEXT,
                    actor_principal_id TEXT,
                    permission_gate TEXT NOT NULL,
                    status TEXT NOT NULL,
                    reason_code TEXT,
                    first_destination_response_id TEXT,
                    source_reference_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    consumed_at TEXT,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(source_thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE,
                    FOREIGN KEY(destination_thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE,
                    CHECK(source_thread_id != destination_thread_id)
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_handoff_source_references (
                    handoff_source_reference_id TEXT PRIMARY KEY,
                    handoff_link_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    source_thread_id TEXT NOT NULL,
                    source_session_segment_id TEXT,
                    destination_thread_id TEXT NOT NULL,
                    destination_session_segment_id TEXT,
                    continuity_turn_id TEXT,
                    artifact_excerpt_ref TEXT,
                    eligibility_status TEXT NOT NULL,
                    decision TEXT NOT NULL,
                    safe_summary TEXT,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    consumed_at TEXT,
                    retention_expires_at TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(handoff_link_id) REFERENCES thread_handoff_links(handoff_link_id) ON DELETE CASCADE,
                    FOREIGN KEY(destination_thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_lifecycle_events (
                    lifecycle_event_id TEXT PRIMARY KEY,
                    thread_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    action TEXT NOT NULL,
                    outcome TEXT NOT NULL,
                    audit_event_id TEXT,
                    occurred_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_participation_decisions (
                    participation_decision_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    thread_id TEXT,
                    session_segment_id TEXT,
                    connector_id TEXT,
                    source_account_id TEXT,
                    source_conversation_id TEXT,
                    source_message_id TEXT,
                    conversation_shape TEXT NOT NULL,
                    decision TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    created_assistant_work INTEGER NOT NULL,
                    occurred_at TEXT NOT NULL,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_reset_events (
                    reset_event_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    conversation_shape TEXT NOT NULL,
                    source_conversation_id TEXT,
                    actor_principal_id TEXT,
                    permission_gate TEXT NOT NULL,
                    prior_session_segment_id TEXT,
                    resulting_session_segment_id TEXT,
                    status TEXT NOT NULL,
                    reason_code TEXT NOT NULL,
                    requested_at TEXT NOT NULL,
                    completed_at TEXT NOT NULL,
                    audit_event_id TEXT,
                    retention_expires_at TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_retention_policies (
                    tenant_id TEXT PRIMARY KEY,
                    retention_expires_at TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_runtime_projections (
                    runtime_projection_id TEXT PRIMARY KEY,
                    thread_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    session_segment_id TEXT,
                    resource_kind TEXT NOT NULL,
                    resource_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    reason_code TEXT,
                    occurred_at TEXT NOT NULL,
                    route TEXT,
                    safe_summary TEXT,
                    retention_expires_at TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_session_segments (
                    session_segment_id TEXT PRIMARY KEY,
                    thread_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    session_id TEXT,
                    generation INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    ended_at TEXT,
                    last_active_at TEXT NOT NULL,
                    reset_from_session_segment_id TEXT,
                    partial_evidence INTEGER NOT NULL DEFAULT 0,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS thread_source_links (
                    source_linkage_id TEXT PRIMARY KEY,
                    thread_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    connector_id TEXT,
                    connector_kind TEXT,
                    source_account_id TEXT,
                    source_conversation_id TEXT,
                    source_message_id TEXT,
                    routing_outcome TEXT NOT NULL,
                    current_flag INTEGER NOT NULL DEFAULT 0,
                    linked_at TEXT NOT NULL,
                    retention_expires_at TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS threads (
                    thread_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    lifecycle_state TEXT NOT NULL,
                    current_session_segment_id TEXT,
                    source_kind TEXT NOT NULL,
                    source_summary TEXT,
                    last_activity_at TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    retention_expires_at TEXT,
                    redaction_status TEXT NOT NULL,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS token_tenant_grants (
                    grant_id TEXT PRIMARY KEY,
                    token_id TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    is_default INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    revoked_at TEXT,
                    granted_by_principal_id TEXT
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS tool_calls (
                    tool_call_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    step_id TEXT NOT NULL,
                    capability_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    input_json TEXT,
                    output_json TEXT,
                    error_text TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL, sandbox_json TEXT, invocation_kind TEXT, skill_id TEXT, sandbox_execution_id TEXT, failure_class TEXT, mcp_server_id TEXT, mcp_server_name TEXT, mcp_tool_name TEXT, mcp_transport_kind TEXT, mcp_session_id TEXT, authorization_result TEXT, workflow_id TEXT, workflow_step_id TEXT, attempt INTEGER NOT NULL DEFAULT 0, computer_use_session_id TEXT, computer_use_action_id TEXT, integration_bindings_json TEXT, tenant_id TEXT,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE,
                    FOREIGN KEY(step_id) REFERENCES steps(step_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS workflow_dependencies (
                    dependency_id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(workflow_id) REFERENCES workflows(workflow_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS workflow_handoffs (
                    handoff_id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(workflow_id) REFERENCES workflows(workflow_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS workflow_steps (
                    workflow_step_id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    runtime_step_id TEXT,
                    active_tool_call_id TEXT,
                    attempt_count INTEGER NOT NULL,
                    max_attempts INTEGER NOT NULL,
                    last_failure_class TEXT,
                    blocked_reason TEXT,
                    document_json TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY(workflow_id) REFERENCES workflows(workflow_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS workflows (
                    workflow_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    environment_scope TEXT NOT NULL,
                    goal TEXT NOT NULL,
                    status TEXT NOT NULL,
                    plan_summary TEXT,
                    failure_summary TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    started_at TEXT,
                    completed_at TEXT,
                    interrupted_at TEXT,
                    document_json TEXT NOT NULL, schedule_id TEXT, schedule_attempt_id TEXT, tenant_id TEXT,
                    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
                );;"#
                .to_string(),
            r#"CREATE TABLE IF NOT EXISTS workspaces (
                    workspace_id TEXT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    is_default INTEGER NOT NULL DEFAULT 0,
                    owner_principal_id TEXT,
                    repair_status TEXT NOT NULL,
                    redaction_status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    archived_at TEXT,
                    document_json TEXT NOT NULL
                );;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_activation_states_principal_updated ON activation_states(principal_id, updated_at DESC, activation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_activation_states_tenant_status ON activation_states(tenant_id, status, updated_at DESC, activation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_agent_profile_active_selections_profile ON agent_profile_active_selections(tenant_id, profile_id, selected_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_agent_profile_audit_events_profile ON agent_profile_audit_events(tenant_id, profile_id, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_agent_profile_overlay_references_profile ON agent_profile_overlay_references(tenant_id, profile_id, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_agent_profile_runtime_projections_resource ON agent_profile_runtime_projections(tenant_id, resource_kind, resource_id, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_agent_profile_runtime_projections_thread ON agent_profile_runtime_projections(tenant_id, thread_id, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_agent_profile_versions_profile ON agent_profile_versions(tenant_id, profile_id, version_number DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_agent_profiles_tenant_status_updated ON agent_profiles(tenant_id, status, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_approvals_status_created ON approvals(status, created_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_approvals_tenant_status_created ON approvals(tenant_id, status, created_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_auth_tokens_last_used_at ON auth_tokens(last_used_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_abuse_restrictions_tenant_active ON billing_abuse_restrictions(tenant_id, status, affected_category, started_at DESC, restriction_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_manual_adjustments_tenant_created ON billing_manual_adjustments(tenant_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_quota_denials_tenant_created ON billing_quota_denials(tenant_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_quota_overrides_tenant_category ON billing_quota_overrides(tenant_id, category, effective_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_quota_periods_tenant_category ON billing_quota_periods(tenant_id, category, period_start DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_tenant_plans_active ON billing_tenant_plans(tenant_id, status, effective_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_usage_events_tenant_created ON billing_usage_events(tenant_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_billing_usage_reservations_pending ON billing_usage_reservations(status, updated_at ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_binding_audit_events_binding ON binding_audit_events(tenant_id, binding_id, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_binding_rules_tenant_scope ON binding_rules(tenant_id, scope_kind, scope_ref);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_binding_runtime_projections_resource ON binding_runtime_projections(tenant_id, resource_kind, resource_id, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_accounts_env_default ON calendar_accounts(environment_scope, account_key, canonical_default, updated_at DESC, integration_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_accounts_env_readiness ON calendar_accounts(environment_scope, readiness_status, updated_at DESC, integration_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_accounts_tenant_readiness ON calendar_accounts(tenant_id, readiness_status, updated_at DESC, calendar_account_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_artifacts_event ON calendar_artifacts(environment_scope, external_event_id, created_at DESC, artifact_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_artifacts_operation ON calendar_artifacts(environment_scope, operation_id, created_at ASC, artifact_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_artifacts_tenant_operation ON calendar_artifacts(tenant_id, operation_id, created_at ASC, artifact_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_operations_delivery ON calendar_operations(environment_scope, delivery_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_operations_env_class_status ON calendar_operations(environment_scope, operation_class, status, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_operations_event ON calendar_operations(environment_scope, external_event_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_operations_run ON calendar_operations(environment_scope, run_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_operations_schedule ON calendar_operations(environment_scope, schedule_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_operations_tenant_account ON calendar_operations(tenant_id, calendar_account_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_calendar_operations_workflow ON calendar_operations(environment_scope, workflow_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_capabilities_kind_status ON capabilities(kind, status);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_capability_visibility_scope ON capability_visibility_policies(tenant_id, scope_kind, scope_ref);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_connector_enablement_changed ON channel_connector_enablement_states(tenant_id, changed_at DESC, connector_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_delivery_outcomes_tenant_connector ON channel_delivery_outcomes(tenant_id, connector_id, occurred_at DESC, delivery_outcome_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_management_audit_tenant_connector ON channel_management_audit_records(tenant_id, connector_id, created_at DESC, audit_event_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_repair_actions_tenant_connector ON channel_repair_actions(tenant_id, connector_id, started_at DESC, repair_action_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_reply_outcomes_tenant_connector ON channel_reply_outcomes(tenant_id, connector_id, occurred_at DESC, reply_outcome_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_route_policies_tenant_connector ON channel_route_policies(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_route_policy_snapshots_tenant_connector ON channel_route_policy_snapshots(tenant_id, connector_id, validated_at DESC, route_policy_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_routing_decisions_tenant_connector ON channel_routing_decisions(tenant_id, connector_id, occurred_at DESC, routing_decision_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_channel_support_evidence_tenant_connector ON channel_support_evidence(tenant_id, connector_id, generated_at DESC, support_evidence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_run_id ON checkpoints(run_id, captured_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_tenant_run_captured ON checkpoints(tenant_id, run_id, captured_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_computer_use_actions_approval ON computer_use_actions(environment_scope, approval_id, requested_at DESC, computer_use_action_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_computer_use_actions_session ON computer_use_actions(environment_scope, computer_use_session_id, requested_at ASC, computer_use_action_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_computer_use_actions_tenant_session ON computer_use_actions(tenant_id, computer_use_session_id, requested_at ASC, computer_use_action_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_computer_use_artifacts_action ON computer_use_artifacts(environment_scope, computer_use_action_id, created_at ASC, artifact_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_computer_use_artifacts_tenant_action ON computer_use_artifacts(tenant_id, computer_use_action_id, created_at ASC, artifact_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_computer_use_sessions_run ON computer_use_sessions(environment_scope, run_id, updated_at DESC, computer_use_session_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_computer_use_sessions_tenant_run ON computer_use_sessions(tenant_id, run_id, updated_at DESC, computer_use_session_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_conformance_kind_area ON connector_conformance_results(connector_kind, area, result);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_conformance_tenant_connector ON connector_conformance_results(tenant_id, connector_id, evidence_timestamp DESC, conformance_result_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_delivery_boundaries_tenant ON connector_delivery_boundaries(tenant_id, connector_id, created_at DESC, boundary_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_diagnostic_redaction_failures_tenant ON connector_diagnostic_redaction_failures(tenant_id, connector_id, occurred_at DESC, redaction_failure_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_diagnostic_states_current ON connector_diagnostic_states(tenant_id, connector_id, evidence_timestamp DESC, diagnostic_state_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_diagnostic_states_reason ON connector_diagnostic_states(tenant_id, reason_code, freshness_state);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_messages_connector_created ON connector_messages(connector_id, created_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_messages_external_lookup ON connector_messages(tenant_id, connector_id, direction, external_message_id) WHERE external_message_id IS NOT NULL;;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_connector_messages_external_tenant_unique ON connector_messages(tenant_id, connector_id, direction, external_message_id) WHERE external_message_id IS NOT NULL;;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_messages_session_created ON connector_messages(session_id, created_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_messages_standard_identity ON connector_messages(tenant_id, connector_account_id, channel_or_conversation_id, provider_message_id, direction) WHERE provider_message_id IS NOT NULL;;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_connector_messages_standard_identity_unique ON connector_messages(tenant_id, connector_account_id, channel_or_conversation_id, provider_message_id, direction, equivalent_rule_id) WHERE provider_message_id IS NOT NULL;;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connector_messages_tenant_connector ON connector_messages(tenant_id, connector_id, created_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connectors_kind_status ON connectors(kind, status);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_connectors_tenant_kind_status ON connectors(tenant_id, kind, status);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_decisions_approval_id ON decisions(approval_id, created_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_decisions_tenant_created ON decisions(tenant_id, created_at DESC, decision_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_attempts_delivery ON delivery_attempts(delivery_id, attempt_number ASC, attempt_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_attempts_tenant_delivery ON delivery_attempts(tenant_id, delivery_id, attempt_number ASC, attempt_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_outcomes_env_run ON delivery_outcomes(environment_scope, run_id, updated_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_outcomes_env_schedule ON delivery_outcomes(environment_scope, schedule_id, updated_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_outcomes_env_source ON delivery_outcomes(environment_scope, source_kind, source_id, updated_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_outcomes_env_target ON delivery_outcomes(environment_scope, chosen_target_id, updated_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_outcomes_env_workflow ON delivery_outcomes(environment_scope, workflow_id, updated_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_outcomes_tenant_updated ON delivery_outcomes(tenant_id, updated_at DESC, delivery_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_preferences_env_scope ON delivery_preferences(environment_scope, scope_kind, integration_id, active, updated_at DESC, preference_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_preferences_tenant_scope ON delivery_preferences(tenant_id, scope_kind, integration_id, active, updated_at DESC, preference_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_summary_windows_env_status ON delivery_summary_windows(environment_scope, status, window_ends_at ASC, summary_window_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_summary_windows_tenant_status ON delivery_summary_windows(tenant_id, status, window_ends_at ASC, summary_window_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_targets_env_status ON delivery_targets(environment_scope, status, updated_at DESC, target_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_delivery_targets_tenant_status ON delivery_targets(tenant_id, status, updated_at DESC, target_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_discord_destinations_tenant_connector ON discord_destination_validations(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_discord_hosted_setups_tenant_state ON discord_hosted_setups(tenant_id, readiness_state, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_discord_smoke_tenant_connector ON discord_smoke_evidence(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_attempts_env_candidate ON evaluation_replay_attempts(environment_scope, candidate_id, created_at DESC, attempt_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_attempts_env_status ON evaluation_replay_attempts(environment_scope, status, created_at DESC, attempt_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_attempts_tenant_candidate ON evaluation_replay_attempts(tenant_id, candidate_id, created_at DESC, attempt_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_campaign_attempt_groups_tenant_campaign ON evaluation_campaign_attempt_groups(tenant_id, campaign_id, updated_at DESC, attempt_group_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_campaign_attempt_groups_tenant_item ON evaluation_campaign_attempt_groups(tenant_id, campaign_item_id, updated_at DESC, attempt_group_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_campaign_items_tenant_campaign ON evaluation_campaign_items(tenant_id, campaign_id, created_at DESC, campaign_item_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_campaign_items_tenant_source ON evaluation_campaign_items(tenant_id, source_type, source_id, created_at DESC, campaign_item_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_campaigns_tenant_idempotency ON evaluation_campaigns(tenant_id, idempotency_key, created_at DESC, campaign_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_campaigns_tenant_status ON evaluation_campaigns(tenant_id, status, created_at DESC, campaign_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_candidate_evidence_tenant_candidate ON evaluation_candidate_evidence(tenant_id, discovered_candidate_id, created_at DESC, evidence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_candidates_env_kind ON evaluation_replay_candidates(environment_scope, candidate_kind, source_kind, updated_at DESC, candidate_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_candidates_env_ready ON evaluation_replay_candidates(environment_scope, readiness_status, updated_at DESC, candidate_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_candidates_tenant_ready ON evaluation_replay_candidates(tenant_id, readiness_status, updated_at DESC, candidate_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_comparisons_env_attempt ON evaluation_comparisons(environment_scope, attempt_id, generated_at DESC, comparison_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_comparisons_env_candidate ON evaluation_comparisons(environment_scope, candidate_id, generated_at DESC, comparison_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_comparisons_tenant_candidate ON evaluation_comparisons(tenant_id, candidate_id, generated_at DESC, comparison_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_dashboard_projections_tenant_generated ON evaluation_dashboard_projections(tenant_id, generated_at DESC, projection_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_discovered_candidates_tenant_ready ON evaluation_discovered_candidates(tenant_id, readiness_status, updated_at DESC, discovered_candidate_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_discovered_candidates_tenant_run ON evaluation_discovered_candidates(tenant_id, discovery_run_id, updated_at DESC, discovered_candidate_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_discovered_candidates_tenant_source ON evaluation_discovered_candidates(tenant_id, source_kind, source_id, updated_at DESC, discovered_candidate_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_discovery_policies_tenant_enabled ON evaluation_discovery_policies(tenant_id, enabled, updated_at DESC, policy_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_discovery_runs_tenant_policy ON evaluation_discovery_runs(tenant_id, policy_id, updated_at DESC, discovery_run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_discovery_runs_tenant_status ON evaluation_discovery_runs(tenant_id, status, updated_at DESC, discovery_run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_fixture_revisions_tenant_fixture ON evaluation_fixture_revisions(tenant_id, fixture_id, revision_number DESC, revision_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_fixtures_env_domain ON evaluation_regression_fixtures(environment_scope, domain_class, updated_at DESC, fixture_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_fixtures_tenant_domain ON evaluation_regression_fixtures(tenant_id, domain_class, updated_at DESC, fixture_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_product_fixtures_tenant_review ON evaluation_product_fixtures(tenant_id, review_state, updated_at DESC, fixture_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_product_fixtures_tenant_source ON evaluation_product_fixtures(tenant_id, source_kind, source_candidate_id, updated_at DESC, fixture_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_retention_applications_tenant_resource ON evaluation_retention_applications(tenant_id, resource_kind, resource_id, applied_at DESC, application_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_suppressions_tenant_target ON evaluation_suppressions(tenant_id, target_kind, target_id, active, created_at DESC, suppression_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_tool_call_inspections_tenant_campaign ON evaluation_tool_call_inspections(tenant_id, campaign_id, updated_at DESC, inspection_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_eval_tool_call_inspections_tenant_item ON evaluation_tool_call_inspections(tenant_id, campaign_item_id, updated_at DESC, inspection_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_category ON events(category, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_env_category ON events(environment_scope, category, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_env_run ON events(environment_scope, run_id, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_env_schedule ON events(environment_scope, schedule_id, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_env_session ON events(environment_scope, session_id, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_resource_scope ON events(resource_kind, resource_id, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_run_id ON events(run_id, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_schedule_id ON events(schedule_id, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_session_id ON events(session_id, occurred_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_tenant_category_time ON events(tenant_id, category, name, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_events_tenant_time ON events(tenant_id, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_diagnostic_results_tenant_latest ON integration_diagnostic_results(tenant_id, integration_id, domain_kind, capability, checked_at DESC, diagnostic_result_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_diagnostic_results_tenant_reason ON integration_diagnostic_results(tenant_id, reason_code, checked_at DESC, diagnostic_result_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_diagnostic_retention_tenant_target ON integration_diagnostic_retention(tenant_id, target_kind, target_id, effective_expires_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_diagnostic_runs_tenant_idempotency ON integration_diagnostic_runs(tenant_id, idempotency_key, started_at DESC, diagnostic_run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_diagnostic_runs_tenant_integration ON integration_diagnostic_runs(tenant_id, integration_id, started_at DESC, diagnostic_run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_diagnostic_runs_tenant_status ON integration_diagnostic_runs(tenant_id, status, started_at DESC, diagnostic_run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_provider_classifications_tenant_reason ON integration_provider_classifications(tenant_id, reason_code, created_at DESC, classification_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_smoke_outcomes_tenant_report ON integration_smoke_probe_outcomes(tenant_id, smoke_report_id, checked_at DESC, probe_outcome_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integration_smoke_reports_tenant_status ON integration_smoke_reports(tenant_id, status, started_at DESC, smoke_report_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integrations_env_domain_account ON integrations(environment_scope, domain_kind, account_key, canonical_default);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integrations_readiness ON integrations(environment_scope, readiness_status, updated_at DESC, integration_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integrations_tenant_domain_account ON integrations(tenant_id, domain_kind, account_key, canonical_default);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_integrations_tenant_readiness ON integrations(tenant_id, readiness_status, updated_at DESC, integration_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_invitations_principal_status ON tenant_invitations(invited_principal_id, status, created_at DESC, invitation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_invitations_tenant_status ON tenant_invitations(tenant_id, status, created_at DESC, invitation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_ambiguous_validation ON live_validation_ambiguous_commits(tenant_id, validation_id, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_approvals_validation_status ON live_validation_approvals(tenant_id, validation_id, status, requested_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_attempts_env_status ON live_validation_attempts(environment_scope, status, updated_at DESC, validation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_attempts_tenant_candidate ON live_validation_attempts(tenant_id, candidate_id, updated_at DESC, validation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_attempts_tenant_status ON live_validation_attempts(tenant_id, status, updated_at DESC, validation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_comparisons_validation ON live_validation_comparisons(tenant_id, validation_id, generated_at DESC, comparison_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_kill_switches_enabled ON live_validation_kill_switches(scope, tenant_id, enabled, changed_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_ledger_outcome ON live_validation_ledger_entries(tenant_id, outcome, updated_at DESC, ledger_entry_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_ledger_validation ON live_validation_ledger_entries(tenant_id, validation_id, updated_at DESC, ledger_entry_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_matrix_tenant_created ON live_validation_support_matrix_snapshots(tenant_id, created_at DESC, snapshot_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_reconciliations_tenant ON live_validation_reconciliation_resolutions(tenant_id, resolved_at DESC, reconciliation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_retention_tenant ON live_validation_retention_policies(tenant_id, applies_to, created_at DESC, policy_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_live_validation_scopes_validation ON live_validation_scopes(tenant_id, validation_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_llm_dispatches_provider_status ON llm_dispatches(provider, status, created_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_llm_dispatches_tenant_created ON llm_dispatches(tenant_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_llm_dispatches_tenant_provider_status ON llm_dispatches(tenant_id, provider, status, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_accounts_env_default ON mail_accounts(environment_scope, account_key, canonical_default, updated_at DESC, integration_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_accounts_env_readiness ON mail_accounts(environment_scope, readiness_status, updated_at DESC, integration_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_accounts_tenant_readiness ON mail_accounts(tenant_id, readiness_status, updated_at DESC, mail_account_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_artifacts_attachment ON mail_artifacts(environment_scope, attachment_ref_id, created_at DESC, artifact_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_artifacts_draft ON mail_artifacts(environment_scope, draft_id, created_at DESC, artifact_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_artifacts_message ON mail_artifacts(environment_scope, message_id, created_at DESC, artifact_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_artifacts_operation ON mail_artifacts(environment_scope, operation_id, created_at ASC, artifact_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_artifacts_tenant_operation ON mail_artifacts(tenant_id, operation_id, created_at ASC, artifact_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_artifacts_thread ON mail_artifacts(environment_scope, thread_id, created_at DESC, artifact_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_delivery ON mail_operations(environment_scope, delivery_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_draft ON mail_operations(environment_scope, draft_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_env_class_status ON mail_operations(environment_scope, operation_class, status, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_env_result ON mail_operations(environment_scope, result_mode, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_message ON mail_operations(environment_scope, message_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_run ON mail_operations(environment_scope, run_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_schedule ON mail_operations(environment_scope, schedule_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_tenant_account ON mail_operations(tenant_id, mail_account_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_thread ON mail_operations(environment_scope, thread_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mail_operations_workflow ON mail_operations(environment_scope, workflow_id, updated_at DESC, operation_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_manager_documents_kind ON manager_documents(doc_kind);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_matrix_event_evidence_tenant_connector ON matrix_event_evidence(tenant_id, connector_id, received_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_matrix_hosted_setups_tenant_state ON matrix_hosted_setups(tenant_id, terminal_state, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_matrix_route_policies_tenant_connector ON matrix_route_policies(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_matrix_smoke_tenant_connector ON matrix_smoke_evidence(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_server_states_status ON mcp_server_states(status, updated_at DESC, server_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_server_states_tenant_status ON mcp_server_states(tenant_id, status, updated_at DESC, server_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_servers_enabled ON mcp_servers(enabled, updated_at DESC, server_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_servers_tenant_enabled ON mcp_servers(tenant_id, enabled, updated_at DESC, server_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_tool_exposure_surface ON mcp_tool_exposure_rules(runtime_surface, exposure_mode, server_id, tool_name);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_tool_exposure_tenant_server_tool ON mcp_tool_exposure_rules(tenant_id, server_id, tool_name, runtime_surface);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_tools_server_status ON mcp_tools(server_id, discovery_status, tool_name);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_mcp_tools_tenant_server_status ON mcp_tools(tenant_id, server_id, discovery_status, tool_name);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_memberships_principal_status ON memberships(principal_id, status, tenant_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_memberships_tenant_status ON memberships(tenant_id, status, role, principal_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_policy_records_consumer_started ON consumer_policy_records(consumer_kind, consumer_id, started_at DESC, policy_record_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_policy_records_status_started ON consumer_policy_records(status, started_at DESC, policy_record_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_policy_records_tenant_started ON consumer_policy_records(tenant_id, started_at DESC, policy_record_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_principals_status ON principals(status, created_at DESC, principal_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_provider_auth_states_tenant_provider ON provider_auth_states(tenant_id, provider_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_provider_checks_provider_created ON provider_checks(provider_id, created_at DESC, check_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_provider_models_provider ON provider_models(provider_id, model_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_provider_preferences_tenant ON provider_preferences(tenant_id, provider_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_rem_actions_occurrence ON reminder_actions(occurrence_id, created_at DESC, action_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_rem_actions_reminder ON reminder_actions(reminder_id, created_at DESC, action_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_rem_occurrences_delivery ON reminder_occurrences(environment_scope, latest_delivery_id, updated_at DESC, occurrence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_rem_occurrences_reminder ON reminder_occurrences(environment_scope, reminder_id, scheduled_for DESC, occurrence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_rem_occurrences_run ON reminder_occurrences(environment_scope, run_id, updated_at DESC, occurrence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_rem_occurrences_state ON reminder_occurrences(environment_scope, state, scheduled_for DESC, occurrence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_rem_occurrences_workflow ON reminder_occurrences(environment_scope, workflow_id, updated_at DESC, occurrence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_reminder_actions_tenant_reminder ON reminder_actions(tenant_id, reminder_id, created_at DESC, action_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_reminder_occurrences_tenant_reminder ON reminder_occurrences(tenant_id, reminder_id, scheduled_for DESC, occurrence_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_reminders_env_due ON reminders(environment_scope, next_due_at, updated_at DESC, reminder_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_reminders_env_state ON reminders(environment_scope, current_state, updated_at DESC, reminder_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_reminders_tenant_due ON reminders(tenant_id, next_due_at, updated_at DESC, reminder_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_reminders_tenant_state ON reminders(tenant_id, current_state, updated_at DESC, reminder_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_runs_reminder ON runs(reminder_id, reminder_occurrence_id, created_at DESC, run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_runs_schedule_linkage ON runs(schedule_id, schedule_attempt_id, created_at DESC, run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_runs_tenant_created ON runs(tenant_id, created_at DESC, run_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_sandbox_executions_profile_requested ON sandbox_executions(profile_id, requested_at DESC, execution_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_sandbox_executions_status_requested ON sandbox_executions(status, requested_at DESC, execution_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_sandbox_executions_tenant_status ON sandbox_executions(tenant_id, status, requested_at DESC, execution_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_schedule_attempts_retry_due ON schedule_dispatch_attempts(dispatch_status, next_retry_at, schedule_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_schedule_attempts_schedule_due ON schedule_dispatch_attempts(schedule_id, due_at DESC, attempt_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_schedule_attempts_tenant_schedule_due ON schedule_dispatch_attempts(tenant_id, schedule_id, due_at DESC, attempt_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_schedule_targets_schedule ON schedule_targets(schedule_id, updated_at DESC, target_ref_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_schedule_targets_tenant_schedule ON schedule_targets(tenant_id, schedule_id, target_ref_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_schedules_env_status_due ON schedules(environment_scope, status, next_due_at, schedule_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_schedules_tenant_status_due ON schedules(tenant_id, status, next_due_at, schedule_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_secret_scope_bindings_consumer_secret ON secret_scope_bindings(consumer_kind, consumer_id, secret_ref);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_secret_scope_bindings_tenant_consumer ON secret_scope_bindings(tenant_id, consumer_kind, consumer_id, secret_ref);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_sessions_channel_peer ON sessions(channel, peer_id, thread_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_sessions_tenant_created ON sessions(tenant_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_setup_attempts_tenant_session ON setup_attempts(tenant_id, setup_session_id, created_at ASC, attempt_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_setup_sessions_tenant_state ON setup_sessions(tenant_id, state, updated_at DESC, setup_session_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_setup_sessions_tenant_target ON setup_sessions(tenant_id, target_id, setup_style);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_setup_sessions_tenant_updated ON setup_sessions(tenant_id, updated_at DESC, setup_session_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_slack_event_evidence_tenant_connector ON slack_event_evidence(tenant_id, connector_id, received_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_slack_hosted_setups_tenant_state ON slack_hosted_setups(tenant_id, terminal_state, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_slack_route_policies_tenant_connector ON slack_route_policies(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_slack_smoke_tenant_connector ON slack_smoke_evidence(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_steps_run_id ON steps(run_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_steps_tenant_run_created ON steps(tenant_id, run_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_steps_workflow_linkage ON steps(workflow_id, workflow_step_id, attempt);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_telegram_allowments_tenant_connector ON telegram_allowments(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_telegram_hosted_setups_tenant_state ON telegram_hosted_setups(tenant_id, terminal_state, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_telegram_smoke_tenant_connector ON telegram_smoke_evidence(tenant_id, connector_id, validated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_telegram_update_evidence_tenant_connector ON telegram_update_evidence(tenant_id, connector_id, received_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tenant_audit_events_principal_created ON tenant_audit_events(principal_id, created_at DESC, audit_event_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tenant_audit_events_tenant_created ON tenant_audit_events(tenant_id, created_at DESC, audit_event_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tenant_audit_events_token_created ON tenant_audit_events(token_id, created_at DESC, audit_event_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tenant_secret_versions_secret ON tenant_secret_versions(tenant_id, secret_id, version_number DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tenant_secrets_tenant_status ON tenant_secrets(tenant_id, status, updated_at DESC, secret_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tenants_kind_status ON tenants(tenant_kind, status, created_at DESC, tenant_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_continuity_preview_items_preview ON thread_continuity_preview_items(continuity_preview_id, item_order ASC, preview_item_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_continuity_preview_items_thread ON thread_continuity_preview_items(tenant_id, thread_id, acceptance_sequence ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_continuity_previews_retention ON thread_continuity_previews(tenant_id, retention_expires_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_continuity_previews_thread ON thread_continuity_previews(tenant_id, thread_id, assembly_completed_at DESC, continuity_preview_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_continuity_turns_retention ON thread_continuity_turns(tenant_id, retention_expires_at);;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_thread_continuity_turns_source_event ON thread_continuity_turns(tenant_id, source_event_key) WHERE source_event_key IS NOT NULL AND source_event_key != '';;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_continuity_turns_window ON thread_continuity_turns(tenant_id, thread_id, session_segment_id, acceptance_sequence DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_conversation_shapes_source ON thread_conversation_shapes(tenant_id, connector_id, source_account_id, source_conversation_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_conversation_shapes_thread ON thread_conversation_shapes(tenant_id, thread_id, updated_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_handoff_links_destination ON thread_handoff_links(tenant_id, destination_thread_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_handoff_links_source ON thread_handoff_links(tenant_id, source_thread_id, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_handoff_source_refs_destination ON thread_handoff_source_references(tenant_id, destination_thread_id, destination_session_segment_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_handoff_source_refs_link ON thread_handoff_source_references(handoff_link_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_lifecycle_events_thread ON thread_lifecycle_events(thread_id, occurred_at DESC, lifecycle_event_id DESC);;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_thread_participation_decisions_source_message ON thread_participation_decisions(tenant_id, connector_id, source_account_id, source_conversation_id, source_message_id) WHERE source_message_id IS NOT NULL AND source_message_id != '';;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_participation_decisions_thread ON thread_participation_decisions(tenant_id, thread_id, occurred_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_reset_events_thread ON thread_reset_events(tenant_id, thread_id, completed_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_runtime_projections_thread ON thread_runtime_projections(thread_id, occurred_at DESC, runtime_projection_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_segments_thread_generation ON thread_session_segments(thread_id, generation ASC, session_segment_id ASC);;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_thread_source_current_unique ON thread_source_links(tenant_id, connector_id, source_account_id, source_conversation_id) WHERE current_flag = 1;;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_thread_source_thread ON thread_source_links(thread_id, linked_at DESC, source_linkage_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_threads_tenant_state_activity ON threads(tenant_id, lifecycle_state, last_activity_at DESC, thread_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_token_grants_tenant_status ON token_tenant_grants(tenant_id, status, token_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_token_grants_token_status ON token_tenant_grants(token_id, status, tenant_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_capability_created ON tool_calls(capability_id, created_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_computer_use_session ON tool_calls(computer_use_session_id, created_at DESC, tool_call_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_mcp_server_created ON tool_calls(mcp_server_id, created_at DESC, tool_call_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_run_step ON tool_calls(run_id, step_id, created_at);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_run_step_created ON tool_calls(run_id, step_id, created_at DESC, tool_call_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_sandbox_execution ON tool_calls(sandbox_execution_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_skill_created ON tool_calls(skill_id, created_at DESC, tool_call_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_tenant_status_created ON tool_calls(tenant_id, status, created_at DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_tenant_step ON tool_calls(tenant_id, step_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_tool_calls_workflow_linkage ON tool_calls(workflow_id, workflow_step_id, attempt, created_at DESC, tool_call_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workflow_dependencies_tenant_workflow ON workflow_dependencies(tenant_id, workflow_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workflow_handoffs_tenant_workflow ON workflow_handoffs(tenant_id, workflow_id);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workflow_steps_tenant_workflow_position ON workflow_steps(tenant_id, workflow_id, position);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workflow_steps_workflow_position ON workflow_steps(workflow_id, position ASC, workflow_step_id ASC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workflows_run_env_created ON workflows(run_id, environment_scope, created_at DESC, workflow_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workflows_schedule_linkage ON workflows(schedule_id, schedule_attempt_id, created_at DESC, workflow_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workflows_tenant_updated ON workflows(tenant_id, updated_at DESC, workflow_id DESC);;"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_workspaces_tenant_status_updated ON workspaces(tenant_id, status, updated_at DESC);;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS uq_binding_rules_active_scope ON binding_rules(tenant_id, scope_kind, scope_ref) WHERE status = 'active';;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS uq_memberships_tenant_principal ON memberships(tenant_id, principal_id);;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS uq_tenant_secrets_ref ON tenant_secrets(tenant_id, secret_ref);;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS uq_token_grants_tenant_token ON token_tenant_grants(tenant_id, token_id);;"#
                .to_string(),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS uq_workspaces_tenant_default ON workspaces(tenant_id) WHERE is_default = 1;;"#
                .to_string(),
        ],
    },
    SchemaMigration {
        version: 2,
        name: "memory_assets".to_string(),
        statements: vec![
            r#"CREATE TABLE IF NOT EXISTS memory_assets (
                asset_id TEXT PRIMARY KEY,
                tenant_id TEXT,
                kind TEXT NOT NULL,
                layer TEXT NOT NULL,
                status TEXT NOT NULL,
                visibility TEXT NOT NULL,
                atom_type TEXT,
                owner_kind TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                supersedes_asset_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                document_json TEXT NOT NULL
            );"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_memory_assets_tenant_layer
                ON memory_assets(tenant_id, layer, status, updated_at DESC);"#
                .to_string(),
            r#"CREATE INDEX IF NOT EXISTS idx_memory_assets_supersedes
                ON memory_assets(supersedes_asset_id);"#
                .to_string(),
        ],
    },
    SchemaMigration {
        version: 3,
        name: "model_role_bindings".to_string(),
        statements: vec![
            // One row per role. `provider_id` empty is not stored: unrouting a
            // role deletes the row so the config default can apply again.
            r#"CREATE TABLE IF NOT EXISTS model_role_bindings (
                role TEXT NOT NULL,
                tenant_id TEXT,
                provider_id TEXT NOT NULL,
                model TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (role, tenant_id)
            );"#
                .to_string(),
        ],
    },
    SchemaMigration {
        version: 4,
        name: "llm_dispatch_tools".to_string(),
        statements: vec![
            // What the model was offered, and what it asked to call.
            //
            // Stored with the dispatch for the same reason the messages are:
            // the record has to show what the model could see, or a turn that
            // called a tool cannot be explained afterwards. Nullable, because
            // every dispatch written before this one has neither, and a plain
            // chat request still has neither.
            r#"ALTER TABLE llm_dispatches ADD COLUMN tools_json TEXT;"#.to_string(),
            r#"ALTER TABLE llm_dispatches ADD COLUMN tool_calls_json TEXT;"#.to_string(),
        ],
    }
    ]
}
