//! SQLite CRUD for the workspace/capability binding domain. Ported from
//! `daemon/internal/store/binding_store.go` (CreateBindingRule,
//! UpdateBindingRule, RemoveBindingRule, RepairBindingRule, ListBindingRules,
//! GetBindingRule, SetCapabilityVisibility, ListCapabilityVisibility,
//! CapabilityVisibilityForScopes, EffectiveCapabilityVisibility,
//! ResolveChannelBinding, ResolveAccountBinding) and `binding_projection.go`
//! (RecordRuntimeBindingEvidence, ListRuntimeBindingEvidence,
//! LatestRuntimeBindingEvidence). Records persist as `document_json` plus
//! denormalized tenant columns; the partial unique index enforces one active
//! rule per (tenant, scope).

use chrono::Utc;
use rusqlite::{Transaction, params};

use crate::SQLiteStore;
use crate::crud::now_rfc3339;

use super::workspaces::{BindingAuditRow, insert_binding_audit_tx};

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

fn default_reason(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn nullable_time_string(value: &Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    value
        .as_ref()
        .filter(|t| t.timestamp() != 0 || t.timestamp_subsec_nanos() != 0)
        .map(now_rfc3339)
}

fn nullable_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn is_unique_constraint_error(err: &str) -> bool {
    err.to_uppercase().contains("UNIQUE")
}

fn scan_binding_rule(raw: &str) -> Result<kura_bindings::BindingRule, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode binding rule document: {e}"))
}

fn scan_visibility_policy(raw: &str) -> Result<kura_bindings::CapabilityVisibilityPolicy, String> {
    serde_json::from_str(raw)
        .map_err(|e| format!("decode capability visibility policy document: {e}"))
}

fn scan_binding_evidence(raw: &str) -> Result<kura_bindings::RuntimeBindingEvidence, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode runtime binding evidence document: {e}"))
}

/// Go `bindingSelectionSummary`.
fn binding_selection_summary(profile_id: &str, workspace_id: &str) -> String {
    let mut parts = Vec::new();
    if !profile_id.trim().is_empty() {
        parts.push("profile".to_string());
    }
    if !workspace_id.trim().is_empty() {
        parts.push("workspace".to_string());
    }
    if parts.is_empty() {
        "no_selection".to_string()
    } else {
        parts.join("+")
    }
}

