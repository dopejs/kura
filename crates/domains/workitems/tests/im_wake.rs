//! Work items slice 2, end to end over the IM message loop with a test
//! connector (docs/harness/work-items.md §12, slice 2).
//!
//! The loop is the production `kura_im::MessageLoop` with the work-item
//! hook installed; inbound messages are driven straight into
//! `process_single_turn`, as the connector runtimes do, and replies land in
//! a recording sender. A scripted provider stands in for the model: in an
//! ordinary turn it calls `work_item.open` when asked to, and in a round
//! (a turn with the report schema as its response format) it answers the
//! report the test scripted for that round.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::future::BoxFuture;
use kura_chat::{CancellationToken, Service, ToolSource, ToolTurn};
use kura_connectors::{Connector, RedactionStatus, Status};
use kura_events::{Bus, Filter};
use kura_im::{MessageLoop, ProcessResult, ReplySender};
use kura_imtypes::{InboundMessage, OutboundReply, SentReply};
use kura_llm::{
    Dispatcher, MessageRole, Provider, ProviderError, ProviderRequest, ProviderResponse,
    StreamEmitter, ToolCall,
};
use kura_router::{SessionKind, SessionRouter};
use kura_runtime::Manager as RuntimeManager;
use kura_store::channel_management::RoutePolicy;
use kura_store::{SQLiteStore, StorePool};
use kura_workitems::{
    Basis, EndReason, ImHook, InputState, ItemState, Manager, OpenTool, OutboxState, Stage,
    WorkItem, WorkItemsConfig,
};

const PROVIDER: &str = "scripted";

// ---------------------------------------------------------------------------
// The scripted model
// ---------------------------------------------------------------------------

/// What the model was asked, reduced to what the tests look at.
#[derive(Debug, Clone)]
struct Seen {
    round: Option<i64>,
    system: String,
    question: String,
    tools: Vec<String>,
}

#[derive(Default)]
struct Script {
    /// The reply of round N (a report), by N. Unscripted rounds wait.
    rounds: Mutex<HashMap<i64, String>>,
    seen: Mutex<Vec<Seen>>,
    /// Round N's first model call waits until `release` is set.
    hold_round: Mutex<Option<i64>>,
    release: AtomicBool,
    held: AtomicBool,
}

impl Script {
    fn round_reply(&self, round: i64, reply: &str) {
        self.rounds.lock().unwrap().insert(round, reply.to_string());
    }

    fn seen_rounds(&self, round: i64) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.round == Some(round))
            .cloned()
            .collect()
    }
}

struct ScriptedProvider(Arc<Script>);

