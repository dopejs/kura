//! Opening items, settling their rounds, and reading them back.
//!
//! A round is an ordinary chat turn. The manager hands out the round to
//! run ([`RoundTurn`]) and takes back how it ended ([`RoundOutcome`]);
//! `driver.rs` runs rounds through the chat service the manager is given
//! ([`Manager::with_chat`]). Everything it writes is scoped to the tenant
//! the caller names.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_events::{Bus, Event, Resource};
use kura_im::ImTurnOrigin;
use kura_store::StorePool;
use kura_store::work_items::{
    WorkItemActivity, WorkItemInputRecord, WorkItemRecord, WorkItemRoundRecord,
    WorkItemRoundSettlement, WorkItemSubscriptionRecord,
};
use kura_threads::ConversationShape;
use serde_json::{Map, Value, json};

use crate::report::{read_report, round_instructions, round_query, unreadable_report};
use crate::stage::{StageFacts, derive_stage};
use crate::types::{
    Attention, Basis, EndReason, Input, InputKind, InputState, ItemState, Mandate, MandateSource,
    MandateSourceKind, Report, Round, RoundState, Sender, Stage, Strength, Subscription,
    SubscriptionRole, WorkItem, WorkItemsConfig,
};

const MAX_TITLE_CHARS: usize = 200;
const MAX_SCOPE_CHARS: usize = 16 * 1024;
/// Items one list returns at most.
pub const MAX_LIST_LIMIT: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkItemError {
    #[error("{0}")]
    Invalid(String),
    /// No such item for this tenant; another tenant's item is not found.
    #[error("work item not found")]
    NotFound,
    #[error("the tenant already holds {limit} open work items")]
    OpenLimit { limit: usize },
    /// A settlement for a round that is not running: it was settled already.
    #[error("work item round {0} is not running")]
    RoundNotRunning(String),
    /// D2: the conversation already has an open owning item.
    #[error("this conversation already has an open work item, {0}; it receives messages here")]
    ConversationOwned(String),
    /// No such outbox row on the item.
    #[error("work item outbox row {0} not found")]
    OutboxRowNotFound(i64),
    /// The row was sent or dropped already; it can no longer be withdrawn.
    #[error("work item outbox row {seq} is {state} already")]
    OutboxRowSettled { seq: i64, state: &'static str },
    /// The row's conversation is being sent to right now.
    #[error("work item outbox row {0} is being sent; try again")]
    OutboxRowSending(i64),
    #[error("work item store: {0}")]
    Store(String),
}

impl WorkItemError {
    /// A stable code for API bodies and logs.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_work_item",
            Self::NotFound => "work_item_not_found",
            Self::OpenLimit { .. } => "work_item_open_limit",
            Self::RoundNotRunning(_) => "work_item_round_not_running",
            Self::ConversationOwned(_) => "work_item_conversation_owned",
            Self::OutboxRowNotFound(_) => "work_item_outbox_row_not_found",
            Self::OutboxRowSettled { .. } => "work_item_outbox_row_settled",
            Self::OutboxRowSending(_) => "work_item_outbox_row_sending",
            Self::Store(_) => "store",
        }
    }
}

/// A request to open an item: on the `api` basis (`POST /v1/work-items`,
/// or `work_item.open` from a web or TUI turn), or on an `im_*` basis when
/// `work_item.open` is called by a turn an IM message started (`origin`).
#[derive(Debug, Clone, Default)]
pub struct OpenRequest {
    pub tenant_id: String,
    /// The principal that asked; empty without one.
    pub principal_id: String,
    /// The HTTP request id the item was asked for under.
    pub request_ref: String,
    pub title: String,
    pub scope: String,
    pub provider: String,
    pub model: String,
    /// The IM conversation the asking turn served; `None` off IM.
    pub origin: Option<ImTurnOrigin>,
    /// Make the item the owner of `origin`'s conversation, so the person
    /// who asked wakes it by writing there (§5.2). Ignored without origin.
    pub subscribe_origin: bool,
}

/// A round to run as a chat turn: a non-streamed query on `thread_id`, with
/// `request_id` as its request id, `instructions` (the mandate) at the head
/// of its system frame, and the report schema as its response format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundTurn {
    pub tenant_id: String,
    pub item_id: String,
    pub seq: i64,
    pub request_id: String,
    pub thread_id: String,
    pub query_text: String,
    pub instructions: String,
    pub provider: String,
    pub model: String,
}

/// How a round's turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum RoundOutcome {
    /// The turn answered; `structured` is its reply as accepted against the
    /// report schema (absent if it somehow was not).
    Answered {
        structured: Option<Value>,
        turn_id: Option<String>,
    },
    /// The reply never conformed to the report schema.
    Unreadable {
        detail: String,
        turn_id: Option<String>,
    },
    /// The turn ended without an answer; `error_code` is the chat error's.
    Failed {
        error_code: String,
        turn_id: Option<String>,
    },
}

