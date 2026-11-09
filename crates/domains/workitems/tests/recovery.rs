//! Work items slice 4: restart recovery (docs/harness/work-items.md §6,
//! §12).
//!
//! A "process" here is a chat service with its own store connection and a
//! manager over one database file. The first process is killed by leaving
//! its round's turn stuck forever inside a side-effecting tool call, after
//! the turn's checkpoint recorded the call as started; a second process,
//! a fresh manager and chat service over the same file, then runs the boot
//! recovery. A scripted provider records what each model call was asked.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::future::BoxFuture;
use kura_chat::{Service, ToolSource, ToolTurn};
use kura_events::Bus;
use kura_im::ReplySender;
use kura_imtypes::{OutboundReply, SentReply};
use kura_llm::{
    Dispatcher, MessageRole, Provider, ProviderError, ProviderRequest, ProviderResponse,
    StreamEmitter, ToolCall, ToolSpec,
};
use kura_store::work_items::WorkItemRoundRecord;
use kura_store::{SQLiteStore, StorePool};
use kura_workitems::{
    EndReason, InputKind, InputState, ItemState, Manager, NotRerun, OpenRequest, Recovery,
    RecoveryMode, ReplyBinding, RoundOutcome, RoundState, Stage, WorkItem, WorkItemsConfig,
};

const PROVIDER: &str = "scripted";
const TOOL: &str = "send_mail";

// ---------------------------------------------------------------------------
// The scripted model
// ---------------------------------------------------------------------------

/// One model call, reduced to what the tests look at.
#[derive(Debug, Clone)]
struct Seen {
    system: String,
    /// Every tool result the call was shown, in order.
    tool_results: Vec<String>,
}

/// In a round's first model call the model asks for `send_mail`; once it
/// has been shown a tool result it answers its report.
#[derive(Default)]
struct Script {
    seen: Mutex<Vec<Seen>>,
    /// Each round model call waits this long first, so racing scans
    /// overlap.
    delay_ms: AtomicUsize,
}

impl Script {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

struct ScriptedProvider(Arc<Script>);

impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        PROVIDER
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            let system = request
                .messages
                .first()
                .filter(|m| m.role == MessageRole::System)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            let tool_results: Vec<String> = request
                .messages
                .iter()
                .filter(|m| m.role == MessageRole::Tool)
                .map(|m| m.content.clone())
                .collect();
            self.0.seen.lock().unwrap().push(Seen {
                system,
                tool_results: tool_results.clone(),
            });
            let delay = self.0.delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay as u64)).await;
            }
            if tool_results.is_empty() && request.tools.iter().any(|t| t.name == TOOL) {
                return Ok(ProviderResponse {
                    tool_calls: vec![ToolCall {
                        call_id: "call_mail".to_string(),
                        name: TOOL.to_string(),
                        arguments: r#"{"to":"ops@example.com"}"#.to_string(),
                    }],
                    finish_reason: "tool_calls".to_string(),
                    ..ProviderResponse::default()
                });
            }
            Ok(ProviderResponse {
                output: r#"{"status":"waiting","summary":"Checked the mail; waiting for CI."}"#
                    .to_string(),
                finish_reason: "stop".to_string(),
                ..ProviderResponse::default()
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        _emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.complete(request)
    }
}

/// A tool with an outside effect (never parallel-safe). In the process
/// that is killed it never returns.
struct SendMail {
    hang: bool,
    calls: Arc<AtomicUsize>,
}

impl kura_core::Tool for SendMail {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TOOL.to_string(),
            description: "Send a mail.".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(
        &'a self,
        _invocation: &'a kura_core::ToolInvocation,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<kura_core::ToolOutput, kura_core::ToolError>>
                + Send
                + 'a,
        >,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let hang = self.hang;
        Box::pin(async move {
            if hang {
                std::future::pending::<()>().await;
            }
            Ok(kura_core::ToolOutput::ok("sent"))
        })
    }
}

struct Tools(Arc<SendMail>);

