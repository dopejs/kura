//! Manager-behavior tests for kura-reminders, ported from the Go
//! daemon/internal/reminders/manager_test.go: tick-driven due occurrence creation with
//! delivery outcome linkage, recurring missed/acknowledged history, workflow-linked
//! acknowledge-on-success / stay-due-on-failure, follow-up link staleness refresh, and
//! the performance smoke.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use common::{FakeWorkflowLauncher, HarnessOptions, TEST_TENANT_ID, harness};
use kura_delivery::OutcomeFilter;
use kura_integrations::DiagnosticReasonCode;
use kura_reminders::{
    ActionKind, BehaviorMode, Clock, CreateInput, FollowUpLink, FollowUpLinkKind, State,
    TransitionInput, WorkflowLaunchConfig, WorkflowLaunchResult,
};
use kura_router::{Session, SessionKind, SessionStatus};
use kura_runtime::{Run, RunStatus};
use kura_scheduler::{Trigger, TriggerKind};

fn dt(s: &str) -> DateTime<Utc> {
    s.parse().expect("valid rfc3339 timestamp")
}

fn once_trigger(fire_at: DateTime<Utc>) -> Trigger {
    Trigger {
        kind: TriggerKind::Once,
        fire_at: Some(fire_at),
        ..Trigger::default()
    }
}

fn minute_cron() -> Trigger {
    Trigger {
        kind: TriggerKind::Cron,
        cron_expr: "*/1 * * * *".to_string(),
        timezone: "UTC".to_string(),
        ..Trigger::default()
    }
}

#[test]
fn tick_creates_due_occurrence_and_links_delivery_outcome() {
    let h = harness(HarnessOptions::default());
    let due_at = dt("2026-04-23T10:05:00Z");

    h.clock.set(due_at - chrono::Duration::minutes(1));
    let reminder = h
        .manager
        .create(&CreateInput {
            title: "Send digest review".to_string(),
            behavior_mode: BehaviorMode::NotifyOnly,
            trigger: once_trigger(due_at),
            ..CreateInput::default()
        })
        .unwrap();

    h.clock.set(due_at);
    h.manager.tick().unwrap();

    let (updated, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok, "expected reminder to exist after tick");
    assert_eq!(
        updated.current_state,
        State::Due,
        "expected reminder state due: {updated:?}"
    );
    assert!(
        !updated.active_occurrence_id.is_empty(),
        "expected active occurrence id: {updated:?}"
    );

    let (occurrence, ok) = h
        .manager
        .get_occurrence(&updated.active_occurrence_id)
        .unwrap();
    assert!(ok, "expected occurrence to exist");
    assert_eq!(
        occurrence.state,
        State::Due,
        "expected occurrence state due: {occurrence:?}"
    );
    assert!(
        !occurrence.latest_delivery_id.is_empty()
            && occurrence.latest_delivery_status == "delivered",
        "expected delivery linkage on occurrence: {occurrence:?}"
    );

    let outcomes = h
        .delivery
        .list_outcomes(OutcomeFilter {
            source_kind: "reminder_occurrence".to_string(),
            source_id: occurrence.occurrence_id.clone(),
            ..OutcomeFilter::default()
        })
        .unwrap();
    assert_eq!(outcomes.len(), 1, "expected 1 delivery outcome");
    assert_eq!(
        outcomes[0].mode,
        kura_delivery::DeliveryMode::Immediate,
        "expected immediate reminder delivery: {:?}",
        outcomes[0]
    );
    assert!(
        !outcomes[0].chosen_target_id.is_empty(),
        "expected a chosen target: {:?}",
        outcomes[0]
    );

    let actions = h.manager.list_actions(&reminder.reminder_id).unwrap();
    assert_eq!(
        actions.len(),
        3,
        "expected created, due, and delivery_linked actions: {actions:?}"
    );
    let mut counts = HashMap::new();
    for action in &actions {
        *counts.entry(action.action_kind).or_insert(0) += 1;
    }
    assert_eq!(counts.get(&ActionKind::Created), Some(&1));
    assert_eq!(counts.get(&ActionKind::Due), Some(&1));
    assert_eq!(counts.get(&ActionKind::DeliveryLinked), Some(&1));
}

