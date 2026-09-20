//! ThreadAccessScope guards tenant-scoped thread/session lifecycle access.
//! Port of daemon/internal/store/tenancy/threads.go.

/// Tenant-scoped access scope for thread-lifecycle records.
#[derive(Debug, Clone, Default)]
pub struct ThreadAccessScope {
    pub tenant_id: String,
}

impl ThreadAccessScope {
    fn allows(&self, thread_tenant_id: &str) -> bool {
        !self.tenant_id.is_empty() && self.tenant_id == thread_tenant_id
    }

    #[must_use]
    pub fn allows_continuity_turn(&self, turn: &kura_threads::ContinuityTurn) -> bool {
        self.allows(&turn.tenant_id)
    }

    #[must_use]
    pub fn allows_continuity_preview(&self, preview: &kura_threads::ContinuityPreview) -> bool {
        self.allows(&preview.tenant_id)
    }

    #[must_use]
    pub fn allows_conversation_shape(
        &self,
        evidence: &kura_threads::ConversationShapeEvidence,
    ) -> bool {
        self.allows(&evidence.tenant_id)
    }

    #[must_use]
    pub fn allows_participation_decision(
        &self,
        decision: &kura_threads::ParticipationDecision,
    ) -> bool {
        self.allows(&decision.tenant_id)
    }

    #[must_use]
    pub fn allows_reset_event(&self, event: &kura_threads::ResetEvent) -> bool {
        self.allows(&event.tenant_id)
    }

    #[must_use]
    pub fn allows_handoff_link(&self, link: &kura_threads::HandoffLink) -> bool {
        self.allows(&link.tenant_id)
    }

    #[must_use]
    pub fn allows_handoff_source_reference(
        &self,
        reference: &kura_threads::HandoffSourceReference,
    ) -> bool {
        self.allows(&reference.tenant_id)
    }
}
