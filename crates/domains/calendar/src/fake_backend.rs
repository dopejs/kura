//! Calendar fake backend (port of `fake_backend.go`): an in-memory, deterministic backend
//! used as the default for `fake_local` bindings and for daemon tests.

use std::collections::HashMap;

use chrono::{NaiveDate, TimeZone, Utc};
use kura_integrations::Resource;
use parking_lot::Mutex;

use crate::{
    AccountProjection, Attendee, AttendeeRequest, AvailabilityQuery, Backend, BusyFreeInput,
    BusyInterval, CalendarError, CancelEventInput, CreateEventInput, Event, EventLifecycleState,
    InvitationStatus, ListEventsInput, RSVPStatus, RecurrenceScope, UpdateAttendeesInput,
    UpdateEventInput, attendee_emails, normalize_timezone, resolve_attendee_requests,
};

#[derive(Debug)]
struct FakeState {
    account: AccountProjection,
    events: HashMap<String, Event>,
}

pub struct FakeBackend {
    inner: Mutex<HashMap<String, FakeState>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        FakeBackend {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_state_locked<'a>(
    inner: &'a mut HashMap<String, FakeState>,
    resource: &Resource,
) -> &'a mut FakeState {
    if !inner.contains_key(&resource.integration_id) {
        let now = Utc::now();
        let account = AccountProjection {
            calendar_account_id: format!("acct_{}", resource.integration_id),
            integration_id: resource.integration_id.clone(),
            domain_kind: resource.domain_kind.clone(),
            environment_scope: resource.environment_scope.clone(),
            account_key: resource
                .account_binding
                .as_ref()
                .map(|b| b.account_key.clone())
                .unwrap_or_default(),
            account_label: resource
                .account_binding
                .as_ref()
                .map(|b| b.account_label.clone())
                .unwrap_or_default(),
            readiness_status: resource.readiness_status.as_str().to_string(),
            canonical_default: resource.canonical_default,
            selection_mode: "explicit".to_string(),
            primary_calendar_ref: "primary".to_string(),
            primary_calendar_label: "Primary Calendar".to_string(),
            primary_timezone: "America/Los_Angeles".to_string(),
            supports_event_inspection: true,
            supports_busy_free: true,
            supports_timed_mutation: true,
            last_synced_at: now,
            updated_at: now,
            ..AccountProjection::default()
        };
        let seed = Event {
            external_event_id: "fake_event_seed".to_string(),
            integration_id: resource.integration_id.clone(),
            calendar_account_id: format!("acct_{}", resource.integration_id),
            calendar_ref: "primary".to_string(),
            title: "Seed Calendar Event".to_string(),
            starts_at: Utc
                .with_ymd_and_hms(2026, 4, 23, 16, 0, 0)
                .single()
                .unwrap(),
            ends_at: Utc
                .with_ymd_and_hms(2026, 4, 23, 16, 30, 0)
                .single()
                .unwrap(),
            timezone: "America/Los_Angeles".to_string(),
            mutation_eligible_in_phase: true,
            lifecycle_state: EventLifecycleState::Active,
            created_at: now,
            updated_at: now,
            ..Event::default()
        };
        inner.insert(
            resource.integration_id.clone(),
            FakeState {
                account,
                events: HashMap::from([("fake_event_seed".to_string(), seed)]),
            },
        );
    }
    inner.get_mut(&resource.integration_id).unwrap()
}

