//! Work items slice 3: the outbox (docs/harness/work-items.md §8, §12).
//!
//! The manager's outbox over a real store, with a reply sender whose
//! failures each test scripts. Retries are made due by sweeping "as of" a
//! later time instead of waiting out the backoff; one test runs the real
//! sweeper thread for the boot sweep.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use kura_events::{Bus, Filter};
use kura_im::ReplySender;
use kura_imtypes::{OutboundReply, SentReply};
use kura_store::{SQLiteStore, StorePool};
use kura_workitems::{
    Manager, OpenRequest, OutboxDropReason, OutboxState, ReplyBinding, RoundOutcome, Stage,
    WorkItem, WorkItemsConfig,
};

/// A platform that fails the calls a test scripts, in order, and accepts
/// the rest.
#[derive(Default)]
struct Platform {
    failures: Mutex<VecDeque<Option<&'static str>>>,
    sent: Mutex<Vec<OutboundReply>>,
}

impl Platform {
    /// The next calls answer these, in order: `Some(error)` fails, `None`
    /// succeeds. Calls after them succeed.
    fn script(&self, answers: &[Option<&'static str>]) {
        self.failures
            .lock()
            .unwrap()
            .extend(answers.iter().copied());
    }

    fn texts(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|reply| reply.content.clone())
            .collect()
    }
}

impl ReplySender for Platform {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        if let Some(Some(error)) = self.failures.lock().unwrap().pop_front() {
            return Err(error.to_string());
        }
        let mut sent = self.sent.lock().unwrap();
        sent.push(reply);
        Ok(SentReply {
            external_message_id: format!("m{}", sent.len()),
        })
    }
}

/// `ReplySender` for an `Arc<Platform>`, so a test keeps its handle.
struct Shared(Arc<Platform>);

impl ReplySender for Shared {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.0.send_reply(reply)
    }
}

fn manager_at(path: &str, bus: &Bus) -> Arc<Manager> {
    let store = SQLiteStore::new(path).expect("store");
    let pool = Arc::new(StorePool::writer_only(Arc::new(parking_lot::Mutex::new(
        store,
    ))));
    Arc::new(Manager::new(
        pool,
        Arc::new(bus.clone()),
        WorkItemsConfig {
            enabled: true,
            ..WorkItemsConfig::default()
        },
    ))
}

/// An idle item: opened, its first round settled as `waiting`.
fn idle_item(manager: &Manager) -> WorkItem {
    let (item, turn) = manager
        .open(OpenRequest {
            tenant_id: "ten_a".to_string(),
            principal_id: "prn_1".to_string(),
            title: "Fix CI".to_string(),
            scope: "Fix CI on main.".to_string(),
            ..OpenRequest::default()
        })
        .expect("open");
    manager
        .settle_round(
            &turn,
            RoundOutcome::Answered {
                structured: Some(serde_json::json!({"status": "waiting", "summary": "s"})),
                turn_id: None,
            },
        )
        .expect("settle");
    item
}

fn binding(key: &str) -> ReplyBinding {
    ReplyBinding {
        conversation_key: key.to_string(),
        connector_id: "slack-main".to_string(),
        conversation_id: "C1".to_string(),
        channel_id: "C1".to_string(),
        peer_id: "C1".to_string(),
        reply_to_external_message_id: "m0".to_string(),
        participation_gated: false,
        source_ref: "delivery_1".to_string(),
    }
}

fn texts(replies: &[&str]) -> Vec<(Option<i64>, String)> {
    replies
        .iter()
        .map(|text| (None, (*text).to_string()))
        .collect()
}

fn states(manager: &Manager, item_id: &str) -> Vec<(i64, OutboxState, i64)> {
    manager
        .get("ten_a", item_id)
        .expect("item")
        .outbox
        .expect("outbox")
        .iter()
        .map(|row| (row.seq, row.state, row.attempts))
        .collect()
}

fn outbox_metric(result: &str) -> u64 {
    kura_telemetry::metrics::registry().counter_value(
        kura_telemetry::metrics::WORK_ITEM_OUTBOX_TOTAL,
        &[("result", result)],
    )
}

