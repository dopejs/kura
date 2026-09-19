//! Feishu/Lark calendar provider (port of calendar.go): maps the Feishu Open Platform Calendar
//! API onto the calendar domain resources. Stateless; the daemon owns the operation ledger.

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use kura_adapterprovider::{Handler, HandlerError, Operation};
use kura_calendar::{
    AccountProjection, Attendee, AttendeeRequest, AvailabilityQuery, BusyInterval,
    CancelEventInput, CreateEventInput, Event, EventLifecycleState, InvitationStatus,
    ListEventsInput, RSVPStatus, UpdateAttendeesInput, UpdateEventInput, attendee_emails,
    resolve_attendee_requests,
};
use kura_integrations::{ReadinessStatus, Resource};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::{Client, FaultKind, ProviderFault, ScopedToken, first_non_empty, parse_token};

pub struct CalendarProvider {
    client: Client,
}

pub fn new_calendar_provider(client: Client) -> CalendarProvider {
    CalendarProvider { client }
}

impl Handler for CalendarProvider {
    fn handle(
        &self,
        op: Operation,
        deadline: Option<Duration>,
    ) -> Result<Option<Box<RawValue>>, HandlerError> {
        if op.domain != "calendar" {
            return Err(HandlerError::Fault(
                ProviderFault {
                    kind: FaultKind::Internal,
                    code: "unsupported_domain".to_string(),
                    message: "adapter serves the calendar domain only".to_string(),
                }
                .to_adapter_fault(),
            ));
        }
        let raw_cred = op
            .credential
            .as_deref()
            .map(|r| r.get().as_bytes())
            .unwrap_or(&[]);
        let token = parse_token(raw_cred).map_err(|f| HandlerError::Fault(f.to_adapter_fault()))?;
        let resource: Resource = op
            .resource
            .as_deref()
            .map(|r| serde_json::from_str(r.get()).unwrap_or_default())
            .unwrap_or_default();
        match self.route(&token, &resource, &op, deadline) {
            Ok(raw) => Ok(Some(raw)),
            Err(pf) if pf.is_ambiguous() => Err(HandlerError::Ambiguous),
            Err(pf) => Err(HandlerError::Fault(pf.to_adapter_fault())),
        }
    }
}

impl CalendarProvider {
    fn route(
        &self,
        token: &ScopedToken,
        resource: &Resource,
        op: &Operation,
        deadline: Option<Duration>,
    ) -> Result<Box<RawValue>, ProviderFault> {
        let payload = op.payload.as_deref();
        match op.operation.as_str() {
            "ProjectAccount" => marshal_result(self.project_account(token, resource, deadline)?),
            "ListEvents" => {
                let input = decode_payload::<ListEventsPayload>(payload)?;
                marshal_result(self.list_events(token, &input.account, &input.input, deadline)?)
            }
            "GetEvent" => {
                let input = decode_payload::<GetEventPayload>(payload)?;
                marshal_result(self.get_event(token, &input.account, &input.event_id, deadline)?)
            }
            "BusyFree" => {
                let input = decode_payload::<BusyFreePayload>(payload)?;
                marshal_result(self.busy_free(token, &input.account, &input.input, deadline)?)
            }
            "CreateEvent" => {
                let input = decode_payload::<CreateEventPayload>(payload)?;
                marshal_result(self.create_event(token, &input.account, &input.input, deadline)?)
            }
            "UpdateEvent" => {
                let input = decode_payload::<UpdateEventPayload>(payload)?;
                marshal_result(self.update_event(token, &input.account, &input.input, deadline)?)
            }
            "CancelEvent" => {
                let input = decode_payload::<CancelEventPayload>(payload)?;
                marshal_result(self.cancel_event(token, &input.account, &input.input, deadline)?)
            }
            "UpdateAttendees" => {
                let input = decode_payload::<UpdateAttendeesPayload>(payload)?;
                marshal_result(self.update_attendees(
                    token,
                    &input.account,
                    &input.input,
                    deadline,
                )?)
            }
            _ => Err(ProviderFault {
                kind: FaultKind::Internal,
                code: "unsupported_operation".to_string(),
                message: "unsupported calendar operation".to_string(),
            }),
        }
    }

