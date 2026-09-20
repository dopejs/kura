//! SQLite CRUD for the agent-profile persona domain. Ported from
//! `daemon/internal/store/profile_store.go` (EnsureDefaultAgentProfile,
//! ListAgentProfiles, GetAgentProfileDetail, CreateAgentProfile,
//! UpdateAgentProfile, ActivateAgentProfile, ListAgentProfileVersions,
//! RollbackAgentProfile, RetireAgentProfile) and `profile_projection.go`
//! (ActiveAgentProfileSelection, RecordRuntimeProfileProjection,
//! ListRuntimeProfileProjections). Records are persisted as serialized
//! `kura_profiles` documents (document_json) plus the denormalized tenant
//! columns the schema keeps for scoping, exactly like the Go port.

use chrono::{DateTime, Utc};
use rusqlite::{Transaction, params};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string, opt_time_string};

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

/// Go `defaultReason`.
fn default_reason(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

/// Go `nullableProfileTime`: nil/zero time maps to SQL NULL.
fn nullable_profile_time(value: &Option<DateTime<Utc>>) -> Option<String> {
    value
        .as_ref()
        .filter(|t| t.timestamp() != 0 || t.timestamp_subsec_nanos() != 0)
        .map(now_rfc3339)
}

/// A chrono-defaulted timestamp (Unix epoch) stands in for Go's zero time.
fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

fn scan_profile_document(raw: &str) -> Result<kura_profiles::AgentProfile, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode agent profile document: {e}"))
}

fn scan_version_document(raw: &str) -> Result<kura_profiles::ProfileVersion, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode agent profile version document: {e}"))
}

fn scan_selection_document(raw: &str) -> Result<kura_profiles::ActiveSelection, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode active selection document: {e}"))
}

fn scan_overlay_document(raw: &str) -> Result<kura_profiles::OverlayReference, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode overlay reference document: {e}"))
}

fn scan_audit_document(raw: &str) -> Result<kura_profiles::AuditEvent, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode profile audit event document: {e}"))
}

fn scan_projection_document(raw: &str) -> Result<kura_profiles::RuntimeProjection, String> {
    serde_json::from_str(raw)
        .map_err(|e| format!("decode runtime profile projection document: {e}"))
}

impl SQLiteStore {
    // --- tx helpers (free functions mirroring Go's insertAgentProfileTx &c.) ---

    pub fn ensure_default_agent_profile(
        &self,
        tenant_id: &str,
    ) -> Result<kura_profiles::AgentProfile, String> {
        let items = self.list_agent_profiles(tenant_id, 1)?;
        if let Some(first) = items.items.into_iter().next() {
            return Ok(first);
        }
        let result = self.create_agent_profile(
            &kura_identity::TenantContext {
                tenant_id: tenant_id.to_string(),
                principal_id: "system".to_string(),
                ..kura_identity::TenantContext::default()
            },
            &kura_profiles::MutationInput {
                display_name: "Default Agent".to_string(),
                display_identity: kura_profiles::DisplayIdentity {
                    name: "Kura".to_string(),
                    safe_summary: "Default personal assistant profile".to_string(),
                    ..kura_profiles::DisplayIdentity::default()
                },
                persona: kura_profiles::Persona {
                    tone: "direct".to_string(),
                    safe_summary: "Concise production-oriented behavior".to_string(),
                    ..kura_profiles::Persona::default()
                },
                default_provider_preference: kura_profiles::DefaultProviderPreference {
                    validation_state: kura_profiles::OverlayValidationState::VALID,
                    ..kura_profiles::DefaultProviderPreference::default()
                },
                safety_defaults: kura_profiles::SafetyDefaults {
                    approval_posture: "ask_for_risky_changes".to_string(),
                    validation_state: kura_profiles::OverlayValidationState::VALID,
                    ..kura_profiles::SafetyDefaults::default()
                },
                legacy_mapping_evidence: kura_profiles::default_legacy_mapping_evidence(),
                activate: true,
                reason_code: "default_seeded".to_string(),
                overlay_references: kura_profiles::default_legacy_overlay_reference_inputs(),
            },
        )?;
        Ok(result.profile)
    }