/// §12 slice 3: the first send fails; the second waits for it, through
/// every retry, and both arrive in order once the first goes through.
#[test]
fn a_reply_waits_for_the_one_before_it_and_both_arrive_in_order_after_the_retry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bus = Bus::new();
    let manager = manager_at(dir.path().to_str().unwrap(), &bus);
    let item = idle_item(&manager);
    let platform = Arc::new(Platform::default());
    let (retries, sent) = (outbox_metric("retry"), outbox_metric("sent"));

    manager
        .enqueue_replies(
            "ten_a",
            &item.item_id,
            &binding("thr_src_1"),
            &texts(&["first", "second"]),
        )
        .expect("enqueue");
    platform.script(&[Some("slack web api status 503")]);
    let flushed = manager.flush_conversation(
        "ten_a",
        &item.item_id,
        "thr_src_1",
        "slack-main",
        &Shared(platform.clone()),
    );
    assert!(flushed.sent_message_ids.is_empty());
    assert_eq!(flushed.waiting, 2);
    assert!(platform.texts().is_empty(), "the second never overtook");
    assert_eq!(
        states(&manager, &item.item_id),
        vec![(1, OutboxState::Failed, 1), (2, OutboxState::Queued, 0)]
    );
    let read = manager.get("ten_a", &item.item_id).unwrap();
    let head = &read.outbox.as_ref().unwrap()[0];
    assert_eq!(head.last_error.as_deref(), Some("send_failed"));
    assert!(
        head.next_attempt_at.unwrap() > Utc::now(),
        "a persisted backoff"
    );
    assert_eq!(
        read.stage,
        Some(Stage::Idle),
        "one failure is not a problem yet"
    );

    // The sweeper's sender: nothing is due before the backoff ends.
    manager.register_reply_sender("slack-main", Arc::new(Shared(platform.clone())));
    let now = Utc::now();
    let next = manager
        .sweep_outbox(now)
        .expect("the retry is due within the horizon");
    assert!(next > now);
    assert!(platform.texts().is_empty());

    // Two more failures in a row: the item shows person/delivery, and the
    // second still waits.
    platform.script(&[Some("connection reset"), Some("slack web api status 503")]);
    manager.sweep_outbox(now + chrono::Duration::seconds(6));
    manager.sweep_outbox(now + chrono::Duration::seconds(20));
    assert_eq!(
        states(&manager, &item.item_id),
        vec![(1, OutboxState::Failed, 3), (2, OutboxState::Queued, 0)]
    );
    assert_eq!(
        manager.get("ten_a", &item.item_id).unwrap().stage,
        Some(Stage::PersonDelivery)
    );

    // The retry goes through: both, in order, in one sweep.
    manager.sweep_outbox(now + chrono::Duration::seconds(60));
    assert_eq!(platform.texts(), vec!["first", "second"]);
    let read = manager.get("ten_a", &item.item_id).unwrap();
    assert_eq!(read.stage, Some(Stage::Idle));
    let rows = read.outbox.unwrap();
    assert!(rows.iter().all(|row| row.state == OutboxState::Sent));
    assert_eq!(rows[0].external_message_id, "m1");
    assert!(rows[0].next_attempt_at.is_none());
    assert!(outbox_metric("retry") >= retries + 3);
    assert!(outbox_metric("sent") >= sent + 2);
}

#[test]
fn another_conversation_and_another_item_do_not_wait() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bus = Bus::new();
    let manager = manager_at(dir.path().to_str().unwrap(), &bus);
    let item = idle_item(&manager);
    let other = idle_item(&manager);
    let platform = Arc::new(Platform::default());
    manager.register_reply_sender("slack-main", Arc::new(Shared(platform.clone())));
    for (id, key, text) in [
        (&item.item_id, "thr_src_1", "stuck"),
        (&item.item_id, "thr_src_1", "behind it"),
        (&item.item_id, "thr_src_2", "elsewhere"),
        (&other.item_id, "thr_src_1", "other item"),
    ] {
        manager
            .enqueue_replies("ten_a", id, &binding(key), &texts(&[text]))
            .unwrap();
    }
    // Only the very first send fails.
    platform.script(&[Some("connection reset")]);
    manager.sweep_outbox(Utc::now());
    let mut sent = platform.texts();
    sent.sort();
    assert_eq!(sent, vec!["elsewhere", "other item"]);
}

