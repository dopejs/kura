//! Calendar operation manager (port of `manager.go`): the single operation ledger,
//! account selection, backend dispatch, artifact capture, and diagnostic failure projection.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_integrations::{BackendKind, ReadinessStatus, Resource};
use uuid::Uuid;

use crate::{
    AccountProjection, Artifact, ArtifactKind, AvailabilityQuery, Backend, CalendarError,
    CancelEventInput, CreateEventInput, Event, EventLifecycleState, GetEventInput, ListEventsInput,
    Operation, OperationClass, OperationFilter, OperationStatus, RecurrenceScope, Selection,
    SourceLinkage, UpdateAttendeesInput, UpdateEventInput, build_attendee_outcome,
};

#[derive(Default)]
struct ManagerInner {
    backends: HashMap<BackendKind, Arc<dyn Backend>>,
    accounts: HashMap<String, AccountProjection>,
    operations: HashMap<String, Operation>,
    op_order: Vec<String>,
    artifacts: HashMap<String, Artifact>,
}

pub struct Manager {
    env: String,
    inner: parking_lot::RwLock<ManagerInner>,
}

impl Manager {
    pub fn new(environment_scope: &str) -> Self {
        let mut inner = ManagerInner::default();
        inner
            .backends
            .insert(BackendKind::FakeLocal, Arc::new(super::FakeBackend::new()));
        Manager {
            env: environment_scope.trim().to_string(),
            inner: parking_lot::RwLock::new(inner),
        }
    }

    /// Install a backend under a backend kind (e.g. `BackendKind::AdapterRpc`). The fake
    /// backend remains registered for `fake_local` bindings.
    pub fn register_backend(&self, kind: BackendKind, backend: Arc<dyn Backend>) {
        self.inner.write().backends.insert(kind, backend);
    }

    pub fn restore(
        &self,
        accounts: Vec<AccountProjection>,
        operations: Vec<Operation>,
        artifacts: Vec<Artifact>,
    ) {
        let mut inner = self.inner.write();
        inner.accounts = accounts
            .into_iter()
            .map(|a| (a.integration_id.clone(), a))
            .collect();
        inner.operations = operations
            .iter()
            .map(|o| (o.operation_id.clone(), o.clone()))
            .collect();
        inner.op_order = operations.iter().map(|o| o.operation_id.clone()).collect();
        inner.artifacts = artifacts
            .iter()
            .map(|a| (a.artifact_id.clone(), a.clone()))
            .collect();

        let mut events_by_integration: HashMap<String, Vec<Event>> = HashMap::new();
        for item in &artifacts {
            if item.kind != ArtifactKind::EventSnapshot
                || item.external_event_id.is_empty()
                || item.starts_at.is_none()
                || item.ends_at.is_none()
            {
                continue;
            }
            let event = Event {
                external_event_id: item.external_event_id.clone(),
                integration_id: item.integration_id.clone(),
                calendar_ref: item.calendar_ref.clone(),
                title: item.title.clone(),
                starts_at: item.starts_at.unwrap(),
                ends_at: item.ends_at.unwrap(),
                timezone: item.timezone.clone(),
                all_day: item.all_day,
                recurrence_summary: item.recurrence_summary.clone(),
                mutation_eligible_in_phase: item.mutation_eligible_in_phase,
                lifecycle_state: parse_event_lifecycle_state(&item.lifecycle_state),
                created_at: item.created_at,
                updated_at: item.created_at,
                ..Event::default()
            };
            events_by_integration
                .entry(item.integration_id.clone())
                .or_default()
                .push(event);
        }
        if let Some(backend) = inner.backends.get(&BackendKind::FakeLocal).cloned() {
            for (integration_id, events) in events_by_integration {
                backend.restore_integration_state(&integration_id, events);
            }
        }
    }

    pub fn list_accounts(
        &self,
        resources: &[Resource],
        selection: &Selection,
    ) -> Result<Vec<AccountProjection>, CalendarError> {
        if !selection.integration_id.trim().is_empty() {
            let (account, _, _, _) = self.select_account(resources, selection)?;
            return Ok(vec![account]);
        }
        let mut items = Vec::new();
        for resource in resources {
            if resource.domain_kind != "calendar" || resource.environment_scope.trim() != self.env {
                continue;
            }
            let sel = Selection {
                integration_id: resource.integration_id.clone(),
                ..Selection::default()
            };
            let (account, _, _, _) = self.select_account(resources, &sel)?;
            items.push(account);
        }
        Ok(items)
    }