fn round_of(question: &str) -> Option<i64> {
    let rest = question.strip_prefix("This is round ")?;
    rest.split_whitespace().next()?.parse().ok()
}

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
            let question = request
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::User)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            // The round's own question, even when a steered message is the
            // last thing said.
            let round = if request.response_format.is_some() {
                request
                    .messages
                    .iter()
                    .filter(|m| m.role == MessageRole::User)
                    .find_map(|m| round_of(&m.content))
            } else {
                None
            };
            let tools: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
            let answered_tool = request.messages.iter().any(|m| m.role == MessageRole::Tool);
            self.0.seen.lock().unwrap().push(Seen {
                round,
                system,
                question: question.clone(),
                tools: tools.clone(),
            });
            if let Some(round) = round {
                let hold = *self.0.hold_round.lock().unwrap() == Some(round);
                if hold && !self.0.held.swap(true, Ordering::SeqCst) {
                    while !self.0.release.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                let reply = self
                    .0
                    .rounds
                    .lock()
                    .unwrap()
                    .get(&round)
                    .cloned()
                    .unwrap_or_else(|| r#"{"status":"waiting","summary":"Waiting."}"#.to_string());
                return Ok(answer(&reply));
            }
            if answered_tool {
                return Ok(answer("Opened a work item for that."));
            }
            if question.contains("please fix") && tools.iter().any(|t| t == "work_item.open") {
                return Ok(ProviderResponse {
                    tool_calls: vec![ToolCall {
                        call_id: "call_open".to_string(),
                        name: "work_item.open".to_string(),
                        arguments: serde_json::json!({
                            "title": "Fix CI",
                            "scope": "Fix the failing CI on main and merge it when I say so."
                        })
                        .to_string(),
                    }],
                    finish_reason: "tool_calls".to_string(),
                    ..ProviderResponse::default()
                });
            }
            Ok(answer(&format!("reply:{question}")))
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

fn answer(text: &str) -> ProviderResponse {
    ProviderResponse {
        output: text.to_string(),
        finish_reason: "stop".to_string(),
        ..ProviderResponse::default()
    }
}

/// Offers `work_item.open` the way the daemon's tool host does.
struct Tools(Arc<OnceLock<Arc<Manager>>>);

impl ToolSource for Tools {
    fn registry(&self, turn: &ToolTurn) -> Arc<kura_core::ToolRegistry> {
        let mut registry = kura_core::ToolRegistry::new();
        if let Some(manager) = self.0.get() {
            if let Some(tool) = OpenTool::for_turn(manager.clone(), turn) {
                registry.register(Arc::new(tool));
            }
        }
        Arc::new(registry)
    }
}

// ---------------------------------------------------------------------------
// The test connector
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Replies {
    sent: Mutex<Vec<OutboundReply>>,
    next: AtomicUsize,
    /// Errors the next sends answer, in order.
    fail: Mutex<Vec<String>>,
}

impl Replies {
    fn texts(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|reply| reply.content.clone())
            .collect()
    }

    fn last(&self) -> Option<String> {
        self.texts().pop()
    }
}

impl ReplySender for Replies {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        {
            let mut fail = self.fail.lock().unwrap();
            if !fail.is_empty() {
                return Err(fail.remove(0));
            }
        }
        self.sent.lock().unwrap().push(reply);
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(SentReply {
            external_message_id: format!("reply_{id}"),
        })
    }
}

struct Harness {
    store: Arc<SQLiteStore>,
    chat: Arc<Service>,
    manager: Arc<Manager>,
    message_loop: MessageLoop,
    script: Arc<Script>,
    replies: Replies,
    bus: Bus,
    next_message: AtomicUsize,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    harness_with(true)
}

/// `hook: false` is a daemon with work items disabled: the loop has no hook.
fn harness_with(hook: bool) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_str().expect("path").to_string();
    // `MessageLoop::new` takes the connector store as an `Arc`, as the
    // daemon and kura-im's own tests build it.
    #[allow(clippy::arc_with_non_send_sync)]
    let store = Arc::new(SQLiteStore::new(&path).expect("store"));
    let bus = Bus::new();
    let script = Arc::new(Script::default());
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(Arc::new(ScriptedProvider(script.clone())));
    dispatcher
        .set_default_provider(PROVIDER)
        .expect("default provider");
    dispatcher.set_default_model("scripted-v1");
    let late_manager: Arc<OnceLock<Arc<Manager>>> = Arc::default();
    let mut chat = Service::new_service(dispatcher, None, None, Some(bus.clone()), None);
    chat.set_tools(Arc::new(Tools(late_manager.clone())));
    let chat = Arc::new(chat);

    let pool = Arc::new(StorePool::writer_only(Arc::new(parking_lot::Mutex::new(
        SQLiteStore::new(&path).expect("manager store"),
    ))));
    let manager = Arc::new(
        Manager::new(
            pool,
            Arc::new(bus.clone()),
            WorkItemsConfig {
                enabled: true,
                ..WorkItemsConfig::default()
            },
        )
        .with_chat(chat.clone()),
    );
    late_manager.set(manager.clone()).ok().expect("set once");

    let runtime = Arc::new(RuntimeManager::new());
    let mut message_loop = MessageLoop::new(
        SessionRouter::new(),
        runtime,
        None,
        Some(bus.clone()),
        store.clone(),
        (*chat).clone(),
    );
    if hook {
        message_loop.set_work_item_hook(Arc::new(ImHook::new(manager.clone())));
    }
    Harness {
        store,
        chat,
        manager,
        message_loop,
        script,
        replies: Replies::default(),
        bus,
        next_message: AtomicUsize::new(0),
        _dir: dir,
    }
}

