//! Port of `daemon/internal/calendar`: calendar domain types, the backend seam,
//! attendee/recurrence/timezone helpers, artifacts, and the live-validation matrix.
//! The operation manager, adapter backend, and fake backend are the next increment.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(OperationClass {
    ListEvents => "list_events",
    GetEvent => "get_event",
    BusyFree => "busy_free",
    CreateEvent => "create_event",
    UpdateEvent => "update_event",
    CancelEvent => "cancel_event",
    UpdateAttendees => "update_attendees",
});

string_enum!(AttendeeRole {
    Required => "required",
    Optional => "optional",
});

string_enum!(RSVPStatus {
    NeedsAction => "needs_action",
    Accepted => "accepted",
    Declined => "declined",
    Tentative => "tentative",
    Unknown => "unknown",
});

string_enum!(InvitationStatus {
    NotRequested => "not_requested",
    Sent => "sent",
    Blocked => "blocked",
    Failed => "failed",
    Unsupported => "unsupported",
});

string_enum!(RecurrenceScope {
    Unspecified => "",
    ThisOccurrence => "this_occurrence",
    ThisAndFollowing => "this_and_following",
    EntireSeries => "entire_series",
});

impl RecurrenceScope {
    #[must_use]
    pub fn valid(self) -> bool {
        matches!(
            self,
            RecurrenceScope::ThisOccurrence
                | RecurrenceScope::ThisAndFollowing
                | RecurrenceScope::EntireSeries
        )
    }
}

string_enum!(NotificationBehavior {
    Silent => "silent",
    Notify => "notify",
    Unsupported => "unsupported",
});

string_enum!(OperationStatus {
    Requested => "requested",
    Completed => "completed",
    Failed => "failed",
    Blocked => "blocked",
    Cancelled => "cancelled",
});

string_enum!(ArtifactKind {
    EventSnapshot => "event_snapshot",
    AvailabilityQuery => "availability_query",
});

string_enum!(EventLifecycleState {
    Active => "active",
    Cancelled => "cancelled",
    StaleSnapshot => "stale_snapshot",
    Unavailable => "unavailable",
});

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CalendarError {
    #[error("calendar integration is unavailable")]
    CalendarUnavailable,
    #[error("calendar integration not found")]
    CalendarIntegrationNotFound,
    #[error("calendar integration selection is invalid")]
    CalendarSelectionInvalid,
    #[error("calendar operation not found")]
    CalendarOperationNotFound,
    #[error("calendar account projection not found")]
    CalendarAccountNotFound,
    #[error("calendar event not found")]
    CalendarEventNotFound,
    #[error("recurring-event mutation is out of scope for phase 29")]
    CalendarRecurringUnsupported,
    #[error("all-day-event mutation is out of scope for phase 29")]
    CalendarAllDayUnsupported,
    #[error("attendee mutation semantics are out of scope for phase 29")]
    CalendarAttendeesUnsupported,
    #[error("alternate-calendar mutation is out of scope for phase 29")]
    CalendarAlternateCalendarDeny,
    #[error("invalid calendar time range")]
    CalendarInvalidTimeRange,
    #[error("attendee update requires at least one add or remove")]
    CalendarAttendeeRequestEmpty,
    #[error("recurrence scope is required for recurring-event mutation")]
    CalendarRecurrenceScopeRequired,
    #[error("recurrence scope is invalid")]
    CalendarRecurrenceScopeInvalid,
    #[error("calendar backend is not configured")]
    CalendarBackendNotConfigured,
    #[error("{0}")]
    Adapter(AdapterFailure),
    #[error("calendar adapter transport error: {0}")]
    AdapterTransport(String),
}

/// A wrapped out-of-process adapter failure carrying the stable, redacted failure class and
/// diagnostics provider kind the Manager records on the operation ledger (FR-006/FR-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFailure {
    pub class: String,
    pub provider_kind: String,
    pub detail: String,
    pub ambiguous: bool,
    pub unavailable: bool,
}

impl AdapterFailure {
    #[must_use]
    pub fn failure_class(&self) -> &str {
        &self.class
    }
}