impl SQLiteStore {
    /// Go `CreateBindingRule`.
    pub fn create_binding_rule(
        &self,
        actor: &kura_identity::TenantContext,
        req: &kura_bindings::CreateBindingRequest,
    ) -> Result<(kura_bindings::BindingRule, String), String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_bindings::BindingError::ExplicitActorRequired.to_string());
        }
        let profile_selectable =
            self.is_profile_selectable(&actor.tenant_id, &req.selected_profile_id)?;
        let workspace_selectable =
            self.is_workspace_selectable(&actor.tenant_id, &req.selected_workspace_id)?;
        let scope_available =
            self.binding_scope_available(&actor.tenant_id, &req.scope_kind, &req.scope_ref)?;
        let mutation = kura_bindings::BindingMutationInput {
            scope_kind: req.scope_kind.clone(),
            scope_ref: req.scope_ref.clone(),
            selected_profile_id: req.selected_profile_id.clone(),
            selected_workspace_id: req.selected_workspace_id.clone(),
            scope_ref_available: scope_available,
            scope_connector_supported: true,
            profile_selectable: profile_selectable || req.selected_profile_id.trim().is_empty(),
            workspace_selectable: workspace_selectable
                || req.selected_workspace_id.trim().is_empty(),
            ..kura_bindings::BindingMutationInput::default()
        };
        kura_bindings::validate_binding_mutation(&mutation).map_err(|e| e.to_string())?;
        let now = Utc::now();
        let audit_id = new_store_id("audit_binding");
        let rule = kura_bindings::BindingRule {
            binding_id: new_store_id("bnd"),
            tenant_id: actor.tenant_id.clone(),
            scope_kind: req.scope_kind.clone(),
            scope_ref: req.scope_ref.trim().to_string(),
            selected_profile_id: req.selected_profile_id.trim().to_string(),
            selected_workspace_id: req.selected_workspace_id.trim().to_string(),
            status: kura_bindings::BindingStatus::ACTIVE,
            repair_status: kura_bindings::RepairStatus::HEALTHY,
            validation_status: kura_bindings::ValidationStatus::VALID,
            actor_principal_id: actor.principal_id.clone(),
            audit_event_id: audit_id.clone(),
            resulting_selection_summary: binding_selection_summary(
                &req.selected_profile_id,
                &req.selected_workspace_id,
            ),
            redaction_status: kura_bindings::RedactionStatus::REDACTED,
            created_at: now,
            updated_at: now,
            disabled_at: None,
            ..kura_bindings::BindingRule::default()
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin create binding rule: {e}"))?;
        if let Err(err) = insert_binding_rule_tx(&tx, &rule) {
            if is_unique_constraint_error(&err) {
                return Err(
                    kura_bindings::invalid_binding_reason("active_binding_already_exists")
                        .to_string(),
                );
            }
            return Err(err);
        }
        insert_binding_audit_tx(
            &tx,
            &BindingAuditRow {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                binding_id: rule.binding_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "binding.created".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "bindings.manage".to_string(),
                reason_code: default_reason(&req.reason_code, "user_created_binding"),
                safe_summary: "Binding created".to_string(),
                resulting_selection_summary: rule.resulting_selection_summary.clone(),
                occurred_at: now,
                ..BindingAuditRow::default()
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit create binding rule: {e}"))?;
        Ok((rule, audit_id))
    }

    /// Go `UpdateBindingRule`.
    pub fn update_binding_rule(
        &self,
        actor: &kura_identity::TenantContext,
        binding_id: &str,
        req: &kura_bindings::UpdateBindingRequest,
    ) -> Result<(kura_bindings::BindingRule, String), String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_bindings::BindingError::ExplicitActorRequired.to_string());
        }
        let mut rule = self
            .get_binding_rule(&actor.tenant_id, binding_id)?
            .ok_or_else(|| "binding not found".to_string())?;
        let previous_summary = rule.resulting_selection_summary.clone();
        let now = Utc::now();
        let audit_id = new_store_id("audit_binding");
        let mut event_kind = "binding.updated".to_string();
        if req.disable {
            rule.status = kura_bindings::BindingStatus::DISABLED;
            rule.disabled_at = Some(now);
            rule.repair_status = kura_bindings::RepairStatus::DISABLED;
            event_kind = "binding.disabled".to_string();
        } else {
            if !req.selected_profile_id.trim().is_empty() {
                rule.selected_profile_id = req.selected_profile_id.trim().to_string();
            }
            if !req.selected_workspace_id.trim().is_empty() {
                rule.selected_workspace_id = req.selected_workspace_id.trim().to_string();
            }
            let profile_selectable =
                self.is_profile_selectable(&actor.tenant_id, &rule.selected_profile_id)?;
            let workspace_selectable =
                self.is_workspace_selectable(&actor.tenant_id, &rule.selected_workspace_id)?;
            let mutation = kura_bindings::BindingMutationInput {
                scope_kind: rule.scope_kind.clone(),
                scope_ref: rule.scope_ref.clone(),
                selected_profile_id: rule.selected_profile_id.clone(),
                selected_workspace_id: rule.selected_workspace_id.clone(),
                scope_ref_available: true,
                scope_connector_supported: true,
                profile_selectable: profile_selectable || rule.selected_profile_id.is_empty(),
                workspace_selectable: workspace_selectable || rule.selected_workspace_id.is_empty(),
                ..kura_bindings::BindingMutationInput::default()
            };
            kura_bindings::validate_binding_mutation(&mutation).map_err(|e| e.to_string())?;
            rule.resulting_selection_summary =
                binding_selection_summary(&rule.selected_profile_id, &rule.selected_workspace_id);
        }
        rule.previous_selection_summary = previous_summary.clone();
        rule.updated_at = now;
        rule.actor_principal_id = actor.principal_id.clone();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin update binding rule: {e}"))?;
        update_binding_rule_tx(&tx, &rule)?;
        insert_binding_audit_tx(
            &tx,
            &BindingAuditRow {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                binding_id: rule.binding_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind,
                outcome: "succeeded".to_string(),
                permission_gate: "bindings.manage".to_string(),
                reason_code: default_reason(&req.reason_code, "user_updated_binding"),
                safe_summary: "Binding updated".to_string(),
                previous_selection_summary: previous_summary,
                resulting_selection_summary: rule.resulting_selection_summary.clone(),
                occurred_at: now,
                ..BindingAuditRow::default()
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit update binding rule: {e}"))?;
        Ok((rule, audit_id))
    }

    /// Go `RemoveBindingRule`: deletes the rule while preserving audit evidence.
    pub fn remove_binding_rule(
        &self,
        actor: &kura_identity::TenantContext,
        binding_id: &str,
    ) -> Result<String, String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_bindings::BindingError::ExplicitActorRequired.to_string());
        }
        let rule = self
            .get_binding_rule(&actor.tenant_id, binding_id)?
            .ok_or_else(|| "binding not found".to_string())?;
        let now = Utc::now();
        let audit_id = new_store_id("audit_binding");
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin remove binding rule: {e}"))?;
        tx.execute(
            "DELETE FROM binding_rules WHERE tenant_id = ?1 AND binding_id = ?2",
            params![actor.tenant_id, rule.binding_id],
        )
        .map_err(|e| format!("delete binding rule: {e}"))?;
        insert_binding_audit_tx(
            &tx,
            &BindingAuditRow {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                binding_id: rule.binding_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "binding.removed".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "bindings.manage".to_string(),
                reason_code: "user_removed_binding".to_string(),
                safe_summary: "Binding removed".to_string(),
                occurred_at: now,
                ..BindingAuditRow::default()
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit remove binding rule: {e}"))?;
        Ok(audit_id)
    }

    /// Go `RepairBindingRule`: recomputes repair status from current references.
    pub fn repair_binding_rule(
        &self,
        actor: &kura_identity::TenantContext,
        binding_id: &str,
    ) -> Result<(kura_bindings::BindingRule, String), String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_bindings::BindingError::ExplicitActorRequired.to_string());
        }
        let mut rule = self
            .get_binding_rule(&actor.tenant_id, binding_id)?
            .ok_or_else(|| "binding not found".to_string())?;
        let repair = self.repair_status_for(&rule)?;
        let now = Utc::now();
        let audit_id = new_store_id("audit_binding");
        rule.repair_status = repair;
        rule.updated_at = now;
        rule.actor_principal_id = actor.principal_id.clone();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin repair binding rule: {e}"))?;
        update_binding_rule_tx(&tx, &rule)?;
        insert_binding_audit_tx(
            &tx,
            &BindingAuditRow {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                binding_id: rule.binding_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "binding.repaired".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "bindings.manage".to_string(),
                reason_code: "user_repaired_binding".to_string(),
                safe_summary: "Binding repair evaluated".to_string(),
                occurred_at: now,
                ..BindingAuditRow::default()
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit repair binding rule: {e}"))?;
        Ok((rule, audit_id))
    }

    /// Go `ListBindingRules` with freshly computed repair status.
    pub fn list_binding_rules(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_bindings::BindingRule>, String> {
        let limit = if limit <= 0 || limit > 200 { 50 } else { limit };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM binding_rules WHERE tenant_id = ?1
                 ORDER BY updated_at DESC, binding_id DESC LIMIT ?2",
            )
            .map_err(|e| format!("list binding rules {tenant_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let mut rule = scan_binding_rule(&raw)?;
            let repair = self.repair_status_for(&rule)?;
            rule.repair_status = repair;
            items.push(rule);
        }
        Ok(items)
    }

    /// Go `GetBindingRule` with freshly computed repair status.
    pub fn get_binding_rule(
        &self,
        tenant_id: &str,
        binding_id: &str,
    ) -> Result<Option<kura_bindings::BindingRule>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM binding_rules WHERE tenant_id = ?1 AND binding_id = ?2",
            )
            .map_err(|e| format!("get binding rule {binding_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), binding_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let mut rule = scan_binding_rule(&raw)?;
        let repair = self.repair_status_for(&rule)?;
        rule.repair_status = repair;
        Ok(Some(rule))
    }

    /// Go `ResolveChannelBinding`.
    pub fn resolve_channel_binding(
        &self,
        tenant_id: &str,
        scope_ref: &str,
    ) -> Result<Option<kura_bindings::BindingRule>, String> {
        self.resolve_active_binding(tenant_id, &kura_bindings::ScopeKind::CHANNEL, scope_ref)
    }

    /// Go `ResolveAccountBinding`.
    pub fn resolve_account_binding(
        &self,
        tenant_id: &str,
        scope_ref: &str,
    ) -> Result<Option<kura_bindings::BindingRule>, String> {
        self.resolve_active_binding(
            tenant_id,
            &kura_bindings::ScopeKind::INTEGRATION_ACCOUNT,
            scope_ref,
        )
    }

    pub fn resolve_active_binding(
        &self,
        tenant_id: &str,
        kind: &kura_bindings::ScopeKind,
        scope_ref: &str,
    ) -> Result<Option<kura_bindings::BindingRule>, String> {
        let scope_ref = scope_ref.trim();
        if scope_ref.is_empty() {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM binding_rules
                 WHERE tenant_id = ?1 AND scope_kind = ?2 AND scope_ref = ?3 AND status = 'active'",
            )
            .map_err(|e| format!("resolve active binding: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), kind.as_str(), scope_ref])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(scan_binding_rule(&raw)?))
    }

    /// Go `repairStatusFor`.
    pub fn repair_status_for(
        &self,
        rule: &kura_bindings::BindingRule,
    ) -> Result<kura_bindings::RepairStatus, String> {
        let mut profile_selectable = true;
        if !rule.selected_profile_id.trim().is_empty() {
            profile_selectable =
                self.is_profile_selectable(&rule.tenant_id, &rule.selected_profile_id)?;
        }
        let mut workspace_selectable = true;
        if !rule.selected_workspace_id.trim().is_empty() {
            workspace_selectable =
                self.is_workspace_selectable(&rule.tenant_id, &rule.selected_workspace_id)?;
        }
        let scope_available =
            self.binding_scope_available(&rule.tenant_id, &rule.scope_kind, &rule.scope_ref)?;
        Ok(kura_bindings::repair_status_for_references(
            &rule.status,
            profile_selectable,
            workspace_selectable,
            scope_available,
            true,
        ))
    }

    /// Go `bindingScopeAvailable`: best-effort liveness check; integration-account
    /// refs are checked against the integrations table.
    pub fn binding_scope_available(
        &self,
        _tenant_id: &str,
        kind: &kura_bindings::ScopeKind,
        scope_ref: &str,
    ) -> Result<bool, String> {
        let scope_ref = scope_ref.trim();
        if scope_ref.is_empty() {
            return Ok(false);
        }
        if *kind != kura_bindings::ScopeKind::INTEGRATION_ACCOUNT {
            return Ok(true);
        }
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(1) FROM integrations WHERE integration_id = ?1 OR account_key = ?2",
                params![scope_ref, scope_ref],
                |row| row.get(0),
            )
            .map_err(|e| format!("check binding scope availability: {e}"))?;
        Ok(count > 0)
    }

    // --- capability visibility ---

    /// Go `SetCapabilityVisibility`: upsert with a stable policy id.
    pub fn set_capability_visibility(
        &self,
        actor: &kura_identity::TenantContext,
        req: &kura_bindings::SetVisibilityRequest,
    ) -> Result<(kura_bindings::CapabilityVisibilityPolicy, String), String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_bindings::BindingError::ExplicitActorRequired.to_string());
        }
        kura_bindings::validate_capability_visibility_mutation(
            &kura_bindings::CapabilityVisibilityMutationInput {
                scope_kind: req.scope_kind.clone(),
                scope_ref: req.scope_ref.clone(),
                capability_id: req.capability_id.clone(),
                visibility: req.visibility.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
        let now = Utc::now();
        let audit_id = new_store_id("audit_binding");
        let mut policy = kura_bindings::CapabilityVisibilityPolicy {
            policy_id: new_store_id("cvp"),
            tenant_id: actor.tenant_id.clone(),
            scope_kind: req.scope_kind.clone(),
            scope_ref: req.scope_ref.trim().to_string(),
            capability_id: req.capability_id.trim().to_string(),
            visibility: req.visibility.clone(),
            actor_principal_id: actor.principal_id.clone(),
            validation_status: kura_bindings::ValidationStatus::VALID,
            redaction_status: kura_bindings::RedactionStatus::REDACTED,
            created_at: now,
            updated_at: now,
        };
        if let Some(existing_id) = self.capability_visibility_id(
            &actor.tenant_id,
            &req.scope_kind,
            &req.scope_ref,
            &req.capability_id,
        )? {
            policy.policy_id = existing_id;
        }
        let document_json = serde_json::to_string(&policy)
            .map_err(|e| format!("marshal capability visibility policy: {e}"))?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin set capability visibility: {e}"))?;
        tx.execute(
            r#"INSERT INTO capability_visibility_policies (
                policy_id, tenant_id, scope_kind, scope_ref, capability_id, visibility,
                actor_principal_id, validation_status, redaction_status, created_at,
                updated_at, document_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(tenant_id, scope_kind, scope_ref, capability_id) DO UPDATE SET
                visibility = excluded.visibility,
                actor_principal_id = excluded.actor_principal_id,
                validation_status = excluded.validation_status,
                updated_at = excluded.updated_at,
                document_json = excluded.document_json"#,
            params![
                policy.policy_id,
                policy.tenant_id,
                policy.scope_kind.as_str(),
                policy.scope_ref,
                policy.capability_id,
                policy.visibility.as_str(),
                policy.actor_principal_id,
                policy.validation_status.as_str(),
                policy.redaction_status.as_str(),
                now_rfc3339(&policy.created_at),
                now_rfc3339(&policy.updated_at),
                document_json,
            ],
        )
        .map_err(|e| format!("upsert capability visibility policy: {e}"))?;
        insert_binding_audit_tx(
            &tx,
            &BindingAuditRow {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "capability_visibility.changed".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "bindings.manage".to_string(),
                reason_code: default_reason(&req.reason_code, "user_set_capability_visibility"),
                safe_summary: "Capability visibility set".to_string(),
                occurred_at: now,
                ..BindingAuditRow::default()
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit set capability visibility: {e}"))?;
        Ok((policy, audit_id))
    }

    pub fn capability_visibility_id(
        &self,
        tenant_id: &str,
        scope_kind: &kura_bindings::VisibilityScopeKind,
        scope_ref: &str,
        capability_id: &str,
    ) -> Result<Option<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT policy_id FROM capability_visibility_policies
                 WHERE tenant_id = ?1 AND scope_kind = ?2 AND scope_ref = ?3 AND capability_id = ?4",
            )
            .map_err(|e| format!("get capability visibility id: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id,
                scope_kind.as_str(),
                scope_ref.trim(),
                capability_id.trim()
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let id: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(id))
    }

    /// Go `ListCapabilityVisibility`.
    pub fn list_capability_visibility(
        &self,
        tenant_id: &str,
        scope_kind: &kura_bindings::VisibilityScopeKind,
        scope_ref: &str,
    ) -> Result<Vec<kura_bindings::CapabilityVisibilityPolicy>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM capability_visibility_policies
                 WHERE tenant_id = ?1 AND scope_kind = ?2 AND scope_ref = ?3
                 ORDER BY capability_id ASC",
            )
            .map_err(|e| format!("list capability visibility: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id.trim(),
                scope_kind.as_str(),
                scope_ref.trim()
            ])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_visibility_policy(&raw)?);
        }
        Ok(items)
    }

    /// Go `CapabilityVisibilityForScopes`: profile- and workspace-scoped policy
    /// keyed by capability id for runtime resolution.
    pub fn capability_visibility_for_scopes(
        &self,
        tenant_id: &str,
        profile_id: &str,
        workspace_id: &str,
    ) -> Result<
        (
            std::collections::HashMap<String, kura_bindings::Visibility>,
            std::collections::HashMap<String, kura_bindings::Visibility>,
        ),
        String,
    > {
        let mut profile_policies = std::collections::HashMap::new();
        let mut workspace_policies = std::collections::HashMap::new();
        if !profile_id.trim().is_empty() {
            let items = self.list_capability_visibility(
                tenant_id,
                &kura_bindings::VisibilityScopeKind::PROFILE,
                profile_id,
            )?;
            for p in items {
                profile_policies.insert(p.capability_id.clone(), p.visibility);
            }
        }
        if !workspace_id.trim().is_empty() {
            let items = self.list_capability_visibility(
                tenant_id,
                &kura_bindings::VisibilityScopeKind::WORKSPACE,
                workspace_id,
            )?;
            for p in items {
                workspace_policies.insert(p.capability_id.clone(), p.visibility);
            }
        }
        Ok((profile_policies, workspace_policies))
    }

    /// Go `EffectiveCapabilityVisibility`.
    pub fn effective_capability_visibility(
        &self,
        tenant_id: &str,
        profile_id: &str,
        workspace_id: &str,
        capability_id: &str,
        limits: &[kura_bindings::ScopeVisibility],
    ) -> Result<kura_bindings::CapabilityDecision, String> {
        let (profile_policies, workspace_policies) =
            self.capability_visibility_for_scopes(tenant_id, profile_id, workspace_id)?;
        Ok(kura_bindings::resolve_capability_visibility(
            &kura_bindings::VisibilityInput {
                capability_id: capability_id.trim().to_string(),
                limits: limits.to_vec(),
                profile_policy: profile_policies
                    .get(capability_id.trim())
                    .cloned()
                    .unwrap_or_default(),
                workspace_policy: workspace_policies
                    .get(capability_id.trim())
                    .cloned()
                    .unwrap_or_default(),
            },
        ))
    }

    // --- runtime binding evidence (binding_projection.go) ---

    /// Go `RecordRuntimeBindingEvidence`: append-only durable evidence.
    pub fn record_runtime_binding_evidence(
        &self,
        mut evidence: kura_bindings::RuntimeBindingEvidence,
    ) -> Result<kura_bindings::RuntimeBindingEvidence, String> {
        if evidence.projection_id.trim().is_empty() {
            evidence.projection_id = new_store_id("brp");
        }
        if evidence.occurred_at.timestamp() == 0
            && evidence.occurred_at.timestamp_subsec_nanos() == 0
        {
            evidence.occurred_at = Utc::now();
        }
        if evidence.redaction_status.is_empty() {
            evidence.redaction_status = kura_bindings::RedactionStatus::REDACTED;
        }
        let capability_summary = serde_json::to_string(&evidence.capability_visibility)
            .map_err(|e| format!("marshal capability visibility summary: {e}"))?;
        let document_json = serde_json::to_string(&evidence)
            .map_err(|e| format!("marshal runtime binding evidence: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO binding_runtime_projections (
                    projection_id, tenant_id, resource_kind, resource_id, selected_profile_id,
                    selected_profile_version_id, selected_workspace_id, binding_scope,
                    binding_id, classification, selection_reason, capability_visibility_summary,
                    occurred_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
                params![
                    evidence.projection_id,
                    evidence.tenant_id,
                    evidence.resource_kind,
                    evidence.resource_id,
                    nullable_string(&evidence.selected_profile_id),
                    nullable_string(&evidence.selected_profile_version_id),
                    nullable_string(&evidence.selected_workspace_id),
                    evidence.binding_scope.as_str(),
                    nullable_string(&evidence.binding_id),
                    evidence.classification.as_str(),
                    evidence.selection_reason,
                    capability_summary,
                    now_rfc3339(&evidence.occurred_at),
                    evidence.redaction_status.as_str(),
                    document_json,
                ],
            )
            .map_err(|e| format!("record runtime binding evidence: {e}"))?;
        Ok(evidence)
    }

    /// Go `ListRuntimeBindingEvidence`: newest first.
    pub fn list_runtime_binding_evidence(
        &self,
        tenant_id: &str,
        resource_kind: &str,
        resource_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_bindings::RuntimeBindingEvidence>, String> {
        let limit = if limit <= 0 || limit > 100 { 20 } else { limit };
        let mut query = String::from(
            "SELECT document_json FROM binding_runtime_projections WHERE tenant_id = ?1",
        );
        let mut args: Vec<rusqlite::types::Value> = vec![tenant_id.trim().to_string().into()];
        if !resource_kind.trim().is_empty() {
            query.push_str(" AND resource_kind = ?");
            args.push(resource_kind.trim().to_string().into());
        }
        if !resource_id.trim().is_empty() {
            query.push_str(" AND resource_id = ?");
            args.push(resource_id.trim().to_string().into());
        }
        query.push_str(" ORDER BY occurred_at DESC, projection_id DESC LIMIT ?");
        args.push(limit.into());
        let mut stmt = self
            .conn
            .prepare(&query)
            .map_err(|e| format!("list runtime binding evidence: {e}"))?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_binding_evidence(&raw)?);
        }
        Ok(items)
    }

    /// Go `LatestRuntimeBindingEvidence`.
    pub fn latest_runtime_binding_evidence(
        &self,
        tenant_id: &str,
        resource_kind: &str,
        resource_id: &str,
    ) -> Result<Option<kura_bindings::RuntimeBindingEvidence>, String> {
        let items = self.list_runtime_binding_evidence(tenant_id, resource_kind, resource_id, 1)?;
        Ok(items.into_iter().next())
    }
}

