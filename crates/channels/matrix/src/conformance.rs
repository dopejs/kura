//! Port of daemon/internal/connectors/matrix/conformance.go helpers living in
//! runtime.go: the Matrix capability profile declaration.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kura_connectors::{
    CapabilityProfile, ConformanceResultStatus, GroupRoomCapabilities, HandoffCapabilities,
    MATRIX_DURABLE_IDENTITY_RULE, MATRIX_DURABLE_IDENTITY_RULE_ID,
    MATRIX_SURFACE_ACCOUNT_PROVISIONING, MATRIX_SURFACE_ALLOWED_ROOM_COMMAND,
    MATRIX_SURFACE_ALLOWED_ROOM_MENTION, MATRIX_SURFACE_CONNECTOR_BACKED_DELIVERY,
    MATRIX_SURFACE_DIRECT_MESSAGE, MATRIX_SURFACE_ENCRYPTED_ROOMS,
    MATRIX_SURFACE_FINAL_ONLY_FOREGROUND_REPLY, MATRIX_SURFACE_HOSTED_HOMESERVER,
    MATRIX_SURFACE_TENANT_PROVIDED_BOT_SETUP, MATRIX_SURFACE_UNDECRYPTABLE_EVENTS,
    MATRIX_SURFACE_UNENCRYPTED_TEXT, SurfaceSupport, core_invariant_areas,
};

use crate::is_unset_time;
use crate::types::Config;

/// Go `ConformanceProfile`: the declared Matrix capability profile for a
/// connector configuration.
#[must_use]
pub fn conformance_profile(cfg: &Config, declared_at: DateTime<Utc>) -> CapabilityProfile {
    let declared_at = if is_unset_time(&declared_at) {
        Utc::now()
    } else {
        declared_at
    };
    let mut core = HashMap::with_capacity(10);
    for area in core_invariant_areas() {
        core.insert(area, ConformanceResultStatus::Pass);
    }
    let mut surfaces = HashMap::new();
    surfaces.insert(
        MATRIX_SURFACE_TENANT_PROVIDED_BOT_SETUP.to_string(),
        SurfaceSupport::Supported,
    );
    surfaces.insert(
        MATRIX_SURFACE_HOSTED_HOMESERVER.to_string(),
        SurfaceSupport::Unsupported,
    );
    surfaces.insert(
        MATRIX_SURFACE_ACCOUNT_PROVISIONING.to_string(),
        SurfaceSupport::Unsupported,
    );
    surfaces.insert(
        MATRIX_SURFACE_DIRECT_MESSAGE.to_string(),
        support_flag(!cfg.allowed_direct_user_ids.is_empty()),
    );
    surfaces.insert(
        MATRIX_SURFACE_ALLOWED_ROOM_MENTION.to_string(),
        support_flag(!cfg.selected_room_ids.is_empty()),
    );
    surfaces.insert(
        MATRIX_SURFACE_ALLOWED_ROOM_COMMAND.to_string(),
        support_flag(!cfg.selected_room_ids.is_empty() || !cfg.configured_commands.is_empty()),
    );
    surfaces.insert(
        MATRIX_SURFACE_UNENCRYPTED_TEXT.to_string(),
        SurfaceSupport::Supported,
    );
    surfaces.insert(
        MATRIX_SURFACE_ENCRYPTED_ROOMS.to_string(),
        SurfaceSupport::Unsupported,
    );
    surfaces.insert(
        MATRIX_SURFACE_UNDECRYPTABLE_EVENTS.to_string(),
        SurfaceSupport::Unsupported,
    );
    surfaces.insert(
        "e2ee_key_session_management".to_string(),
        SurfaceSupport::Unsupported,
    );
    surfaces.insert(
        MATRIX_SURFACE_FINAL_ONLY_FOREGROUND_REPLY.to_string(),
        SurfaceSupport::Supported,
    );
    surfaces.insert(
        MATRIX_SURFACE_CONNECTOR_BACKED_DELIVERY.to_string(),
        SurfaceSupport::Supported,
    );
    surfaces.insert("whatsapp".to_string(), SurfaceSupport::Unsupported);
    surfaces.insert("bridge_automation".to_string(), SurfaceSupport::Unsupported);
    surfaces.insert("media".to_string(), SurfaceSupport::Unsupported);
    surfaces.insert("voice".to_string(), SurfaceSupport::Unsupported);
    surfaces.insert("calls".to_string(), SurfaceSupport::Unsupported);
    surfaces.insert("reactions".to_string(), SurfaceSupport::Unsupported);
    surfaces.insert(
        "thinking_visibility".to_string(),
        SurfaceSupport::Unsupported,
    );
    surfaces.insert(
        "incremental_visible_updates".to_string(),
        SurfaceSupport::Unsupported,
    );
    surfaces.insert(
        "blocked_route_classification".to_string(),
        SurfaceSupport::Supported,
    );
    surfaces.insert(
        "standard_durable_identity".to_string(),
        SurfaceSupport::Supported,
    );

    CapabilityProfile {
        profile_id: format!("profile_matrix_{}", cfg.connector_id.trim()),
        connector_id: cfg.connector_id.trim().to_string(),
        connector_kind: crate::types::CONNECTOR_KIND.to_string(),
        core_invariant_results: core,
        provider_surface_results: surfaces,
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(support_flag(!cfg.selected_room_ids.is_empty())),
            allowlist_evidence: Some(support_flag(!cfg.selected_room_ids.is_empty())),
            unsupported_source_evidence: Some(SurfaceSupport::Limited),
            duplicate_message_evidence: Some(SurfaceSupport::Supported),
            edited_message_evidence: Some(SurfaceSupport::Unsupported),
            deleted_message_evidence: Some(SurfaceSupport::Unsupported),
        },
        handoff_capabilities: HandoffCapabilities {
            source_support: Some(support_flag(!cfg.selected_room_ids.is_empty())),
            destination_support: Some(support_flag(!cfg.selected_room_ids.is_empty())),
            first_response_source_references: Some(SurfaceSupport::Supported),
        },
        equivalent_durable_identity_rule_id: MATRIX_DURABLE_IDENTITY_RULE_ID.to_string(),
        equivalent_durable_identity_rule: MATRIX_DURABLE_IDENTITY_RULE.to_string(),
        declared_at,
        ..CapabilityProfile::default()
    }
}

/// Go `supportFlag`.
#[must_use]
pub fn support_flag(enabled: bool) -> SurfaceSupport {
    if enabled {
        SurfaceSupport::Supported
    } else {
        SurfaceSupport::Unsupported
    }
}