impl ToolSource for Tools {
    fn registry(&self, _turn: &ToolTurn) -> Arc<kura_core::ToolRegistry> {
        let mut registry = kura_core::ToolRegistry::new();
        registry.register(self.0.clone());
        Arc::new(registry)
    }
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

struct Process {
    manager: Arc<Manager>,
    script: Arc<Script>,
    tool_calls: Arc<AtomicUsize>,
}

/// A daemon over the database at `path`: a manager and a chat service with
/// a store of its own (so turns leave checkpoints), whose `send_mail`
/// hangs when `hang`.
fn process(path: &str, config: WorkItemsConfig, hang: bool) -> Process {
    let script = Arc::new(Script::default());
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(Arc::new(ScriptedProvider(script.clone())));
    dispatcher
        .set_default_provider(PROVIDER)
        .expect("default provider");
    dispatcher.set_default_model("scripted-v1");
    let chat_store: Arc<dyn kura_chat::ChatStore> = Arc::new(std::sync::Mutex::new(
        SQLiteStore::new(path).expect("chat store"),
    ));
    let mut chat = Service::new_service(dispatcher, None, None, None, Some(chat_store));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    chat.set_tools(Arc::new(Tools(Arc::new(SendMail {
        hang,
        calls: tool_calls.clone(),
    }))));
    let pool = Arc::new(StorePool::writer_only(Arc::new(parking_lot::Mutex::new(
        SQLiteStore::new(path).expect("manager store"),
    ))));
    let manager =
        Arc::new(Manager::new(pool, Arc::new(Bus::new()), config).with_chat(Arc::new(chat)));
    Process {
        manager,
        script,
        tool_calls,
    }
}

fn enabled() -> WorkItemsConfig {
    WorkItemsConfig {
        enabled: true,
        ..WorkItemsConfig::default()
    }
}

fn open(manager: &Manager) -> (WorkItem, kura_workitems::RoundTurn) {
    manager
        .open(OpenRequest {
            tenant_id: "ten_a".to_string(),
            principal_id: "prn_1".to_string(),
            title: "Fix CI".to_string(),
            scope: "Fix CI on main and mail ops when it is green.".to_string(),
            ..OpenRequest::default()
        })
        .expect("open")
}

#[derive(Default)]
struct Platform(Mutex<Vec<String>>);

impl ReplySender for Platform {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        let mut sent = self.0.lock().unwrap();
        sent.push(reply.content);
        Ok(SentReply {
            external_message_id: format!("m{}", sent.len()),
        })
    }
}

/// The item says something to a conversation, and it is sent.
fn sent_reply(manager: &Manager, item_id: &str, text: &str) {
    let binding = ReplyBinding {
        conversation_key: "thr_src_1".to_string(),
        connector_id: "slack-main".to_string(),
        conversation_id: "C1".to_string(),
        channel_id: "C1".to_string(),
        peer_id: "C1".to_string(),
        reply_to_external_message_id: "m0".to_string(),
        participation_gated: false,
        source_ref: "delivery_1".to_string(),
    };
    manager
        .enqueue_replies("ten_a", item_id, &binding, &[(Some(1), text.to_string())])
        .expect("enqueue");
    let flushed = manager.flush_conversation(
        "ten_a",
        item_id,
        "thr_src_1",
        "slack-main",
        &Platform::default(),
    );
    assert_eq!(flushed.sent_message_ids.len(), 1);
}