fn insert_binding_rule_tx(
    tx: &Transaction,
    rule: &kura_bindings::BindingRule,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(rule).map_err(|e| format!("marshal binding rule: {e}"))?;
    tx.execute(
        r#"INSERT INTO binding_rules (
            binding_id, tenant_id, scope_kind, scope_ref, selected_profile_id,
            selected_profile_version_id, selected_workspace_id, status, repair_status,
            validation_status, actor_principal_id, audit_event_id, previous_selection_summary,
            resulting_selection_summary, redaction_status, created_at, updated_at,
            disabled_at, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"#,
        params![
            rule.binding_id,
            rule.tenant_id,
            rule.scope_kind.as_str(),
            rule.scope_ref,
            nullable_string(&rule.selected_profile_id),
            nullable_string(&rule.selected_profile_version_id),
            nullable_string(&rule.selected_workspace_id),
            rule.status.as_str(),
            rule.repair_status.as_str(),
            rule.validation_status.as_str(),
            rule.actor_principal_id,
            rule.audit_event_id,
            nullable_string(&rule.previous_selection_summary),
            nullable_string(&rule.resulting_selection_summary),
            rule.redaction_status.as_str(),
            now_rfc3339(&rule.created_at),
            now_rfc3339(&rule.updated_at),
            nullable_time_string(&rule.disabled_at),
            document_json,
        ],
    )
    .map_err(|e| format!("insert binding rule {}: {e}", rule.binding_id))?;
    Ok(())
}