#[test]
fn recurring_reminders_mark_missed_and_preserve_acknowledged_history() {
    let h = harness(HarnessOptions::default());
    let start = dt("2026-04-23T10:00:00Z");

    h.clock.set(start);
    let reminder = h
        .manager
        .create(&CreateInput {
            title: "Recurring follow-up".to_string(),
            behavior_mode: BehaviorMode::NotifyOnly,
            trigger: minute_cron(),
            ..CreateInput::default()
        })
        .unwrap();

    h.clock.set(start + chrono::Duration::minutes(1));
    h.manager.tick().unwrap();
    let (first_reminder, _) = h.manager.get(&reminder.reminder_id).unwrap();
    let first_occurrence_id = first_reminder.active_occurrence_id.clone();
    assert!(!first_occurrence_id.is_empty(), "expected first occurrence");

    h.clock.set(start + chrono::Duration::minutes(2));
    h.manager.tick().unwrap();
    h.manager.tick().unwrap();

    let (first_occurrence, ok) = h.manager.get_occurrence(&first_occurrence_id).unwrap();
    assert!(ok, "expected first occurrence to exist");
    assert_eq!(
        first_occurrence.state,
        State::Missed,
        "expected first occurrence missed: {first_occurrence:?}"
    );

    let (second_reminder, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok, "expected reminder to exist");
    let second_occurrence_id = second_reminder.active_occurrence_id.clone();
    assert!(
        !second_occurrence_id.is_empty() && second_occurrence_id != first_occurrence_id,
        "expected rollover to a new occurrence: {second_reminder:?}"
    );

    let (_, second_occurrence, _) = h
        .manager
        .acknowledge(
            &reminder.reminder_id,
            &TransitionInput {
                occurrence_id: second_occurrence_id.clone(),
                actor_kind: kura_reminders::ActorKind::User,
                reason: "seen".to_string(),
                ..TransitionInput::default()
            },
        )
        .unwrap();
    assert_eq!(
        second_occurrence.state,
        State::Acknowledged,
        "expected second occurrence acknowledged"
    );

    h.clock.set(start + chrono::Duration::minutes(3));
    h.manager.tick().unwrap();
    let (final_reminder, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok, "expected reminder to exist");
    assert!(
        !final_reminder.active_occurrence_id.is_empty()
            && final_reminder.active_occurrence_id != second_occurrence_id,
        "expected new active occurrence after acknowledged history: {final_reminder:?}"
    );
    let (third_occurrence, ok) = h
        .manager
        .get_occurrence(&final_reminder.active_occurrence_id)
        .unwrap();
    assert!(ok, "expected third occurrence to exist");
    assert_eq!(
        third_occurrence.state,
        State::Due,
        "expected third occurrence due: {third_occurrence:?}"
    );

    let (preserved_second, ok) = h.manager.get_occurrence(&second_occurrence_id).unwrap();
    assert!(ok, "expected acknowledged history occurrence to exist");
    assert_eq!(
        preserved_second.state,
        State::Acknowledged,
        "expected acknowledged history preserved: {preserved_second:?}"
    );
}

#[test]
fn workflow_linked_reminder_acknowledges_on_success_and_stays_due_on_failure() {
    let success = harness(HarnessOptions {
        workflow_launcher: Some(Arc::new(FakeWorkflowLauncher::ok(WorkflowLaunchResult {
            run_id: "run_reminder_1".to_string(),
            workflow_id: "wf_reminder_1".to_string(),
        }))),
        ..HarnessOptions::default()
    });
    let due_at = dt("2026-04-23T11:00:00Z");
    success.clock.set(due_at - chrono::Duration::minutes(1));
    let success_reminder = success
        .manager
        .create(&CreateInput {
            title: "Launch follow-up workflow".to_string(),
            behavior_mode: BehaviorMode::LaunchWorkflow,
            trigger: once_trigger(due_at),
            workflow_launch_config: Some(WorkflowLaunchConfig {
                entrypoint: "operator".to_string(),
                ..WorkflowLaunchConfig::default()
            }),
            ..CreateInput::default()
        })
        .unwrap();
    success.clock.set(due_at);
    success.manager.tick().unwrap();
    let (success_current, ok) = success.manager.get(&success_reminder.reminder_id).unwrap();
    assert!(ok, "expected success reminder to exist");
    assert_eq!(
        success_current.current_state,
        State::Acknowledged,
        "expected acknowledged reminder after workflow launch: {success_current:?}"
    );
    let (success_occurrence, ok) = success
        .manager
        .get_occurrence(&success_current.active_occurrence_id)
        .unwrap();
    assert!(ok, "expected success occurrence to exist");
    assert_eq!(success_occurrence.state, State::Acknowledged);
    assert_eq!(success_occurrence.run_id, "run_reminder_1");
    assert_eq!(success_occurrence.workflow_id, "wf_reminder_1");

    let failure = harness(HarnessOptions {
        workflow_launcher: Some(Arc::new(FakeWorkflowLauncher::failing("launch failed"))),
        ..HarnessOptions::default()
    });
    failure.clock.set(due_at - chrono::Duration::minutes(1));
    let failure_reminder = failure
        .manager
        .create(&CreateInput {
            title: "Fail workflow launch".to_string(),
            behavior_mode: BehaviorMode::LaunchWorkflow,
            trigger: once_trigger(due_at),
            workflow_launch_config: Some(WorkflowLaunchConfig {
                entrypoint: "operator".to_string(),
                ..WorkflowLaunchConfig::default()
            }),
            ..CreateInput::default()
        })
        .unwrap();
    failure.clock.set(due_at);
    failure.manager.tick().unwrap();
    let (failure_current, ok) = failure.manager.get(&failure_reminder.reminder_id).unwrap();
    assert!(ok, "expected failure reminder to exist");
    let (failure_occurrence, ok) = failure
        .manager
        .get_occurrence(&failure_current.active_occurrence_id)
        .unwrap();
    assert!(ok, "expected failure occurrence to exist");
    assert_eq!(
        failure_occurrence.state,
        State::Due,
        "expected occurrence to remain due after launch failure: {failure_occurrence:?}"
    );

    failure
        .clock
        .set(due_at + chrono::Duration::milliseconds(20));
    failure.manager.tick().unwrap();
    let (failure_occurrence, ok) = failure
        .manager
        .get_occurrence(&failure_current.active_occurrence_id)
        .unwrap();
    assert!(ok, "expected failure occurrence to exist");
    assert_eq!(
        failure_occurrence.state,
        State::Overdue,
        "expected occurrence overdue after unhandled launch failure: {failure_occurrence:?}"
    );
}

