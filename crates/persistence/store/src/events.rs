//! SQLite CRUD for the runtime-event ledger (append + list). Ported from
//! `daemon/internal/store/store.go` (AppendEvent, ListEvents). Tenant auto-binding via the
//! personal-tenant cache is deferred to the tenancy package; the legacy path binds only the
//! caller-provided tenant id.

use rusqlite::{Row, params, params_from_iter, types::Value};

use crate::SQLiteStore;
use crate::crud::{decode_map, now_rfc3339, null_string, parse_rfc3339};

fn scan_event(row: &Row) -> Result<kura_events::Event, String> {
    let sequence: i64 = row.get(0).map_err(|e| e.to_string())?;
    let event_id: String = row.get(1).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(2).map_err(|e| e.to_string())?;
    let category: String = row.get(3).map_err(|e| e.to_string())?;
    let name: String = row.get(4).map_err(|e| e.to_string())?;
    let occurred_at: String = row.get(5).map_err(|e| e.to_string())?;
    let session_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let run_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let workflow_step_id: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let schedule_id: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let schedule_attempt_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let step_id: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let connector_id: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let capability_id: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let resource_kind: String = row.get(15).map_err(|e| e.to_string())?;
    let resource_id: String = row.get(16).map_err(|e| e.to_string())?;
    let payload_json: Option<String> = row.get(17).map_err(|e| e.to_string())?;

    Ok(kura_events::Event {
        event_id,
        sequence,
        environment_scope,
        category,
        name,
        occurred_at: parse_rfc3339(&occurred_at)?,
        scope: kura_events::Scope {
            session_id: session_id.unwrap_or_default(),
            run_id: run_id.unwrap_or_default(),
            workflow_id: workflow_id.unwrap_or_default(),
            workflow_step_id: workflow_step_id.unwrap_or_default(),
            schedule_id: schedule_id.unwrap_or_default(),
            schedule_attempt_id: schedule_attempt_id.unwrap_or_default(),
            step_id: step_id.unwrap_or_default(),
            connector_id: connector_id.unwrap_or_default(),
            capability_id: capability_id.unwrap_or_default(),
            ..kura_events::Scope::default()
        },
        resource: kura_events::Resource {
            kind: resource_kind,
            id: resource_id,
        },
        payload: decode_map(&payload_json)?,
        ..kura_events::Event::default()
    })
}

impl SQLiteStore {
    pub fn append_event(&self, event: &kura_events::Event) -> Result<kura_events::Event, String> {
        let payload_json = serde_json::to_string(&event.payload)
            .map_err(|e| format!("marshal event payload: {e}"))?;

        // Go AppendEvent: auto-bind tenant_id for non-global categories so
        // legacy callers satisfy the T077 CHECK constraint — event.TenantID
        // first, then the cached default personal tenant.
        let mut event = event.clone();
        // An empty event_id would collide on the ON CONFLICT(event_id)
        // DO NOTHING guard: the first empty-id event inserts and every
        // later one is silently dropped. Callers that publish through the
        // bus get ids assigned there, but direct-append paths (system.*,
        // memory.*, ...) reach here first — assign here so the ledger
        // never loses events.
        if event.event_id.trim().is_empty() {
            let hex = uuid::Uuid::new_v4().simple().to_string();
            event.event_id = format!("evt_{}", &hex[..16]);
        }
        if event.occurred_at == chrono::DateTime::<chrono::Utc>::MIN_UTC {
            event.occurred_at = chrono::Utc::now();
        }
        if event.tenant_id.trim().is_empty() && !kura_events::is_global_category(&event.category) {
            if let Some(tenant_id) = self.resolve_default_tenant_binding() {
                event.tenant_id = tenant_id;
            }
        }
        let event = &event;

        self.conn
            .execute(
                r#"INSERT INTO events (
                    event_id, environment_scope, category, name, occurred_at, session_id, run_id,
                    workflow_id, workflow_step_id, schedule_id, schedule_attempt_id, step_id,
                    connector_id, capability_id, resource_kind, resource_id, payload_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(event_id) DO NOTHING"#,
                params![
                    event.event_id,
                    event.environment_scope,
                    event.category,
                    event.name,
                    now_rfc3339(&event.occurred_at),
                    null_string(&event.scope.session_id),
                    null_string(&event.scope.run_id),
                    null_string(&event.scope.workflow_id),
                    null_string(&event.scope.workflow_step_id),
                    null_string(&event.scope.schedule_id),
                    null_string(&event.scope.schedule_attempt_id),
                    null_string(&event.scope.step_id),
                    null_string(&event.scope.connector_id),
                    null_string(&event.scope.capability_id),
                    event.resource.kind,
                    event.resource.id,
                    payload_json,
                    null_string(&event.tenant_id),
                ],
            )
            .map_err(|e| format!("append event {}: {e}", event.event_id))?;

        let sequence: i64 = self
            .conn
            .query_row(
                "SELECT rowid FROM events WHERE event_id = ?1",
                params![event.event_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("load event sequence {}: {e}", event.event_id))?;

        let mut out = event.clone();
        out.sequence = sequence;
        Ok(out)
    }

    pub fn list_events(
        &self,
        filter: &kura_events::Filter,
    ) -> Result<Vec<kura_events::Event>, String> {
        let mut sql = String::from(
            r#"SELECT rowid, event_id, environment_scope, category, name, occurred_at, session_id,
                run_id, workflow_id, workflow_step_id, schedule_id, schedule_attempt_id, step_id,
                connector_id, capability_id, resource_kind, resource_id, payload_json
            FROM events
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(filter.environment_scope.trim().to_string()));
        }
        if !filter.category.trim().is_empty() {
            sql.push_str(" AND category = ?");
            args.push(Value::Text(filter.category.trim().to_string()));
        }
        if !filter.run_id.trim().is_empty() {
            sql.push_str(" AND run_id = ?");
            args.push(Value::Text(filter.run_id.trim().to_string()));
        }
        if !filter.session_id.trim().is_empty() {
            sql.push_str(" AND session_id = ?");
            args.push(Value::Text(filter.session_id.trim().to_string()));
        }
        if !filter.schedule_id.trim().is_empty() {
            sql.push_str(" AND schedule_id = ?");
            args.push(Value::Text(filter.schedule_id.trim().to_string()));
        }
        if !filter.schedule_attempt_id.trim().is_empty() {
            sql.push_str(" AND schedule_attempt_id = ?");
            args.push(Value::Text(filter.schedule_attempt_id.trim().to_string()));
        }
        if !filter.resource_kind.trim().is_empty() {
            sql.push_str(" AND resource_kind = ?");
            args.push(Value::Text(filter.resource_kind.trim().to_string()));
        }
        if filter.cursor > 0 {
            sql.push_str(" AND rowid > ?");
            args.push(Value::Integer(filter.cursor));
        }
        sql.push_str(" ORDER BY rowid ASC");

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list events: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_event(row)?);
        }
        Ok(items)
    }
}