/// A Slack connector that enforces group participation.
fn slack(tenant_id: &str) -> Connector {
    let mut connector = Connector {
        tenant_id: tenant_id.to_string(),
        connector_id: "slack-main".to_string(),
        kind: "slack".to_string(),
        display_name: "Slack".to_string(),
        status: Status::Healthy,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ..Connector::default()
    };
    connector.capability_profile = serde_json::json!({
        "group_room_mention_evidence": "supported",
        "group_room_allowlist_evidence": "supported",
    })
    .as_object()
    .expect("object")
    .clone();
    connector
}

/// Lets the connector receive `C1` and, when given, only these senders.
fn allow(store: &SQLiteStore, tenant_id: &str, senders: &[&str]) {
    store
        .save_channel_route_policy(&RoutePolicy {
            route_policy_id: format!("route_policy_{tenant_id}"),
            tenant_id: tenant_id.to_string(),
            connector_id: "slack-main".to_string(),
            eligible_senders: senders.iter().map(|s| (*s).to_string()).collect(),
            eligible_conversations: Vec::new(),
            eligible_rooms: vec!["C1".to_string()],
            eligible_channels: vec!["C1".to_string()],
            invocation_gates: Vec::new(),
            background_delivery_eligible: false,
            validation_state: "valid".to_string(),
            reason_code: String::new(),
            validated_at: Utc::now(),
            audit_event_id: String::new(),
            redaction_status: RedactionStatus::Redacted,
        })
        .expect("save route policy");
}

impl Harness {
    /// A message in channel `C1` of `tenant_id`'s workspace.
    fn say(&self, tenant_id: &str, author: &str, text: &str, mentioned: bool) -> ProcessResult {
        let n = self.next_message.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("msg_{n}");
        let inbound = InboundMessage {
            tenant_id: tenant_id.to_string(),
            connector_id: "slack-main".to_string(),
            connector_kind: "slack".to_string(),
            external_message_id: id.clone(),
            account_id: "workspace_1".to_string(),
            connector_account_id: "workspace_1".to_string(),
            channel_id: "C1".to_string(),
            channel_or_conversation_id: "C1".to_string(),
            provider_message_id: id,
            equivalent_rule_id: "slack_workspace_conversation_message_id".to_string(),
            peer_id: "C1".to_string(),
            thread_id: "thread_root".to_string(),
            author_id: author.to_string(),
            content: text.to_string(),
            kind: SessionKind::Group,
            mentioned,
            received_at: Utc::now(),
            ..InboundMessage::default()
        };
        self.message_loop
            .process_single_turn(
                &slack(tenant_id),
                &inbound,
                &self.replies,
                &CancellationToken::new(),
            )
            .expect("process inbound")
    }

    /// Asks for work in `C1` as `U1` and waits for the item's first round.
    fn open(&self, tenant_id: &str) -> WorkItem {
        let result = self.say(tenant_id, "U1", "@kura please fix CI on main", true);
        assert_eq!(result.outcome, "accepted", "{result:?}");
        assert_eq!(
            self.replies.last().as_deref(),
            Some("Opened a work item for that.")
        );
        let items = self.manager.list(tenant_id, None, 10).expect("list items");
        assert_eq!(items.len(), 1, "one item opened");
        self.settled(tenant_id, &items[0].item_id, 1)
    }