    pub fn list_agent_profiles(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> Result<kura_profiles::ListResponse, String> {
        let limit = if limit <= 0 || limit > 200 { 50 } else { limit };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT p.document_json,
                       CASE WHEN s.profile_id IS NOT NULL THEN 1 ELSE 0 END,
                       (SELECT COUNT(1) FROM agent_profile_overlay_references o
                        WHERE o.tenant_id = p.tenant_id AND o.profile_id = p.profile_id
                          AND o.profile_version_id = p.active_version_id)
                FROM agent_profiles p
                LEFT JOIN agent_profile_active_selections s
                  ON s.tenant_id = p.tenant_id AND s.profile_id = p.profile_id
                 AND s.selection_scope = 'tenant_default'
                WHERE p.tenant_id = ?1
                ORDER BY p.updated_at DESC, p.profile_id DESC
                LIMIT ?2"#,
            )
            .map_err(|e| format!("list agent profiles {tenant_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let tenant_default: i64 = row.get(1).map_err(|e| e.to_string())?;
            let overlay_count: i64 = row.get(2).map_err(|e| e.to_string())?;
            let mut profile = scan_profile_document(&raw)?;
            profile.tenant_default = tenant_default == 1;
            profile.overlay_reference_count = overlay_count;
            items.push(kura_profiles::redact_profile(profile));
        }
        Ok(kura_profiles::ListResponse {
            tenant_id: tenant_id.to_string(),
            page: kura_profiles::Page {
                limit,
                next_cursor: String::new(),
                order: "updated_at_desc".to_string(),
            },
            items,
        })
    }

    pub fn get_agent_profile_detail(
        &self,
        tenant_id: &str,
        profile_id: &str,
    ) -> Result<Option<kura_profiles::ProfileDetail>, String> {
        let Some(profile) = self.get_agent_profile(tenant_id, profile_id)? else {
            return Ok(None);
        };
        let versions = self.list_agent_profile_versions(tenant_id, profile_id, 50)?;
        let overlays = self.list_agent_profile_overlays(tenant_id, profile_id)?;
        let audits = self.list_agent_profile_audit_events(tenant_id, profile_id, 25)?;
        Ok(Some(kura_profiles::ProfileDetail {
            profile: kura_profiles::redact_profile(profile),
            versions,
            overlay_references: overlays,
            audit_events: audits,
        }))
    }

    /// Go `CreateAgentProfile`. Returns Err for validation / explicit-actor
    /// failures (reason codes embedded in the message) and `Ok` with the
    /// mutation result otherwise.
    pub fn create_agent_profile(
        &self,
        actor: &kura_identity::TenantContext,
        input: &kura_profiles::MutationInput,
    ) -> Result<kura_profiles::MutationResult, String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_profiles::ProfilesError::ExplicitActorRequired.to_string());
        }
        kura_profiles::validate_mutation(input).map_err(|e| e.to_string())?;
        self.validate_profile_mutation_against_store(input)?;
        let now = Utc::now();
        let profile_id = new_store_id("prof");
        let version_id = new_store_id("profv");
        let audit_id = new_store_id("audit_profile");
        let status = if input.activate {
            kura_profiles::Status::ACTIVE
        } else {
            kura_profiles::Status::DRAFT
        };
        let mut profile = kura_profiles::AgentProfile {
            profile_id: profile_id.clone(),
            tenant_id: actor.tenant_id.clone(),
            display_name: input.display_name.trim().to_string(),
            display_identity: input.display_identity.clone(),
            persona: input.persona.clone(),
            default_provider_preference: input.default_provider_preference.clone(),
            safety_defaults: input.safety_defaults.clone(),
            legacy_mapping_evidence: input.legacy_mapping_evidence.clone(),
            status,
            active_version_id: version_id.clone(),
            created_at: now,
            updated_at: now,
            created_by_principal_id: actor.principal_id.clone(),
            updated_by_principal_id: actor.principal_id.clone(),
            redaction_status: kura_profiles::RedactionStatus::REDACTED,
            ..kura_profiles::AgentProfile::default()
        };
        profile = kura_profiles::redact_profile(profile);
        let version = kura_profiles::ProfileVersion {
            profile_version_id: version_id.clone(),
            profile_id: profile_id.clone(),
            tenant_id: actor.tenant_id.clone(),
            version_number: 1,
            source_version_id: String::new(),
            change_kind: kura_profiles::ChangeKind::CREATED,
            change_summary: "Created profile".to_string(),
            snapshot: profile.clone(),
            rollback_eligibility: kura_profiles::RollbackEligibility::ELIGIBLE,
            actor_principal_id: actor.principal_id.clone(),
            created_at: now,
            audit_event_id: audit_id.clone(),
            redaction_status: kura_profiles::RedactionStatus::REDACTED,
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin create agent profile: {e}"))?;
        insert_agent_profile_tx(&tx, &profile)?;
        insert_agent_profile_version_tx(&tx, &version)?;
        replace_overlay_references_tx(
            &tx,
            &actor.tenant_id,
            &profile_id,
            &version_id,
            &input.overlay_references,
            now,
        )?;
        let mut selection = kura_profiles::ActiveSelection::default();
        if input.activate {
            selection = upsert_active_selection_tx(
                &tx,
                actor,
                &profile,
                &version_id,
                kura_profiles::SelectionReason::DEFAULT_SEEDED,
                &audit_id,
                now,
            )?;
        }
        insert_profile_audit_tx(
            &tx,
            &kura_profiles::AuditEvent {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                profile_id: profile_id.clone(),
                profile_version_id: version_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "profile.created".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "profiles.manage".to_string(),
                reason_code: default_reason(&input.reason_code, "user_created_profile"),
                safe_summary: "Profile created".to_string(),
                occurred_at: now,
                redaction_status: kura_profiles::RedactionStatus::REDACTED,
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit create agent profile: {e}"))?;
        Ok(kura_profiles::MutationResult {
            profile,
            version,
            selection,
            audit_event_id: audit_id,
        })
    }

    pub fn update_agent_profile(
        &self,
        actor: &kura_identity::TenantContext,
        profile_id: &str,
        input: &kura_profiles::MutationInput,
    ) -> Result<kura_profiles::MutationResult, String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_profiles::ProfilesError::ExplicitActorRequired.to_string());
        }
        kura_profiles::validate_mutation(input).map_err(|e| e.to_string())?;
        self.validate_profile_mutation_against_store(input)?;
        let mut current = self
            .get_agent_profile(&actor.tenant_id, profile_id)?
            .ok_or_else(|| "agent profile not found".to_string())?;
        let now = Utc::now();
        let version_number = self.next_agent_profile_version(&actor.tenant_id, profile_id)?;
        let source_version_id = current.active_version_id.clone();
        let version_id = new_store_id("profv");
        let audit_id = new_store_id("audit_profile");
        current.display_name = input.display_name.trim().to_string();
        current.display_identity = input.display_identity.clone();
        current.persona = input.persona.clone();
        current.default_provider_preference = input.default_provider_preference.clone();
        current.safety_defaults = input.safety_defaults.clone();
        if !input.legacy_mapping_evidence.is_empty() {
            current.legacy_mapping_evidence = input.legacy_mapping_evidence.clone();
        }
        current.active_version_id = version_id.clone();
        current.updated_at = now;
        current.updated_by_principal_id = actor.principal_id.clone();
        current = kura_profiles::redact_profile(current);
        let version = kura_profiles::ProfileVersion {
            profile_version_id: version_id.clone(),
            profile_id: profile_id.to_string(),
            tenant_id: actor.tenant_id.clone(),
            version_number,
            source_version_id,
            change_kind: kura_profiles::ChangeKind::UPDATED,
            change_summary: "Updated profile".to_string(),
            snapshot: current.clone(),
            rollback_eligibility: kura_profiles::RollbackEligibility::ELIGIBLE,
            actor_principal_id: actor.principal_id.clone(),
            created_at: now,
            audit_event_id: audit_id.clone(),
            redaction_status: kura_profiles::RedactionStatus::REDACTED,
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin update agent profile: {e}"))?;
        update_agent_profile_tx(&tx, &current)?;
        insert_agent_profile_version_tx(&tx, &version)?;
        replace_overlay_references_tx(
            &tx,
            &actor.tenant_id,
            profile_id,
            &version_id,
            &input.overlay_references,
            now,
        )?;
        insert_profile_audit_tx(
            &tx,
            &kura_profiles::AuditEvent {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                profile_id: profile_id.to_string(),
                profile_version_id: version_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "profile.updated".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "profiles.manage".to_string(),
                reason_code: default_reason(&input.reason_code, "user_updated_profile"),
                safe_summary: "Profile updated".to_string(),
                occurred_at: now,
                redaction_status: kura_profiles::RedactionStatus::REDACTED,
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit update agent profile: {e}"))?;
        Ok(kura_profiles::MutationResult {
            profile: current,
            version,
            selection: kura_profiles::ActiveSelection::default(),
            audit_event_id: audit_id,
        })
    }

    pub fn activate_agent_profile(
        &self,
        actor: &kura_identity::TenantContext,
        profile_id: &str,
        input: &kura_profiles::ActivationInput,
    ) -> Result<kura_profiles::ActiveSelection, String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_profiles::ProfilesError::ExplicitActorRequired.to_string());
        }
        let mut profile = self
            .get_agent_profile(&actor.tenant_id, profile_id)?
            .ok_or_else(|| "agent profile not found".to_string())?;
        let version_id = if input.profile_version_id.trim().is_empty() {
            profile.active_version_id.clone()
        } else {
            input.profile_version_id.clone()
        };
        let version = self
            .get_agent_profile_version(&actor.tenant_id, profile_id, &version_id)?
            .ok_or_else(|| "agent profile not found".to_string())?;
        kura_profiles::can_activate(&profile, &version).map_err(|e| e.to_string())?;
        let now = Utc::now();
        let audit_id = new_store_id("audit_profile");
        profile.status = kura_profiles::Status::ACTIVE;
        profile.active_version_id = version_id.clone();
        profile.updated_at = now;
        profile.updated_by_principal_id = actor.principal_id.clone();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin activate agent profile: {e}"))?;
        let selection = upsert_active_selection_tx(
            &tx,
            actor,
            &profile,
            &version_id,
            kura_profiles::SelectionReason::USER_ACTIVATED,
            &audit_id,
            now,
        )?;
        update_agent_profile_tx(&tx, &profile)?;
        insert_profile_audit_tx(
            &tx,
            &kura_profiles::AuditEvent {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                profile_id: profile_id.to_string(),
                profile_version_id: version_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "profile.activated".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "profiles.manage".to_string(),
                reason_code: default_reason(&input.reason_code, "user_selected_default"),
                safe_summary: "Profile activated".to_string(),
                occurred_at: now,
                redaction_status: kura_profiles::RedactionStatus::REDACTED,
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit activate agent profile: {e}"))?;
        Ok(selection)
    }

    pub fn list_agent_profile_versions(
        &self,
        tenant_id: &str,
        profile_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_profiles::ProfileVersion>, String> {
        let limit = if limit <= 0 || limit > 200 { 50 } else { limit };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM agent_profile_versions
                 WHERE tenant_id = ?1 AND profile_id = ?2
                 ORDER BY version_number DESC LIMIT ?3",
            )
            .map_err(|e| format!("list agent profile versions {tenant_id}/{profile_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, profile_id, limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_version_document(&raw)?);
        }
        Ok(items)
    }

    pub fn rollback_agent_profile(
        &self,
        actor: &kura_identity::TenantContext,
        profile_id: &str,
        input: &kura_profiles::RollbackInput,
    ) -> Result<kura_profiles::MutationResult, String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_profiles::ProfilesError::ExplicitActorRequired.to_string());
        }
        if input.source_profile_version_id.trim().is_empty() {
            return Err(kura_profiles::ProfilesError::ProfileNotActivatable.to_string());
        }
        let source = self
            .get_agent_profile_version(
                &actor.tenant_id,
                profile_id,
                &input.source_profile_version_id,
            )?
            .ok_or_else(|| "agent profile not found".to_string())?;
        let mut current = self
            .get_agent_profile(&actor.tenant_id, profile_id)?
            .ok_or_else(|| "agent profile not found".to_string())?;
        if kura_profiles::rollback_eligibility_for(&current, &source)
            != kura_profiles::RollbackEligibility::ELIGIBLE
        {
            return Err(kura_profiles::ProfilesError::ProfileNotActivatable.to_string());
        }
        let was_tenant_default = self.is_tenant_default_profile(&actor.tenant_id, profile_id)?;
        let source_overlays = self.list_agent_profile_overlays_for_version(
            &actor.tenant_id,
            profile_id,
            &source.profile_version_id,
        )?;
        let overlay_inputs: Vec<kura_profiles::OverlayReferenceInput> = source_overlays
            .iter()
            .map(|overlay| kura_profiles::OverlayReferenceInput {
                reference_kind: overlay.reference_kind.clone(),
                reference_uri: overlay.reference_uri.clone(),
                scope: overlay.scope.clone(),
            })
            .collect();
        let mutation_input = kura_profiles::MutationInput {
            display_name: source.snapshot.display_name.clone(),
            display_identity: source.snapshot.display_identity.clone(),
            persona: source.snapshot.persona.clone(),
            default_provider_preference: source.snapshot.default_provider_preference.clone(),
            safety_defaults: source.snapshot.safety_defaults.clone(),
            legacy_mapping_evidence: source.snapshot.legacy_mapping_evidence.clone(),
            overlay_references: overlay_inputs.clone(),
            ..kura_profiles::MutationInput::default()
        };
        kura_profiles::validate_mutation(&mutation_input).map_err(|e| e.to_string())?;
        self.validate_profile_mutation_against_store(&mutation_input)?;
        let now = Utc::now();
        let version_number = self.next_agent_profile_version(&actor.tenant_id, profile_id)?;
        let version_id = new_store_id("profv");
        let audit_id = new_store_id("audit_profile");
        current.display_name = source.snapshot.display_name.trim().to_string();
        current.display_identity = source.snapshot.display_identity.clone();
        current.persona = source.snapshot.persona.clone();
        current.default_provider_preference = source.snapshot.default_provider_preference.clone();
        current.safety_defaults = source.snapshot.safety_defaults.clone();
        current.legacy_mapping_evidence = source.snapshot.legacy_mapping_evidence.clone();
        current.active_version_id = version_id.clone();
        current.updated_at = now;
        current.updated_by_principal_id = actor.principal_id.clone();
        current = kura_profiles::redact_profile(current);
        let version = kura_profiles::ProfileVersion {
            profile_version_id: version_id.clone(),
            profile_id: profile_id.to_string(),
            tenant_id: actor.tenant_id.clone(),
            version_number,
            source_version_id: source.profile_version_id.clone(),
            change_kind: kura_profiles::ChangeKind::ROLLED_BACK,
            change_summary: "Rolled back profile".to_string(),
            snapshot: current.clone(),
            rollback_eligibility: kura_profiles::RollbackEligibility::ELIGIBLE,
            actor_principal_id: actor.principal_id.clone(),
            created_at: now,
            audit_event_id: audit_id.clone(),
            redaction_status: kura_profiles::RedactionStatus::REDACTED,
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin rollback agent profile: {e}"))?;
        update_agent_profile_tx(&tx, &current)?;
        insert_agent_profile_version_tx(&tx, &version)?;
        replace_overlay_references_tx(
            &tx,
            &actor.tenant_id,
            profile_id,
            &version_id,
            &overlay_inputs,
            now,
        )?;
        let mut selection = kura_profiles::ActiveSelection::default();
        if was_tenant_default {
            selection = upsert_active_selection_tx(
                &tx,
                actor,
                &current,
                &version_id,
                kura_profiles::SelectionReason::ROLLBACK_ACTIVATED,
                &audit_id,
                now,
            )?;
        }
        insert_profile_audit_tx(
            &tx,
            &kura_profiles::AuditEvent {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                profile_id: profile_id.to_string(),
                profile_version_id: version_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "profile.rolled_back".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "profiles.manage".to_string(),
                reason_code: default_reason(&input.reason_code, "operator_reverted_persona"),
                safe_summary: "Profile rolled back".to_string(),
                occurred_at: now,
                redaction_status: kura_profiles::RedactionStatus::REDACTED,
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit rollback agent profile: {e}"))?;
        Ok(kura_profiles::MutationResult {
            profile: current,
            version,
            selection,
            audit_event_id: audit_id,
        })
    }

    pub fn retire_agent_profile(
        &self,
        actor: &kura_identity::TenantContext,
        profile_id: &str,
        status: kura_profiles::Status,
        input: &kura_profiles::RetirementInput,
    ) -> Result<kura_profiles::MutationResult, String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_profiles::ProfilesError::ExplicitActorRequired.to_string());
        }
        let mut profile = self
            .get_agent_profile(&actor.tenant_id, profile_id)?
            .ok_or_else(|| "agent profile not found".to_string())?;
        if status != kura_profiles::Status::ARCHIVED && status != kura_profiles::Status::DISABLED {
            return Err(format!("unsupported retirement status {}", status.as_str()));
        }
        let was_tenant_default = self.is_tenant_default_profile(&actor.tenant_id, profile_id)?;
        let now = Utc::now();
        let version_number = self.next_agent_profile_version(&actor.tenant_id, profile_id)?;
        let version_id = new_store_id("profv");
        let audit_id = new_store_id("audit_profile");
        profile.status = status.clone();
        profile.active_version_id = version_id.clone();
        profile.updated_at = now;
        profile.updated_by_principal_id = actor.principal_id.clone();
        if status == kura_profiles::Status::ARCHIVED {
            profile.archived_at = Some(now);
        } else {
            profile.disabled_at = Some(now);
        }
        let mut version = kura_profiles::ProfileVersion {
            profile_version_id: version_id.clone(),
            profile_id: profile_id.to_string(),
            tenant_id: actor.tenant_id.clone(),
            version_number,
            source_version_id: String::new(),
            change_kind: kura_profiles::ChangeKind::ARCHIVED,
            change_summary: "Retired profile".to_string(),
            snapshot: profile.clone(),
            rollback_eligibility: kura_profiles::RollbackEligibility::PROFILE_ARCHIVED,
            actor_principal_id: actor.principal_id.clone(),
            created_at: now,
            audit_event_id: audit_id.clone(),
            redaction_status: kura_profiles::RedactionStatus::REDACTED,
        };
        let mut event_kind = "profile.archived";
        if status == kura_profiles::Status::DISABLED {
            version.change_kind = kura_profiles::ChangeKind::DISABLED;
            version.rollback_eligibility = kura_profiles::RollbackEligibility::PROFILE_DISABLED;
            event_kind = "profile.disabled";
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin retire agent profile: {e}"))?;
        update_agent_profile_tx(&tx, &profile)?;
        insert_agent_profile_version_tx(&tx, &version)?;
        tx.execute(
            "DELETE FROM agent_profile_active_selections WHERE tenant_id = ?1 AND profile_id = ?2",
            params![actor.tenant_id, profile_id],
        )
        .map_err(|e| format!("delete active selections for retired profile: {e}"))?;
        let mut selection = kura_profiles::ActiveSelection::default();
        if was_tenant_default {
            let default_profile = ensure_default_agent_profile_tx(&tx, &actor.tenant_id, now)?;
            selection = upsert_active_selection_tx(
                &tx,
                &kura_identity::TenantContext {
                    tenant_id: actor.tenant_id.clone(),
                    principal_id: "system".to_string(),
                    ..kura_identity::TenantContext::default()
                },
                &default_profile,
                &default_profile.active_version_id,
                kura_profiles::SelectionReason::SYSTEM_FALLBACK,
                &audit_id,
                now,
            )?;
        }
        insert_profile_audit_tx(
            &tx,
            &kura_profiles::AuditEvent {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                profile_id: profile_id.to_string(),
                profile_version_id: version_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: event_kind.to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "profiles.manage".to_string(),
                reason_code: default_reason(&input.reason_code, "operator_retired_profile"),
                safe_summary: "Profile retired".to_string(),
                occurred_at: now,
                redaction_status: kura_profiles::RedactionStatus::REDACTED,
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit retire agent profile: {e}"))?;
        Ok(kura_profiles::MutationResult {
            profile,
            version,
            selection,
            audit_event_id: audit_id,
        })
    }

    // --- runtime projection DAOs (profile_projection.go) ---

    /// Go `ActiveAgentProfileSelection`: returns the tenant-default active
    /// selection joined with its profile; lazily ensures the default profile
    /// exists when no selection is recorded yet.
    pub fn active_agent_profile_selection(
        &self,
        tenant_id: &str,
    ) -> Result<Option<(kura_profiles::AgentProfile, kura_profiles::ActiveSelection)>, String> {
        let found: Result<(String, String), rusqlite::Error> = self.conn.query_row(
            r#"SELECT p.document_json, s.document_json
            FROM agent_profile_active_selections s
            JOIN agent_profiles p ON p.tenant_id = s.tenant_id AND p.profile_id = s.profile_id
            WHERE s.tenant_id = ?1 AND s.selection_scope = 'tenant_default'"#,
            params![tenant_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );
        match found {
            Ok((profile_raw, selection_raw)) => {
                let profile = scan_profile_document(&profile_raw)?;
                let selection = scan_selection_document(&selection_raw)?;
                Ok(Some((profile, selection)))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let default = self.ensure_default_agent_profile(tenant_id)?;
                self.active_agent_profile_selection(&default.tenant_id)
            }
            Err(e) => Err(format!("load active agent profile selection: {e}")),
        }
    }

    pub fn record_runtime_profile_projection(
        &self,
        mut projection: kura_profiles::RuntimeProjection,
    ) -> Result<kura_profiles::RuntimeProjection, String> {
        if projection.runtime_profile_projection_id.is_empty() {
            projection.runtime_profile_projection_id = new_store_id("rpp");
        }
        if is_unset_time(&projection.occurred_at) {
            projection.occurred_at = Utc::now();
        }
        let document_json = serde_json::to_string(&projection)
            .map_err(|e| format!("marshal runtime profile projection: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO agent_profile_runtime_projections (
                    runtime_profile_projection_id, tenant_id, profile_id, profile_version_id,
                    selection_id, resource_kind, resource_id, thread_id, session_segment_id,
                    run_id, workflow_id, handoff_link_id, selection_scope, selection_reason,
                    safe_display_name, safe_summary, occurred_at, retention_expires_at,
                    redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"#,
                params![
                    projection.runtime_profile_projection_id,
                    projection.tenant_id,
                    projection.profile_id,
                    projection.profile_version_id,
                    projection.selection_id,
                    enum_str(&projection.resource_kind),
                    projection.resource_id,
                    null_string(&projection.thread_id),
                    null_string(&projection.session_segment_id),
                    null_string(&projection.run_id),
                    null_string(&projection.workflow_id),
                    null_string(&projection.handoff_link_id),
                    projection.selection_scope,
                    enum_str(&projection.selection_reason),
                    projection.safe_display_name,
                    projection.safe_summary,
                    now_rfc3339(&projection.occurred_at),
                    opt_time_string(&projection.retention_expires_at),
                    enum_str(&projection.redaction_status),
                    document_json,
                ],
            )
            .map_err(|e| format!("record runtime profile projection: {e}"))?;
        Ok(projection)
    }

    pub fn list_runtime_profile_projections(
        &self,
        tenant_id: &str,
        resource_kind: &str,
        resource_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_profiles::RuntimeProjection>, String> {
        let limit = if limit <= 0 || limit > 100 { 20 } else { limit };
        let mut query = String::from(
            "SELECT document_json FROM agent_profile_runtime_projections WHERE tenant_id = ?1",
        );
        let mut args: Vec<rusqlite::types::Value> = vec![tenant_id.to_string().into()];
        if !resource_kind.trim().is_empty() {
            query.push_str(" AND resource_kind = ?");
            args.push(resource_kind.trim().to_string().into());
        }
        if !resource_id.trim().is_empty() {
            query.push_str(" AND resource_id = ?");
            args.push(resource_id.trim().to_string().into());
        }
        if !thread_id.trim().is_empty() {
            query.push_str(" AND thread_id = ?");
            args.push(thread_id.trim().to_string().into());
        }
        query.push_str(" ORDER BY occurred_at DESC, runtime_profile_projection_id DESC LIMIT ?");
        args.push(limit.into());
        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| format!("list runtime profile projections: {e}"))?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_projection_document(&raw)?);
        }
        Ok(items)
    }

    // --- helpers ---

    pub fn get_agent_profile(
        &self,
        tenant_id: &str,
        profile_id: &str,
    ) -> Result<Option<kura_profiles::AgentProfile>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM agent_profiles WHERE tenant_id = ?1 AND profile_id = ?2",
            )
            .map_err(|e| format!("get agent profile {profile_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, profile_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(scan_profile_document(&raw)?))
    }

    /// Go `ListProviderModels`-backed validation: a non-empty provider
    /// preference must resolve to an available model.
    pub fn validate_profile_mutation_against_store(
        &self,
        input: &kura_profiles::MutationInput,
    ) -> Result<(), String> {
        let provider_id = input.default_provider_preference.provider_id.trim();
        if provider_id.is_empty() {
            return Ok(());
        }
        let models = self.list_provider_models()?;
        let model_id = input.default_provider_preference.model.trim();
        let mut provider_known = false;
        let mut available_default = false;
        let mut available_any = false;
        for model in &models {
            if model.provider_id.trim() != provider_id {
                continue;
            }
            provider_known = true;
            if model.available {
                available_any = true;
            }
            if model_id.is_empty() {
                if model.default && model.available {
                    available_default = true;
                }
                continue;
            }
            if model.model_id.trim() != model_id {
                continue;
            }
            if !model.available {
                return Err(
                    kura_profiles::invalid_profile_reason("provider_model_unavailable").to_string(),
                );
            }
            if !input
                .default_provider_preference
                .reasoning_level
                .trim()
                .is_empty()
                && !model
                    .reasoning_levels
                    .iter()
                    .any(|r| r.trim() == input.default_provider_preference.reasoning_level.trim())
            {
                return Err(kura_profiles::invalid_profile_reason(
                    "reasoning_level_unsupported_for_model",
                )
                .to_string());
            }
            return Ok(());
        }
        if !provider_known {
            return Err(
                kura_profiles::invalid_profile_reason("provider_not_available").to_string(),
            );
        }
        if model_id.is_empty() && (available_default || available_any) {
            return Ok(());
        }
        Err(kura_profiles::invalid_profile_reason("provider_model_not_available").to_string())
    }

    pub fn is_tenant_default_profile(
        &self,
        tenant_id: &str,
        profile_id: &str,
    ) -> Result<bool, String> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(1) FROM agent_profile_active_selections
                 WHERE tenant_id = ?1 AND profile_id = ?2 AND selection_scope = 'tenant_default'",
                params![tenant_id, profile_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("check tenant default profile: {e}"))?;
        Ok(count > 0)
    }

    pub fn next_agent_profile_version(
        &self,
        tenant_id: &str,
        profile_id: &str,
    ) -> Result<i64, String> {
        let max: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(version_number) FROM agent_profile_versions WHERE tenant_id = ?1 AND profile_id = ?2",
                params![tenant_id, profile_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(max.unwrap_or(0) + 1)
    }

    pub fn get_agent_profile_version(
        &self,
        tenant_id: &str,
        profile_id: &str,
        version_id: &str,
    ) -> Result<Option<kura_profiles::ProfileVersion>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM agent_profile_versions
                 WHERE tenant_id = ?1 AND profile_id = ?2 AND profile_version_id = ?3",
            )
            .map_err(|e| format!("get agent profile version {version_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, profile_id, version_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(scan_version_document(&raw)?))
    }

    /// Go `listAgentProfileOverlays`: overlays attached to the active version.
    pub fn list_agent_profile_overlays(
        &self,
        tenant_id: &str,
        profile_id: &str,
    ) -> Result<Vec<kura_profiles::OverlayReference>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT o.document_json
                FROM agent_profile_overlay_references o
                JOIN agent_profiles p ON p.tenant_id = o.tenant_id AND p.profile_id = o.profile_id
                 AND p.active_version_id = o.profile_version_id
                WHERE o.tenant_id = ?1 AND o.profile_id = ?2
                ORDER BY o.created_at ASC"#,
            )
            .map_err(|e| format!("list agent profile overlays: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, profile_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_overlay_document(&raw)?);
        }
        Ok(items)
    }

    pub fn list_agent_profile_overlays_for_version(
        &self,
        tenant_id: &str,
        profile_id: &str,
        version_id: &str,
    ) -> Result<Vec<kura_profiles::OverlayReference>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM agent_profile_overlay_references
                 WHERE tenant_id = ?1 AND profile_id = ?2 AND profile_version_id = ?3
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("list agent profile overlays for version: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, profile_id, version_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_overlay_document(&raw)?);
        }
        Ok(items)
    }

    pub fn list_agent_profile_audit_events(
        &self,
        tenant_id: &str,
        profile_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_profiles::AuditEvent>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM agent_profile_audit_events
                 WHERE tenant_id = ?1 AND profile_id = ?2
                 ORDER BY occurred_at DESC LIMIT ?3",
            )
            .map_err(|e| format!("list agent profile audit events: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, profile_id, limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_audit_document(&raw)?);
        }
        Ok(items)
    }

    /// Go `profileSelectable`/IsProfileSelectable: active profile for the tenant.
    pub fn is_profile_selectable(&self, tenant_id: &str, profile_id: &str) -> Result<bool, String> {
        let profile_id = profile_id.trim();
        if profile_id.is_empty() {
            return Ok(false);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT status FROM agent_profiles WHERE tenant_id = ?1 AND profile_id = ?2")
            .map_err(|e| format!("check profile selectable: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), profile_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(false);
        };
        let status: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(status == "active")
    }
}

fn insert_agent_profile_tx(
    tx: &Transaction,
    profile: &kura_profiles::AgentProfile,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(profile).map_err(|e| format!("marshal agent profile: {e}"))?;
    tx.execute(
        r#"INSERT INTO agent_profiles (
            profile_id, tenant_id, display_name, status, active_version_id, created_at,
            updated_at, archived_at, disabled_at, created_by_principal_id,
            updated_by_principal_id, redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
        params![
            profile.profile_id,
            profile.tenant_id,
            profile.display_name,
            profile.status.as_str(),
            profile.active_version_id,
            now_rfc3339(&profile.created_at),
            now_rfc3339(&profile.updated_at),
            nullable_profile_time(&profile.archived_at),
            nullable_profile_time(&profile.disabled_at),
            profile.created_by_principal_id,
            profile.updated_by_principal_id,
            profile.redaction_status.as_str(),
            document_json,
        ],
    )
    .map_err(|e| format!("insert agent profile {}: {e}", profile.profile_id))?;
    Ok(())
}

fn ensure_default_agent_profile_tx(
    tx: &Transaction,
    tenant_id: &str,
    now: DateTime<Utc>,
) -> Result<kura_profiles::AgentProfile, String> {
    let mut stmt = tx
        .prepare(
            "SELECT document_json FROM agent_profiles
             WHERE tenant_id = ?1 AND display_name = 'Default Agent' AND status = 'active'
             ORDER BY created_at ASC LIMIT 1",
        )
        .map_err(|e| format!("ensure default agent profile: {e}"))?;
    let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        return scan_profile_document(&raw);
    }
    let profile_id = new_store_id("prof");
    let version_id = new_store_id("profv");
    let profile = kura_profiles::AgentProfile {
        profile_id: profile_id.clone(),
        tenant_id: tenant_id.to_string(),
        display_name: "Default Agent".to_string(),
        display_identity: kura_profiles::DisplayIdentity {
            name: "Kura".to_string(),
            safe_summary: "Default personal assistant profile".to_string(),
            ..kura_profiles::DisplayIdentity::default()
        },
        persona: kura_profiles::Persona {
            tone: "direct".to_string(),
            safe_summary: "Concise production-oriented behavior".to_string(),
            ..kura_profiles::Persona::default()
        },
        default_provider_preference: kura_profiles::DefaultProviderPreference {
            validation_state: kura_profiles::OverlayValidationState::VALID,
            ..kura_profiles::DefaultProviderPreference::default()
        },
        safety_defaults: kura_profiles::SafetyDefaults {
            approval_posture: "ask_for_risky_changes".to_string(),
            validation_state: kura_profiles::OverlayValidationState::VALID,
            ..kura_profiles::SafetyDefaults::default()
        },
        legacy_mapping_evidence: kura_profiles::default_legacy_mapping_evidence(),
        status: kura_profiles::Status::ACTIVE,
        active_version_id: version_id.clone(),
        created_at: now,
        updated_at: now,
        created_by_principal_id: "system".to_string(),
        updated_by_principal_id: "system".to_string(),
        redaction_status: kura_profiles::RedactionStatus::REDACTED,
        ..kura_profiles::AgentProfile::default()
    };
    let version = kura_profiles::ProfileVersion {
        profile_version_id: version_id.clone(),
        profile_id: profile_id.clone(),
        tenant_id: tenant_id.to_string(),
        version_number: 1,
        change_kind: kura_profiles::ChangeKind::CREATED,
        change_summary: "Seeded default profile".to_string(),
        snapshot: profile.clone(),
        rollback_eligibility: kura_profiles::RollbackEligibility::ELIGIBLE,
        actor_principal_id: "system".to_string(),
        created_at: now,
        redaction_status: kura_profiles::RedactionStatus::REDACTED,
        ..kura_profiles::ProfileVersion::default()
    };
    insert_agent_profile_tx(tx, &profile)?;
    insert_agent_profile_version_tx(tx, &version)?;
    replace_overlay_references_tx(
        tx,
        tenant_id,
        &profile_id,
        &version_id,
        &kura_profiles::default_legacy_overlay_reference_inputs(),
        now,
    )?;
    Ok(profile)
}

