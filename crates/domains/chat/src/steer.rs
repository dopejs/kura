//! Reaching a running turn from outside it (agent-runtime-program.md 2.8,
//! G8): sending it a message, or cancelling it.
//!
//! A turn is addressed by the tenant it runs for and its request id, the
//! `X-Request-Id` its query was sent with. The client chooses that id, or the
//! server echoes the one it generated, so the client always has it; and it
//! is the id 2.7 already recognises a retry by. A tenant reaches only its own
//! turns: the key carries the tenant, so another tenant's request id names
//! nothing here.
//!
//! A message is delivered to the turn's inbox, and the loop reads the inbox
//! before every model call (see `kura_core::TurnInbox`). Accepted means the
//! model will read it, unless the turn fails first. Once the model has
//! answered with nothing waiting, the inbox is closed in the same step and
//! later messages are refused as `finished` rather than accepted and lost.
//!
//! Only a non-streamed query registers. A streamed query is a single dispatch
//! with no loop to read an inbox, so it has nothing a message could change.

use std::collections::HashMap;
use std::sync::Arc;

use crate::cancel::CancellationToken;

/// Longest message accepted, in bytes.
pub const MAX_STEER_MESSAGE_BYTES: usize = 32 * 1024;

/// Messages a turn holds that its loop has not read yet. A sender that runs
/// ahead of the model is refused rather than buffered without bound.
pub const MAX_PENDING_STEER_MESSAGES: usize = 8;

/// Why a turn could not be reached.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SteerError {
    /// No turn with that request id is running for that tenant.
    #[error("no running turn has request id {0}")]
    NotFound(String),
    /// The turn has answered or failed; nothing it would read remains.
    #[error("the turn has finished")]
    Finished,
    /// The turn has not read the messages it already holds.
    #[error("the turn already holds {MAX_PENDING_STEER_MESSAGES} unread messages")]
    Full,
    /// The message is empty or too long.
    #[error("{0}")]
    Invalid(String),
}

impl SteerError {
    /// Stable machine code, as `ChatError::code` gives one.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            SteerError::NotFound(_) => "turn_not_found",
            SteerError::Finished => "turn_finished",
            SteerError::Full => "turn_inbox_full",
            SteerError::Invalid(_) => "invalid_steer_message",
        }
    }
}

/// Turns of this process that can be reached, by tenant and request id.
#[derive(Default)]
pub(crate) struct ActiveTurns(parking_lot::Mutex<HashMap<(String, String), Arc<ActiveTurn>>>);

/// One reachable turn.
pub(crate) struct ActiveTurn {
    pub(crate) inbox: Arc<SteerInbox>,
    cancel: CancellationToken,
}

/// Held for the whole of a turn; dropping it closes the inbox and makes the
/// turn unreachable.
pub(crate) struct ActiveTurnGuard {
    turns: Arc<ActiveTurns>,
    key: (String, String),
    turn: Arc<ActiveTurn>,
}

impl ActiveTurnGuard {
    pub(crate) fn inbox(&self) -> Arc<SteerInbox> {
        Arc::clone(&self.turn.inbox)
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        kura_core::TurnInbox::close(self.turn.inbox.as_ref());
        let mut turns = self.turns.0.lock();
        // Only this turn's own entry: a later turn may have taken the key.
        if turns
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.turn))
        {
            turns.remove(&self.key);
        }
    }
}

impl ActiveTurns {
    /// Makes a turn reachable. `None` when it has no request id, or another
    /// turn running here already answers to the same one: the first keeps
    /// it, and this one runs unreachable rather than taking messages meant
    /// for the other.
    pub(crate) fn register(
        self: &Arc<Self>,
        tenant_id: &str,
        request_id: &str,
        cancel: &CancellationToken,
    ) -> Option<ActiveTurnGuard> {
        let request_id = request_id.trim();
        if request_id.is_empty() {
            return None;
        }
        let key = (tenant_id.trim().to_string(), request_id.to_string());
        let turn = Arc::new(ActiveTurn {
            inbox: Arc::new(SteerInbox::default()),
            cancel: cancel.clone(),
        });
        let mut turns = self.0.lock();
        if turns.contains_key(&key) {
            count("steer", "duplicate");
            return None;
        }
        turns.insert(key.clone(), Arc::clone(&turn));
        Some(ActiveTurnGuard {
            turns: Arc::clone(self),
            key,
            turn,
        })
    }

