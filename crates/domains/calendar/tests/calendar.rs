use chrono::{TimeZone, Utc};
use kura_calendar::{
    Attendee, AttendeeRequest, BusyFreeInput, CalendarError, CancelEventInput, CreateEventInput,
    EventLifecycleState, GetEventInput, InvitationStatus, ListEventsInput, Manager,
    NotificationBehavior, OperationFilter, OperationStatus, RecurrenceScope, Selection,
    UpdateAttendeesInput, UpdateEventInput, attendee_emails, build_attendee_outcome,
    live_validation_matrix_rows, normalize_timezone, resolve_attendee_requests,
};
use kura_integrations::{BackendBinding, BackendKind, ReadinessStatus, Resource};

fn test_resource(integration_id: &str, env: &str, canonical_default: bool) -> Resource {
    Resource {
        integration_id: integration_id.to_string(),
        domain_kind: "calendar".to_string(),
        environment_scope: env.to_string(),
        readiness_status: ReadinessStatus::Healthy,
        canonical_default,
        backend_binding: BackendBinding {
            backend_kind: BackendKind::FakeLocal,
            ..BackendBinding::default()
        },
        ..Resource::default()
    }
}

#[test]
fn resolve_attendee_requests_synthesizes_from_emails() {
    let requests =
        resolve_attendee_requests(&[], &["a@x.com".to_string(), " b@x.com ".to_string()]);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].email, "a@x.com");
    assert_eq!(requests[1].email, "b@x.com");
    assert_eq!(requests[0].role, "required");
}

#[test]
fn resolve_attendee_requests_prefers_explicit() {
    let req = vec![AttendeeRequest {
        email: "a@x.com".to_string(),
        ..AttendeeRequest::default()
    }];
    let requests = resolve_attendee_requests(&req, &["ignored@x.com".to_string()]);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].role, "required");
}

#[test]
fn attendee_emails_and_outcome() {
    let details = vec![Attendee {
        email: "a@x.com".to_string(),
        invitation_status: InvitationStatus::Unsupported.as_str().to_string(),
        ..Attendee::default()
    }];
    assert_eq!(attendee_emails(&details), vec!["a@x.com"]);
    let outcome = build_attendee_outcome(true, &details).expect("outcome");
    assert!(outcome.unsupported);
    assert_eq!(
        outcome.notification_behavior,
        NotificationBehavior::Unsupported
    );
    assert!(build_attendee_outcome(false, &[]).is_none());
}

#[test]
fn normalize_timezone_falls_back_to_utc() {
    assert_eq!(normalize_timezone("Asia/Shanghai", "UTC"), "Asia/Shanghai");
    assert_eq!(normalize_timezone("", "Asia/Tokyo"), "Asia/Tokyo");
    assert_eq!(normalize_timezone("", ""), "UTC");
}

#[test]
fn recurrence_scope_validity() {
    assert!(RecurrenceScope::ThisOccurrence.valid());
    assert!(RecurrenceScope::ThisAndFollowing.valid());
    assert!(RecurrenceScope::EntireSeries.valid());
    assert!(!RecurrenceScope::Unspecified.valid());
}

#[test]
fn live_validation_rows_cover_calendar_classes() {
    assert_eq!(live_validation_matrix_rows().len(), 4);
}

#[test]
fn list_accounts_returns_all_calendar_resources_in_env() {
    let manager = Manager::new("test");
    let resources = vec![
        test_resource("cal_1", "test", true),
        test_resource("cal_2", "test", false),
        test_resource("cal_3", "other", true),
    ];
    let accounts = manager
        .list_accounts(&resources, &Selection::default())
        .unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].integration_id, "cal_1");
    assert_eq!(accounts[1].integration_id, "cal_2");
}

#[test]
fn create_event_records_completed_operation_and_artifact() {
    let manager = Manager::new("test");
    let resources = vec![test_resource("cal_1", "test", true)];
    let input = CreateEventInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        title: "Lunch".to_string(),
        starts_at: Utc.with_ymd_and_hms(2026, 5, 1, 12, 0, 0).single().unwrap(),
        ends_at: Utc.with_ymd_and_hms(2026, 5, 1, 13, 0, 0).single().unwrap(),
        ..CreateEventInput::default()
    };
    let (account, event, operation, artifacts) = manager.create_event(&resources, &input).unwrap();
    assert_eq!(account.integration_id, "cal_1");
    assert_eq!(operation.status, OperationStatus::Completed);
    assert_eq!(operation.external_event_id, event.external_event_id);
    assert_eq!(artifacts.len(), 1);

    let ops = manager.list_operations(&OperationFilter::default());
    assert_eq!(ops.len(), 1);
    assert_eq!(
        ops[0].operation_class,
        kura_calendar::OperationClass::CreateEvent
    );

    let list_input = ListEventsInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        ..ListEventsInput::default()
    };
    let (_, events, _, _) = manager.list_events(&resources, &list_input).unwrap();
    assert_eq!(events.len(), 2); // seed + created
}