/// Work items for every tenant on this daemon.
pub struct Manager {
    pub(crate) store: Arc<StorePool>,
    bus: Arc<Bus>,
    pub(crate) config: WorkItemsConfig,
    /// Runs rounds (`driver.rs`). Absent in tests that only record.
    pub(crate) chat: Option<Arc<kura_chat::Service>>,
    /// Where each ordinary IM turn running now came from, by (tenant,
    /// run id), for `work_item.open` (registered by the IM hook).
    origins: parking_lot::Mutex<HashMap<(String, String), ImTurnOrigin>>,
    /// Reply senders, the conversations being sent to now, and the
    /// sweeper's signal (`outbox.rs`).
    pub(crate) outbox: crate::outbox::Outbox,
}

impl Manager {
    #[must_use]
    pub fn new(store: Arc<StorePool>, bus: Arc<Bus>, config: WorkItemsConfig) -> Self {
        Self {
            store,
            bus,
            config,
            chat: None,
            origins: parking_lot::Mutex::default(),
            outbox: crate::outbox::Outbox::default(),
        }
    }

    /// The chat service rounds run on.
    #[must_use]
    pub fn with_chat(mut self, chat: Arc<kura_chat::Service>) -> Self {
        self.chat = Some(chat);
        self
    }

    #[must_use]
    pub fn config(&self) -> &WorkItemsConfig {
        &self.config
    }

    /// Records where an ordinary IM turn came from while it runs.
    pub fn register_turn_origin(&self, origin: &ImTurnOrigin) {
        if origin.tenant_id.trim().is_empty() || origin.run_id.trim().is_empty() {
            return;
        }
        self.origins.lock().insert(
            (origin.tenant_id.clone(), origin.run_id.clone()),
            origin.clone(),
        );
    }

    pub fn unregister_turn_origin(&self, tenant_id: &str, run_id: &str) {
        self.origins
            .lock()
            .remove(&(tenant_id.to_string(), run_id.to_string()));
    }

    /// The IM conversation the turn `run_id` of `tenant_id` serves, if it
    /// is an ordinary IM turn running now.
    #[must_use]
    pub fn turn_origin(&self, tenant_id: &str, run_id: &str) -> Option<ImTurnOrigin> {
        if run_id.trim().is_empty() {
            return None;
        }
        self.origins
            .lock()
            .get(&(tenant_id.to_string(), run_id.to_string()))
            .cloned()
    }

