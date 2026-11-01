//! Checking the model's final answer before a turn accepts it.

/// Judges the model's final answer, the text it gave without asking for a
/// tool (agent-runtime-program.md 2.8, G7). A rejected answer is sent back
/// once with `feedback` and the model answers again in the same turn.
pub trait AnswerCheck: Send + Sync {
    fn check(&self, answer: &str) -> Result<(), AnswerRejection>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerRejection {
    /// Why, for the caller.
    pub reason: String,
    /// What the model is told when it is asked to answer again.
    pub feedback: String,
}