#[test]
fn create_event_rejects_invalid_time_range() {
    let manager = Manager::new("test");
    let resources = vec![test_resource("cal_1", "test", true)];
    let input = CreateEventInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        starts_at: Utc.with_ymd_and_hms(2026, 5, 1, 13, 0, 0).single().unwrap(),
        ends_at: Utc.with_ymd_and_hms(2026, 5, 1, 12, 0, 0).single().unwrap(),
        ..CreateEventInput::default()
    };
    let err = manager.create_event(&resources, &input).unwrap_err();
    assert!(matches!(err, CalendarError::CalendarInvalidTimeRange));
}

#[test]
fn get_event_missing_records_failed_operation() {
    let manager = Manager::new("test");
    let resources = vec![test_resource("cal_1", "test", true)];
    let input = GetEventInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        external_event_id: "nope".to_string(),
        ..GetEventInput::default()
    };
    let err = manager.get_event(&resources, &input).unwrap_err();
    assert!(matches!(err, CalendarError::CalendarEventNotFound));

    let ops = manager.list_operations(&OperationFilter::default());
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].status, OperationStatus::Failed);
    assert_eq!(ops[0].failure_class, "not_found");
    assert!(ops[0].diagnostic_failure.is_some());
}

#[test]
fn busy_free_reports_seed_conflict() {
    let manager = Manager::new("test");
    let resources = vec![test_resource("cal_1", "test", true)];
    let input = BusyFreeInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        window_start: Utc
            .with_ymd_and_hms(2026, 4, 23, 16, 0, 0)
            .single()
            .unwrap(),
        window_end: Utc
            .with_ymd_and_hms(2026, 4, 23, 17, 0, 0)
            .single()
            .unwrap(),
        ..BusyFreeInput::default()
    };
    let (_, query, operation, _) = manager.busy_free(&resources, &input).unwrap();
    assert_eq!(query.conflict_count, 1);
    assert_eq!(query.query_id, operation.operation_id);
}

#[test]
fn cancel_event_marks_cancelled() {
    let manager = Manager::new("test");
    let resources = vec![test_resource("cal_1", "test", true)];
    let input = CancelEventInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        external_event_id: "fake_event_seed".to_string(),
        ..CancelEventInput::default()
    };
    let (_, event, _, _) = manager.cancel_event(&resources, &input).unwrap();
    assert_eq!(event.lifecycle_state, EventLifecycleState::Cancelled);
}

#[test]
fn update_attendees_adds_and_notifies() {
    let manager = Manager::new("test");
    let resources = vec![test_resource("cal_1", "test", true)];
    let input = UpdateAttendeesInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        external_event_id: "fake_event_seed".to_string(),
        add_attendees: vec![AttendeeRequest {
            email: "x@y.com".to_string(),
            ..AttendeeRequest::default()
        }],
        notify: true,
        ..UpdateAttendeesInput::default()
    };
    let (_, event, operation, _) = manager.update_attendees(&resources, &input).unwrap();
    assert_eq!(event.attendees, vec!["x@y.com"]);
    let outcome = operation.attendee_outcome.expect("outcome");
    assert_eq!(outcome.notification_behavior, NotificationBehavior::Notify);
}

#[test]
fn update_recurring_event_requires_scope() {
    let manager = Manager::new("test");
    let resources = vec![test_resource("cal_1", "test", true)];
    let create = CreateEventInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        title: "Standup".to_string(),
        starts_at: Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).single().unwrap(),
        ends_at: Utc.with_ymd_and_hms(2026, 5, 1, 9, 30, 0).single().unwrap(),
        recurring: true,
        recurrence_rule: "FREQ=DAILY".to_string(),
        ..CreateEventInput::default()
    };
    let (_, created, _, _) = manager.create_event(&resources, &create).unwrap();
    assert!(created.recurring);

    let update = UpdateEventInput {
        selection: Selection {
            integration_id: "cal_1".to_string(),
        },
        external_event_id: created.external_event_id,
        title: "Standup (moved)".to_string(),
        starts_at: Utc.with_ymd_and_hms(2026, 5, 2, 9, 0, 0).single().unwrap(),
        ends_at: Utc.with_ymd_and_hms(2026, 5, 2, 9, 30, 0).single().unwrap(),
        recurring: true,
        recurrence_rule: "FREQ=DAILY".to_string(),
        recurrence_scope: RecurrenceScope::Unspecified,
        ..UpdateEventInput::default()
    };
    let err = manager.update_event(&resources, &update).unwrap_err();
    assert!(matches!(
        err,
        CalendarError::CalendarRecurrenceScopeRequired
    ));
}
