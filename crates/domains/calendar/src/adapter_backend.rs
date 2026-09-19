//! Calendar adapter backend (port of `adapter_backend.go`): dispatches each operation to an
//! out-of-process integration adapter over the capability RPC contract (Roadmap 59). It does
//! provider request/response mapping only; the Manager retains the operation ledger.

use std::time::Duration;

use kura_integrations::Resource;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{
    AccountProjection, AdapterFailure, AvailabilityQuery, Backend, BusyFreeInput, CalendarError,
    CancelEventInput, CreateEventInput, Event, ListEventsInput, UpdateAttendeesInput,
    UpdateEventInput,
};

const DOMAIN_CALENDAR: &str = "calendar";

pub struct AdapterBackend {
    client: kura_adapterrpc::Client,
    deadline: Duration,
    provider_kind: String,
}

impl AdapterBackend {
    /// Build a calendar adapter backend over the given RPC client. A zero deadline uses the
    /// client default.
    pub fn new(client: kura_adapterrpc::Client, deadline: Duration) -> Self {
        AdapterBackend {
            client,
            deadline,
            provider_kind: String::new(),
        }
    }

    /// Record the diagnostics provider kind this adapter serves (e.g. "feishu_lark").
    pub fn with_provider_kind(mut self, kind: &str) -> Self {
        self.provider_kind = kind.to_string();
        self
    }

    #[must_use]
    pub fn provider_kind(&self) -> &str {
        &self.provider_kind
    }

    fn dispatch<R, P, O>(
        &self,
        operation: &str,
        resource: Option<&R>,
        payload: Option<&P>,
        out: Option<&mut O>,
    ) -> Result<(), CalendarError>
    where
        R: Serialize + ?Sized,
        P: Serialize + ?Sized,
        O: DeserializeOwned,
    {
        let result = if self.deadline.is_zero() {
            self.client
                .dispatch(DOMAIN_CALENDAR, operation, resource, payload, out)
        } else {
            self.client.dispatch_with_timeout(
                self.deadline,
                DOMAIN_CALENDAR,
                operation,
                resource,
                payload,
                out,
            )
        };
        self.map_err(result)
    }

    fn map_err(&self, err: Result<(), kura_adapterrpc::Error>) -> Result<(), CalendarError> {
        match err {
            Ok(()) => Ok(()),
            Err(e) => {
                if kura_adapterrpc::is_ambiguous(&e) {
                    return Err(CalendarError::Adapter(AdapterFailure {
                        class: "ambiguous_commit".to_string(),
                        provider_kind: self.provider_kind.clone(),
                        detail: e.to_string(),
                        ambiguous: true,
                        unavailable: false,
                    }));
                }
                if let kura_adapterrpc::Error::Adapter(ae) = &e {
                    return Err(CalendarError::Adapter(AdapterFailure {
                        class: stable_failure_class(ae),
                        provider_kind: self.provider_kind.clone(),
                        detail: ae.detail.clone(),
                        ambiguous: false,
                        unavailable: ae.kind == kura_adapterrpc::FailureKind::Unavailable,
                    }));
                }
                Err(CalendarError::AdapterTransport(e.to_string()))
            }
        }
    }
}

fn stable_failure_class(ae: &kura_adapterrpc::AdapterError) -> String {
    if !ae.detail.is_empty() {
        return ae.detail.clone();
    }
    match ae.kind {
        kura_adapterrpc::FailureKind::Auth => "user_access_token_invalid".to_string(),
        kura_adapterrpc::FailureKind::Scope => "scope_not_granted".to_string(),
        kura_adapterrpc::FailureKind::RateLimited => "rate_limited".to_string(),
        kura_adapterrpc::FailureKind::Unavailable => "service_unavailable".to_string(),
        kura_adapterrpc::FailureKind::Malformed => "malformed_provider_response".to_string(),
        _ => "provider_internal_error".to_string(),
    }
}

impl Backend for AdapterBackend {
    fn project_account(&self, resource: &Resource) -> Result<AccountProjection, CalendarError> {
        let mut out = AccountProjection::default();
        self.dispatch::<Resource, serde_json::Value, AccountProjection>(
            "ProjectAccount",
            Some(resource),
            None,
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn list_events(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &ListEventsInput,
    ) -> Result<Vec<Event>, CalendarError> {
        let mut out: Vec<Event> = Vec::new();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, Vec<Event>>(
            "ListEvents",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn get_event(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        event_id: &str,
    ) -> Result<Event, CalendarError> {
        let mut out = Event::default();
        let payload = serde_json::json!({ "account": account, "eventId": event_id });
        self.dispatch::<Resource, serde_json::Value, Event>(
            "GetEvent",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn busy_free(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &BusyFreeInput,
    ) -> Result<AvailabilityQuery, CalendarError> {
        let mut out = AvailabilityQuery::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, AvailabilityQuery>(
            "BusyFree",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn create_event(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &CreateEventInput,
    ) -> Result<Event, CalendarError> {
        let mut out = Event::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, Event>(
            "CreateEvent",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn update_event(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &UpdateEventInput,
    ) -> Result<Event, CalendarError> {
        let mut out = Event::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, Event>(
            "UpdateEvent",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn cancel_event(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &CancelEventInput,
    ) -> Result<Event, CalendarError> {
        let mut out = Event::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, Event>(
            "CancelEvent",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn update_attendees(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &UpdateAttendeesInput,
    ) -> Result<Event, CalendarError> {
        let mut out = Event::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, Event>(
            "UpdateAttendees",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn restore_integration_state(&self, _integration_id: &str, _events: Vec<Event>) {
        // The adapter is stateless; restore is daemon-owned.
    }
}