/// Runs the item's first round in `killed` and leaves it stuck inside
/// `send_mail`, its checkpoint saying the call started: the process died
/// there. Returns once the call has started.
fn kill_during_the_mail(killed: &Process, turn: kura_workitems::RoundTurn) {
    let manager = killed.manager.clone();
    std::thread::spawn(move || {
        let admitted = Service::admit(
            &Service::new_service(Arc::new(Dispatcher::new()), None, None, None, None),
            "ten_a",
        )
        .expect("no gate");
        manager.run_round(&turn, admitted);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while killed.tool_calls.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "the round never called send_mail"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn recovery_inputs(item: &WorkItem) -> Vec<kura_workitems::Input> {
    item.inputs
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|input| input.kind == InputKind::Recovery)
        .collect()
}

fn recovered_metric(mode: &str) -> u64 {
    kura_telemetry::metrics::registry().counter_value(
        kura_telemetry::metrics::WORK_ITEM_RECOVERED_TOTAL,
        &[("mode", mode)],
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// §12 slice 4: killed during a side-effecting call, the round is rerun at
/// boot under its own request id; it resumes from its checkpoint, so the
/// model reads `INTERRUPTED_CALL_OUTPUT` for the call instead of the call
/// running again, and the recovery note quotes what the item already sent.
#[test]
fn a_round_killed_during_a_side_effecting_call_resumes_and_reads_the_interruption_and_the_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let killed = process(&path, enabled(), true);
    let (item, turn) = open(&killed.manager);
    sent_reply(
        &killed.manager,
        &item.item_id,
        "Found the flaky test; mailing ops.",
    );
    kill_during_the_mail(&killed, turn.clone());

    let resumed_before = recovered_metric("resumed");
    let booted = process(&path, enabled(), false);
    let recovered = booted.manager.recover_interrupted_rounds();
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].item_id, item.item_id);
    assert_eq!(recovered[0].seq, 1);
    let Recovery::Rerun { mode, round } = &recovered[0].recovery else {
        panic!("not rerun: {recovered:?}");
    };
    assert_eq!(*mode, RecoveryMode::Resumed);
    assert_eq!(
        round.report.as_ref().map(|r| r.summary.as_str()),
        Some("Checked the mail; waiting for CI.")
    );
    assert!(recovered_metric("resumed") > resumed_before);

    // The mail was not sent again: the call is reported as interrupted.
    assert_eq!(booted.tool_calls.load(Ordering::SeqCst), 0);
    let seen = booted.script.seen();
    assert_eq!(seen.len(), 1, "one model call: the report");
    assert_eq!(seen[0].tool_results.len(), 1);
    // Framed as tool output, as every result is.
    assert!(
        seen[0].tool_results[0].contains(kura_core::INTERRUPTED_CALL_OUTPUT),
        "{:?}",
        seen[0].tool_results
    );
    // The note, after the mandate, with the outbound log quoted.
    let system = &seen[0].system;
    let mandate = system.find("Fix CI on main").expect("the mandate");
    let note = system.find("Recovery note").expect("the recovery note");
    assert!(mandate < note);
    assert!(system.contains("the same round, resumed"));
    assert!(system.contains(
        "<work_item_outbound seq=\"1\" state=\"sent\">\nFound the flaky test; mailing ops.\n"
    ));

    let read = booted.manager.get("ten_a", &item.item_id).unwrap();
    let rounds = read.rounds.clone().unwrap();
    assert_eq!(rounds.len(), 1, "the same round, not a new one");
    assert_eq!(rounds[0].request_id, turn.request_id);
    assert_eq!(rounds[0].query_text, turn.query_text);
    assert_eq!(rounds[0].state, RoundState::Settled);
    assert_eq!(read.stage, Some(Stage::Idle));
    let notes = recovery_inputs(&read);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].state, InputState::Consumed);
    assert_eq!(notes[0].round_seq, Some(1));
    assert!(notes[0].body.contains("Found the flaky test"));
    // An API item has no conversation to answer in: nothing new is queued.
    assert_eq!(read.outbox.unwrap().len(), 1);
}

