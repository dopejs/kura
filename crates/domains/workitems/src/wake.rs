//! Inputs, the start of a round, and the ends a person or a revalidation
//! brings (work-items.md §5.2–§5.4, slice 2).
//!
//! Every decision that depends on what the item's rows say is taken under
//! the store's writer lock, together with the write that acts on it: an
//! input cannot slip between "a round is running" and "steer it", and two
//! wakes cannot both start the same round.

use chrono::{Duration, Utc};
use kura_store::SQLiteStore;
use kura_store::work_items::{WorkItemInputRecord, WorkItemRoundRecord};
use serde_json::json;

use crate::manager::{
    Manager, RoundTurn, WorkItemError, log_change, round_request_id, stage_name, to_view,
};
use crate::report::{QuotedInput, frame_input, round_instructions, round_query};
use crate::types::{
    EndReason, InputKind, ItemState, ReportStatus, RevokeReason, RoundState, Stage, WorkItem,
};

/// How an input reached its item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPath {
    /// Delivered to the running round's turn; the model reads it before its
    /// next call.
    Steered { round_seq: i64 },
    /// Waits for the next round.
    Queued,
}

impl InputPath {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Steered { .. } => "steered",
            Self::Queued => "queued",
        }
    }
}

/// Why no round starts now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoRound {
    Ended,
    /// A round of the item is running; it picks up what is pending when it
    /// settles.
    Running,
    NothingPending,
    /// The item waits for a person (a failed round, a question, a blocked
    /// or failed report) and nothing pending is a person's.
    Held,
    /// The item started its rounds for this rolling hour (D7).
    HourlyCap {
        limit: u32,
    },
}

/// The round that would start now, as read from the store.
struct Planned {
    turn: RoundTurn,
    item: WorkItem,
    /// The last pending input the round's question quotes.
    upto_input_seq: i64,
    /// A person's input answers the item's question.
    clear_attention: bool,
}

