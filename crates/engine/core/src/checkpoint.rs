//! Where a turn stands, so a turn whose process died can be picked up from
//! its last settled tool call instead of from the question
//! (agent-runtime-program.md 2.7).
//!
//! The loop reports every change to a [`TurnJournal`]: the calls a round
//! asked for, each call as it starts, each result as it settles. A turn
//! resumed from that record replays what settled, runs what never started,
//! and does not run again a call that started with side effects and never
//! reported back: whether it took effect is unknown, so the model is told
//! so and decides. Repeating it would be the one wrong answer.

use serde::{Deserialize, Serialize};

/// What the model reads in place of a call with side effects that started
/// and never reported a result.
pub const INTERRUPTED_CALL_OUTPUT: &str = "this call was interrupted before it reported a result, \
     and it was not run again: whether it took effect is unknown. Check before repeating it.";

/// Every round of a turn that asked for tools, in order. Only the last
/// round can have calls that have not settled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCheckpoint {
    pub rounds: Vec<CheckpointRound>,
}

impl TurnCheckpoint {
    /// Calls that settled, across every round.
    #[must_use]
    pub fn settled_calls(&self) -> usize {
        self.rounds
            .iter()
            .flat_map(|round| &round.calls)
            .filter(|call| matches!(call.state, CallState::Settled { .. }))
            .count()
    }
}

/// The calls one round asked for, in the order the model asked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRound {
    pub calls: Vec<CheckpointCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub state: CallState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CallState {
    /// Asked for, not started.
    Planned,
    /// Started and has not reported back.
    Started,
    /// Reported back; this is what the model reads.
    Settled { output: String, success: bool },
}

/// Receives the turn's checkpoint every time it changes.
///
/// Called on the loop's own task, between and around tool calls, so it
/// should be quick. It cannot fail the turn: a checkpoint that was not
/// written only means the turn cannot be resumed from that point, which is
/// where every turn stood before there were checkpoints. Implementations
/// report their own failures.
pub trait TurnJournal: Send + Sync {
    fn record(&self, checkpoint: &TurnCheckpoint);
}

/// What resuming a turn found in its checkpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumeStats {
    /// Rounds carried over from the checkpoint.
    pub rounds: usize,
    /// Calls whose settled result was replayed rather than run.
    pub replayed_calls: usize,
    /// Calls with side effects that had started and not settled; reported
    /// to the model as [`INTERRUPTED_CALL_OUTPUT`] rather than run again.
    pub interrupted_calls: usize,
    /// Calls that had not settled and were safe to run, so they ran.
    pub rerun_calls: usize,
}