/// §12 slice 4: a checkpoint older than 24 h is not resumed. The round
/// runs fresh from its question, still with the note, and the tool runs.
#[test]
fn a_checkpoint_older_than_a_day_gives_a_fresh_round_with_the_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let killed = process(&path, enabled(), true);
    let (item, turn) = open(&killed.manager);
    kill_during_the_mail(&killed, turn);

    // The process stayed down for 25 hours.
    let store = SQLiteStore::new(&path).expect("store");
    let checkpoints = store
        .turn_checkpoints_for_request("ten_a", &item.thread_id, &format!("{}_r1", item.item_id))
        .unwrap();
    assert_eq!(checkpoints.len(), 1, "the killed turn left its checkpoint");
    let mut old = checkpoints[0].clone();
    old.updated_at = Utc::now() - chrono::Duration::hours(25);
    store.put_turn_checkpoint(&old).unwrap();

    let booted = process(&path, enabled(), false);
    let recovered = booted.manager.recover_interrupted_rounds();
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    let Recovery::Rerun { mode, round } = &recovered[0].recovery else {
        panic!("not rerun: {recovered:?}");
    };
    assert_eq!(*mode, RecoveryMode::Fresh);
    assert!(round.report.is_some());

    let seen = booted.script.seen();
    assert!(
        seen[0].tool_results.is_empty(),
        "the round started from its question"
    );
    assert!(seen[0].system.contains("Recovery note"));
    assert!(seen[0].system.contains("could not be recovered"));
    assert!(seen[0].system.contains("it has sent nothing so far"));
    assert_eq!(booted.tool_calls.load(Ordering::SeqCst), 1, "a fresh run");
    assert!(
        store
            .turn_checkpoints_for_request("ten_a", &item.thread_id, &format!("{}_r1", item.item_id))
            .unwrap()
            .is_empty(),
        "the stale checkpoint is gone"
    );
    let read = booted.manager.get("ten_a", &item.item_id).unwrap();
    assert_eq!(recovery_inputs(&read).len(), 1);
    assert_eq!(read.rounds.unwrap()[0].state, RoundState::Settled);
}

/// `recoverOnBoot: false`: nothing reruns. The round ends as interrupted,
/// its input is pending again for the next round, and the item waits for a
/// person.
#[test]
fn with_recovery_off_nothing_reruns_and_the_item_asks_for_a_person() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let first = process(&path, enabled(), false);
    let (item, turn) = open(&first.manager);
    first
        .manager
        .settle_round(
            &turn,
            RoundOutcome::Answered {
                structured: Some(serde_json::json!({"status": "waiting", "summary": "s"})),
                turn_id: None,
            },
        )
        .unwrap();
    first
        .manager
        .add_input(
            "ten_a",
            &item.item_id,
            InputKind::Message,
            true,
            "pin it",
            "d1",
        )
        .unwrap();
    // Round 2 started, consuming the input, and the process died.
    let store = SQLiteStore::new(&path).expect("store");
    assert!(
        store
            .start_work_item_round(
                &WorkItemRoundRecord {
                    item_id: item.item_id.clone(),
                    seq: 2,
                    tenant_id: "ten_a".to_string(),
                    request_id: format!("{}_r2", item.item_id),
                    query_text: "This is round 2 of the work item.".to_string(),
                    state: "running".to_string(),
                    outcome: None,
                    report_json: None,
                    error_code: None,
                    turn_id: None,
                    started_at: Utc::now(),
                    finished_at: None,
                },
                1,
                false,
            )
            .unwrap()
    );

    let skipped_before = recovered_metric("skipped");
    let booted = process(
        &path,
        WorkItemsConfig {
            recover_on_boot: false,
            ..enabled()
        },
        false,
    );
    let recovered = booted.manager.recover_interrupted_rounds();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].recovery,
        Recovery::Interrupted {
            reason: NotRerun::RecoverOff
        }
    );
    assert!(recovered_metric("skipped") > skipped_before);
    assert!(booted.script.seen().is_empty(), "no model call");

    let read = booted.manager.get("ten_a", &item.item_id).unwrap();
    let rounds = read.rounds.clone().unwrap();
    assert_eq!(rounds[1].state, RoundState::Failed);
    assert_eq!(rounds[1].outcome.as_deref(), Some("interrupted"));
    assert_eq!(rounds[1].error_code.as_deref(), Some("interrupted"));
    assert_eq!(read.stage, Some(Stage::PersonDelivery));
    let inputs = read.inputs.unwrap();
    assert_eq!(inputs.len(), 1, "no recovery note without a rerun");
    assert_eq!(inputs[0].state, InputState::Pending);
    assert_eq!(inputs[0].round_seq, None);
    // A second boot finds nothing left behind.
    assert!(
        process(
            &path,
            WorkItemsConfig {
                recover_on_boot: false,
                ..enabled()
            },
            false
        )
        .manager
        .recover_interrupted_rounds()
        .is_empty()
    );
}