    fn project_account(
        &self,
        token: &ScopedToken,
        resource: &Resource,
        deadline: Option<Duration>,
    ) -> Result<AccountProjection, ProviderFault> {
        let mut out = FeishuPrimaryResp::default();
        self.client.call(
            deadline,
            "POST",
            "/open-apis/calendar/v4/calendars/primary?user_id_type=open_id",
            &token.access_token,
            Some(&Value::Object(Default::default())),
            Some(&mut out),
            false,
        )?;
        let Some(primary) = out.calendars.first() else {
            return Err(ProviderFault {
                kind: FaultKind::Unavailable,
                code: "primary_calendar_missing".to_string(),
                message: "no primary calendar returned".to_string(),
            });
        };
        let now = Utc::now();
        let account_type = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_type.clone())
            .unwrap_or_default();
        Ok(AccountProjection {
            calendar_account_id: format!("fl_{}", primary.user_id),
            integration_id: resource.integration_id.clone(),
            domain_kind: "calendar".to_string(),
            environment_scope: resource.environment_scope.clone(),
            account_key: primary.user_id.clone(),
            account_label: primary.calendar.summary.clone(),
            readiness_status: ReadinessStatus::Healthy.as_str().to_string(),
            canonical_default: account_type == "primary",
            primary_calendar_ref: primary.calendar.calendar_id.clone(),
            primary_calendar_label: primary.calendar.summary.clone(),
            primary_timezone: normalize_tz(resource),
            supports_event_inspection: true,
            supports_busy_free: true,
            supports_timed_mutation: true,
            last_synced_at: now,
            updated_at: now,
            ..AccountProjection::default()
        })
    }

    fn list_events(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &ListEventsInput,
        deadline: Option<Duration>,
    ) -> Result<Vec<Event>, ProviderFault> {
        let mut query: Vec<String> = Vec::new();
        if let Some(start) = input.starts_at {
            query.push(format!("start_time={}", start.timestamp()));
        }
        if let Some(end) = input.ends_at {
            query.push(format!("end_time={}", end.timestamp()));
        }
        query.push("page_size=100".to_string());
        let path = format!(
            "/open-apis/calendar/v4/calendars/{}/events?{}",
            account.primary_calendar_ref,
            query.join("&")
        );
        let mut out = ItemsResp::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        Ok(out
            .items
            .iter()
            .map(|item| map_event(account, item))
            .collect())
    }

    fn get_event(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        event_id: &str,
        deadline: Option<Duration>,
    ) -> Result<Event, ProviderFault> {
        let path = format!(
            "/open-apis/calendar/v4/calendars/{}/events/{}",
            account.primary_calendar_ref, event_id
        );
        let mut out = EventResp::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        Ok(map_event(account, &out.event))
    }

    fn busy_free(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &kura_calendar::BusyFreeInput,
        deadline: Option<Duration>,
    ) -> Result<AvailabilityQuery, ProviderFault> {
        let body = serde_json::json!({
            "time_min": input.window_start.to_rfc3339(),
            "time_max": input.window_end.to_rfc3339(),
            "user_id": account.account_key,
        });
        let mut out = FreebusyResp::default();
        self.client.call(
            deadline,
            "POST",
            "/open-apis/calendar/v4/freebusy/list?user_id_type=open_id",
            &token.access_token,
            Some(&body),
            Some(&mut out),
            false,
        )?;
        let mut intervals = Vec::new();
        for fb in &out.freebusy_list {
            if let (Ok(start), Ok(end)) = (
                DateTime::parse_from_rfc3339(&fb.start_time),
                DateTime::parse_from_rfc3339(&fb.end_time),
            ) {
                intervals.push(BusyInterval {
                    starts_at: start.with_timezone(&Utc),
                    ends_at: end.with_timezone(&Utc),
                });
            }
        }
        Ok(AvailabilityQuery {
            integration_id: account.integration_id.clone(),
            calendar_account_id: account.calendar_account_id.clone(),
            window_start: input.window_start,
            window_end: input.window_end,
            timezone: first_non_empty(&[&input.timezone, &account.primary_timezone]),
            busy_intervals: intervals.clone(),
            conflict_count: intervals.len() as i64,
            result_summary: format!("{} busy interval(s)", intervals.len()),
            ..AvailabilityQuery::default()
        })
    }

    fn create_event(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &CreateEventInput,
        deadline: Option<Duration>,
    ) -> Result<Event, ProviderFault> {
        let requests = resolve_attendee_requests(&input.attendee_requests, &input.attendees);
        let body = write_event_body(EventBodyInput {
            title: input.title.clone(),
            description: input.description.clone(),
            location: input.location.clone(),
            tz: first_non_empty(&[&input.timezone, &account.primary_timezone]),
            starts_at: input.starts_at,
            ends_at: input.ends_at,
            all_day: input.all_day,
            start_date: input.start_date.clone(),
            end_date: input.end_date.clone(),
            recurrence_rule: input.recurrence_rule.clone(),
            attendees: requests.clone(),
        });
        let path = format!(
            "/open-apis/calendar/v4/calendars/{}/events?need_notification={}",
            account.primary_calendar_ref, input.notify_attendees
        );
        let mut out = EventResp::default();
        self.client.call(
            deadline,
            "POST",
            &path,
            &token.access_token,
            Some(&body),
            Some(&mut out),
            true,
        )?;
        let mut event = map_event(account, &out.event);
        apply_invitation_status(&mut event, &requests, input.notify_attendees);
        Ok(event)
    }

    fn update_event(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &UpdateEventInput,
        deadline: Option<Duration>,
    ) -> Result<Event, ProviderFault> {
        let requests = resolve_attendee_requests(&input.attendee_requests, &input.attendees);
        let body = write_event_body(EventBodyInput {
            title: input.title.clone(),
            description: input.description.clone(),
            location: input.location.clone(),
            tz: first_non_empty(&[&input.timezone, &account.primary_timezone]),
            starts_at: input.starts_at,
            ends_at: input.ends_at,
            all_day: input.all_day,
            start_date: input.start_date.clone(),
            end_date: input.end_date.clone(),
            recurrence_rule: input.recurrence_rule.clone(),
            attendees: requests.clone(),
        });
        let path = format!(
            "/open-apis/calendar/v4/calendars/{}/events/{}?need_notification={}",
            account.primary_calendar_ref, input.external_event_id, input.notify_attendees
        );
        let mut out = EventResp::default();
        self.client.call(
            deadline,
            "PATCH",
            &path,
            &token.access_token,
            Some(&body),
            Some(&mut out),
            true,
        )?;
        let mut event = map_event(account, &out.event);
        if event.external_event_id.is_empty() {
            event.external_event_id = input.external_event_id.clone();
        }
        apply_invitation_status(&mut event, &requests, input.notify_attendees);
        Ok(event)
    }

    fn update_attendees(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &UpdateAttendeesInput,
        deadline: Option<Duration>,
    ) -> Result<Event, ProviderFault> {
        let event_path = format!(
            "/open-apis/calendar/v4/calendars/{}/events/{}",
            account.primary_calendar_ref, input.external_event_id
        );
        if !input.add_attendees.is_empty() {
            let body = serde_json::json!({
                "attendees": attendee_body(&resolve_attendee_requests(&input.add_attendees, &[])),
                "need_notification": input.notify,
            });
            self.client.call(
                deadline,
                "POST",
                &format!("{event_path}/attendees"),
                &token.access_token,
                Some(&body),
                None::<&mut Value>,
                true,
            )?;
        }
        if !input.remove_attendees.is_empty() {
            let mut current = EventResp::default();
            self.client.call(
                deadline,
                "GET",
                &event_path,
                &token.access_token,
                None::<&Value>,
                Some(&mut current),
                false,
            )?;
            let ids = resolve_attendee_ids(&current.event.attendees, &input.remove_attendees);
            if !ids.is_empty() {
                let body =
                    serde_json::json!({ "attendee_ids": ids, "need_notification": input.notify });
                self.client.call(
                    deadline,
                    "POST",
                    &format!("{event_path}/attendees/batch_delete"),
                    &token.access_token,
                    Some(&body),
                    None::<&mut Value>,
                    true,
                )?;
            }
        }
        let mut out = EventResp::default();
        self.client.call(
            deadline,
            "GET",
            &event_path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        let mut event = map_event(account, &out.event);
        if event.external_event_id.is_empty() {
            event.external_event_id = input.external_event_id.clone();
        }
        Ok(event)
    }

    fn cancel_event(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &CancelEventInput,
        deadline: Option<Duration>,
    ) -> Result<Event, ProviderFault> {
        let path = format!(
            "/open-apis/calendar/v4/calendars/{}/events/{}",
            account.primary_calendar_ref, input.external_event_id
        );
        self.client.call(
            deadline,
            "DELETE",
            &path,
            &token.access_token,
            None::<&Value>,
            None::<&mut Value>,
            true,
        )?;
        let now = Utc::now();
        Ok(Event {
            external_event_id: input.external_event_id.clone(),
            integration_id: account.integration_id.clone(),
            calendar_account_id: account.calendar_account_id.clone(),
            calendar_ref: account.primary_calendar_ref.clone(),
            lifecycle_state: EventLifecycleState::Cancelled,
            cancelled_at: Some(now),
            updated_at: now,
            ..Event::default()
        })
    }
}