impl Manager {
    /// Records an input for an open item (§3.3). While a round runs it is
    /// steered into that round's turn; otherwise, or when the turn refuses
    /// it (`turn_finished`, `turn_inbox_full`), it waits for the next round.
    /// Nothing is dropped.
    ///
    /// Slice 2 records a person's input only from the mandate's own sender,
    /// so `author_is_person` also marks the input as the requester's.
    pub fn add_input(
        &self,
        tenant_id: &str,
        item_id: &str,
        kind: InputKind,
        author_is_person: bool,
        body: &str,
        source_ref: &str,
    ) -> Result<(i64, InputPath), WorkItemError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(WorkItemError::Invalid("an input needs a body".to_string()));
        }
        let store = self.store.write();
        let (record, activity) = store
            .get_work_item(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
            .ok_or(WorkItemError::NotFound)?;
        if record.state != ItemState::Open.as_str() {
            return Err(WorkItemError::Invalid(
                "the work item has ended".to_string(),
            ));
        }
        let mut path = InputPath::Queued;
        if activity.last_round_state.as_deref() == Some(RoundState::Running.as_str()) {
            if let Some(chat) = &self.chat {
                let text = frame_input(&QuotedInput {
                    kind: kind.as_str(),
                    from_requester: author_is_person,
                    body,
                });
                // The writer lock is held: the round cannot settle, nor a
                // new one start, between this send and the write below.
                if chat
                    .steer_turn(tenant_id, &round_request_id(item_id, record.seq), &text)
                    .is_ok()
                {
                    path = InputPath::Steered {
                        round_seq: record.seq,
                    };
                }
            }
        }
        let (state, round_seq) = match path {
            InputPath::Steered { round_seq } => ("steered", Some(round_seq)),
            InputPath::Queued => ("pending", None),
        };
        let seq = store
            .append_work_item_input(&WorkItemInputRecord {
                item_id: record.item_id.clone(),
                seq: 0,
                tenant_id: record.tenant_id.clone(),
                kind: kind.as_str().to_string(),
                author_is_person,
                body: body.to_string(),
                source_ref: source_ref.trim().to_string(),
                state: state.to_string(),
                round_seq,
                created_at: Utc::now(),
            })
            .map_err(WorkItemError::Store)?;
        drop(store);
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::WORK_ITEM_INPUTS_TOTAL,
            &[("kind", kind.as_str()), ("path", path.as_str())],
        );
        Ok((seq, path))
    }

    /// Starts the item's next round if one should run now (§5.3): the item
    /// is open, no round of it runs, inputs are pending, nothing holds it
    /// for a person (a person's input releases any hold), and the hourly
    /// cap allows. The pending inputs become the round's question and are
    /// consumed by it; a person's input clears the item's question.
    pub(crate) fn start_round(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Result<RoundTurn, NoRound>, WorkItemError> {
        let store = self.store.write();
        let planned = match self.plan_round(&store, tenant_id, item_id)? {
            Ok(planned) => planned,
            Err(no) => return Ok(Err(no)),
        };
        let round = WorkItemRoundRecord {
            item_id: planned.turn.item_id.clone(),
            seq: planned.turn.seq,
            tenant_id: planned.turn.tenant_id.clone(),
            request_id: planned.turn.request_id.clone(),
            query_text: planned.turn.query_text.clone(),
            state: RoundState::Running.as_str().to_string(),
            outcome: None,
            report_json: None,
            error_code: None,
            turn_id: None,
            started_at: Utc::now(),
            finished_at: None,
        };
        let started = store
            .start_work_item_round(&round, planned.upto_input_seq, planned.clear_attention)
            .map_err(WorkItemError::Store)?;
        drop(store);
        if !started {
            return Ok(Err(NoRound::Running));
        }
        let turn = planned.turn;
        if planned.clear_attention {
            self.publish(
                &planned.item,
                "attention_cleared",
                json!({"roundSeq": turn.seq, "by": "round"}),
            );
        }
        log_change(
            &planned.item,
            stage_name(planned.item.stage),
            Some(Stage::Working),
            &format!("round {} started", turn.seq),
        );
        Ok(Ok(turn))
    }

    /// Whether a round would start now, without starting it: the driver
    /// asks before it takes a turn slot, so a slot (and the turn rate) is
    /// not spent on an item with nothing to do.
    pub(crate) fn round_due(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Result<(), NoRound>, WorkItemError> {
        let store = self.store.read();
        Ok(self.plan_round(&store, tenant_id, item_id)?.map(|_| ()))
    }

    fn plan_round(
        &self,
        store: &SQLiteStore,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Result<Planned, NoRound>, WorkItemError> {
        let (record, activity) = store
            .get_work_item(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
            .ok_or(WorkItemError::NotFound)?;
        let last_round = activity.last_round_state.clone();
        let item = to_view(record, &activity)?;
        if item.state != ItemState::Open {
            return Ok(Err(NoRound::Ended));
        }
        if last_round.as_deref() == Some(RoundState::Running.as_str()) {
            return Ok(Err(NoRound::Running));
        }
        let pending: Vec<_> = store
            .list_work_item_inputs(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
            .into_iter()
            .filter(|input| input.state == "pending")
            .collect();
        let Some(last) = pending.last() else {
            return Ok(Err(NoRound::NothingPending));
        };
        let upto_input_seq = last.seq;
        let person = pending.iter().any(|input| input.author_is_person);
        let held = last_round.as_deref() == Some(RoundState::Failed.as_str())
            || item.attention.is_some()
            || matches!(
                item.last_report.as_ref().map(|report| report.status),
                Some(ReportStatus::Blocked | ReportStatus::Failed)
            );
        if held && !person {
            return Ok(Err(NoRound::Held));
        }
        let limit = self.config.max_rounds_per_item_per_hour;
        if limit > 0 {
            let started = store
                .count_work_item_rounds_since(tenant_id, item_id, Utc::now() - Duration::hours(1))
                .map_err(WorkItemError::Store)?;
            if started >= i64::from(limit) {
                return Ok(Err(NoRound::HourlyCap { limit }));
            }
        }
        let seq = item.seq + 1;
        let quoted: Vec<QuotedInput<'_>> = pending
            .iter()
            .map(|input| QuotedInput {
                kind: &input.kind,
                from_requester: input.author_is_person,
                body: &input.body,
            })
            .collect();
        let turn = RoundTurn {
            tenant_id: item.tenant_id.clone(),
            item_id: item.item_id.clone(),
            seq,
            request_id: round_request_id(&item.item_id, seq),
            thread_id: item.thread_id.clone(),
            query_text: round_query(seq, &quoted),
            instructions: round_instructions(&item.title, &item.mandate.scope_text),
            provider: item.provider.clone(),
            model: item.model.clone(),
        };
        let clear_attention = person && item.attention.is_some();
        Ok(Ok(Planned {
            turn,
            item,
            upto_input_seq,
            clear_attention,
        }))
    }

    /// Stops the item for good (§5.4: a person, through the API). Its
    /// subscriptions are dropped and a running round's turn is asked to
    /// cancel; whatever that round reports, the item stays stopped.
    /// Stopping a stopped item changes nothing.
    pub fn stop(&self, tenant_id: &str, item_id: &str) -> Result<WorkItem, WorkItemError> {
        let (tenant_id, item_id) = (tenant_id.trim(), item_id.trim());
        let (record, activity) = self
            .store
            .read()
            .get_work_item(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
            .ok_or(WorkItemError::NotFound)?;
        let running = activity.last_round_state.as_deref() == Some(RoundState::Running.as_str());
        let seq = record.seq;
        let before = to_view(record, &activity)?;
        if before.end_reason == Some(EndReason::Stopped) {
            return Ok(before);
        }
        let applied = self
            .store
            .write()
            .end_work_item_finally(tenant_id, item_id, EndReason::Stopped.as_str(), Utc::now())
            .map_err(WorkItemError::Store)?;
        if !applied {
            // Another stop got there first.
            return self.get_summary(tenant_id, item_id);
        }
        let cancelled = running
            && self.chat.as_ref().is_some_and(|chat| {
                chat.cancel_turn(tenant_id, &round_request_id(item_id, seq))
                    .is_ok()
            });
        let after = self.get_summary(tenant_id, item_id)?;
        log_change(
            &after,
            stage_name(before.stage),
            None,
            "stopped by a person",
        );
        self.publish(&after, "stopped", json!({"roundCancelled": cancelled}));
        Ok(after)
    }

    /// Ends an item that ended on its own report as `revoked`: its mandate
    /// no longer holds (§5.4). Its subscriptions are dropped, so the
    /// conversation is free again.
    pub(crate) fn revoke(
        &self,
        item: &WorkItem,
        reason: RevokeReason,
    ) -> Result<(), WorkItemError> {
        let applied = self
            .store
            .write()
            .end_work_item_finally(
                &item.tenant_id,
                &item.item_id,
                EndReason::Revoked.as_str(),
                Utc::now(),
            )
            .map_err(WorkItemError::Store)?;
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::WORK_ITEM_MANDATE_CHECKS_TOTAL,
            &[("result", "revoked")],
        );
        if applied {
            let after = self.get_summary(&item.tenant_id, &item.item_id)?;
            log_change(
                &after,
                "ended",
                None,
                &format!("mandate revoked: {}", reason.as_str()),
            );
            self.publish(
                &after,
                "mandate_revoked",
                json!({"reason": reason.as_str()}),
            );
        }
        Ok(())
    }

    /// Opens again an item that ended on its own report, for a person's
    /// message that passed revalidation. `false` at the open-items cap or
    /// when the item is no longer one that can reopen.
    pub(crate) fn reopen(&self, item: &WorkItem) -> Result<bool, WorkItemError> {
        let store = self.store.write();
        if self.config.max_open_items > 0 {
            let open = store
                .count_open_work_items(&item.tenant_id)
                .map_err(WorkItemError::Store)?;
            if usize::try_from(open).unwrap_or(usize::MAX) >= self.config.max_open_items {
                return Ok(false);
            }
        }
        store
            .reopen_work_item(&item.tenant_id, &item.item_id, Utc::now())
            .map_err(WorkItemError::Store)
    }

    /// The `api` basis check (§5.4): the principal is still an active
    /// member of the tenant. Every role may chat, so any active membership
    /// passes. An item opened without a principal (a daemon without auth,
    /// or a web or TUI turn, whose principal the tool does not see) fails
    /// closed.
    pub(crate) fn principal_is_member(
        &self,
        tenant_id: &str,
        principal_id: &str,
    ) -> Result<bool, WorkItemError> {
        let principal_id = principal_id.trim();
        if principal_id.is_empty() {
            return Ok(false);
        }
        let memberships = self
            .store
            .read()
            .list_principal_memberships(principal_id)
            .map_err(WorkItemError::Store)?;
        Ok(memberships.iter().any(|membership| {
            membership.tenant_id == tenant_id
                && membership.status == kura_identity::LifecycleStatus::Active
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kura_events::Bus;
    use kura_store::StorePool;
    use serde_json::json;

    use super::*;
    use crate::manager::{OpenRequest, RoundOutcome};
    use crate::types::{Basis, WorkItemsConfig};

    fn manager(config: WorkItemsConfig) -> Manager {
        let dir = std::env::temp_dir().join(format!(
            "kura-workitems-wake-{}",
            uuid::Uuid::now_v7().simple()
        ));
        let store = kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store");
        let pool = Arc::new(StorePool::writer_only(Arc::new(parking_lot::Mutex::new(
            store,
        ))));
        Manager::new(pool, Arc::new(Bus::new()), config)
    }

    fn enabled() -> WorkItemsConfig {
        WorkItemsConfig {
            enabled: true,
            ..WorkItemsConfig::default()
        }
    }

    fn open(manager: &Manager, principal: &str) -> (WorkItem, RoundTurn) {
        manager
            .open(OpenRequest {
                tenant_id: "ten_a".to_string(),
                principal_id: principal.to_string(),
                title: "Fix CI".to_string(),
                scope: "Fix CI on main.".to_string(),
                ..OpenRequest::default()
            })
            .expect("open")
    }

    fn report(manager: &Manager, turn: &RoundTurn, status: &str) {
        manager
            .settle_round(
                turn,
                RoundOutcome::Answered {
                    structured: Some(json!({"status": status, "summary": "s"})),
                    turn_id: None,
                },
            )
            .expect("settle");
    }

    #[test]
    fn an_input_waits_while_a_round_runs_without_a_turn_to_steer() {
        let manager = manager(enabled());
        let (item, _) = open(&manager, "prn_1");
        // No chat service: nothing can be steered, so the input waits.
        let (seq, path) = manager
            .add_input(
                "ten_a",
                &item.item_id,
                InputKind::Message,
                true,
                " hi ",
                "d1",
            )
            .expect("input");
        assert_eq!((seq, path), (1, InputPath::Queued));
        assert_eq!(
            manager.start_round("ten_a", &item.item_id).unwrap(),
            Err(NoRound::Running)
        );
        assert!(matches!(
            manager.add_input("ten_a", &item.item_id, InputKind::Message, true, "  ", ""),
            Err(WorkItemError::Invalid(_))
        ));
        assert_eq!(
            manager.add_input("ten_b", &item.item_id, InputKind::Message, true, "x", ""),
            Err(WorkItemError::NotFound)
        );
    }

    #[test]
    fn a_person_releases_a_hold_and_answers_the_question_but_others_do_not() {
        let manager = manager(enabled());
        let (item, turn) = open(&manager, "prn_1");
        manager
            .settle_round(
                &turn,
                RoundOutcome::Answered {
                    structured: Some(json!({
                        "status": "blocked",
                        "summary": "Two fixes.",
                        "attention": {"question": "Which one?"}
                    })),
                    turn_id: None,
                },
            )
            .unwrap();
        manager
            .add_input(
                "ten_a",
                &item.item_id,
                InputKind::Event,
                false,
                "CI red",
                "",
            )
            .unwrap();
        assert_eq!(
            manager.start_round("ten_a", &item.item_id).unwrap(),
            Err(NoRound::Held)
        );
        manager
            .add_input(
                "ten_a",
                &item.item_id,
                InputKind::Message,
                true,
                "pin it",
                "",
            )
            .unwrap();
        let next = manager
            .start_round("ten_a", &item.item_id)
            .unwrap()
            .expect("a person's input starts a round");
        assert_eq!(next.seq, 2);
        assert_eq!(next.request_id, format!("{}_r2", item.item_id));
        // Both inputs are quoted, each as who wrote it; the mandate is in
        // the instructions only.
        assert!(next.query_text.contains("from=\"other\">\nCI red"));
        assert!(next.query_text.contains("from=\"requester\">\npin it"));
        assert!(next.instructions.contains("Fix CI on main."));
        assert!(!next.query_text.contains("Fix CI on main."));
        let read = manager.get("ten_a", &item.item_id).unwrap();
        assert!(read.attention.is_none(), "the person answered the question");
        assert_eq!(read.stage, Some(Stage::Working));
        assert!(
            read.inputs
                .unwrap()
                .iter()
                .all(|input| input.round_seq == Some(2))
        );
        // Only one round at a time.
        assert_eq!(
            manager.start_round("ten_a", &item.item_id).unwrap(),
            Err(NoRound::Running)
        );
    }

    #[test]
    fn the_hourly_cap_keeps_inputs_queued() {
        let manager = manager(WorkItemsConfig {
            max_rounds_per_item_per_hour: 1,
            ..enabled()
        });
        let (item, turn) = open(&manager, "prn_1");
        report(&manager, &turn, "waiting");
        manager
            .add_input("ten_a", &item.item_id, InputKind::Message, true, "go", "")
            .unwrap();
        assert_eq!(
            manager.start_round("ten_a", &item.item_id).unwrap(),
            Err(NoRound::HourlyCap { limit: 1 })
        );
        let read = manager.get("ten_a", &item.item_id).unwrap();
        assert_eq!(read.stage, Some(Stage::Queued));
        assert_eq!(
            manager.round_due("ten_a", &item.item_id).unwrap(),
            Err(NoRound::HourlyCap { limit: 1 })
        );
    }

    #[test]
    fn a_stop_is_final_even_for_the_round_still_running() {
        let manager = manager(enabled());
        let (item, turn) = open(&manager, "prn_1");
        assert_eq!(
            manager.stop("ten_b", &item.item_id),
            Err(WorkItemError::NotFound)
        );
        let stopped = manager.stop("ten_a", &item.item_id).unwrap();
        assert_eq!(stopped.end_reason, Some(EndReason::Stopped));
        report(&manager, &turn, "progress");
        let read = manager.get("ten_a", &item.item_id).unwrap();
        assert_eq!(read.state, ItemState::Ended);
        assert_eq!(read.end_reason, Some(EndReason::Stopped));
        assert!(matches!(
            manager.add_input("ten_a", &item.item_id, InputKind::Message, true, "x", ""),
            Err(WorkItemError::Invalid(_))
        ));
        assert!(!manager.reopen(&read).unwrap());
    }

    #[test]
    fn an_api_mandate_holds_while_its_principal_is_an_active_member() {
        let manager = manager(enabled());
        let (item, turn) = open(&manager, "prn_1");
        report(&manager, &turn, "ready");
        let item = manager.get("ten_a", &item.item_id).unwrap();
        assert_eq!(item.mandate.basis, Basis::Api);
        assert_eq!(
            manager.revalidate(&item, None).unwrap(),
            Err(RevokeReason::PrincipalNotMember)
        );
        let membership = |status| kura_identity::Membership {
            membership_id: "mem_1".to_string(),
            tenant_id: "ten_a".to_string(),
            principal_id: "prn_1".to_string(),
            role: kura_identity::Role::Viewer,
            status,
            invitation_id: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            accepted_at: None,
            removed_at: None,
        };
        manager
            .store
            .write()
            .upsert_membership(&membership(kura_identity::LifecycleStatus::Active))
            .unwrap();
        assert_eq!(manager.revalidate(&item, None).unwrap(), Ok(()));
        manager
            .store
            .write()
            .upsert_membership(&membership(kura_identity::LifecycleStatus::Removed))
            .unwrap();
        assert_eq!(
            manager.revalidate(&item, None).unwrap(),
            Err(RevokeReason::PrincipalNotMember)
        );
        // Without a principal the mandate cannot be shown to hold.
        let (anonymous, turn) = open(&manager, "");
        report(&manager, &turn, "ready");
        let anonymous = manager.get("ten_a", &anonymous.item_id).unwrap();
        assert_eq!(
            manager.revalidate(&anonymous, None).unwrap(),
            Err(RevokeReason::PrincipalNotMember)
        );
        manager
            .revoke(&anonymous, RevokeReason::PrincipalNotMember)
            .unwrap();
        assert_eq!(
            manager.get("ten_a", &anonymous.item_id).unwrap().end_reason,
            Some(EndReason::Revoked)
        );
    }

    #[test]
    fn the_open_tool_is_never_offered_to_a_round() {
        let manager = Arc::new(manager(enabled()));
        let turn = |thread: &str, tenant: &str| kura_chat::ToolTurn {
            tenant_id: tenant.to_string(),
            thread_id: thread.to_string(),
            ..kura_chat::ToolTurn::default()
        };
        assert!(
            crate::OpenTool::for_turn(manager.clone(), &turn("thr_wi_0193", "ten_a")).is_none()
        );
        assert!(crate::OpenTool::for_turn(manager.clone(), &turn("thr_src_1", "")).is_none());
        assert!(crate::OpenTool::for_turn(manager, &turn("thr_src_1", "ten_a")).is_some());
    }

    #[test]
    fn one_open_owner_per_conversation() {
        let manager = manager(enabled());
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
            external_message_id: "m1".to_string(),
            route_policy_id: String::new(),
        };
        let request = OpenRequest {
            tenant_id: "ten_a".to_string(),
            title: "t".to_string(),
            scope: "s".to_string(),
            origin: Some(origin),
            subscribe_origin: true,
            ..OpenRequest::default()
        };
        let (first, _) = manager.open(request.clone()).expect("open");
        assert_eq!(first.mandate.basis, Basis::ImDirect);
        assert_eq!(first.mandate.source.reference, "delivery_1");
        assert_eq!(
            manager.open(request.clone()).unwrap_err(),
            WorkItemError::ConversationOwned(first.item_id.clone())
        );
        // Without subscribing, a second item is fine.
        manager
            .open(OpenRequest {
                subscribe_origin: false,
                ..request
            })
            .expect("unsubscribed open");
    }
}
