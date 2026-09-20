use thiserror::Error;

/// Errors returned by thread lifecycle, source, continuity, and handoff logic.
///
/// Mirrors the sentinel errors of `daemon/internal/threads`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ThreadsError {
    #[error("required audit evidence must be recorded before lifecycle mutation")]
    AuditEvidenceRequired,
    #[error("lifecycle transition is not allowed")]
    LifecycleTransitionNotAllowed,
    #[error("lifecycle mutation conflicted with concurrent thread update")]
    LifecycleMutationConflict,
    #[error("thread source or session is not eligible for reopen")]
    LifecycleReopenNotEligible,
    #[error(
        "source continuation key requires tenant, connector, source account, and source conversation"
    )]
    InvalidSourceContinuationKey,
    #[error("invalid conversation shape")]
    InvalidConversationShape,
    #[error("handoff source and destination threads must be different")]
    HandoffSameThread,
    #[error("handoff requires connectors.manage and source/destination permission")]
    HandoffPermissionDenied,
    #[error("handoff source or destination is not eligible")]
    HandoffNotEligible,
    #[error("continuity turn requires tenant, thread, and session segment")]
    ContinuityTurnMissingIdentity,
}