fn update_agent_profile_tx(
    tx: &Transaction,
    profile: &kura_profiles::AgentProfile,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(profile).map_err(|e| format!("marshal agent profile: {e}"))?;
    tx.execute(
        r#"UPDATE agent_profiles SET
            display_name = ?1, status = ?2, active_version_id = ?3, updated_at = ?4,
            archived_at = ?5, disabled_at = ?6, updated_by_principal_id = ?7,
            redaction_status = ?8, document_json = ?9
        WHERE tenant_id = ?10 AND profile_id = ?11"#,
        params![
            profile.display_name,
            profile.status.as_str(),
            profile.active_version_id,
            now_rfc3339(&profile.updated_at),
            nullable_profile_time(&profile.archived_at),
            nullable_profile_time(&profile.disabled_at),
            profile.updated_by_principal_id,
            profile.redaction_status.as_str(),
            document_json,
            profile.tenant_id,
            profile.profile_id,
        ],
    )
    .map_err(|e| format!("update agent profile {}: {e}", profile.profile_id))?;
    Ok(())
}

fn insert_agent_profile_version_tx(
    tx: &Transaction,
    version: &kura_profiles::ProfileVersion,
) -> Result<(), String> {
    let document_json = serde_json::to_string(version)
        .map_err(|e| format!("marshal agent profile version: {e}"))?;
    tx.execute(
        r#"INSERT INTO agent_profile_versions (
            profile_version_id, profile_id, tenant_id, version_number, source_version_id,
            change_kind, change_summary, rollback_eligibility, actor_principal_id, created_at,
            audit_event_id, redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
        params![
            version.profile_version_id,
            version.profile_id,
            version.tenant_id,
            version.version_number,
            null_string(&version.source_version_id),
            version.change_kind.as_str(),
            version.change_summary,
            version.rollback_eligibility.as_str(),
            null_string(&version.actor_principal_id),
            now_rfc3339(&version.created_at),
            null_string(&version.audit_event_id),
            version.redaction_status.as_str(),
            document_json,
        ],
    )
    .map_err(|e| {
        format!(
            "insert agent profile version {}: {e}",
            version.profile_version_id
        )
    })?;
    Ok(())
}