    pub fn list_events(
        &self,
        resources: &[Resource],
        input: &ListEventsInput,
    ) -> Result<(AccountProjection, Vec<Event>, Operation, Vec<Artifact>), CalendarError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::ListEvents,
            &selection_mode,
            &account.primary_timezone,
            &summarize_window(input.starts_at, input.ends_at),
            &input.source,
        );
        let items = match backend.list_events(&resource, &account, input) {
            Ok(items) => items,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        let artifacts: Vec<Artifact> = items
            .iter()
            .map(|item| event_artifact(&operation, item))
            .collect();
        let operation = self.complete_operation(operation, artifacts.clone(), None, "");
        Ok((account, items, operation, artifacts))
    }

    pub fn get_event(
        &self,
        resources: &[Resource],
        input: &GetEventInput,
    ) -> Result<(AccountProjection, Event, Operation, Vec<Artifact>), CalendarError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::GetEvent,
            &selection_mode,
            &account.primary_timezone,
            &input.external_event_id,
            &input.source,
        );
        let item = match backend.get_event(&resource, &account, &input.external_event_id) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        let artifact = event_artifact(&operation, &item);
        let operation = self.complete_operation(
            operation,
            vec![artifact.clone()],
            None,
            &item.external_event_id,
        );
        Ok((account, item, operation, vec![artifact]))
    }

    pub fn busy_free(
        &self,
        resources: &[Resource],
        input: &crate::BusyFreeInput,
    ) -> Result<
        (
            AccountProjection,
            AvailabilityQuery,
            Operation,
            Vec<Artifact>,
        ),
        CalendarError,
    > {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let timezone = crate::normalize_timezone(&input.timezone, &account.primary_timezone);
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::BusyFree,
            &selection_mode,
            &timezone,
            &summarize_window(Some(input.window_start), Some(input.window_end)),
            &input.source,
        );
        let mut query = match backend.busy_free(&resource, &account, input) {
            Ok(query) => query,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        query.query_id = operation.operation_id.clone();
        query.operation_id = operation.operation_id.clone();
        query.created_at = operation.updated_at;
        let artifact = availability_artifact(&operation, &query);
        let operation =
            self.complete_operation(operation, vec![artifact.clone()], Some(&query), "");
        Ok((account, query, operation, vec![artifact]))
    }

    pub fn create_event(
        &self,
        resources: &[Resource],
        input: &CreateEventInput,
    ) -> Result<(AccountProjection, Event, Operation, Vec<Artifact>), CalendarError> {
        if !input.all_day && input.ends_at <= input.starts_at {
            return Err(CalendarError::CalendarInvalidTimeRange);
        }
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let timezone = crate::normalize_timezone(&input.timezone, &account.primary_timezone);
        let mut operation = self.new_operation(
            &account,
            &resource,
            OperationClass::CreateEvent,
            &selection_mode,
            &timezone,
            &input.title,
            &input.source,
        );
        let item = match backend.create_event(&resource, &account, input) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        operation.attendee_outcome =
            build_attendee_outcome(input.notify_attendees, &item.attendee_details);
        let artifact = event_artifact(&operation, &item);
        let operation = self.complete_operation(
            operation,
            vec![artifact.clone()],
            None,
            &item.external_event_id,
        );
        Ok((account, item, operation, vec![artifact]))
    }

    pub fn update_event(
        &self,
        resources: &[Resource],
        input: &UpdateEventInput,
    ) -> Result<(AccountProjection, Event, Operation, Vec<Artifact>), CalendarError> {
        validate_recurrence_scope(input.recurrence_scope)?;
        if !input.all_day && input.ends_at <= input.starts_at {
            return Err(CalendarError::CalendarInvalidTimeRange);
        }
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let timezone = crate::normalize_timezone(&input.timezone, &account.primary_timezone);
        let mut operation = self.new_operation(
            &account,
            &resource,
            OperationClass::UpdateEvent,
            &selection_mode,
            &timezone,
            &input.external_event_id,
            &input.source,
        );
        let item = match backend.update_event(&resource, &account, input) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        operation.attendee_outcome =
            build_attendee_outcome(input.notify_attendees, &item.attendee_details);
        operation.recurrence_scope = input.recurrence_scope.as_str().to_string();
        operation.original_external_event_id = input.external_event_id.trim().to_string();
        operation.resulting_series_id = item.series_id.clone();
        let artifact = event_artifact(&operation, &item);
        let operation = self.complete_operation(
            operation,
            vec![artifact.clone()],
            None,
            &item.external_event_id,
        );
        Ok((account, item, operation, vec![artifact]))
    }

    pub fn update_attendees(
        &self,
        resources: &[Resource],
        input: &UpdateAttendeesInput,
    ) -> Result<(AccountProjection, Event, Operation, Vec<Artifact>), CalendarError> {
        if input.add_attendees.is_empty() && input.remove_attendees.is_empty() {
            return Err(CalendarError::CalendarAttendeeRequestEmpty);
        }
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let mut operation = self.new_operation(
            &account,
            &resource,
            OperationClass::UpdateAttendees,
            &selection_mode,
            &account.primary_timezone,
            &input.external_event_id,
            &input.source,
        );
        let item = match backend.update_attendees(&resource, &account, input) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        operation.attendee_outcome = build_attendee_outcome(input.notify, &item.attendee_details);
        let artifact = event_artifact(&operation, &item);
        let operation = self.complete_operation(
            operation,
            vec![artifact.clone()],
            None,
            &item.external_event_id,
        );
        Ok((account, item, operation, vec![artifact]))
    }

    pub fn cancel_event(
        &self,
        resources: &[Resource],
        input: &CancelEventInput,
    ) -> Result<(AccountProjection, Event, Operation, Vec<Artifact>), CalendarError> {
        validate_recurrence_scope(input.recurrence_scope)?;
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let mut operation = self.new_operation(
            &account,
            &resource,
            OperationClass::CancelEvent,
            &selection_mode,
            &account.primary_timezone,
            &input.external_event_id,
            &input.source,
        );
        let item = match backend.cancel_event(&resource, &account, input) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        operation.recurrence_scope = input.recurrence_scope.as_str().to_string();
        operation.original_external_event_id = input.external_event_id.trim().to_string();
        operation.resulting_series_id = item.series_id.clone();
        let artifact = event_artifact(&operation, &item);
        let operation = self.complete_operation(
            operation,
            vec![artifact.clone()],
            None,
            &item.external_event_id,
        );
        Ok((account, item, operation, vec![artifact]))
    }

    pub fn list_operations(&self, filter: &OperationFilter) -> Vec<Operation> {
        let inner = self.inner.read();
        let mut items = Vec::new();
        for id in &inner.op_order {
            let item = &inner.operations[id];
            if !filter.integration_id.is_empty() && item.integration_id != filter.integration_id {
                continue;
            }
            if !filter.run_id.is_empty() && item.run_id != filter.run_id {
                continue;
            }
            if !filter.workflow_id.is_empty() && item.workflow_id != filter.workflow_id {
                continue;
            }
            if !filter.schedule_id.is_empty() && item.schedule_id != filter.schedule_id {
                continue;
            }
            if !filter.delivery_id.is_empty() && item.delivery_id != filter.delivery_id {
                continue;
            }
            if filter.operation_class != OperationClass::default()
                && item.operation_class != filter.operation_class
            {
                continue;
            }
            if filter.status != OperationStatus::default() && item.status != filter.status {
                continue;
            }
            if !filter.external_event_id.is_empty()
                && item.external_event_id != filter.external_event_id
            {
                continue;
            }
            items.push(item.clone());
        }
        items
    }

    pub fn get_operation(&self, operation_id: &str) -> Option<Operation> {
        self.inner
            .read()
            .operations
            .get(operation_id.trim())
            .cloned()
    }

    pub fn get_account(&self, integration_id: &str) -> Option<AccountProjection> {
        self.inner
            .read()
            .accounts
            .get(integration_id.trim())
            .cloned()
    }

    pub fn store_operation(&self, item: Operation) {
        let mut inner = self.inner.write();
        if !inner.operations.contains_key(&item.operation_id) {
            inner.op_order.push(item.operation_id.clone());
        }
        inner.operations.insert(item.operation_id.clone(), item);
    }

    pub fn list_artifacts(&self, operation_id: &str) -> Vec<Artifact> {
        let inner = self.inner.read();
        inner
            .artifacts
            .values()
            .filter(|item| operation_id.is_empty() || item.operation_id == operation_id)
            .cloned()
            .collect()
    }

    fn select_account(
        &self,
        resources: &[Resource],
        selection: &Selection,
    ) -> Result<(AccountProjection, Resource, Arc<dyn Backend>, String), CalendarError> {
        let explicit = selection.integration_id.trim();
        if !explicit.is_empty() {
            for resource in resources {
                if resource.integration_id != explicit {
                    continue;
                }
                return self.project_resource(resource, "explicit");
            }
            return Err(CalendarError::CalendarIntegrationNotFound);
        }
        let selected = resources.iter().find(|r| {
            r.domain_kind == "calendar"
                && r.environment_scope.trim() == self.env
                && r.canonical_default
        });
        match selected {
            Some(resource) => self.project_resource(resource, "canonical_default"),
            None => Err(CalendarError::CalendarSelectionInvalid),
        }
    }

    fn project_resource(
        &self,
        resource: &Resource,
        selection_mode: &str,
    ) -> Result<(AccountProjection, Resource, Arc<dyn Backend>, String), CalendarError> {
        if resource.domain_kind != "calendar" || resource.environment_scope.trim() != self.env {
            return Err(CalendarError::CalendarSelectionInvalid);
        }
        if resource.readiness_status != ReadinessStatus::Healthy
            && resource.readiness_status != ReadinessStatus::Degraded
        {
            return Err(CalendarError::CalendarUnavailable);
        }
        let backend = self
            .inner
            .read()
            .backends
            .get(&resource.backend_binding.backend_kind)
            .cloned();
        let Some(backend) = backend else {
            return Err(CalendarError::CalendarBackendNotConfigured);
        };
        let mut account = backend.project_account(resource)?;
        account.selection_mode = selection_mode.to_string();
        self.inner
            .write()
            .accounts
            .insert(account.integration_id.clone(), account.clone());
        Ok((
            account,
            resource.clone(),
            backend,
            selection_mode.to_string(),
        ))
    }

    fn new_operation(
        &self,
        account: &AccountProjection,
        resource: &Resource,
        class: OperationClass,
        selection_mode: &str,
        timezone: &str,
        request_summary: &str,
        source: &SourceLinkage,
    ) -> Operation {
        let now = Utc::now();
        let mut operation_id = source.operation_id.trim().to_string();
        if operation_id.is_empty() {
            operation_id = new_operation_id();
        }
        let item = Operation {
            operation_id: operation_id.clone(),
            operation_class: class,
            status: OperationStatus::Requested,
            integration_id: resource.integration_id.clone(),
            calendar_account_id: account.calendar_account_id.clone(),
            environment_scope: resource.environment_scope.clone(),
            calendar_ref: account.primary_calendar_ref.clone(),
            selection_mode: selection_mode.to_string(),
            timezone_used: timezone.to_string(),
            request_summary: request_summary.to_string(),
            run_id: source.run_id.trim().to_string(),
            step_id: source.step_id.trim().to_string(),
            tool_call_id: source.tool_call_id.trim().to_string(),
            workflow_id: source.workflow_id.trim().to_string(),
            workflow_step_id: source.workflow_step_id.trim().to_string(),
            schedule_id: source.schedule_id.trim().to_string(),
            schedule_attempt_id: source.schedule_attempt_id.trim().to_string(),
            delivery_id: source.delivery_id.trim().to_string(),
            created_at: now,
            updated_at: now,
            ..Operation::default()
        };
        let mut inner = self.inner.write();
        inner.operations.insert(operation_id.clone(), item.clone());
        inner.op_order.push(operation_id);
        item
    }

    fn complete_operation(
        &self,
        mut operation: Operation,
        artifacts: Vec<Artifact>,
        query: Option<&AvailabilityQuery>,
        external_event_id: &str,
    ) -> Operation {
        let now = Utc::now();
        operation.status = OperationStatus::Completed;
        operation.external_event_id = external_event_id.to_string();
        operation.completed_at = Some(now);
        operation.updated_at = now;
        operation.artifact_ids = Vec::with_capacity(artifacts.len());
        let mut inner = self.inner.write();
        for item in &artifacts {
            operation.artifact_ids.push(item.artifact_id.clone());
            inner
                .artifacts
                .insert(item.artifact_id.clone(), item.clone());
        }
        if let Some(query) = query {
            operation.availability_query_id = query.query_id.clone();
        }
        inner
            .operations
            .insert(operation.operation_id.clone(), operation.clone());
        operation
    }

    fn fail_operation(
        &self,
        mut operation: Operation,
        class: &str,
        provider_kind: &str,
        reason: &str,
    ) -> Operation {
        let now = Utc::now();
        operation.status = OperationStatus::Failed;
        operation.failure_class = class.to_string();
        operation.failure_reason = reason.to_string();
        let diagnostic = kura_integrations::diagnostic_failure_for_operation_failure(
            "calendar",
            provider_kind,
            &operation.integration_id,
            operation.operation_class.as_str(),
            class,
            reason,
            calendar_operation_side_effecting(operation.operation_class),
            now,
        );
        operation.diagnostic_failure = Some(diagnostic);
        operation.completed_at = Some(now);
        operation.updated_at = now;
        self.inner
            .write()
            .operations
            .insert(operation.operation_id.clone(), operation.clone());
        operation
    }
}