/// A stopped item's round is ended, never rerun; the item stays stopped.
#[test]
fn a_stopped_items_round_left_running_is_not_rerun() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let first = process(&path, enabled(), false);
    let (item, _) = open(&first.manager);
    first.manager.stop("ten_a", &item.item_id).unwrap();

    let booted = process(&path, enabled(), false);
    let recovered = booted.manager.recover_interrupted_rounds();
    assert_eq!(
        recovered.iter().map(|r| &r.recovery).collect::<Vec<_>>(),
        [&Recovery::Interrupted {
            reason: NotRerun::ItemEnded
        }]
    );
    assert!(booted.script.seen().is_empty());
    let read = booted.manager.get("ten_a", &item.item_id).unwrap();
    assert_eq!(read.state, ItemState::Ended);
    assert_eq!(read.end_reason, Some(EndReason::Stopped));
    assert_eq!(read.rounds.unwrap()[0].state, RoundState::Failed);
    assert!(recovery_inputs(&booted.manager.get("ten_a", &item.item_id).unwrap()).is_empty());
}

/// Two daemons' scans over the same rows, and one of them run twice at
/// once: the round is rerun exactly once.
#[test]
fn racing_scans_rerun_a_round_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let first = process(&path, enabled(), false);
    let (item, _) = open(&first.manager);

    let a = process(&path, enabled(), false);
    let b = process(&path, enabled(), false);
    for p in [&a, &b] {
        p.script.delay_ms.store(100, Ordering::SeqCst);
    }
    let go = Arc::new(AtomicBool::new(false));
    let scans: Vec<_> = [a.manager.clone(), a.manager.clone(), b.manager.clone()]
        .into_iter()
        .map(|manager| {
            let go = go.clone();
            std::thread::spawn(move || {
                while !go.load(Ordering::SeqCst) {
                    std::hint::spin_loop();
                }
                manager.recover_interrupted_rounds()
            })
        })
        .collect();
    go.store(true, Ordering::SeqCst);
    let results: Vec<_> = scans
        .into_iter()
        .flat_map(|scan| scan.join().expect("scan"))
        .collect();
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(matches!(results[0].recovery, Recovery::Rerun { .. }));
    // Each rerun asks for the mail, then reports: two calls, once.
    assert_eq!(a.script.seen().len() + b.script.seen().len(), 2);
    assert_eq!(
        a.tool_calls.load(Ordering::SeqCst) + b.tool_calls.load(Ordering::SeqCst),
        1
    );
    let read = a.manager.get("ten_a", &item.item_id).unwrap();
    assert_eq!(recovery_inputs(&read).len(), 1);
    assert_eq!(read.rounds.unwrap()[0].state, RoundState::Settled);

    // Started once per manager, however often it is asked.
    assert!(a.manager.start_recovery().unwrap());
    assert!(!a.manager.start_recovery().unwrap());
}

/// A round two earlier boots reran, and whose process died both times, is
/// not tried a third time: it waits for a person.
#[test]
fn a_round_rerun_twice_already_waits_for_a_person() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let first = process(&path, enabled(), false);
    let (item, _) = open(&first.manager);
    let store = SQLiteStore::new(&path).expect("store");
    for _ in 0..2 {
        // An earlier boot claims the round, and dies in its rerun.
        let note = kura_store::work_items::WorkItemInputRecord {
            item_id: item.item_id.clone(),
            seq: 0,
            tenant_id: "ten_a".to_string(),
            kind: "recovery".to_string(),
            author_is_person: false,
            body: "The daemon restarted.".to_string(),
            source_ref: String::new(),
            state: "consumed".to_string(),
            round_seq: Some(1),
            created_at: Utc::now(),
        };
        let boot = Utc::now() + chrono::Duration::milliseconds(1);
        std::thread::sleep(Duration::from_millis(2));
        assert!(
            store
                .claim_work_item_round_recovery("ten_a", &item.item_id, 1, boot, &note, boot)
                .unwrap()
                .is_some()
        );
    }
    std::thread::sleep(Duration::from_millis(2));

    let booted = process(&path, enabled(), false);
    let recovered = booted.manager.recover_interrupted_rounds();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].recovery,
        Recovery::Interrupted {
            reason: NotRerun::RecoveryLimit
        }
    );
    assert!(booted.script.seen().is_empty());
    let read = booted.manager.get("ten_a", &item.item_id).unwrap();
    assert_eq!(read.stage, Some(Stage::PersonDelivery));
    assert_eq!(recovery_inputs(&read).len(), 2);
}