fn upsert_active_selection_tx(
    tx: &Transaction,
    actor: &kura_identity::TenantContext,
    profile: &kura_profiles::AgentProfile,
    version_id: &str,
    reason: kura_profiles::SelectionReason,
    audit_id: &str,
    now: DateTime<Utc>,
) -> Result<kura_profiles::ActiveSelection, String> {
    let selection = kura_profiles::ActiveSelection {
        selection_id: new_store_id("sel"),
        tenant_id: actor.tenant_id.clone(),
        profile_id: profile.profile_id.clone(),
        profile_version_id: version_id.to_string(),
        selection_scope: kura_profiles::SELECTION_SCOPE_TENANT_DEFAULT.to_string(),
        selection_reason: reason,
        selected_by_principal_id: actor.principal_id.clone(),
        selected_at: now,
        audit_event_id: audit_id.to_string(),
        redaction_status: kura_profiles::RedactionStatus::REDACTED,
    };
    let document_json =
        serde_json::to_string(&selection).map_err(|e| format!("marshal active selection: {e}"))?;
    tx.execute(
        r#"INSERT INTO agent_profile_active_selections (
            selection_id, tenant_id, profile_id, profile_version_id, selection_scope,
            selection_reason, selected_by_principal_id, selected_at, audit_event_id,
            redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(tenant_id, selection_scope) DO UPDATE SET
            selection_id = excluded.selection_id,
            profile_id = excluded.profile_id,
            profile_version_id = excluded.profile_version_id,
            selection_reason = excluded.selection_reason,
            selected_by_principal_id = excluded.selected_by_principal_id,
            selected_at = excluded.selected_at,
            audit_event_id = excluded.audit_event_id,
            redaction_status = excluded.redaction_status,
            document_json = excluded.document_json"#,
        params![
            selection.selection_id,
            selection.tenant_id,
            selection.profile_id,
            selection.profile_version_id,
            selection.selection_scope,
            selection.selection_reason.as_str(),
            selection.selected_by_principal_id,
            now_rfc3339(&selection.selected_at),
            selection.audit_event_id,
            selection.redaction_status.as_str(),
            document_json,
        ],
    )
    .map_err(|e| format!("upsert active selection: {e}"))?;
    Ok(selection)
}