impl std::fmt::Display for AdapterFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.detail.is_empty() {
            f.write_str(&self.detail)
        } else {
            f.write_str(&self.class)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttendeeRequest {
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rsvp: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub invitation_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttendeeOutcome {
    pub notification_requested: bool,
    pub notification_behavior: NotificationBehavior,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,
    pub ambiguous: bool,
    pub unsupported: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unsupported_reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProjection {
    pub calendar_account_id: String,
    pub integration_id: String,
    pub domain_kind: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_label: String,
    pub readiness_status: String,
    pub canonical_default: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selection_mode: String,
    pub primary_calendar_ref: String,
    pub primary_calendar_label: String,
    pub primary_timezone: String,
    pub supports_event_inspection: bool,
    pub supports_busy_free: bool,
    pub supports_timed_mutation: bool,
    pub last_synced_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub external_event_id: String,
    pub integration_id: String,
    pub calendar_account_id: String,
    pub calendar_ref: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start_date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub end_date: String,
    pub recurring: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recurrence_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recurrence_rule: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub series_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub occurrence_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_starts_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendee_details: Vec<Attendee>,
    pub mutation_eligible_in_phase: bool,
    pub lifecycle_state: EventLifecycleState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusyInterval {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityQuery {
    pub query_id: String,
    pub operation_id: String,
    pub integration_id: String,
    pub calendar_account_id: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub busy_intervals: Vec<BusyInterval>,
    pub conflict_count: i64,
    pub result_summary: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    pub operation_id: String,
    pub kind: ArtifactKind,
    pub integration_id: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub calendar_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recurrence_summary: String,
    pub mutation_eligible_in_phase: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lifecycle_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability_query: Option<AvailabilityQuery>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: String,
    pub operation_class: OperationClass,
    pub status: OperationStatus,
    pub integration_id: String,
    pub calendar_account_id: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub calendar_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selection_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone_used: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_failure: Option<kura_integrations::DiagnosticFailureProjection>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attendee_outcome: Option<AttendeeOutcome>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recurrence_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub original_external_event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resulting_series_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_query_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub operation_id: String,
    pub operation_class: OperationClass,
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_event_id: String,
    pub status: OperationStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone_used: String,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub integration_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLinkage {
    pub operation_id: String,
    pub run_id: String,
    pub step_id: String,
    pub tool_call_id: String,
    pub workflow_id: String,
    pub workflow_step_id: String,
    pub schedule_id: String,
    pub schedule_attempt_id: String,
    pub delivery_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub operation_class: OperationClass,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub calendar_ref: String,
    pub all_day: bool,
    pub recurring: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListEventsInput {
    pub selection: Selection,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetEventInput {
    pub selection: Selection,
    pub external_event_id: String,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusyFreeInput {
    pub selection: Selection,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub timezone: String,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEventInput {
    pub selection: Selection,
    pub title: String,
    pub description: String,
    pub location: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub all_day: bool,
    pub start_date: String,
    pub end_date: String,
    pub recurring: bool,
    pub recurrence_rule: String,
    pub attendees: Vec<String>,
    pub attendee_requests: Vec<AttendeeRequest>,
    pub notify_attendees: bool,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEventInput {
    pub selection: Selection,
    pub external_event_id: String,
    pub title: String,
    pub description: String,
    pub location: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub all_day: bool,
    pub start_date: String,
    pub end_date: String,
    pub recurring: bool,
    pub recurrence_rule: String,
    pub recurrence_scope: RecurrenceScope,
    pub attendees: Vec<String>,
    pub attendee_requests: Vec<AttendeeRequest>,
    pub notify_attendees: bool,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAttendeesInput {
    pub selection: Selection,
    pub external_event_id: String,
    pub add_attendees: Vec<AttendeeRequest>,
    pub remove_attendees: Vec<String>,
    pub notify: bool,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelEventInput {
    pub selection: Selection,
    pub external_event_id: String,
    pub reason: String,
    pub recurrence_scope: RecurrenceScope,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default)]
pub struct OperationFilter {
    pub integration_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub schedule_id: String,
    pub delivery_id: String,
    pub operation_class: OperationClass,
    pub status: OperationStatus,
    pub external_event_id: String,
}

pub trait Backend: Send + Sync {
    fn project_account(
        &self,
        resource: &kura_integrations::Resource,
    ) -> Result<AccountProjection, CalendarError>;
    fn list_events(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &ListEventsInput,
    ) -> Result<Vec<Event>, CalendarError>;
    fn get_event(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        event_id: &str,
    ) -> Result<Event, CalendarError>;
    fn busy_free(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &BusyFreeInput,
    ) -> Result<AvailabilityQuery, CalendarError>;
    fn create_event(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &CreateEventInput,
    ) -> Result<Event, CalendarError>;
    fn update_event(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &UpdateEventInput,
    ) -> Result<Event, CalendarError>;
    fn cancel_event(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &CancelEventInput,
    ) -> Result<Event, CalendarError>;
    fn update_attendees(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &UpdateAttendeesInput,
    ) -> Result<Event, CalendarError>;
    fn restore_integration_state(&self, integration_id: &str, events: Vec<Event>);
}

#[must_use]
pub fn resolve_attendee_requests(
    requests: &[AttendeeRequest],
    emails: &[String],
) -> Vec<AttendeeRequest> {
    if !requests.is_empty() {
        return requests
            .iter()
            .filter(|r| !r.email.trim().is_empty())
            .map(|r| {
                let mut r = r.clone();
                if r.role.is_empty() {
                    r.role = AttendeeRole::Required.as_str().to_string();
                }
                r
            })
            .collect();
    }
    emails
        .iter()
        .filter(|email| !email.trim().is_empty())
        .map(|email| AttendeeRequest {
            email: email.trim().to_string(),
            role: AttendeeRole::Required.as_str().to_string(),
            ..AttendeeRequest::default()
        })
        .collect()
}

#[must_use]
pub fn attendee_emails(details: &[Attendee]) -> Vec<String> {
    if details.is_empty() {
        return Vec::new();
    }
    details.iter().map(|a| a.email.clone()).collect()
}

#[must_use]
pub fn build_attendee_outcome(notify: bool, details: &[Attendee]) -> Option<AttendeeOutcome> {
    if details.is_empty() && !notify {
        return None;
    }
    let mut out = AttendeeOutcome {
        notification_requested: notify,
        attendees: details.to_vec(),
        ..AttendeeOutcome::default()
    };
    let mut behavior = if notify {
        NotificationBehavior::Notify
    } else {
        NotificationBehavior::Silent
    };
    for a in details {
        if a.invitation_status == InvitationStatus::Unsupported.as_str() {
            out.unsupported = true;
            out.unsupported_reason =
                "provider does not support the requested attendee notification behavior"
                    .to_string();
            behavior = NotificationBehavior::Unsupported;
        }
    }
    out.notification_behavior = behavior;
    Some(out)
}

#[must_use]
pub fn normalize_timezone(requested: &str, fallback: &str) -> String {
    if !requested.trim().is_empty() {
        return requested.trim().to_string();
    }
    if !fallback.trim().is_empty() {
        return fallback.trim().to_string();
    }
    "UTC".to_string()
}

#[must_use]
pub fn clone_operation_summaries(items: &[OperationSummary]) -> Vec<OperationSummary> {
    items.to_vec()
}

#[must_use]
pub fn live_validation_matrix_rows() -> Vec<kura_livevalidation::MatrixRow> {
    let classes = [
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::CALENDAR_EVENT_CREATE),
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::CALENDAR_EVENT_UPDATE),
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::CALENDAR_EVENT_CANCEL),
        kura_livevalidation::ToolClass::from(
            kura_livevalidation::ToolClass::CALENDAR_ATTENDEE_UPDATE,
        ),
    ];
    let mut rows = Vec::new();
    for tool_class in classes {
        if let Some(row) = kura_livevalidation::default_matrix_row(&tool_class) {
            rows.push(row);
        }
    }
    rows
}

mod adapter_backend;
mod fake_backend;
mod manager;
pub use adapter_backend::*;
pub use fake_backend::*;
pub use manager::*;
