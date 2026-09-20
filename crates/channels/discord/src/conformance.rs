//! Conformance declarations (port of runtime.go's conformanceProfileForEvidence
//! and helpers): the Discord capability profile built from the config and the
//! hosted-setup evidence.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use kura_connectors::{
    CapabilityProfile, ConformanceArea, ConformanceResultStatus, GroupRoomCapabilities,
    HandoffCapabilities, SurfaceSupport, core_invariant_areas,
};

use crate::config::Config;
use crate::destinations::selected_destinations_valid;
use crate::readiness::HostedSetup;

/// Go `ConformanceProfile`: the config-only declaration, which never passes
/// core invariants without validated hosted evidence.
#[must_use]
pub fn conformance_profile(cfg: &Config, declared_at: DateTime<Utc>) -> CapabilityProfile {
    conformance_profile_for_evidence(cfg, &HostedSetup::default(), false, declared_at)
}

/// Go `ConformanceProfileForSetup`: validated when the hosted setup is ready
/// and every selected destination is valid.
#[must_use]
pub fn conformance_profile_for_setup(
    cfg: &Config,
    setup: &HostedSetup,
    declared_at: DateTime<Utc>,
) -> CapabilityProfile {
    let validated = setup.hosted_ready && selected_destinations_valid(&setup.destinations);
    conformance_profile_for_evidence(cfg, setup, validated, declared_at)
}