/// §12 slice 3: a conversation the platform says is gone drops its row; the
/// row after it goes on, and the drop is announced without the body.
#[test]
fn a_gone_conversation_drops_the_row_and_unblocks_the_next() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bus = Bus::new();
    let manager = manager_at(dir.path().to_str().unwrap(), &bus);
    let item = idle_item(&manager);
    let (events, _unsubscribe) = bus.subscribe(Filter {
        category: "work_item".to_string(),
        ..Filter::default()
    });
    let platform = Arc::new(Platform::default());
    manager.register_reply_sender("slack-main", Arc::new(Shared(platform.clone())));
    let dropped_before = outbox_metric("dropped");

    manager
        .enqueue_replies(
            "ten_a",
            &item.item_id,
            &binding("thr_src_1"),
            &texts(&["secret plan", "after it"]),
        )
        .unwrap();
    platform.script(&[Some("slack web api error: channel_not_found")]);
    manager.sweep_outbox(Utc::now());

    assert_eq!(platform.texts(), vec!["after it"]);
    let rows = manager.get("ten_a", &item.item_id).unwrap().outbox.unwrap();
    assert_eq!(rows[0].state, OutboxState::Dropped);
    assert_eq!(
        rows[0].drop_reason,
        Some(OutboxDropReason::ConversationGone)
    );
    assert!(rows[0].dropped_at.is_some());
    assert_eq!(rows[1].state, OutboxState::Sent);
    assert!(outbox_metric("dropped") > dropped_before);
    let drop = std::iter::from_fn(|| events.try_recv().ok())
        .find(|event| event.name == "work_item.outbox_dropped")
        .expect("outbox_dropped event");
    assert_eq!(drop.tenant_id, "ten_a");
    assert_eq!(drop.payload["outboxSeq"], 1);
    assert_eq!(drop.payload["reason"], "conversation_gone");
    assert_eq!(drop.payload["itemId"], item.item_id.as_str());
    assert!(
        !serde_json::to_string(&drop.payload)
            .unwrap()
            .contains("secret"),
        "the event carried a body"
    );

    // A gated conversation the route policy no longer receives is
    // connector_gone, checked before the platform is even asked.
    let gated = ReplyBinding {
        participation_gated: true,
        ..binding("thr_src_9")
    };
    manager
        .enqueue_replies(
            "ten_a",
            &item.item_id,
            &gated,
            &texts(&["to a closed room"]),
        )
        .unwrap();
    manager.sweep_outbox(Utc::now());
    let rows = manager.get("ten_a", &item.item_id).unwrap().outbox.unwrap();
    assert_eq!(rows[2].drop_reason, Some(OutboxDropReason::ConnectorGone));
    assert_eq!(platform.texts(), vec!["after it"], "nothing more was sent");
}

/// §12 slice 3: the daemon stops mid-retry. A new process over the same
/// database (a fresh manager, its own sweeper) resumes: its boot sweep
/// sends what was queued at once, and the retry when it is due.
#[test]
fn a_restart_mid_retry_resumes_from_the_stored_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let item_id = {
        let bus = Bus::new();
        let before = manager_at(&path, &bus);
        let item = idle_item(&before);
        before
            .enqueue_replies(
                "ten_a",
                &item.item_id,
                &binding("thr_src_1"),
                &texts(&["retried"]),
            )
            .unwrap();
        let platform = Arc::new(Platform::default());
        platform.script(&[Some("slack web api status 502")]);
        before.flush_conversation(
            "ten_a",
            &item.item_id,
            "thr_src_1",
            "slack-main",
            &Shared(platform),
        );
        // Queued, never tried: the process stopped before sending it.
        before
            .enqueue_replies(
                "ten_a",
                &item.item_id,
                &binding("thr_src_2"),
                &texts(&["never tried"]),
            )
            .unwrap();
        item.item_id
    };

    let bus = Bus::new();
    let after = manager_at(&path, &bus);
    assert_eq!(
        states(&after, &item_id),
        vec![(1, OutboxState::Failed, 1), (2, OutboxState::Queued, 0)],
        "the rows and their backoff survived"
    );
    let platform = Arc::new(Platform::default());
    after.register_reply_sender("slack-main", Arc::new(Shared(platform.clone())));
    after.start_outbox_sweeper().expect("sweeper");
    let deadline = Instant::now() + Duration::from_secs(10);
    while platform.texts().is_empty() {
        assert!(Instant::now() < deadline, "the boot sweep sent nothing");
        std::thread::sleep(Duration::from_millis(10));
    }
    after.stop_outbox_sweeper();
    assert_eq!(
        platform.texts(),
        vec!["never tried"],
        "the retry waits its backoff"
    );

    after.sweep_outbox(Utc::now() + chrono::Duration::seconds(10));
    assert_eq!(platform.texts(), vec!["never tried", "retried"]);
    assert!(
        states(&after, &item_id)
            .iter()
            .all(|(_, state, _)| *state == OutboxState::Sent)
    );
}

#[test]
fn a_reply_with_no_sender_waits_and_another_tenant_cannot_touch_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bus = Bus::new();
    let manager = manager_at(dir.path().to_str().unwrap(), &bus);
    let item = idle_item(&manager);
    manager
        .enqueue_replies(
            "ten_a",
            &item.item_id,
            &binding("thr_src_1"),
            &texts(&["hello"]),
        )
        .unwrap();
    // No connector runtime registered a sender: the row waits, untried.
    assert_eq!(manager.sweep_outbox(Utc::now()), None);
    assert_eq!(
        states(&manager, &item.item_id),
        vec![(1, OutboxState::Queued, 0)]
    );
    assert!(manager.withdraw_outbox("ten_b", &item.item_id, 1).is_err());
    assert!(manager.get("ten_b", &item.item_id).is_err());
    let withdrawn = manager.withdraw_outbox("ten_a", &item.item_id, 1).unwrap();
    let row = &withdrawn.outbox.unwrap()[0];
    assert_eq!(row.drop_reason, Some(OutboxDropReason::Withdrawn));
}