impl Backend for FakeBackend {
    fn project_account(&self, resource: &Resource) -> Result<AccountProjection, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        state.account.readiness_status = resource.readiness_status.as_str().to_string();
        state.account.canonical_default = resource.canonical_default;
        state.account.account_key = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_key.clone())
            .unwrap_or_default();
        state.account.account_label = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_label.clone())
            .unwrap_or_default();
        state.account.updated_at = now;
        state.account.last_synced_at = now;
        Ok(state.account.clone())
    }

    fn list_events(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        input: &ListEventsInput,
    ) -> Result<Vec<Event>, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let mut items: Vec<Event> = state
            .events
            .values()
            .filter(|item| input.starts_at.map_or(true, |s| item.ends_at >= s))
            .filter(|item| input.ends_at.map_or(true, |e| item.starts_at <= e))
            .cloned()
            .collect();
        items.sort_by(|a, b| {
            a.starts_at
                .cmp(&b.starts_at)
                .then_with(|| a.external_event_id.cmp(&b.external_event_id))
        });
        Ok(items)
    }

    fn get_event(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        event_id: &str,
    ) -> Result<Event, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        match state.events.get(event_id.trim()) {
            Some(item) => Ok(item.clone()),
            None => Err(CalendarError::CalendarEventNotFound),
        }
    }

    fn busy_free(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &BusyFreeInput,
    ) -> Result<AvailabilityQuery, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let mut items: Vec<BusyInterval> = state
            .events
            .values()
            .filter(|item| item.lifecycle_state != EventLifecycleState::Cancelled)
            .filter(|item| {
                !(item.ends_at < input.window_start || item.starts_at > input.window_end)
            })
            .map(|item| BusyInterval {
                starts_at: item.starts_at,
                ends_at: item.ends_at,
            })
            .collect();
        items.sort_by(|a, b| a.starts_at.cmp(&b.starts_at));
        Ok(AvailabilityQuery {
            integration_id: account.integration_id.clone(),
            calendar_account_id: account.calendar_account_id.clone(),
            window_start: input.window_start,
            window_end: input.window_end,
            timezone: normalize_timezone(&input.timezone, &account.primary_timezone),
            busy_intervals: items.clone(),
            conflict_count: items.len() as i64,
            result_summary: format!("{} busy interval(s)", items.len()),
            created_at: Utc::now(),
            ..AvailabilityQuery::default()
        })
    }

    fn create_event(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &CreateEventInput,
    ) -> Result<Event, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        let event_id = format!(
            "evt_{}_{}",
            resource.integration_id.replace('-', "_"),
            now.timestamp_nanos_opt().unwrap_or(0)
        );
        let (mut starts_at, mut ends_at) = (input.starts_at, input.ends_at);
        if input.all_day {
            (starts_at, ends_at) =
                all_day_bounds(&input.start_date, &input.end_date, starts_at, ends_at);
        }
        let recurring = input.recurring || !input.recurrence_rule.trim().is_empty();
        let mut item = Event {
            external_event_id: event_id.clone(),
            integration_id: account.integration_id.clone(),
            calendar_account_id: account.calendar_account_id.clone(),
            calendar_ref: account.primary_calendar_ref.clone(),
            title: input.title.trim().to_string(),
            description: input.description.trim().to_string(),
            location: input.location.trim().to_string(),
            starts_at,
            ends_at,
            timezone: normalize_timezone(&input.timezone, &account.primary_timezone),
            all_day: input.all_day,
            start_date: input.start_date.trim().to_string(),
            end_date: input.end_date.trim().to_string(),
            recurring,
            recurrence_rule: input.recurrence_rule.trim().to_string(),
            mutation_eligible_in_phase: true,
            lifecycle_state: EventLifecycleState::Active,
            created_at: now,
            updated_at: now,
            ..Event::default()
        };
        if recurring {
            item.series_id = event_id;
        }
        item.attendee_details = fake_invite(
            &resolve_attendee_requests(&input.attendee_requests, &input.attendees),
            input.notify_attendees,
        );
        item.attendees = attendee_emails(&item.attendee_details);
        state
            .events
            .insert(item.external_event_id.clone(), item.clone());
        Ok(item)
    }

    fn update_event(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &UpdateEventInput,
    ) -> Result<Event, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let Some(mut item) = state.events.get(input.external_event_id.trim()).cloned() else {
            return Err(CalendarError::CalendarEventNotFound);
        };
        if item.recurring && input.recurrence_scope == RecurrenceScope::Unspecified {
            return Err(CalendarError::CalendarRecurrenceScopeRequired);
        }
        let original = item.starts_at;
        item.title = input.title.trim().to_string();
        item.description = input.description.trim().to_string();
        item.location = input.location.trim().to_string();
        let (mut starts_at, mut ends_at) = (input.starts_at, input.ends_at);
        if input.all_day {
            (starts_at, ends_at) =
                all_day_bounds(&input.start_date, &input.end_date, starts_at, ends_at);
            item.all_day = true;
            item.start_date = input.start_date.trim().to_string();
            item.end_date = input.end_date.trim().to_string();
        }
        item.starts_at = starts_at;
        item.ends_at = ends_at;
        item.timezone = normalize_timezone(&input.timezone, &account.primary_timezone);
        item.updated_at = Utc::now();
        if item.recurring {
            apply_recurrence_identity(&mut item, input.recurrence_scope, original);
        }
        let requests = resolve_attendee_requests(&input.attendee_requests, &input.attendees);
        if !requests.is_empty() {
            item.attendee_details = fake_invite(&requests, input.notify_attendees);
            item.attendees = attendee_emails(&item.attendee_details);
        }
        state
            .events
            .insert(item.external_event_id.clone(), item.clone());
        Ok(item)
    }

    fn update_attendees(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        input: &UpdateAttendeesInput,
    ) -> Result<Event, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let Some(mut item) = state.events.get(input.external_event_id.trim()).cloned() else {
            return Err(CalendarError::CalendarEventNotFound);
        };
        let mut by_email: HashMap<String, Attendee> = item
            .attendee_details
            .iter()
            .map(|a| (a.email.to_lowercase(), a.clone()))
            .collect();
        for email in &input.remove_attendees {
            by_email.remove(&email.trim().to_lowercase());
        }
        for added in fake_invite(
            &resolve_attendee_requests(&input.add_attendees, &[]),
            input.notify,
        ) {
            by_email.insert(added.email.to_lowercase(), added);
        }
        let mut details: Vec<Attendee> = by_email.into_values().collect();
        details.sort_by(|a, b| a.email.cmp(&b.email));
        item.attendee_details = details.clone();
        item.attendees = attendee_emails(&details);
        item.updated_at = Utc::now();
        state
            .events
            .insert(item.external_event_id.clone(), item.clone());
        Ok(item)
    }

    fn cancel_event(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        input: &CancelEventInput,
    ) -> Result<Event, CalendarError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let Some(mut item) = state.events.get(input.external_event_id.trim()).cloned() else {
            return Err(CalendarError::CalendarEventNotFound);
        };
        if item.recurring && input.recurrence_scope == RecurrenceScope::Unspecified {
            return Err(CalendarError::CalendarRecurrenceScopeRequired);
        }
        let now = Utc::now();
        item.updated_at = now;
        item.cancelled_at = Some(now);
        if item.recurring && input.recurrence_scope == RecurrenceScope::ThisOccurrence {
            let starts = item.starts_at;
            apply_recurrence_identity(&mut item, RecurrenceScope::ThisOccurrence, starts);
        } else {
            item.lifecycle_state = EventLifecycleState::Cancelled;
        }
        state
            .events
            .insert(item.external_event_id.clone(), item.clone());
        Ok(item)
    }

    fn restore_integration_state(&self, integration_id: &str, events: Vec<Event>) {
        let mut inner = self.inner.lock();
        let trimmed = integration_id.trim();
        let state = inner
            .entry(trimmed.to_string())
            .or_insert_with(|| FakeState {
                account: AccountProjection {
                    calendar_account_id: format!("acct_{trimmed}"),
                    integration_id: trimmed.to_string(),
                    domain_kind: "calendar".to_string(),
                    ..AccountProjection::default()
                },
                events: HashMap::new(),
            });
        state.events = events
            .into_iter()
            .map(|e| (e.external_event_id.clone(), e))
            .collect();
    }
}