/// Go `conformanceProfileForEvidence`.
#[must_use]
pub fn conformance_profile_for_evidence(
    cfg: &Config,
    setup: &HostedSetup,
    validated: bool,
    mut declared_at: DateTime<Utc>,
) -> CapabilityProfile {
    if declared_at == DateTime::<Utc>::default() {
        declared_at = Utc::now();
    }
    let core_status = if validated {
        ConformanceResultStatus::Pass
    } else {
        ConformanceResultStatus::Fail
    };
    let mut core: HashMap<ConformanceArea, ConformanceResultStatus> = HashMap::new();
    for area in core_invariant_areas() {
        core.insert(area, core_status);
    }
    let incremental_support = if core_status == ConformanceResultStatus::Pass {
        SurfaceSupport::Limited
    } else {
        SurfaceSupport::Unsupported
    };
    let allowlist_configured =
        !cfg.allowed_guild_ids.is_empty() || !cfg.allowed_channel_ids.is_empty();
    let surfaces = HashMap::from([
        (
            "direct_message".to_string(),
            support_flag(cfg.respond_in_dm),
        ),
        (
            "group_channel".to_string(),
            support_flag(allowlist_configured),
        ),
        (
            "mention_gating".to_string(),
            support_flag(cfg.require_mention),
        ),
        ("room".to_string(), SurfaceSupport::Unsupported),
        ("voice".to_string(), SurfaceSupport::Unsupported),
        ("thread_reply".to_string(), SurfaceSupport::Limited),
        ("thinking_visibility".to_string(), SurfaceSupport::Limited),
        (
            "incremental_visible_updates".to_string(),
            incremental_support,
        ),
        ("rich_media".to_string(), SurfaceSupport::Unsupported),
        (
            "placeholder_card_update".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "provider_specific_stop".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "connector_backed_delivery".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "final_only_foreground_reply".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "thinking_plus_final_reply".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "thinking_plus_incremental".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "equivalent_durable_identity".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "standard_durable_identity".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "blocked_route_classification".to_string(),
            SurfaceSupport::Supported,
        ),
    ]);
    CapabilityProfile {
        profile_id: format!("profile_discord_{}", cfg.connector_id),
        tenant_id: setup.tenant_id.clone(),
        connector_id: cfg.connector_id.clone(),
        connector_kind: "discord".to_string(),
        core_invariant_results: core,
        provider_surface_results: surfaces,
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(support_flag(cfg.require_mention)),
            allowlist_evidence: Some(support_flag(allowlist_configured)),
            unsupported_source_evidence: Some(SurfaceSupport::Limited),
            duplicate_message_evidence: Some(SurfaceSupport::Supported),
            edited_message_evidence: Some(SurfaceSupport::Unsupported),
            deleted_message_evidence: Some(SurfaceSupport::Unsupported),
        },
        handoff_capabilities: HandoffCapabilities {
            source_support: Some(support_flag(allowlist_configured)),
            destination_support: Some(support_flag(allowlist_configured)),
            first_response_source_references: Some(SurfaceSupport::Supported),
        },
        equivalent_durable_identity_rule_id: "discord_message_id".to_string(),
        equivalent_durable_identity_rule:
            "tenant_id + connector_account_id + channel_or_conversation_id + provider_message_id"
                .to_string(),
        declared_at,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destinations::{DestinationType, DestinationValidation, DestinationValidationState};
    use crate::readiness::{CredentialState, HostedSetupInput, evaluate_hosted_setup};
    use kura_connectors::{ConformanceResultStatus, SurfaceSupport, validate_capability_profile};

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-07T10:00:00Z")
            .expect("parse")
            .with_timezone(&Utc)
    }

    // Go TestDiscordConformanceProfileRequiresValidatedHostedEvidence
    #[test]
    fn conformance_profile_requires_validated_hosted_evidence() {
        let cfg = Config {
            connector_id: "discord-main".to_string(),
            bot_token: "fake-token".to_string(),
            require_mention: true,
            respond_in_dm: true,
            allowed_guild_ids: vec!["guild_1".to_string()],
            allowed_channel_ids: vec!["channel_1".to_string()],
            ..Config::default()
        };
        let now = ts();

        let local_only = conformance_profile(&cfg, now);
        assert!(
            validate_capability_profile(&local_only).is_err(),
            "expected local config projection to fail core invariants without validated hosted evidence"
        );

        let setup = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_discord".to_string(),
            connector_id: cfg.connector_id.clone(),
            display_name: "Discord Main".to_string(),
            credential: CredentialState::Valid,
            respond_in_dm: true,
            require_mention: true,
            destinations: vec![
                DestinationValidation {
                    destination_id: "guild_redacted".to_string(),
                    destination_type: DestinationType::Guild,
                    selected: true,
                    validation_state: DestinationValidationState::Valid,
                    ..DestinationValidation::default()
                },
                DestinationValidation {
                    destination_id: "channel_redacted".to_string(),
                    destination_type: DestinationType::Channel,
                    selected: true,
                    validation_state: DestinationValidationState::Valid,
                    ..DestinationValidation::default()
                },
            ],
            validated_at: now,
            ..HostedSetupInput::default()
        });
        let profile = conformance_profile_for_setup(&cfg, &setup, now);
        validate_capability_profile(&profile)
            .expect("validated hosted Discord profile passes core invariants");
        assert_eq!(
            profile.provider_surface_results.get("voice"),
            Some(&SurfaceSupport::Unsupported)
        );
        assert_eq!(
            profile
                .provider_surface_results
                .get("connector_backed_delivery"),
            Some(&SurfaceSupport::Supported)
        );
    }

    // Go TestDiscordConformanceProfileDeclaresSurfacesWithoutSyntheticCorePasses
    #[test]
    fn conformance_profile_declares_surfaces_without_synthetic_core_passes() {
        let profile = conformance_profile(
            &Config {
                connector_id: "discord-main".to_string(),
                require_mention: true,
                respond_in_dm: true,
                ..Config::default()
            },
            ts(),
        );
        assert!(
            validate_capability_profile(&profile).is_err(),
            "expected declared Discord profile without matrix evidence to fail core invariant validation"
        );
        assert_eq!(
            profile.provider_surface_results.get("direct_message"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            profile.provider_surface_results.get("thread_reply"),
            Some(&SurfaceSupport::Limited)
        );
        assert_eq!(
            profile
                .provider_surface_results
                .get("incremental_visible_updates"),
            Some(&SurfaceSupport::Unsupported)
        );
    }

    #[test]
    fn conformance_profile_exact_surface_declarations() {
        let cfg = Config {
            connector_id: "discord-main".to_string(),
            require_mention: true,
            respond_in_dm: true,
            allowed_guild_ids: vec!["guild_1".to_string()],
            ..Config::default()
        };
        let profile = conformance_profile(&cfg, ts());
        let surfaces = &profile.provider_surface_results;
        assert_eq!(
            surfaces.get("direct_message"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            surfaces.get("group_channel"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            surfaces.get("mention_gating"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(surfaces.get("room"), Some(&SurfaceSupport::Unsupported));
        assert_eq!(surfaces.get("voice"), Some(&SurfaceSupport::Unsupported));
        assert_eq!(surfaces.get("thread_reply"), Some(&SurfaceSupport::Limited));
        assert_eq!(
            surfaces.get("thinking_visibility"),
            Some(&SurfaceSupport::Limited)
        );
        assert_eq!(
            surfaces.get("rich_media"),
            Some(&SurfaceSupport::Unsupported)
        );
        assert_eq!(
            surfaces.get("placeholder_card_update"),
            Some(&SurfaceSupport::Unsupported)
        );
        assert_eq!(
            surfaces.get("provider_specific_stop"),
            Some(&SurfaceSupport::Unsupported)
        );
        assert_eq!(
            surfaces.get("connector_backed_delivery"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            surfaces.get("final_only_foreground_reply"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            surfaces.get("thinking_plus_final_reply"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            surfaces.get("thinking_plus_incremental"),
            Some(&SurfaceSupport::Unsupported)
        );
        assert_eq!(
            surfaces.get("equivalent_durable_identity"),
            Some(&SurfaceSupport::Unsupported)
        );
        assert_eq!(
            surfaces.get("standard_durable_identity"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            surfaces.get("blocked_route_classification"),
            Some(&SurfaceSupport::Supported)
        );
        assert_eq!(
            surfaces.get("incremental_visible_updates"),
            Some(&SurfaceSupport::Unsupported)
        );
        assert_eq!(
            profile.equivalent_durable_identity_rule_id,
            "discord_message_id"
        );
        assert_eq!(
            profile.equivalent_durable_identity_rule,
            "tenant_id + connector_account_id + channel_or_conversation_id + provider_message_id"
        );
        assert_eq!(profile.profile_id, "profile_discord_discord-main");
    }

    #[test]
    fn conformance_profile_passes_with_allowlist_only_limited_surfaces() {
        // With validated hosted evidence the core invariants pass and
        // incremental updates become Limited (Go conformanceProfileForEvidence).
        let cfg = Config {
            connector_id: "discord-main".to_string(),
            allowed_channel_ids: vec!["channel_1".to_string()],
            ..Config::default()
        };
        let now = ts();
        let setup = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_discord".to_string(),
            connector_id: cfg.connector_id.clone(),
            display_name: "Discord Main".to_string(),
            credential: CredentialState::Valid,
            destinations: vec![DestinationValidation {
                destination_id: "channel_1".to_string(),
                destination_type: DestinationType::Channel,
                selected: true,
                validation_state: DestinationValidationState::Valid,
                ..DestinationValidation::default()
            }],
            validated_at: now,
            ..HostedSetupInput::default()
        });
        let profile = conformance_profile_for_setup(&cfg, &setup, now);
        assert_eq!(
            profile
                .core_invariant_results
                .get(&kura_connectors::ConformanceArea::Redaction),
            Some(&ConformanceResultStatus::Pass)
        );
        assert_eq!(
            profile
                .provider_surface_results
                .get("incremental_visible_updates"),
            Some(&SurfaceSupport::Limited)
        );
        validate_capability_profile(&profile).expect("passes core invariants");
    }
}