    /// Waits until round `seq` of the item is no longer running.
    fn settled(&self, tenant_id: &str, item_id: &str, seq: i64) -> WorkItem {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let item = self.manager.get(tenant_id, item_id).expect("get item");
            let rounds = item.rounds.clone().unwrap_or_default();
            if rounds
                .iter()
                .any(|round| round.seq == seq && round.state != kura_workitems::RoundState::Running)
            {
                return item;
            }
            assert!(
                Instant::now() < deadline,
                "round {seq} never settled: {item:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Work item events only: a subscriber holds 16 unread events, and the IM
/// loop's own evidence would crowd them out.
fn work_item_events() -> Filter {
    Filter {
        category: "work_item".to_string(),
        ..Filter::default()
    }
}

/// §12 slice 2: open → idle → the person's message wakes the item → reply.
#[test]
fn an_item_opened_in_a_group_wakes_on_its_requesters_message_and_replies() {
    let h = harness();
    allow(&h.store, "ten_a", &[]);
    let (events, _unsubscribe) = h.bus.subscribe(work_item_events());

    let item = h.open("ten_a");
    assert_eq!(item.state, ItemState::Open);
    assert_eq!(item.stage, Some(Stage::Idle), "{item:?}");
    assert_eq!(item.mandate.basis, Basis::ImMention);
    assert_eq!(item.mandate.sender.connector_id, "slack-main");
    assert_eq!(item.mandate.sender.author_id, "U1");
    assert_eq!(item.mandate.source.conversation, "C1");
    assert_eq!(item.mandate.receiving_policy_id, "route_policy_ten_a");
    let subs = item.subscriptions.clone().expect("subscriptions");
    assert_eq!(subs.len(), 1);
    assert!(subs[0].source_key.starts_with("thr_src_"));
    assert!(subs[0].dropped_at.is_none());

    // The ordinary turn was offered the tool; the round was not, and its
    // mandate is in the system frame, not in the question.
    let first = h.script.seen_rounds(1);
    assert!(!first.is_empty());
    assert!(
        first[0]
            .system
            .contains("Fix the failing CI on main and merge it when I say so."),
        "{}",
        first[0].system
    );
    assert!(!first[0].question.contains("Fix the failing CI"));
    assert!(!first[0].tools.iter().any(|t| t == "work_item.open"));

    // The requester writes again, without mentioning the agent: the item
    // takes it, runs round 2 inline and replies with the report.
    h.script.round_reply(
        2,
        r#"{"status":"ready","summary":"Merged the fix; CI is green."}"#,
    );
    let result = h.say("ten_a", "U1", "go ahead and merge it", false);
    assert_eq!(result.outcome, "work_item", "{result:?}");
    assert_eq!(result.reason_code, "work_item_round");
    assert!(result.run.run_id.is_empty(), "no ordinary turn ran");
    assert_eq!(
        h.replies.last().as_deref(),
        Some("Merged the fix; CI is green.")
    );

    let item = h.manager.get("ten_a", &item.item_id).expect("item");
    assert_eq!(item.state, ItemState::Ended);
    assert_eq!(item.end_reason, Some(EndReason::Completed));
    let inputs = item.inputs.expect("inputs");
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].body, "go ahead and merge it");
    assert!(inputs[0].author_is_person);
    assert_eq!(inputs[0].state, InputState::Consumed);
    assert_eq!(inputs[0].round_seq, Some(2));
    let second = h.script.seen_rounds(2);
    assert!(
        second[0].question.contains(
            "<work_item_input kind=\"message\" from=\"requester\">\ngo ahead and merge it\n</work_item_input>"
        ),
        "{}",
        second[0].question
    );

    let names: Vec<String> = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| event.category == "work_item")
        .map(|event| {
            assert_eq!(event.tenant_id, "ten_a");
            let body = serde_json::to_string(&event.payload).unwrap();
            assert!(
                !body.contains("merge it"),
                "an event carried a message body"
            );
            event.name
        })
        .collect();
    assert!(names.contains(&"work_item.woken".to_string()), "{names:?}");
    assert_eq!(names.first().map(String::as_str), Some("work_item.opened"));
    assert_eq!(names.last().map(String::as_str), Some("work_item.ended"));
}

#[test]
fn a_message_from_someone_else_on_the_owned_conversation_falls_through() {
    let h = harness();
    allow(&h.store, "ten_a", &[]);
    let item = h.open("ten_a");

    // Not the requester: the ordinary path (participation, then a turn).
    let result = h.say("ten_a", "U2", "@kura what is the weather", true);
    assert_eq!(result.outcome, "accepted", "{result:?}");
    assert_eq!(
        h.replies.last().as_deref(),
        Some("reply:@kura what is the weather")
    );
    // Unaddressed chatter from someone else is ignored as before.
    let result = h.say("ten_a", "U2", "lunch?", false);
    assert_eq!(result.outcome, "ignored");

    let item = h.manager.get("ten_a", &item.item_id).expect("item");
    assert!(item.inputs.expect("inputs").is_empty());
    assert_eq!(item.seq, 1, "no round ran");
}