    fn find(&self, tenant_id: &str, request_id: &str) -> Result<Arc<ActiveTurn>, SteerError> {
        let request_id = request_id.trim();
        self.0
            .lock()
            .get(&(tenant_id.trim().to_string(), request_id.to_string()))
            .cloned()
            .ok_or_else(|| SteerError::NotFound(request_id.to_string()))
    }

    /// Delivers `message` to the turn; returns how many it now holds unread.
    pub(crate) fn send(
        &self,
        tenant_id: &str,
        request_id: &str,
        message: &str,
    ) -> Result<usize, SteerError> {
        let outcome = self.try_send(tenant_id, request_id, message);
        count("steer", outcome_label(&outcome));
        outcome
    }

    fn try_send(
        &self,
        tenant_id: &str,
        request_id: &str,
        message: &str,
    ) -> Result<usize, SteerError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(SteerError::Invalid("message is required".to_string()));
        }
        if message.len() > MAX_STEER_MESSAGE_BYTES {
            return Err(SteerError::Invalid(format!(
                "message is {} bytes; at most {MAX_STEER_MESSAGE_BYTES} are accepted",
                message.len()
            )));
        }
        self.find(tenant_id, request_id)?
            .inbox
            .push(message.to_string())
    }

    /// Cancels the turn. The turn ends as a cancelled dispatch, the way it
    /// would if its caller had gone away.
    pub(crate) fn cancel(&self, tenant_id: &str, request_id: &str) -> Result<(), SteerError> {
        let outcome = self.find(tenant_id, request_id).and_then(|turn| {
            if turn.inbox.is_closed() {
                return Err(SteerError::Finished);
            }
            turn.cancel.kill();
            Ok(())
        });
        count("cancel", outcome_label(&outcome));
        outcome
    }
}

impl crate::Service {
    /// Sends `message` to the running turn `request_id` of `tenant_id`; the
    /// model reads it before its next call. Returns how many messages the
    /// turn holds unread, this one included.
    pub fn steer_turn(
        &self,
        tenant_id: &str,
        request_id: &str,
        message: &str,
    ) -> Result<usize, SteerError> {
        self.active_turns.send(tenant_id, request_id, message)
    }

    /// Cancels the running turn `request_id` of `tenant_id`.
    pub fn cancel_turn(&self, tenant_id: &str, request_id: &str) -> Result<(), SteerError> {
        self.active_turns.cancel(tenant_id, request_id)
    }
}

/// A turn's unread messages.
#[derive(Default)]
pub(crate) struct SteerInbox(parking_lot::Mutex<InboxState>);

#[derive(Default)]
struct InboxState {
    pending: Vec<String>,
    closed: bool,
}

impl SteerInbox {
    fn push(&self, message: String) -> Result<usize, SteerError> {
        let mut state = self.0.lock();
        if state.closed {
            return Err(SteerError::Finished);
        }
        if state.pending.len() >= MAX_PENDING_STEER_MESSAGES {
            return Err(SteerError::Full);
        }
        state.pending.push(message);
        Ok(state.pending.len())
    }

    fn is_closed(&self) -> bool {
        self.0.lock().closed
    }
}

impl kura_core::TurnInbox for SteerInbox {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().pending)
    }

    fn take_or_close(&self) -> Vec<String> {
        let mut state = self.0.lock();
        if state.pending.is_empty() {
            state.closed = true;
        }
        std::mem::take(&mut state.pending)
    }

    fn close(&self) {
        self.0.lock().closed = true;
    }
}