fn update_binding_rule_tx(
    tx: &Transaction,
    rule: &kura_bindings::BindingRule,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(rule).map_err(|e| format!("marshal binding rule: {e}"))?;
    tx.execute(
        r#"UPDATE binding_rules SET
            scope_kind = ?1, scope_ref = ?2, selected_profile_id = ?3,
            selected_profile_version_id = ?4, selected_workspace_id = ?5, status = ?6,
            repair_status = ?7, validation_status = ?8, actor_principal_id = ?9,
            resulting_selection_summary = ?10, redaction_status = ?11, updated_at = ?12,
            disabled_at = ?13, document_json = ?14
        WHERE tenant_id = ?15 AND binding_id = ?16"#,
        params![
            rule.scope_kind.as_str(),
            rule.scope_ref,
            nullable_string(&rule.selected_profile_id),
            nullable_string(&rule.selected_profile_version_id),
            nullable_string(&rule.selected_workspace_id),
            rule.status.as_str(),
            rule.repair_status.as_str(),
            rule.validation_status.as_str(),
            rule.actor_principal_id,
            nullable_string(&rule.resulting_selection_summary),
            rule.redaction_status.as_str(),
            now_rfc3339(&rule.updated_at),
            nullable_time_string(&rule.disabled_at),
            document_json,
            rule.tenant_id,
            rule.binding_id,
        ],
    )
    .map_err(|e| format!("update binding rule {}: {e}", rule.binding_id))?;
    Ok(())
}