fn replace_overlay_references_tx(
    tx: &Transaction,
    tenant_id: &str,
    profile_id: &str,
    version_id: &str,
    inputs: &[kura_profiles::OverlayReferenceInput],
    now: DateTime<Utc>,
) -> Result<(), String> {
    for input in inputs {
        let mut overlay = kura_profiles::normalize_overlay(input);
        overlay.overlay_reference_id = new_store_id("ovr");
        overlay.tenant_id = tenant_id.to_string();
        overlay.profile_id = profile_id.to_string();
        overlay.profile_version_id = version_id.to_string();
        overlay.created_at = now;
        overlay.updated_at = now;
        let document_json = serde_json::to_string(&overlay)
            .map_err(|e| format!("marshal overlay reference: {e}"))?;
        tx.execute(
            r#"INSERT INTO agent_profile_overlay_references (
                overlay_reference_id, profile_id, profile_version_id, tenant_id, reference_kind,
                scope, reference_uri, safe_display_label, validation_state, failure_reason_code,
                last_validated_at, created_at, updated_at, redaction_status, document_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
            params![
                overlay.overlay_reference_id,
                overlay.profile_id,
                overlay.profile_version_id,
                overlay.tenant_id,
                overlay.reference_kind,
                overlay.scope,
                overlay.reference_uri,
                overlay.safe_display_label,
                overlay.validation_state.as_str(),
                null_string(&overlay.failure_reason_code),
                opt_time_string(&overlay.last_validated_at),
                now_rfc3339(&overlay.created_at),
                now_rfc3339(&overlay.updated_at),
                overlay.redaction_status.as_str(),
                document_json,
            ],
        )
        .map_err(|e| format!("insert overlay reference: {e}"))?;
    }
    Ok(())
}

fn insert_profile_audit_tx(
    tx: &Transaction,
    audit: &kura_profiles::AuditEvent,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(audit).map_err(|e| format!("marshal profile audit event: {e}"))?;
    tx.execute(
        r#"INSERT INTO agent_profile_audit_events (
            audit_event_id, tenant_id, profile_id, profile_version_id, actor_principal_id,
            event_kind, outcome, permission_gate, reason_code, safe_summary, occurred_at,
            redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
        params![
            audit.audit_event_id,
            audit.tenant_id,
            null_string(&audit.profile_id),
            null_string(&audit.profile_version_id),
            null_string(&audit.actor_principal_id),
            audit.event_kind,
            audit.outcome,
            null_string(&audit.permission_gate),
            audit.reason_code,
            null_string(&audit.safe_summary),
            now_rfc3339(&audit.occurred_at),
            audit.redaction_status.as_str(),
            document_json,
        ],
    )
    .map_err(|e| format!("insert profile audit event {}: {e}", audit.audit_event_id))?;
    Ok(())
}