fn validate_recurrence_scope(scope: RecurrenceScope) -> Result<(), CalendarError> {
    if scope != RecurrenceScope::Unspecified && !scope.valid() {
        return Err(CalendarError::CalendarRecurrenceScopeInvalid);
    }
    Ok(())
}

#[must_use]
fn calendar_operation_side_effecting(class: OperationClass) -> bool {
    matches!(
        class,
        OperationClass::CreateEvent
            | OperationClass::UpdateEvent
            | OperationClass::CancelEvent
            | OperationClass::UpdateAttendees
    )
}

#[must_use]
fn summarize_window(starts_at: Option<DateTime<Utc>>, ends_at: Option<DateTime<Utc>>) -> String {
    match (starts_at, ends_at) {
        (None, None) => String::new(),
        (Some(s), Some(e)) => format!("{}/{}", s.to_rfc3339(), e.to_rfc3339()),
        (Some(s), None) => s.to_rfc3339(),
        (None, Some(e)) => e.to_rfc3339(),
    }
}

#[must_use]
pub fn new_operation_id() -> String {
    new_id("calendar_op")
}

#[must_use]
fn new_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

#[must_use]
pub fn event_artifact(operation: &Operation, item: &Event) -> Artifact {
    Artifact {
        artifact_id: new_id("calendar_artifact"),
        operation_id: operation.operation_id.clone(),
        kind: ArtifactKind::EventSnapshot,
        integration_id: operation.integration_id.clone(),
        environment_scope: operation.environment_scope.clone(),
        external_event_id: item.external_event_id.clone(),
        calendar_ref: item.calendar_ref.clone(),
        title: item.title.clone(),
        starts_at: Some(item.starts_at),
        ends_at: Some(item.ends_at),
        timezone: item.timezone.clone(),
        all_day: item.all_day,
        recurrence_summary: item.recurrence_summary.clone(),
        mutation_eligible_in_phase: item.mutation_eligible_in_phase,
        lifecycle_state: item.lifecycle_state.as_str().to_string(),
        created_at: Utc::now(),
        ..Artifact::default()
    }
}

#[must_use]
pub fn availability_artifact(operation: &Operation, query: &AvailabilityQuery) -> Artifact {
    Artifact {
        artifact_id: new_id("calendar_artifact"),
        operation_id: operation.operation_id.clone(),
        kind: ArtifactKind::AvailabilityQuery,
        integration_id: operation.integration_id.clone(),
        environment_scope: operation.environment_scope.clone(),
        calendar_ref: operation.calendar_ref.clone(),
        timezone: query.timezone.clone(),
        availability_query: Some(query.clone()),
        created_at: Utc::now(),
        ..Artifact::default()
    }
}

#[must_use]
fn failure_class_and_provider(default_class: &str, err: &CalendarError) -> (String, String) {
    if let CalendarError::Adapter(af) = err {
        return (af.class.clone(), af.provider_kind.clone());
    }
    (default_class.to_string(), String::new())
}

#[must_use]
fn parse_event_lifecycle_state(value: &str) -> EventLifecycleState {
    match value {
        "cancelled" => EventLifecycleState::Cancelled,
        "stale_snapshot" => EventLifecycleState::StaleSnapshot,
        "unavailable" => EventLifecycleState::Unavailable,
        _ => EventLifecycleState::Active,
    }
}