fn all_day_bounds(
    start_date: &str,
    end_date: &str,
    fallback_start: chrono::DateTime<Utc>,
    fallback_end: chrono::DateTime<Utc>,
) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    let mut start = fallback_start;
    let mut end = fallback_end;
    if let Ok(s) = NaiveDate::parse_from_str(start_date.trim(), "%Y-%m-%d") {
        start = s.and_hms_opt(0, 0, 0).unwrap().and_utc();
    }
    if let Ok(e) = NaiveDate::parse_from_str(end_date.trim(), "%Y-%m-%d") {
        end = e.and_hms_opt(0, 0, 0).unwrap().and_utc();
    }
    (start, end)
}

fn apply_recurrence_identity(
    item: &mut Event,
    scope: RecurrenceScope,
    original_start: chrono::DateTime<Utc>,
) {
    if item.series_id.is_empty() {
        item.series_id = item.external_event_id.clone();
    }
    match scope {
        RecurrenceScope::ThisOccurrence => {
            item.original_starts_at = Some(original_start);
            item.occurrence_id = format!("{}::{}", item.series_id, original_start.timestamp());
        }
        _ => {
            item.occurrence_id = String::new();
        }
    }
}

fn fake_invite(requests: &[AttendeeRequest], notify: bool) -> Vec<Attendee> {
    if requests.is_empty() {
        return Vec::new();
    }
    let invitation = if notify {
        InvitationStatus::Sent.as_str()
    } else {
        InvitationStatus::NotRequested.as_str()
    };
    requests
        .iter()
        .map(|r| Attendee {
            email: r.email.trim().to_string(),
            display_name: r.display_name.clone(),
            role: r.role.clone(),
            rsvp: RSVPStatus::NeedsAction.as_str().to_string(),
            invitation_status: invitation.to_string(),
            ..Attendee::default()
        })
        .collect()
}