#[test]
fn refreshes_follow_up_link_staleness() {
    let h = harness(HarnessOptions::default());
    let now = h.clock.now();

    let session = Session {
        session_id: "session_existing".to_string(),
        kind: SessionKind::Direct,
        status: SessionStatus::Active,
        channel: "local".to_string(),
        account_id: "local".to_string(),
        peer_id: "chat".to_string(),
        thread_id: String::new(),
        routing_key: "direct:local:local:chat".to_string(),
        generation: 1,
        created_at: now,
        updated_at: now,
        last_active_at: now,
        last_reset_at: None,
        active_profile_projection: None,
    };
    h.store
        .lock()
        .upsert_session_for_tenant_safe(&session, TEST_TENANT_ID)
        .unwrap();

    let run = Run {
        run_id: "run_existing".to_string(),
        session_id: session.session_id.clone(),
        entrypoint: "operator".to_string(),
        status: RunStatus::Completed,
        goal: "existing work".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    };
    h.store
        .lock()
        .upsert_run_for_tenant_safe(&run, TEST_TENANT_ID)
        .unwrap();

    let existing_run_reminder = h
        .manager
        .create(&CreateInput {
            title: "Follow up existing run".to_string(),
            behavior_mode: BehaviorMode::NotifyOnly,
            trigger: once_trigger(now + chrono::Duration::hours(1)),
            follow_up_link: Some(FollowUpLink {
                link_kind: FollowUpLinkKind::Run,
                source_id: run.run_id.clone(),
                ..FollowUpLink::default()
            }),
            ..CreateInput::default()
        })
        .unwrap();
    let (refreshed_run_reminder, ok) = h.manager.get(&existing_run_reminder.reminder_id).unwrap();
    assert!(ok, "expected run reminder to exist");
    let link = refreshed_run_reminder
        .follow_up_link
        .as_ref()
        .expect("expected follow-up link on reminder");
    assert!(
        !link.stale,
        "expected existing run link to stay fresh: {link:?}"
    );

    let missing_workflow_reminder = h
        .manager
        .create(&CreateInput {
            title: "Follow up missing workflow".to_string(),
            behavior_mode: BehaviorMode::NotifyOnly,
            trigger: once_trigger(now + chrono::Duration::hours(2)),
            follow_up_link: Some(FollowUpLink {
                link_kind: FollowUpLinkKind::Workflow,
                source_id: "wf_missing".to_string(),
                ..FollowUpLink::default()
            }),
            ..CreateInput::default()
        })
        .unwrap();
    let (refreshed_workflow_reminder, ok) = h
        .manager
        .get(&missing_workflow_reminder.reminder_id)
        .unwrap();
    assert!(ok, "expected workflow reminder to exist");
    let link = refreshed_workflow_reminder
        .follow_up_link
        .as_ref()
        .expect("expected follow-up link on reminder");
    assert!(
        link.stale,
        "expected missing workflow link to be stale: {link:?}"
    );
    assert_eq!(link.source_display_state, "stale");
    assert!(link.last_checked_at.is_some(), "expected lastCheckedAt set");
    let diagnostic = link
        .diagnostic_failure
        .as_ref()
        .expect("expected stale follow-up diagnostic projection");
    assert_eq!(
        diagnostic.reason_code,
        DiagnosticReasonCode::OperatorActionNeeded
    );
}