#[test]
fn a_stopped_item_does_not_wake() {
    let h = harness();
    allow(&h.store, "ten_a", &[]);
    let item = h.open("ten_a");
    let stopped = h.manager.stop("ten_a", &item.item_id).expect("stop");
    assert_eq!(stopped.end_reason, Some(EndReason::Stopped));
    // Stopping again changes nothing.
    assert_eq!(
        h.manager
            .stop("ten_a", &item.item_id)
            .expect("stop again")
            .end_reason,
        Some(EndReason::Stopped)
    );

    let result = h.say("ten_a", "U1", "go ahead and merge it", false);
    assert_eq!(result.outcome, "ignored", "{result:?}");
    let result = h.say("ten_a", "U1", "@kura are you there", true);
    assert_eq!(result.outcome, "accepted");
    assert_eq!(
        h.replies.last().as_deref(),
        Some("reply:@kura are you there")
    );

    let item = h.manager.get("ten_a", &item.item_id).expect("item");
    assert_eq!(item.state, ItemState::Ended);
    assert_eq!(item.end_reason, Some(EndReason::Stopped));
    assert!(item.inputs.expect("inputs").is_empty());
    let subs = item.subscriptions.expect("subscriptions");
    assert_eq!(subs[0].drop_reason.as_deref(), Some("stopped"));
}

