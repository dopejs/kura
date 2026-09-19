use std::fmt;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::error::ThreadsError;
use crate::redaction::RedactionStatus;

/// Where a thread's traffic originates. Go: `SourceKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Chat,
    Channel,
    Workflow,
    Schedule,
    Shell,
    Legacy,
}

/// Outcome of routing a source event to a thread. Go: `RoutingOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingOutcome {
    Accepted,
    Ignored,
    Blocked,
    Duplicate,
    Disabled,
    Unsupported,
    Failed,
    UnknownSource,
    StaleSource,
    InaccessibleTenantBinding,
}

/// Link between a thread and an external source conversation.
/// Go: `SourceLinkage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLinkage {
    pub source_linkage_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub source_kind: SourceKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_message_id: String,
    pub routing_outcome: RoutingOutcome,
    pub current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// Stable dedup/continuation identity for a source conversation.
/// Go: `SourceContinuationKey`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceContinuationKey {
    pub tenant_id: String,
    pub connector_id: String,
    pub source_account_id: String,
    pub source_conversation_id: String,
}

fn normalize_key_part(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Go: `NormalizeSourceContinuationKey` — trims and lowercases every part and
/// rejects keys missing any part.
pub fn normalize_source_continuation_key(
    key: &SourceContinuationKey,
) -> Result<SourceContinuationKey, ThreadsError> {
    let normalized = SourceContinuationKey {
        tenant_id: normalize_key_part(&key.tenant_id),
        connector_id: normalize_key_part(&key.connector_id),
        source_account_id: normalize_key_part(&key.source_account_id),
        source_conversation_id: normalize_key_part(&key.source_conversation_id),
    };
    if normalized.tenant_id.is_empty()
        || normalized.connector_id.is_empty()
        || normalized.source_account_id.is_empty()
        || normalized.source_conversation_id.is_empty()
    {
        return Err(ThreadsError::InvalidSourceContinuationKey);
    }
    Ok(normalized)
}

/// Go: `SourceContinuationKey.String` — NUL-joined stable key string.
impl fmt::Display for SourceContinuationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\u{0}{}\u{0}{}\u{0}{}",
            self.tenant_id, self.connector_id, self.source_account_id, self.source_conversation_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestNormalizeSourceContinuationKey.
    #[test]
    fn normalize_source_continuation_key_trims_lowercases_and_validates() {
        let key = normalize_source_continuation_key(&SourceContinuationKey {
            tenant_id: " ten_1 ".to_string(),
            connector_id: " Slack-Main ".to_string(),
            source_account_id: " Workspace_A ".to_string(),
            source_conversation_id: " Channel_A ".to_string(),
        })
        .expect("normalize");
        assert_eq!(key.tenant_id, "ten_1");
        assert_eq!(key.connector_id, "slack-main");
        assert_eq!(key.source_account_id, "workspace_a");
        assert_eq!(key.source_conversation_id, "channel_a");
        assert_eq!(
            key.to_string(),
            "ten_1\u{0}slack-main\u{0}workspace_a\u{0}channel_a"
        );

        let incomplete = normalize_source_continuation_key(&SourceContinuationKey {
            tenant_id: "ten_1".to_string(),
            connector_id: String::new(),
            source_account_id: String::new(),
            source_conversation_id: String::new(),
        });
        assert_eq!(
            incomplete.unwrap_err(),
            ThreadsError::InvalidSourceContinuationKey
        );
    }
}