#[test]
fn performance_smoke() {
    let h = harness(HarnessOptions::default());
    let base = dt("2026-04-23T13:00:00Z");
    let due_at = base + chrono::Duration::minutes(1);
    h.clock.set(base);

    for _ in 0..100 {
        h.manager
            .create(&CreateInput {
                title: "Perf reminder".to_string(),
                behavior_mode: BehaviorMode::NotifyOnly,
                trigger: once_trigger(due_at),
                ..CreateInput::default()
            })
            .unwrap();
    }

    // Shared CI runners run debug builds noticeably slower than a dev
    // machine; scale the smoke bounds there so the gate catches order-of-
    // magnitude regressions without flaking on scheduler jitter.
    let bound_scale: u32 = if std::env::var_os("CI").is_some() {
        10
    } else {
        1
    };

    let list_started = Instant::now();
    let items = h.manager.list().unwrap();
    let list_elapsed = list_started.elapsed();
    assert_eq!(items.len(), 100, "expected 100 reminders");
    assert!(
        list_elapsed < std::time::Duration::from_millis(500) * bound_scale,
        "expected reminder inspect smoke under 500ms x{bound_scale}, got {list_elapsed:?}"
    );

    h.clock.set(due_at);
    let tick_started = Instant::now();
    h.manager.tick().unwrap();
    let tick_elapsed = tick_started.elapsed();
    assert!(
        tick_elapsed < std::time::Duration::from_secs(1) * bound_scale,
        "expected due detection smoke under 1s x{bound_scale}, got {tick_elapsed:?}"
    );

    let (first, ok) = h.manager.get(&items[0].reminder_id).unwrap();
    assert!(ok, "expected first reminder to exist");
    let occurrence_started = Instant::now();
    let (occurrence, ok) = h
        .manager
        .get_occurrence(&first.active_occurrence_id)
        .unwrap();
    let occurrence_elapsed = occurrence_started.elapsed();
    assert!(ok, "expected occurrence to exist");
    assert!(
        !occurrence.latest_delivery_id.is_empty(),
        "expected delivery linkage on occurrence: {occurrence:?}"
    );
    assert!(
        occurrence_elapsed < std::time::Duration::from_millis(500),
        "expected occurrence projection smoke under 500ms, got {occurrence_elapsed:?}"
    );

    let ack_started = Instant::now();
    let (_, acknowledged, _) = h
        .manager
        .acknowledge(
            &first.reminder_id,
            &TransitionInput {
                occurrence_id: first.active_occurrence_id.clone(),
                ..TransitionInput::default()
            },
        )
        .unwrap();
    let ack_elapsed = ack_started.elapsed();
    assert_eq!(acknowledged.state, State::Acknowledged);
    assert!(
        ack_elapsed < std::time::Duration::from_secs(1),
        "expected occurrence transition persistence smoke under 1s, got {ack_elapsed:?}"
    );
    eprintln!(
        "reminder performance smoke: inspect={list_elapsed:?} due_tick={tick_elapsed:?} occurrence_projection={occurrence_elapsed:?} acknowledge={ack_elapsed:?}"
    );
}

#[test]
fn start_close_lifecycle_is_idempotent() {
    let h = harness(HarnessOptions::default());
    h.manager.start().unwrap();
    h.manager.start().unwrap(); // second start is a no-op
    h.manager.close();
    h.manager.close(); // close after close is a no-op
}

#[test]
fn cancel_clears_schedule_and_preserves_occurrence_history() {
    let h = harness(HarnessOptions::default());
    let due_at = dt("2026-04-23T12:00:00Z");
    h.clock.set(due_at - chrono::Duration::minutes(1));
    let reminder = h
        .manager
        .create(&CreateInput {
            title: "Cancel me".to_string(),
            behavior_mode: BehaviorMode::NotifyOnly,
            trigger: once_trigger(due_at),
            ..CreateInput::default()
        })
        .unwrap();
    h.clock.set(due_at);
    h.manager.tick().unwrap();
    let (due, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok);
    let occurrence_id = due.active_occurrence_id.clone();

    let (cancelled, occurrence, action) = h
        .manager
        .cancel(
            &reminder.reminder_id,
            &TransitionInput {
                occurrence_id,
                reason: "no longer needed".to_string(),
                ..TransitionInput::default()
            },
        )
        .unwrap();
    assert_eq!(cancelled.current_state, State::Cancelled);
    assert!(cancelled.next_due_at.is_none());
    assert!(cancelled.active_occurrence_id.is_empty());
    assert!(cancelled.cancelled_at.is_some());
    assert_eq!(occurrence.state, State::Cancelled);
    assert_eq!(action.action_kind, ActionKind::Cancelled);

    // A cancelled reminder is a no-op for the tick loop.
    h.manager.tick().unwrap();
    let (after, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok);
    assert_eq!(after.current_state, State::Cancelled);
}