#[test]
fn an_ended_item_resumes_on_its_requesters_message_after_revalidation() {
    let h = harness();
    allow(&h.store, "ten_a", &["U1", "U2"]);
    h.script
        .round_reply(1, r#"{"status":"ready","summary":"Fixed."}"#);
    let item = h.open("ten_a");
    assert_eq!(item.end_reason, Some(EndReason::Completed));

    h.script.round_reply(
        2,
        r#"{"status":"waiting","summary":"Looking at the new failure."}"#,
    );
    let result = h.say("ten_a", "U1", "it broke again", false);
    assert_eq!(result.outcome, "work_item", "{result:?}");
    assert_eq!(
        h.replies.last().as_deref(),
        Some("Looking at the new failure.")
    );
    let item = h.manager.get("ten_a", &item.item_id).expect("item");
    assert_eq!(item.state, ItemState::Open);
    assert_eq!(item.end_reason, None);
    assert_eq!(item.seq, 2);
}

#[test]
fn a_revoked_allowlist_entry_falls_through() {
    let h = harness();
    allow(&h.store, "ten_a", &["U1"]);
    // Open and idle: the requester is taken off the allowlist.
    let item = h.open("ten_a");
    assert_eq!(item.stage, Some(Stage::Idle));
    allow(&h.store, "ten_a", &["U9"]);
    let result = h.say("ten_a", "U1", "go ahead and merge it", true);
    assert_ne!(result.outcome, "work_item", "{result:?}");
    assert_eq!(
        result.outcome, "denied",
        "the ordinary gate refuses as before"
    );
    let open = h.manager.get("ten_a", &item.item_id).expect("item");
    assert_eq!(open.state, ItemState::Open, "a live item is not cancelled");
    assert!(open.inputs.expect("inputs").is_empty());

    // Ended on its own report: revalidation fails, the item is revoked and
    // its conversation is free again.
    let h = harness();
    allow(&h.store, "ten_a", &["U1"]);
    h.script
        .round_reply(1, r#"{"status":"ready","summary":"Fixed."}"#);
    let item = h.open("ten_a");
    assert_eq!(item.end_reason, Some(EndReason::Completed));
    allow(&h.store, "ten_a", &["U9"]);
    let (events, _unsubscribe) = h.bus.subscribe(work_item_events());
    let result = h.say("ten_a", "U1", "it broke again", true);
    assert_eq!(result.outcome, "denied", "{result:?}");
    let revoked = h.manager.get("ten_a", &item.item_id).expect("item");
    assert_eq!(revoked.state, ItemState::Ended);
    assert_eq!(revoked.end_reason, Some(EndReason::Revoked));
    assert_eq!(
        revoked.subscriptions.expect("subs")[0]
            .drop_reason
            .as_deref(),
        Some("revoked")
    );
    let revocation = std::iter::from_fn(|| events.try_recv().ok())
        .find(|event| event.name == "work_item.mandate_revoked")
        .expect("mandate_revoked event");
    assert_eq!(revocation.payload["reason"], "sender_not_allowed");

    // Allowed again later: the revoked item stays ended; the message takes
    // the ordinary path.
    allow(&h.store, "ten_a", &["U1"]);
    let result = h.say("ten_a", "U1", "@kura hello", true);
    assert_eq!(result.outcome, "accepted");
    let still = h.manager.get("ten_a", &item.item_id).expect("item");
    assert_eq!(still.end_reason, Some(EndReason::Revoked));
}

#[test]
fn another_tenants_conversation_with_the_same_ids_never_reaches_the_item() {
    let h = harness();
    allow(&h.store, "ten_a", &[]);
    allow(&h.store, "ten_b", &[]);
    let item = h.open("ten_a");

    // Same connector id, workspace, channel and author, another tenant.
    let result = h.say("ten_b", "U1", "go ahead and merge it", true);
    assert_eq!(result.outcome, "accepted", "{result:?}");
    let mine = h.manager.get("ten_a", &item.item_id).expect("item");
    assert!(mine.inputs.expect("inputs").is_empty());
    assert!(h.manager.get("ten_b", &item.item_id).is_err());
    assert!(h.manager.list("ten_b", None, 10).expect("list").is_empty());
    assert!(h.manager.stop("ten_b", &item.item_id).is_err());
}

#[test]
fn a_message_while_a_round_runs_steers_it() {
    let h = harness();
    allow(&h.store, "ten_a", &[]);
    *h.script.hold_round.lock().unwrap() = Some(1);
    let result = h.say("ten_a", "U1", "@kura please fix CI on main", true);
    assert_eq!(result.outcome, "accepted");
    let item_id = h.manager.list("ten_a", None, 10).expect("list")[0]
        .item_id
        .clone();
    // Round 1 is held inside its first model call.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !h.script.held.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "round 1 never started");
        std::thread::sleep(Duration::from_millis(5));
    }

    let result = h.say("ten_a", "U1", "also bump the lockfile", false);
    assert_eq!(result.outcome, "work_item", "{result:?}");
    assert_eq!(result.reason_code, "work_item_steered");
    assert_eq!(
        h.replies.last().as_deref(),
        Some("Added to the work item's running round.")
    );
    h.script.release.store(true, Ordering::SeqCst);
    let item = h.settled("ten_a", &item_id, 1);

    let inputs = item.inputs.expect("inputs");
    assert_eq!(inputs[0].state, InputState::Steered);
    assert_eq!(inputs[0].round_seq, Some(1));
    // The round read it on its next model call, framed as an input.
    let calls = h.script.seen_rounds(1);
    assert!(calls.len() >= 2, "{calls:?}");
    assert!(
        calls
            .last()
            .unwrap()
            .question
            .contains("from=\"requester\">\nalso bump the lockfile"),
        "{calls:?}"
    );
    // Nothing is left for another round.
    assert_eq!(item.seq, 1);
    assert_eq!(item.stage, Some(Stage::Idle));
}

/// §12 slice 3 over the real loop: a round's reply that fails to send is
/// kept in the outbox, not lost, and the message still counts as taken.
/// The requester's next message sends it first, then the new round's reply,
/// in order.
#[test]
fn a_reply_that_fails_to_send_goes_out_first_on_the_next_message() {
    let h = harness();
    allow(&h.store, "ten_a", &[]);
    let item = h.open("ten_a");

    h.script.round_reply(
        2,
        r#"{"status":"progress","summary":"Found the flaky test."}"#,
    );
    h.replies
        .fail
        .lock()
        .unwrap()
        .push("slack web api status 503".to_string());
    let sent_before = h.replies.texts().len();
    let result = h.say("ten_a", "U1", "any news?", false);
    assert_eq!(result.outcome, "work_item", "{result:?}");
    assert_eq!(result.reason_code, "work_item_round");
    assert_eq!(result.reply, "Found the flaky test.");
    assert_eq!(
        h.replies.texts().len(),
        sent_before,
        "nothing reached the chat"
    );
    let read = h.manager.get("ten_a", &item.item_id).expect("item");
    let outbox = read.outbox.expect("outbox");
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].state, OutboxState::Failed);
    assert_eq!(outbox[0].round_seq, Some(2));
    assert_eq!(outbox[0].connector_id, "slack-main");
    assert!(outbox[0].conversation_key.starts_with("thr_src_"));

    h.script
        .round_reply(3, r#"{"status":"waiting","summary":"Pushed the fix."}"#);
    let result = h.say("ten_a", "U1", "and now?", false);
    assert_eq!(result.outcome, "work_item", "{result:?}");
    assert_eq!(
        h.replies.texts()[sent_before..].to_vec(),
        vec!["Found the flaky test.", "Pushed the fix."]
    );
    let outbox = h
        .manager
        .get("ten_a", &item.item_id)
        .expect("item")
        .outbox
        .expect("outbox");
    assert!(outbox.iter().all(|row| row.state == OutboxState::Sent));
    assert!(outbox.iter().all(|row| !row.external_message_id.is_empty()));
}