// ---- payload shapes ----

#[derive(Debug, Default, Deserialize)]
struct ListEventsPayload {
    account: AccountProjection,
    input: ListEventsInput,
}

#[derive(Debug, Default, Deserialize)]
struct GetEventPayload {
    account: AccountProjection,
    #[serde(rename = "eventId")]
    event_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct BusyFreePayload {
    account: AccountProjection,
    input: kura_calendar::BusyFreeInput,
}

#[derive(Debug, Default, Deserialize)]
struct CreateEventPayload {
    account: AccountProjection,
    input: CreateEventInput,
}

#[derive(Debug, Default, Deserialize)]
struct UpdateEventPayload {
    account: AccountProjection,
    input: UpdateEventInput,
}

#[derive(Debug, Default, Deserialize)]
struct CancelEventPayload {
    account: AccountProjection,
    input: CancelEventInput,
}

#[derive(Debug, Default, Deserialize)]
struct UpdateAttendeesPayload {
    account: AccountProjection,
    input: UpdateAttendeesInput,
}

// ---- Feishu response shapes ----

#[derive(Debug, Default, Deserialize)]
struct FeishuTimeInfo {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    timezone: String,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuLocation {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuEvent {
    #[serde(rename = "event_id", default)]
    event_id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "start_time", default)]
    start_time: FeishuTimeInfo,
    #[serde(rename = "end_time", default)]
    end_time: FeishuTimeInfo,
    #[serde(default)]
    status: String,
    #[serde(default)]
    location: FeishuLocation,
    #[serde(default)]
    recurrence: String,
    #[serde(default)]
    attendees: Vec<FeishuAttendee>,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct FeishuAttendee {
    #[serde(rename = "type", default)]
    attendee_type: String,
    #[serde(rename = "attendee_id", default)]
    attendee_id: String,
    #[serde(rename = "rsvp_status", default)]
    rsvp_status: String,
    #[serde(rename = "display_name", default)]
    display_name: String,
    #[serde(rename = "is_optional", default)]
    is_optional: bool,
    #[serde(rename = "third_party_email", default)]
    third_party_email: String,
    #[serde(rename = "user_id", default)]
    user_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuPrimaryResp {
    #[serde(default)]
    calendars: Vec<FeishuPrimaryCalendar>,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuPrimaryCalendar {
    #[serde(default)]
    calendar: FeishuCalendarInfo,
    #[serde(rename = "user_id", default)]
    user_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct FeishuCalendarInfo {
    #[serde(rename = "calendar_id", default)]
    calendar_id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    role: String,
}

#[derive(Debug, Default, Deserialize)]
struct ItemsResp {
    #[serde(default)]
    items: Vec<FeishuEvent>,
}

#[derive(Debug, Default, Deserialize)]
struct EventResp {
    #[serde(default)]
    event: FeishuEvent,
}

#[derive(Debug, Default, Deserialize)]
struct FreebusyResp {
    #[serde(rename = "freebusy_list", default)]
    freebusy_list: Vec<FreebusyInterval>,
}

#[derive(Debug, Default, Deserialize)]
struct FreebusyInterval {
    #[serde(rename = "start_time", default)]
    start_time: String,
    #[serde(rename = "end_time", default)]
    end_time: String,
}

// ---- mapping helpers ----

struct EventBodyInput {
    title: String,
    description: String,
    location: String,
    tz: String,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    all_day: bool,
    start_date: String,
    end_date: String,
    recurrence_rule: String,
    attendees: Vec<AttendeeRequest>,
}

fn write_event_body(input: EventBodyInput) -> Value {
    let mut start = serde_json::Map::new();
    start.insert("timezone".to_string(), Value::String(input.tz.clone()));
    let mut end = serde_json::Map::new();
    end.insert("timezone".to_string(), Value::String(input.tz.clone()));
    if input.all_day {
        start.insert(
            "date".to_string(),
            Value::String(first_non_empty(&[
                &input.start_date,
                &input.starts_at.format("%Y-%m-%d").to_string(),
            ])),
        );
        end.insert(
            "date".to_string(),
            Value::String(first_non_empty(&[
                &input.end_date,
                &input.ends_at.format("%Y-%m-%d").to_string(),
            ])),
        );
    } else {
        start.insert(
            "timestamp".to_string(),
            Value::String(input.starts_at.timestamp().to_string()),
        );
        end.insert(
            "timestamp".to_string(),
            Value::String(input.ends_at.timestamp().to_string()),
        );
    }
    let mut body = serde_json::Map::new();
    body.insert("summary".to_string(), Value::String(input.title));
    body.insert("description".to_string(), Value::String(input.description));
    body.insert("start_time".to_string(), Value::Object(start));
    body.insert("end_time".to_string(), Value::Object(end));
    if !input.location.trim().is_empty() {
        let mut loc = serde_json::Map::new();
        loc.insert("name".to_string(), Value::String(input.location));
        body.insert("location".to_string(), Value::Object(loc));
    }
    if !input.recurrence_rule.trim().is_empty() {
        body.insert(
            "recurrence".to_string(),
            Value::String(input.recurrence_rule),
        );
    }
    let attendees = attendee_body(&input.attendees);
    if !attendees.is_empty() {
        body.insert("attendees".to_string(), Value::Array(attendees));
    }
    Value::Object(body)
}

fn attendee_body(requests: &[AttendeeRequest]) -> Vec<Value> {
    if requests.is_empty() {
        return Vec::new();
    }
    requests
        .iter()
        .map(|r| {
            serde_json::json!({
                "type": "third_party",
                "third_party_email": r.email,
                "is_optional": r.role == "optional",
            })
        })
        .collect()
}

fn map_attendees(items: &[FeishuAttendee]) -> Vec<Attendee> {
    if items.is_empty() {
        return Vec::new();
    }
    items
        .iter()
        .map(|a| Attendee {
            email: first_non_empty(&[&a.third_party_email, &a.display_name]),
            display_name: a.display_name.clone(),
            role: if a.is_optional {
                "optional".to_string()
            } else {
                "required".to_string()
            },
            rsvp: map_rsvp(&a.rsvp_status).as_str().to_string(),
            ..Attendee::default()
        })
        .collect()
}

fn map_rsvp(s: &str) -> RSVPStatus {
    match s.trim().to_lowercase().as_str() {
        "accept" | "accepted" => RSVPStatus::Accepted,
        "decline" | "declined" => RSVPStatus::Declined,
        "tentative" => RSVPStatus::Tentative,
        "needs_action" | "" => RSVPStatus::NeedsAction,
        _ => RSVPStatus::Unknown,
    }
}

fn apply_invitation_status(event: &mut Event, requests: &[AttendeeRequest], notify: bool) {
    if event.attendee_details.is_empty() && !requests.is_empty() {
        event.attendee_details = requests
            .iter()
            .map(|r| Attendee {
                email: r.email.clone(),
                display_name: r.display_name.clone(),
                role: r.role.clone(),
                rsvp: RSVPStatus::NeedsAction.as_str().to_string(),
                ..Attendee::default()
            })
            .collect();
    }
    let status = if notify {
        InvitationStatus::Sent.as_str()
    } else {
        InvitationStatus::NotRequested.as_str()
    };
    for detail in &mut event.attendee_details {
        detail.invitation_status = status.to_string();
    }
    event.attendees = attendee_emails(&event.attendee_details);
}

fn resolve_attendee_ids(current: &[FeishuAttendee], remove_emails: &[String]) -> Vec<String> {
    let want: std::collections::HashSet<String> = remove_emails
        .iter()
        .map(|e| e.trim().to_lowercase())
        .collect();
    current
        .iter()
        .filter(|a| {
            !a.attendee_id.is_empty()
                && want.contains(
                    &first_non_empty(&[&a.third_party_email, &a.display_name]).to_lowercase(),
                )
        })
        .map(|a| a.attendee_id.clone())
        .collect()
}

fn map_event(account: &AccountProjection, item: &FeishuEvent) -> Event {
    let lifecycle = if item.status.eq_ignore_ascii_case("cancelled") {
        EventLifecycleState::Cancelled
    } else {
        EventLifecycleState::Active
    };
    let now = Utc::now();
    let attendees = map_attendees(&item.attendees);
    let all_day = !item.start_time.date.is_empty();
    let recurring = !item.recurrence.trim().is_empty();
    let mut event = Event {
        external_event_id: item.event_id.clone(),
        integration_id: account.integration_id.clone(),
        calendar_account_id: account.calendar_account_id.clone(),
        calendar_ref: account.primary_calendar_ref.clone(),
        title: item.summary.clone(),
        description: item.description.clone(),
        location: item.location.name.clone(),
        starts_at: parse_feishu_time(&item.start_time),
        ends_at: parse_feishu_time(&item.end_time),
        timezone: first_non_empty(&[&item.start_time.timezone, &account.primary_timezone]),
        all_day,
        start_date: item.start_time.date.clone(),
        end_date: item.end_time.date.clone(),
        recurring,
        recurrence_summary: item.recurrence.clone(),
        recurrence_rule: item.recurrence.clone(),
        attendees: attendee_emails(&attendees),
        attendee_details: attendees,
        mutation_eligible_in_phase: true,
        lifecycle_state: lifecycle,
        updated_at: now,
        ..Event::default()
    };
    if recurring {
        event.series_id = item.event_id.clone();
    }
    event
}

fn parse_feishu_time(t: &FeishuTimeInfo) -> DateTime<Utc> {
    if !t.timestamp.trim().is_empty() {
        if let Ok(sec) = t.timestamp.trim().parse::<i64>() {
            if let Some(ts) = DateTime::from_timestamp(sec, 0) {
                return ts;
            }
        }
    }
    if !t.date.trim().is_empty() {
        if let Ok(d) = NaiveDate::parse_from_str(t.date.trim(), "%Y-%m-%d") {
            return d.and_hms_opt(0, 0, 0).unwrap().and_utc();
        }
    }
    DateTime::<Utc>::default()
}

fn normalize_tz(_resource: &Resource) -> String {
    "UTC".to_string()
}

fn decode_payload<T: DeserializeOwned>(payload: Option<&RawValue>) -> Result<T, ProviderFault> {
    let raw = payload.ok_or_else(|| ProviderFault {
        kind: FaultKind::Internal,
        code: "empty_payload".to_string(),
        message: "operation payload missing".to_string(),
    })?;
    serde_json::from_str(raw.get()).map_err(|_| ProviderFault {
        kind: FaultKind::Internal,
        code: "payload_decode_failed".to_string(),
        message: "operation payload unreadable".to_string(),
    })
}

fn marshal_result<T: Serialize>(value: T) -> Result<Box<RawValue>, ProviderFault> {
    serde_json::value::to_raw_value(&value).map_err(|_| ProviderFault {
        kind: FaultKind::Internal,
        code: "result_encode_failed".to_string(),
        message: "result encode failed".to_string(),
    })
}