/// Over the hourly cap, the round is not rerun; the cap counts the rerun
/// as a start but not the round's own earlier one.
#[test]
fn the_hourly_cap_holds_a_rerun() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let capped = WorkItemsConfig {
        max_rounds_per_item_per_hour: 2,
        ..enabled()
    };
    let first = process(&path, capped.clone(), false);
    let (item, _) = open(&first.manager);
    // Round 1 alone this hour: a rerun of it is allowed.
    let booted = process(&path, capped, false);
    assert!(matches!(
        booted.manager.recover_interrupted_rounds()[0].recovery,
        Recovery::Rerun { .. }
    ));
    // Round 2 started after rounds 1 (and its rerun) this hour: capped.
    first
        .manager
        .add_input("ten_a", &item.item_id, InputKind::Message, true, "go", "d1")
        .unwrap();
    let store = SQLiteStore::new(&path).expect("store");
    assert!(
        store
            .start_work_item_round(
                &WorkItemRoundRecord {
                    item_id: item.item_id.clone(),
                    seq: 2,
                    tenant_id: "ten_a".to_string(),
                    request_id: format!("{}_r2", item.item_id),
                    query_text: "This is round 2 of the work item.".to_string(),
                    state: "running".to_string(),
                    outcome: None,
                    report_json: None,
                    error_code: None,
                    turn_id: None,
                    started_at: Utc::now(),
                    finished_at: None,
                },
                10,
                false,
            )
            .unwrap()
    );
    let capped_config = WorkItemsConfig {
        max_rounds_per_item_per_hour: 1,
        ..enabled()
    };
    let booted = process(&path, capped_config, false);
    assert_eq!(
        booted.manager.recover_interrupted_rounds()[0].recovery,
        Recovery::Interrupted {
            reason: NotRerun::HourlyCap
        }
    );
    let read = booted.manager.get("ten_a", &item.item_id).unwrap();
    assert_eq!(read.stage, Some(Stage::PersonDelivery));
    // The input goes back to the item for its next round.
    assert!(
        read.inputs
            .unwrap()
            .iter()
            .any(|input| input.kind == InputKind::Message && input.state == InputState::Pending)
    );
}

/// An item that owns a conversation answers there after its rerun: the
/// waking message died with the process, so the report goes where the
/// item's last reply went, through the outbox.
#[test]
fn a_rerun_round_of_an_im_item_reports_to_its_conversation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().unwrap().to_string();
    let first = process(&path, enabled(), false);
    let origin = kura_im::ImTurnOrigin {
        tenant_id: "ten_a".to_string(),
        run_id: "run_1".to_string(),
        connector_id: "slack-main".to_string(),
        connector_kind: "slack".to_string(),
        source_conversation_id: "C1".to_string(),
        source_key: "thr_src_1".to_string(),
        author_id: "U1".to_string(),
        shape: kura_threads::ConversationShape::DirectMessage,
        mentioned: false,
        delivery_id: "delivery_1".to_string(),
        external_message_id: "m0".to_string(),
        route_policy_id: String::new(),
    };
    let (item, _) = first
        .manager
        .open(OpenRequest {
            tenant_id: "ten_a".to_string(),
            title: "Fix CI".to_string(),
            scope: "Fix CI on main.".to_string(),
            origin: Some(origin),
            subscribe_origin: true,
            ..OpenRequest::default()
        })
        .expect("open");
    sent_reply(&first.manager, &item.item_id, "On it.");

    let booted = process(&path, enabled(), false);
    let recovered = booted.manager.recover_interrupted_rounds();
    assert!(matches!(recovered[0].recovery, Recovery::Rerun { .. }));
    let outbox = booted
        .manager
        .get("ten_a", &item.item_id)
        .unwrap()
        .outbox
        .unwrap();
    assert_eq!(outbox.len(), 2);
    assert_eq!(outbox[1].conversation_key, "thr_src_1");
    assert_eq!(outbox[1].round_seq, Some(1));
    assert_eq!(outbox[1].body, "Checked the mail; waiting for CI.");
}