/// Off means unchanged: with work items disabled the loop has no hook, so
/// the requester's message on a conversation an item owns (from a run with
/// the feature on) takes the ordinary path, and no outbox row is written.
#[test]
fn with_work_items_disabled_the_im_path_is_unchanged() {
    let h = harness_with(false);
    allow(&h.store, "ten_a", &[]);
    let conversation = InboundMessage {
        tenant_id: "ten_a".to_string(),
        connector_id: "slack-main".to_string(),
        account_id: "workspace_1".to_string(),
        connector_account_id: "workspace_1".to_string(),
        channel_id: "C1".to_string(),
        channel_or_conversation_id: "C1".to_string(),
        peer_id: "C1".to_string(),
        thread_id: "thread_root".to_string(),
        kind: SessionKind::Group,
        ..InboundMessage::default()
    };
    let key = kura_im::thread_id_for_source(
        &kura_im::source_continuation_key(&slack("ten_a"), &conversation).expect("source key"),
    );
    // The key the hook would match: what an enabled daemon subscribes when
    // the item is opened from this conversation.
    let enabled = harness();
    allow(&enabled.store, "ten_a", &[]);
    let opened = enabled.open("ten_a");
    assert_eq!(opened.subscriptions.expect("subs")[0].source_key, key);
    let (item, _) = h
        .manager
        .open(kura_workitems::OpenRequest {
            tenant_id: "ten_a".to_string(),
            title: "Fix CI".to_string(),
            scope: "Fix CI on main.".to_string(),
            origin: Some(kura_im::ImTurnOrigin {
                tenant_id: "ten_a".to_string(),
                run_id: "run_1".to_string(),
                connector_id: "slack-main".to_string(),
                connector_kind: "slack".to_string(),
                source_conversation_id: "C1".to_string(),
                source_key: key,
                author_id: "U1".to_string(),
                shape: kura_threads::ConversationShape::Room,
                mentioned: true,
                delivery_id: "delivery_0".to_string(),
                external_message_id: "m0".to_string(),
                route_policy_id: String::new(),
            }),
            subscribe_origin: true,
            ..kura_workitems::OpenRequest::default()
        })
        .expect("open");
    assert_eq!(item.subscriptions.map_or(1, |subs| subs.len()), 1);

    // Unaddressed: ignored by the participation gate, as before.
    let result = h.say("ten_a", "U1", "go ahead and merge it", false);
    assert_eq!(result.outcome, "ignored", "{result:?}");
    // Addressed: an ordinary turn, answered inline.
    let result = h.say("ten_a", "U1", "@kura hello there", true);
    assert_eq!(result.outcome, "accepted", "{result:?}");
    assert_eq!(h.replies.last().as_deref(), Some("reply:@kura hello there"));
    let read = h.manager.get("ten_a", &item.item_id).expect("item");
    assert!(read.inputs.expect("inputs").is_empty());
    assert!(read.outbox.expect("outbox").is_empty());
}
