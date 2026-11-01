//! Steering a turn while it runs (agent-runtime-program.md 2.8, G8).
//!
//! Someone watching a turn work can say more before it answers: narrow the
//! question, correct a wrong turn, add what they forgot. The loop reads a
//! [`TurnInbox`] before every model call, so a message sent while a tool ran
//! is in front of the model on the very next round, and when the model
//! answers it checks once more: a message that arrived while the answer was
//! being written turns that answer into an interim one, and the turn goes on.
//!
//! The last check and closing the inbox are one step
//! ([`TurnInbox::take_or_close`]), so a message is either read by the model
//! or refused to its sender. Never accepted and then dropped because the
//! turn happened to finish in between.

/// Messages sent to a running turn, oldest first.
///
/// Implementations are shared between the turn and whoever delivers
/// messages to it, so every method takes `&self`, and none may block for
/// long: they are called on the loop's own task.
pub trait TurnInbox: Send + Sync {
    /// What has arrived since the last take.
    fn take(&self) -> Vec<String>;

    /// [`TurnInbox::take`], and, when nothing is waiting, close the inbox
    /// in the same step so later sends are refused. Called when the model
    /// has answered: an empty inbox ends the turn.
    fn take_or_close(&self) -> Vec<String>;

    /// Refuse later sends. Called when the turn ends any other way, so a
    /// message cannot be accepted by a turn that will never read it.
    fn close(&self);
}