fn outcome_label<T>(outcome: &Result<T, SteerError>) -> &'static str {
    match outcome {
        Ok(_) => "accepted",
        Err(SteerError::NotFound(_)) => "not_found",
        Err(SteerError::Finished) => "finished",
        Err(SteerError::Full) => "full",
        Err(SteerError::Invalid(_)) => "invalid",
    }
}

fn count(action: &str, outcome: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::CHAT_TURN_CONTROLS_TOTAL,
        &[("action", action), ("outcome", outcome)],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_core::TurnInbox;

    fn running(turns: &Arc<ActiveTurns>, request_id: &str) -> ActiveTurnGuard {
        turns
            .register("ten_a", request_id, &CancellationToken::new())
            .expect("registered")
    }

    #[test]
    fn a_message_reaches_the_turn_it_names_and_no_other() {
        let turns = Arc::new(ActiveTurns::default());
        let guard = running(&turns, "req_1");

        assert_eq!(turns.send("ten_a", " req_1 ", "only PDFs"), Ok(1));
        assert_eq!(
            turns.send("ten_b", "req_1", "hello"),
            Err(SteerError::NotFound("req_1".into()))
        );
        assert_eq!(
            turns.send("ten_a", "req_2", "hello"),
            Err(SteerError::NotFound("req_2".into()))
        );
        assert_eq!(guard.inbox().take(), vec!["only PDFs".to_string()]);
    }

    #[test]
    fn nothing_is_accepted_after_the_turn_answers() {
        let turns = Arc::new(ActiveTurns::default());
        let guard = running(&turns, "req_1");
        assert!(guard.inbox().take_or_close().is_empty());

        assert_eq!(
            turns.send("ten_a", "req_1", "too late"),
            Err(SteerError::Finished)
        );
        assert_eq!(turns.cancel("ten_a", "req_1"), Err(SteerError::Finished));
        drop(guard);
        assert!(matches!(
            turns.send("ten_a", "req_1", "too late"),
            Err(SteerError::NotFound(_))
        ));
    }

    #[test]
    fn a_waiting_message_keeps_the_inbox_open() {
        let turns = Arc::new(ActiveTurns::default());
        let guard = running(&turns, "req_1");
        turns.send("ten_a", "req_1", "one more thing").unwrap();

        assert_eq!(guard.inbox().take_or_close(), vec!["one more thing"]);
        assert_eq!(turns.send("ten_a", "req_1", "and another"), Ok(1));
    }

    #[test]
    fn unread_messages_are_bounded_and_size_checked() {
        let turns = Arc::new(ActiveTurns::default());
        let _guard = running(&turns, "req_1");
        for expected in 1..=MAX_PENDING_STEER_MESSAGES {
            assert_eq!(turns.send("ten_a", "req_1", "m"), Ok(expected));
        }
        assert_eq!(turns.send("ten_a", "req_1", "m"), Err(SteerError::Full));
        assert!(matches!(
            turns.send("ten_a", "req_1", "  "),
            Err(SteerError::Invalid(_))
        ));
        let long = "x".repeat(MAX_STEER_MESSAGE_BYTES + 1);
        assert!(matches!(
            turns.send("ten_a", "req_1", &long),
            Err(SteerError::Invalid(_))
        ));
    }

    #[test]
    fn cancelling_kills_the_turns_token() {
        let turns = Arc::new(ActiveTurns::default());
        let cancel = CancellationToken::new();
        let _guard = turns.register("ten_a", "req_1", &cancel).unwrap();

        turns.cancel("ten_a", "req_1").unwrap();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn the_first_turn_keeps_a_shared_request_id() {
        let turns = Arc::new(ActiveTurns::default());
        let first = running(&turns, "req_1");
        assert!(
            turns
                .register("ten_a", "req_1", &CancellationToken::new())
                .is_none()
        );
        assert!(
            turns
                .register("ten_a", "", &CancellationToken::new())
                .is_none()
        );

        turns.send("ten_a", "req_1", "for the first").unwrap();
        assert_eq!(first.inbox().take(), vec!["for the first"]);
        drop(first);
        // Gone with it; the id is free again.
        assert!(running(&turns, "req_1").inbox().take().is_empty());
    }
}