    /// Opens an item with an explicit mandate, with its own thread and its
    /// first round recorded as running. The caller runs that round and
    /// hands back how it ended to [`Self::settle_round`] (or hands both to
    /// the driver). With an IM origin and `subscribe_origin`, the item also
    /// becomes the owner of that conversation (D2): refused while another
    /// open item owns it.
    pub fn open(&self, request: OpenRequest) -> Result<(WorkItem, RoundTurn), WorkItemError> {
        check_open(&request)?;
        let tenant_id = request.tenant_id.trim().to_string();
        let title = request.title.trim().to_string();
        let scope = request.scope.trim().to_string();

        let now = Utc::now();
        let hex = uuid::Uuid::now_v7().simple().to_string();
        let item_id = format!("wi_{hex}");
        let thread_id = format!("thr_wi_{hex}");
        let seq = 1;
        let mandate = match &request.origin {
            None => Mandate {
                basis: Basis::Api,
                strength: Strength::Explicit,
                policy_hash: String::new(),
                matched_rules: Vec::new(),
                sender: Sender {
                    principal_id: request.principal_id.trim().to_string(),
                    ..Sender::default()
                },
                source: MandateSource {
                    kind: MandateSourceKind::Api,
                    reference: request.request_ref.trim().to_string(),
                    conversation: String::new(),
                },
                receiving_policy_id: String::new(),
                scope_text: scope.clone(),
                created_at: now,
            },
            Some(origin) => Mandate {
                // The gate it already passed: a DM, or a group/room
                // participation decision (allowlist, sender, mention).
                basis: if origin.shape == ConversationShape::DirectMessage {
                    Basis::ImDirect
                } else {
                    Basis::ImMention
                },
                strength: Strength::Explicit,
                policy_hash: String::new(),
                matched_rules: Vec::new(),
                sender: Sender {
                    connector_id: origin.connector_id.clone(),
                    author_id: origin.author_id.clone(),
                    principal_id: String::new(),
                },
                source: MandateSource {
                    kind: MandateSourceKind::Im,
                    reference: origin.delivery_id.clone(),
                    conversation: origin.source_conversation_id.clone(),
                },
                receiving_policy_id: origin.route_policy_id.clone(),
                scope_text: scope.clone(),
                created_at: now,
            },
        };
        let owner = request
            .origin
            .as_ref()
            .filter(|origin| request.subscribe_origin && !origin.source_key.is_empty())
            .filter(|origin| !origin.author_id.is_empty())
            .map(|origin| WorkItemSubscriptionRecord {
                subscription_id: format!("wis_{}", uuid::Uuid::now_v7().simple()),
                item_id: item_id.clone(),
                tenant_id: tenant_id.clone(),
                source_kind: "im".to_string(),
                source_key: origin.source_key.clone(),
                connector_id: origin.connector_id.clone(),
                conversation_id: origin.source_conversation_id.clone(),
                role: SubscriptionRole::Owner.as_str().to_string(),
                created_at: now,
                last_event_at: now,
                dropped_at: None,
                drop_reason: None,
            });
        let turn = RoundTurn {
            tenant_id: tenant_id.clone(),
            item_id: item_id.clone(),
            seq,
            request_id: round_request_id(&item_id, seq),
            thread_id: thread_id.clone(),
            query_text: round_query(seq, &[]),
            instructions: round_instructions(&title, &scope),
            provider: request.provider.trim().to_string(),
            model: request.model.trim().to_string(),
        };
        let item = WorkItemRecord {
            item_id: item_id.clone(),
            tenant_id: tenant_id.clone(),
            title,
            state: ItemState::Open.as_str().to_string(),
            end_reason: None,
            thread_id: thread_id.clone(),
            provider: turn.provider.clone(),
            model: turn.model.clone(),
            seq,
            mandate_json: serde_json::to_string(&mandate)
                .map_err(|err| WorkItemError::Store(format!("encode mandate: {err}")))?,
            last_report_json: None,
            attention_json: None,
            policy_hash_seen: String::new(),
            created_at: now,
            updated_at: now,
        };
        let round = WorkItemRoundRecord {
            item_id: item_id.clone(),
            seq,
            tenant_id: tenant_id.clone(),
            request_id: turn.request_id.clone(),
            query_text: turn.query_text.clone(),
            state: RoundState::Running.as_str().to_string(),
            outcome: None,
            report_json: None,
            error_code: None,
            turn_id: None,
            started_at: now,
            finished_at: None,
        };
        {
            // One writer hold: the cap is checked against the rows this
            // write would join, so two opens cannot both take the last slot.
            let store = self.store.write();
            if self.config.max_open_items > 0 {
                let open = store
                    .count_open_work_items(&tenant_id)
                    .map_err(WorkItemError::Store)?;
                if usize::try_from(open).unwrap_or(usize::MAX) >= self.config.max_open_items {
                    return Err(WorkItemError::OpenLimit {
                        limit: self.config.max_open_items,
                    });
                }
            }
            match &owner {
                None => {
                    create_thread(&store, &tenant_id, &thread_id, now)?;
                    store
                        .create_work_item(&item, &round)
                        .map_err(WorkItemError::Store)?;
                }
                Some(owner) => {
                    // The ownership check and the write are one transaction;
                    // the thread is written only once the item is.
                    store
                        .create_work_item_with_owner(&item, &round, owner)
                        .map_err(WorkItemError::Store)?
                        .map_err(WorkItemError::ConversationOwned)?;
                    create_thread(&store, &tenant_id, &thread_id, now)?;
                }
            }
        }
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::WORK_ITEMS_OPENED_TOTAL,
            &[("basis", mandate.basis.as_str())],
        );
        let view = self.get_summary(&tenant_id, &item_id)?;
        log_change(
            &view,
            "opened",
            view.stage,
            &format!("basis {}", mandate.basis.as_str()),
        );
        self.publish(
            &view,
            "opened",
            json!({
                "strength": mandate.strength.as_str(),
                "threadId": thread_id,
                "roundSeq": seq,
                "requestId": turn.request_id,
            }),
        );
        Ok((view, turn))
    }

    /// Records how a round ended and what that does to its item. A reply
    /// that is not a readable report is recorded as `blocked` with reason
    /// `report_unreadable`, so a person sees the item.
    pub fn settle_round(
        &self,
        turn: &RoundTurn,
        outcome: RoundOutcome,
    ) -> Result<WorkItem, WorkItemError> {
        let before = self.get_summary(&turn.tenant_id, &turn.item_id)?;
        let (round_state, label, report, error_code, turn_id) = match outcome {
            RoundOutcome::Answered {
                structured,
                turn_id,
            } => {
                let read = match structured {
                    Some(value) => read_report(&value),
                    None => Err("the turn answered without a structured reply".to_string()),
                };
                match read {
                    Ok(report) => (
                        RoundState::Settled,
                        report.status.as_str().to_string(),
                        Some(report),
                        None,
                        turn_id,
                    ),
                    Err(detail) => unreadable(&detail, turn_id),
                }
            }
            RoundOutcome::Unreadable { detail, turn_id } => unreadable(&detail, turn_id),
            RoundOutcome::Failed {
                error_code,
                turn_id,
            } => (
                RoundState::Failed,
                error_code.clone(),
                None,
                Some(error_code),
                turn_id,
            ),
        };
        let ends = report.as_ref().and_then(|report| report.status.ends_with());
        let attention = report.as_ref().and_then(|report| report.attention.clone());
        let report_json = report
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| WorkItemError::Store(format!("encode report: {err}")))?;
        let settlement = WorkItemRoundSettlement {
            round_state: round_state.as_str().to_string(),
            outcome: label.clone(),
            report_json: report_json.clone(),
            error_code,
            turn_id: turn_id.clone(),
            finished_at: Utc::now(),
            item_state: if ends.is_some() {
                ItemState::Ended
            } else {
                ItemState::Open
            }
            .as_str()
            .to_string(),
            end_reason: ends.map(|reason| reason.as_str().to_string()),
            last_report_json: report_json,
            attention_json: attention
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|err| WorkItemError::Store(format!("encode attention: {err}")))?,
        };
        let applied = self
            .store
            .write()
            .settle_work_item_round(&turn.tenant_id, &turn.item_id, turn.seq, &settlement)
            .map_err(WorkItemError::Store)?;
        if !applied {
            return Err(WorkItemError::RoundNotRunning(turn.request_id.clone()));
        }

        let metric = match round_state {
            RoundState::Failed => "turn_error",
            _ => label.as_str(),
        };
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::WORK_ITEM_ROUNDS_TOTAL,
            &[("outcome", metric)],
        );
        let after = self.get_summary(&turn.tenant_id, &turn.item_id)?;
        let reason = format!("round {} {}", turn.seq, label);
        log_change(&after, stage_name(before.stage), after.stage, &reason);
        let mut payload = json!({
            "roundSeq": turn.seq,
            "requestId": turn.request_id,
            "state": round_state.as_str(),
            "outcome": label,
        });
        if let Some(stage) = after.stage {
            payload["stage"] = json!(stage.as_str());
        }
        if let Some(turn_id) = &turn_id {
            payload["turnId"] = json!(turn_id);
        }
        self.publish(&after, "round_settled", payload);
        if let Some(attention) = &attention {
            self.publish(
                &after,
                "attention_raised",
                json!({"roundSeq": turn.seq, "optionCount": attention.options.len()}),
            );
        }
        if let Some(reason) = ends {
            self.publish(
                &after,
                "ended",
                json!({"endReason": reason.as_str(), "roundSeq": turn.seq}),
            );
        }
        Ok(after)
    }

    /// The item with its rounds and inputs, if `tenant_id` owns it.
    pub fn get(&self, tenant_id: &str, item_id: &str) -> Result<WorkItem, WorkItemError> {
        let store = self.store.read();
        let (record, activity) = store
            .get_work_item(tenant_id.trim(), item_id.trim())
            .map_err(WorkItemError::Store)?
            .ok_or(WorkItemError::NotFound)?;
        let rounds = store
            .list_work_item_rounds(&record.tenant_id, &record.item_id)
            .map_err(WorkItemError::Store)?;
        let inputs = store
            .list_work_item_inputs(&record.tenant_id, &record.item_id)
            .map_err(WorkItemError::Store)?;
        let subscriptions = store
            .list_work_item_subscriptions(&record.tenant_id, &record.item_id)
            .map_err(WorkItemError::Store)?;
        let outbox = store
            .list_work_item_outbox(&record.tenant_id, &record.item_id)
            .map_err(WorkItemError::Store)?;
        drop(store);
        let mut item = to_view(record, &activity)?;
        item.rounds = Some(
            rounds
                .into_iter()
                .map(round_view)
                .collect::<Result<_, _>>()?,
        );
        item.inputs = Some(
            inputs
                .into_iter()
                .map(input_view)
                .collect::<Result<_, _>>()?,
        );
        item.subscriptions = Some(
            subscriptions
                .into_iter()
                .map(subscription_view)
                .collect::<Result<_, _>>()?,
        );
        item.outbox = Some(
            outbox
                .into_iter()
                .map(crate::outbox::row_view)
                .collect::<Result<_, _>>()?,
        );
        Ok(item)
    }

    /// The tenant's items, most recently updated first, without rounds or
    /// inputs. `limit` is clamped to 1..=[`MAX_LIST_LIMIT`].
    pub fn list(
        &self,
        tenant_id: &str,
        state: Option<ItemState>,
        limit: usize,
    ) -> Result<Vec<WorkItem>, WorkItemError> {
        let rows = self
            .store
            .read()
            .list_work_items(
                tenant_id.trim(),
                state.map(ItemState::as_str),
                limit.clamp(1, MAX_LIST_LIMIT),
            )
            .map_err(WorkItemError::Store)?;
        rows.into_iter()
            .map(|(record, activity)| to_view(record, &activity))
            .collect()
    }

    pub(crate) fn get_summary(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<WorkItem, WorkItemError> {
        let (record, activity) = self
            .store
            .read()
            .get_work_item(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
            .ok_or(WorkItemError::NotFound)?;
        to_view(record, &activity)
    }

    /// Records the event and announces it. Events carry the tenant, the
    /// item and the mandate's basis; never a title, a scope or a report.
    pub(crate) fn publish(&self, item: &WorkItem, name: &str, extra: Value) {
        let mut payload = Map::new();
        payload.insert("itemId".to_string(), json!(item.item_id));
        payload.insert("basis".to_string(), json!(item.mandate.basis.as_str()));
        if let Value::Object(extra) = extra {
            payload.extend(extra);
        }
        let event = Event {
            category: "work_item".to_string(),
            name: format!("work_item.{name}"),
            tenant_id: item.tenant_id.clone(),
            resource: Resource {
                kind: "work_item".to_string(),
                id: item.item_id.clone(),
            },
            payload,
            ..Event::default()
        };
        let event = match self.store.write().append_event(&event) {
            Ok(recorded) => recorded,
            Err(err) => {
                eprintln!(
                    "[kura] work item: recording event {} for {} failed: {err}",
                    event.name, item.item_id
                );
                event
            }
        };
        self.bus.publish(event);
    }
}

/// What [`Manager::open`] refuses before touching the store, so a caller
/// can refuse a request before it spends anything on it.
pub fn check_open(request: &OpenRequest) -> Result<(), WorkItemError> {
    if request.tenant_id.trim().is_empty() {
        return Err(WorkItemError::Invalid(
            "a work item needs a tenant".to_string(),
        ));
    }
    check_text("title", request.title.trim(), MAX_TITLE_CHARS)?;
    check_text("scope", request.scope.trim(), MAX_SCOPE_CHARS)
}

/// A round's chat request id: `<item>_r<seq>`, so `wi_<hex>_r<seq>`. The
/// same round always gets the same id, which is what lets a rerun find its
/// checkpoint.
#[must_use]
pub fn round_request_id(item_id: &str, seq: i64) -> String {
    format!("{item_id}_r{seq}")
}

type Settled = (
    RoundState,
    String,
    Option<Report>,
    Option<String>,
    Option<String>,
);

fn unreadable(detail: &str, turn_id: Option<String>) -> Settled {
    (
        RoundState::Settled,
        "report_unreadable".to_string(),
        Some(unreadable_report(detail)),
        None,
        turn_id,
    )
}

fn check_text(field: &str, value: &str, max_chars: usize) -> Result<(), WorkItemError> {
    if value.is_empty() {
        return Err(WorkItemError::Invalid(format!("{field} is required")));
    }
    if value.chars().count() > max_chars {
        return Err(WorkItemError::Invalid(format!(
            "{field} is longer than {max_chars} characters"
        )));
    }
    Ok(())
}

/// The item's own thread: its rounds' history is the item's history (D1).
fn create_thread(
    store: &kura_store::SQLiteStore,
    tenant_id: &str,
    thread_id: &str,
    now: DateTime<Utc>,
) -> Result<(), WorkItemError> {
    let retention = store
        .thread_retention_expiry(tenant_id, now)
        .map_err(WorkItemError::Store)?;
    let segment_id = format!("seg_{thread_id}");
    store
        .upsert_thread(&kura_threads::Thread {
            thread_id: thread_id.to_string(),
            tenant_id: tenant_id.to_string(),
            lifecycle_state: kura_threads::LifecycleState::Active,
            current_session_segment_id: segment_id.clone(),
            source_kind: kura_threads::SourceKind::Chat,
            source_summary: "Work item".to_string(),
            last_activity_at: now,
            created_at: now,
            updated_at: now,
            retention_expires_at: Some(retention),
            redaction_status: kura_threads::RedactionStatus::Redacted,
        })
        .map_err(WorkItemError::Store)?;
    store
        .upsert_thread_session_segment(&kura_threads::SessionSegment {
            session_segment_id: segment_id,
            thread_id: thread_id.to_string(),
            tenant_id: tenant_id.to_string(),
            session_id: String::new(),
            generation: 1,
            state: "active".to_string(),
            started_at: now,
            ended_at: None,
            last_active_at: now,
            reset_from_session_segment_id: String::new(),
            partial_evidence: false,
        })
        .map_err(WorkItemError::Store)
}

fn decode<T: serde::de::DeserializeOwned>(what: &str, json: &str) -> Result<T, WorkItemError> {
    serde_json::from_str(json).map_err(|err| WorkItemError::Store(format!("decode {what}: {err}")))
}

pub(crate) fn to_view(
    record: WorkItemRecord,
    activity: &WorkItemActivity,
) -> Result<WorkItem, WorkItemError> {
    let state = ItemState::parse(&record.state).map_err(WorkItemError::Store)?;
    let end_reason = record
        .end_reason
        .as_deref()
        .map(EndReason::parse)
        .transpose()
        .map_err(WorkItemError::Store)?;
    let mandate: Mandate = decode("mandate", &record.mandate_json)?;
    let last_report: Option<Report> = record
        .last_report_json
        .as_deref()
        .map(|json| decode("report", json))
        .transpose()?;
    let attention: Option<Attention> = record
        .attention_json
        .as_deref()
        .map(|json| decode("attention", json))
        .transpose()?;
    let last_round = activity
        .last_round_state
        .as_deref()
        .map(RoundState::parse)
        .transpose()
        .map_err(WorkItemError::Store)?;
    let stage = derive_stage(&StageFacts {
        ended: state == ItemState::Ended,
        // A reply that failed 3 times in a row holds the item here too,
        // though the outbox keeps retrying it (§8).
        delivery_problem: last_round == Some(RoundState::Failed) || activity.outbox_problem,
        // Linking approvals arrives with slice 5.
        approval_pending: false,
        attention: attention.is_some(),
        last_report: last_report.as_ref().map(|report| report.status),
        // A round consumes every input pending when it starts, and a round
        // only starts past a blocked or failed report on a person's input:
        // a running round means a person spoke since the report.
        person_input_since_report: activity.pending_person_inputs > 0
            || last_round == Some(RoundState::Running),
        pending_inputs: activity.pending_inputs > 0,
        round_running: last_round == Some(RoundState::Running),
    });
    Ok(WorkItem {
        item_id: record.item_id,
        tenant_id: record.tenant_id,
        title: record.title,
        state,
        end_reason,
        stage,
        thread_id: record.thread_id,
        provider: record.provider,
        model: record.model,
        seq: record.seq,
        mandate,
        last_report,
        attention,
        created_at: record.created_at,
        updated_at: record.updated_at,
        rounds: None,
        inputs: None,
        subscriptions: None,
        outbox: None,
    })
}

fn subscription_view(record: WorkItemSubscriptionRecord) -> Result<Subscription, WorkItemError> {
    Ok(Subscription {
        subscription_id: record.subscription_id,
        source_kind: record.source_kind,
        source_key: record.source_key,
        connector_id: record.connector_id,
        conversation_id: record.conversation_id,
        role: SubscriptionRole::parse(&record.role).map_err(WorkItemError::Store)?,
        created_at: record.created_at,
        last_event_at: record.last_event_at,
        dropped_at: record.dropped_at,
        drop_reason: record.drop_reason,
    })
}

fn round_view(record: WorkItemRoundRecord) -> Result<Round, WorkItemError> {
    Ok(Round {
        seq: record.seq,
        request_id: record.request_id,
        state: RoundState::parse(&record.state).map_err(WorkItemError::Store)?,
        query_text: record.query_text,
        outcome: record.outcome,
        report: record
            .report_json
            .as_deref()
            .map(|json| decode("round report", json))
            .transpose()?,
        error_code: record.error_code,
        turn_id: record.turn_id,
        started_at: record.started_at,
        finished_at: record.finished_at,
    })
}

pub(crate) fn input_view(record: WorkItemInputRecord) -> Result<Input, WorkItemError> {
    Ok(Input {
        seq: record.seq,
        kind: InputKind::parse(&record.kind).map_err(WorkItemError::Store)?,
        author_is_person: record.author_is_person,
        body: record.body,
        source_ref: record.source_ref,
        state: InputState::parse(&record.state).map_err(WorkItemError::Store)?,
        round_seq: record.round_seq,
        created_at: record.created_at,
    })
}

pub(crate) fn stage_name(stage: Option<Stage>) -> &'static str {
    stage.map_or("ended", Stage::as_str)
}

