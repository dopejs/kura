//! Setup target catalog (port of `targets.go`).

use crate::permissions::{PERMISSION_INTEGRATIONS_MANAGE, PERMISSION_SECRETS_MANAGE};
use crate::types::*;

/// Returns the built-in target catalog for a tenant, sorted by target id.
#[must_use]
pub fn catalog_targets(tenant_id: &str) -> Vec<SetupTarget> {
    let tenant_id = tenant_id.trim().to_string();
    let mut targets = vec![
        setup_target(
            TARGET_OPENAI_COMPATIBLE,
            &tenant_id,
            TargetKind::Provider,
            SetupStyle::SubmittedSecret,
            "OpenAI-compatible provider",
            vec!["metadata_read"],
        ),
        setup_target(
            TARGET_FEISHU_LARK,
            &tenant_id,
            TargetKind::Integration,
            SetupStyle::OAuth,
            "Feishu/Lark OAuth",
            vec!["metadata_read"],
        ),
        setup_target(
            TARGET_DISCORD_CONNECTOR,
            &tenant_id,
            TargetKind::Connector,
            SetupStyle::SubmittedSecret,
            "Discord connector",
            vec!["metadata_read", "destination_validation"],
        ),
        setup_target(
            TARGET_TELEGRAM_CONNECTOR,
            &tenant_id,
            TargetKind::Connector,
            SetupStyle::SubmittedSecret,
            "Telegram connector",
            vec!["metadata_read", "allowment_validation"],
        ),
        setup_target(
            TARGET_SLACK_CONNECTOR,
            &tenant_id,
            TargetKind::Connector,
            SetupStyle::OAuth,
            "Slack connector",
            vec![
                "metadata_read",
                "route_policy_validation",
                "workspace_validation",
            ],
        ),
        setup_target(
            TARGET_MATRIX_CONNECTOR,
            &tenant_id,
            TargetKind::Connector,
            SetupStyle::SubmittedSecret,
            "Matrix connector",
            vec![
                "metadata_read",
                "route_policy_validation",
                "homeserver_validation",
            ],
        ),
    ];
    targets.sort_by(|a, b| a.target_id.cmp(&b.target_id));
    targets
}

/// Finds a target by id, returning an unsupported placeholder when absent.
#[must_use]
pub fn target_by_id(tenant_id: &str, target_id: &str) -> (SetupTarget, bool) {
    let target_id = target_id.trim();
    for target in catalog_targets(tenant_id) {
        if target.target_id == target_id {
            return (target, true);
        }
    }
    let unsupported = SetupTarget {
        target_id: target_id.to_string(),
        tenant_id: tenant_id.trim().to_string(),
        target_kind: TargetKind::Provider,
        setup_style: SetupStyle::Unsupported,
        display_name: crate::helpers::first_non_empty(&[target_id, "Unsupported target"]),
        support_status: SupportStatus::Unsupported,
        proof_target: false,
        required_permissions: vec![
            PERMISSION_SECRETS_MANAGE.to_string(),
            PERMISSION_INTEGRATIONS_MANAGE.to_string(),
        ],
        ..Default::default()
    };
    (unsupported, false)
}

#[allow(clippy::too_many_arguments)]
fn setup_target(
    target_id: &str,
    tenant_id: &str,
    kind: TargetKind,
    style: SetupStyle,
    display_name: &str,
    limited_safe_capabilities: Vec<&str>,
) -> SetupTarget {
    SetupTarget {
        target_id: target_id.to_string(),
        tenant_id: tenant_id.to_string(),
        target_kind: kind,
        setup_style: style,
        display_name: display_name.to_string(),
        proof_target: true,
        support_status: SupportStatus::Supported,
        required_permissions: vec![
            PERMISSION_SECRETS_MANAGE.to_string(),
            PERMISSION_INTEGRATIONS_MANAGE.to_string(),
        ],
        limited_safe_capabilities: limited_safe_capabilities
            .into_iter()
            .map(str::to_string)
            .collect(),
        ..Default::default()
    }
}

impl Default for SetupTarget {
    fn default() -> Self {
        SetupTarget {
            target_id: String::new(),
            tenant_id: String::new(),
            target_kind: TargetKind::Provider,
            setup_style: SetupStyle::Unsupported,
            display_name: String::new(),
            proof_target: false,
            support_status: SupportStatus::Unsupported,
            required_permissions: Vec::new(),
            limited_safe_capabilities: Vec::new(),
            current_session_id: String::new(),
            current_state: SetupState::default(),
            diagnostic_result_id: String::new(),
        }
    }
}