#[test]
fn snooze_requires_snoozed_until_and_reschedules_next_due() {
    let h = harness(HarnessOptions::default());
    let due_at = dt("2026-04-23T12:00:00Z");
    h.clock.set(due_at - chrono::Duration::minutes(1));
    let reminder = h
        .manager
        .create(&CreateInput {
            title: "Snooze me".to_string(),
            behavior_mode: BehaviorMode::NotifyOnly,
            trigger: once_trigger(due_at),
            ..CreateInput::default()
        })
        .unwrap();
    h.clock.set(due_at);
    h.manager.tick().unwrap();
    let (due, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok);
    assert_eq!(due.current_state, State::Due);

    let err = h
        .manager
        .snooze(&reminder.reminder_id, &TransitionInput::default())
        .unwrap_err();
    assert_eq!(err, kura_reminders::ReminderError::SnoozeRequired);

    let until = dt("2026-04-23T15:00:00Z");
    let (snoozed, occurrence, action) = h
        .manager
        .snooze(
            &reminder.reminder_id,
            &TransitionInput {
                occurrence_id: due.active_occurrence_id.clone(),
                snoozed_until: Some(until),
                reason: "later".to_string(),
                ..TransitionInput::default()
            },
        )
        .unwrap();
    assert_eq!(snoozed.current_state, State::Snoozed);
    assert_eq!(snoozed.next_due_at, Some(until));
    assert_eq!(occurrence.state, State::Snoozed);
    assert_eq!(occurrence.snoozed_until, Some(until));
    assert_eq!(action.action_kind, ActionKind::Snoozed);

    // Before the snooze window elapses, tick leaves the occurrence snoozed.
    h.manager.tick().unwrap();
    let (still, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok);
    assert_eq!(still.current_state, State::Snoozed);

    // Once the window elapses, tick makes it due again (and delivers).
    h.clock.set(until + chrono::Duration::seconds(1));
    h.manager.tick().unwrap();
    let (redue, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok);
    assert_eq!(redue.current_state, State::Due);
    let (redue_occ, ok) = h
        .manager
        .get_occurrence(&redue.active_occurrence_id)
        .unwrap();
    assert!(ok);
    assert_eq!(redue_occ.state, State::Due);
    assert!(!redue_occ.latest_delivery_id.is_empty());
}

#[test]
fn reschedule_requires_trigger_and_rearms_reminder() {
    let h = harness(HarnessOptions::default());
    let due_at = dt("2026-04-23T12:00:00Z");
    h.clock.set(due_at - chrono::Duration::minutes(1));
    let reminder = h
        .manager
        .create(&CreateInput {
            title: "Reschedule me".to_string(),
            behavior_mode: BehaviorMode::NotifyOnly,
            trigger: once_trigger(due_at),
            ..CreateInput::default()
        })
        .unwrap();

    let err = h
        .manager
        .reschedule(&reminder.reminder_id, &TransitionInput::default())
        .unwrap_err();
    assert_eq!(err, kura_reminders::ReminderError::InvalidTrigger);

    let later = dt("2026-04-24T09:00:00Z");
    let (rescheduled, _, action) = h
        .manager
        .reschedule(
            &reminder.reminder_id,
            &TransitionInput {
                trigger: Some(once_trigger(later)),
                reason: "move".to_string(),
                ..TransitionInput::default()
            },
        )
        .unwrap();
    assert_eq!(rescheduled.current_state, State::Pending);
    assert_eq!(rescheduled.next_due_at, Some(later));
    assert!(rescheduled.active_occurrence_id.is_empty());
    assert_eq!(action.action_kind, ActionKind::Rescheduled);

    // Nothing is due until the new time.
    h.manager.tick().unwrap();
    let (still, ok) = h.manager.get(&reminder.reminder_id).unwrap();
    assert!(ok);
    assert_eq!(still.current_state, State::Pending);
}