/// One line per change: tenant, item, from -> to, and why.
pub(crate) fn log_change(item: &WorkItem, from: &str, to: Option<Stage>, reason: &str) {
    let to = match (to, item.end_reason) {
        (Some(stage), _) => stage.as_str().to_string(),
        (None, Some(reason)) => format!("ended/{}", reason.as_str()),
        (None, None) => "ended".to_string(),
    };
    eprintln!(
        "[kura] work item: tenant={} item={} {from} -> {to} ({reason})",
        item.tenant_id, item.item_id
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ReportStatus;

    fn manager(config: WorkItemsConfig) -> (Manager, Arc<Bus>) {
        let dir =
            std::env::temp_dir().join(format!("kura-workitems-{}", uuid::Uuid::now_v7().simple()));
        let store = kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store");
        let pool = Arc::new(StorePool::writer_only(Arc::new(parking_lot::Mutex::new(
            store,
        ))));
        let bus = Arc::new(Bus::new());
        (Manager::new(pool, bus.clone(), config), bus)
    }

    fn enabled() -> WorkItemsConfig {
        WorkItemsConfig {
            enabled: true,
            ..WorkItemsConfig::default()
        }
    }

    fn open_as(manager: &Manager, tenant: &str) -> (WorkItem, RoundTurn) {
        manager
            .open(OpenRequest {
                tenant_id: tenant.to_string(),
                principal_id: "prn_1".to_string(),
                request_ref: "req_1".to_string(),
                title: " Fix CI ".to_string(),
                scope: "Find why main fails and fix it.".to_string(),
                ..OpenRequest::default()
            })
            .expect("open")
    }

    fn answered(value: Value) -> RoundOutcome {
        RoundOutcome::Answered {
            structured: Some(value),
            turn_id: Some("trn_1".to_string()),
        }
    }

    #[test]
    fn an_opened_item_is_working_on_its_first_round_under_an_api_mandate() {
        let (manager, _) = manager(enabled());
        let (item, turn) = open_as(&manager, "ten_a");
        assert_eq!(item.state, ItemState::Open);
        assert_eq!(item.stage, Some(Stage::Working));
        assert_eq!(item.title, "Fix CI");
        assert_eq!(item.mandate.basis, Basis::Api);
        assert_eq!(item.mandate.strength, Strength::Explicit);
        assert_eq!(item.mandate.sender.principal_id, "prn_1");
        assert_eq!(item.mandate.scope_text, "Find why main fails and fix it.");
        assert!(item.item_id.starts_with("wi_"));
        assert_eq!(item.thread_id, format!("thr_{}", item.item_id));
        assert_eq!(turn.request_id, format!("{}_r1", item.item_id));
        assert_eq!(turn.seq, 1);

        let read = manager.get("ten_a", &item.item_id).expect("get");
        let rounds = read.rounds.expect("rounds");
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].state, RoundState::Running);
        assert_eq!(rounds[0].query_text, turn.query_text);
        assert!(read.inputs.expect("inputs").is_empty());
        // The item's own thread exists, so later rounds have continuity.
        let thread = manager
            .store
            .read()
            .get_thread_for_tenant("ten_a", &item.thread_id)
            .expect("thread lookup");
        assert!(thread.is_some());
    }

    #[test]
    fn each_report_status_moves_the_item_as_the_design_says() {
        let (manager, _) = manager(enabled());
        for (status, state, stage, end) in [
            ("progress", ItemState::Open, Some(Stage::Idle), None),
            ("waiting", ItemState::Open, Some(Stage::Idle), None),
            ("blocked", ItemState::Open, Some(Stage::PersonBlocked), None),
            ("failed", ItemState::Open, Some(Stage::PersonFailed), None),
            ("ready", ItemState::Ended, None, Some(EndReason::Completed)),
            ("dropped", ItemState::Ended, None, Some(EndReason::Dropped)),
        ] {
            let (_, turn) = open_as(&manager, "ten_a");
            let item = manager
                .settle_round(&turn, answered(json!({"status": status, "summary": "s"})))
                .expect("settle");
            assert_eq!(item.state, state, "{status}");
            assert_eq!(item.stage, stage, "{status}");
            assert_eq!(item.end_reason, end, "{status}");
            assert_eq!(
                item.last_report.as_ref().map(|r| r.status.as_str()),
                Some(status)
            );
        }
    }

    #[test]
    fn a_report_that_cannot_be_read_blocks_the_item_as_report_unreadable() {
        let (manager, _) = manager(enabled());
        for outcome in [
            RoundOutcome::Unreadable {
                detail: "the reply is not JSON".to_string(),
                turn_id: Some("trn_1".to_string()),
            },
            answered(json!({"status": "finished", "summary": "s"})),
            RoundOutcome::Answered {
                structured: None,
                turn_id: None,
            },
        ] {
            let (_, turn) = open_as(&manager, "ten_a");
            let item = manager.settle_round(&turn, outcome).expect("settle");
            assert_eq!(item.state, ItemState::Open);
            assert_eq!(item.stage, Some(Stage::PersonBlocked));
            let report = item.last_report.expect("report");
            assert_eq!(report.status, ReportStatus::Blocked);
            assert_eq!(
                report.reason,
                Some(crate::types::ReportReason::ReportUnreadable)
            );
            let round = manager.get("ten_a", &item.item_id).unwrap().rounds.unwrap();
            assert_eq!(round[0].state, RoundState::Settled);
            assert_eq!(round[0].outcome.as_deref(), Some("report_unreadable"));
        }
    }

    #[test]
    fn a_turn_that_failed_shows_the_item_as_not_delivered() {
        let (manager, _) = manager(enabled());
        let (_, turn) = open_as(&manager, "ten_a");
        let item = manager
            .settle_round(
                &turn,
                RoundOutcome::Failed {
                    error_code: "provider".to_string(),
                    turn_id: None,
                },
            )
            .expect("settle");
        assert_eq!(item.stage, Some(Stage::PersonDelivery));
        assert!(item.last_report.is_none());
        let rounds = manager.get("ten_a", &item.item_id).unwrap().rounds.unwrap();
        assert_eq!(rounds[0].state, RoundState::Failed);
        assert_eq!(rounds[0].error_code.as_deref(), Some("provider"));
    }

    #[test]
    fn attention_in_a_report_asks_a_person_and_a_round_settles_once() {
        let (manager, bus) = manager(enabled());
        let (events, _unsubscribe) = bus.subscribe(kura_events::Filter::default());
        let (_, turn) = open_as(&manager, "ten_a");
        let outcome = answered(json!({
            "status": "waiting",
            "summary": "Two fixes are possible.",
            "attention": {"question": "Which one?", "options": ["pin", "patch"]}
        }));
        let item = manager
            .settle_round(&turn, outcome.clone())
            .expect("settle");
        assert_eq!(item.stage, Some(Stage::PersonAnswer));
        assert_eq!(item.attention.expect("attention").question, "Which one?");
        assert_eq!(
            manager.settle_round(&turn, outcome),
            Err(WorkItemError::RoundNotRunning(turn.request_id.clone()))
        );

        let names: Vec<String> = std::iter::from_fn(|| events.try_recv().ok())
            .map(|event| {
                assert_eq!(event.tenant_id, "ten_a");
                // Events never carry what was said.
                let body = serde_json::to_string(&event.payload).unwrap();
                assert!(!body.contains("Which one?") && !body.contains("Fix CI"));
                event.name
            })
            .collect();
        assert_eq!(
            names,
            [
                "work_item.opened",
                "work_item.round_settled",
                "work_item.attention_raised"
            ]
        );
    }

    #[test]
    fn another_tenants_item_is_not_found_and_not_listed() {
        let (manager, _) = manager(enabled());
        let (mine, _) = open_as(&manager, "ten_a");
        let (theirs, _) = open_as(&manager, "ten_b");
        assert_eq!(
            manager.get("ten_b", &mine.item_id),
            Err(WorkItemError::NotFound)
        );
        let listed: Vec<String> = manager
            .list("ten_b", None, 50)
            .unwrap()
            .into_iter()
            .map(|item| item.item_id)
            .collect();
        assert_eq!(listed, [theirs.item_id]);
    }

    #[test]
    fn the_open_cap_counts_only_open_items_of_the_tenant() {
        let (manager, _) = manager(WorkItemsConfig {
            enabled: true,
            max_open_items: 1,
            ..WorkItemsConfig::default()
        });
        let (_, turn) = open_as(&manager, "ten_a");
        let refused = manager.open(OpenRequest {
            tenant_id: "ten_a".to_string(),
            title: "t".to_string(),
            scope: "s".to_string(),
            ..OpenRequest::default()
        });
        assert_eq!(refused.unwrap_err(), WorkItemError::OpenLimit { limit: 1 });
        // Another tenant has its own cap.
        open_as(&manager, "ten_b");
        // An ended item frees the slot.
        manager
            .settle_round(&turn, answered(json!({"status": "ready", "summary": "s"})))
            .unwrap();
        open_as(&manager, "ten_a");
    }

    #[test]
    fn an_open_without_a_tenant_title_or_scope_is_refused() {
        let (manager, _) = manager(enabled());
        for request in [
            OpenRequest {
                title: "t".to_string(),
                scope: "s".to_string(),
                ..OpenRequest::default()
            },
            OpenRequest {
                tenant_id: "ten_a".to_string(),
                title: "  ".to_string(),
                scope: "s".to_string(),
                ..OpenRequest::default()
            },
            OpenRequest {
                tenant_id: "ten_a".to_string(),
                title: "t".to_string(),
                scope: "x".repeat(MAX_SCOPE_CHARS + 1),
                ..OpenRequest::default()
            },
        ] {
            assert!(matches!(
                manager.open(request),
                Err(WorkItemError::Invalid(_))
            ));
        }
        assert!(manager.list("ten_a", None, 50).unwrap().is_empty());
    }
}
