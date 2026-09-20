//! Port of daemon/internal/imtypes. See rs/MIGRATION.md for conventions.
//!
//! IM message types shared by every channel connector: the persisted
//! [`MessageRecord`] delivery ledger row, the in-memory [`InboundMessage`]
//! payload connectors hand to the runtime, and the outbound reply DTOs
//! ([`OutboundReply`], [`ReplyEdit`], [`ThinkingSignal`], [`SentReply`]) used
//! by connector `Transport` implementations.
//!
//! Only `MessageRecord` and `ReplyCapabilities` carry JSON tags in Go; the
//! remaining DTOs are in-memory only. Serde camelCase is applied uniformly
//! per workspace convention, and Go `omitempty` maps to
//! `skip_serializing_if` helpers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use kura_router::SessionKind;

/// Delivery direction of a [`MessageRecord`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryDirection {
    #[default]
    Inbound,
    Outbound,
}

impl DeliveryDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryDirection::Inbound => "inbound",
            DeliveryDirection::Outbound => "outbound",
        }
    }
}

/// Lifecycle status of a [`MessageRecord`] delivery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    #[default]
    Received,
    Thinking,
    Processing,
    Streaming,
    Replied,
    Partial,
    Failed,
}

impl DeliveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryStatus::Received => "received",
            DeliveryStatus::Thinking => "thinking",
            DeliveryStatus::Processing => "processing",
            DeliveryStatus::Streaming => "streaming",
            DeliveryStatus::Replied => "replied",
            DeliveryStatus::Partial => "partial",
            DeliveryStatus::Failed => "failed",
        }
    }
}

/// What a connector transport can do when replying.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyCapabilities {
    pub supports_thinking: bool,
    pub supports_streaming: bool,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub max_message_length: i64,
}

/// Persisted delivery ledger record for one inbound or outbound IM message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub delivery_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub direction: DeliveryDirection,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_or_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub equivalent_rule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    pub channel_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peer_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author_id: String,
    pub content: String,
    pub status: DeliveryStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply_to_external_message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_to_delivery_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub foreground_outcome_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub background_delivery_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_boundary_kind: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Normalized inbound message handed from a connector transport to the
/// runtime for routing and execution.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundMessage {
    pub connector_id: String,
    pub connector_kind: String,
    pub external_message_id: String,
    pub tenant_id: String,
    pub account_id: String,
    pub connector_account_id: String,
    pub channel_or_conversation_id: String,
    pub provider_message_id: String,
    pub equivalent_rule_id: String,
    pub channel_id: String,
    pub guild_id: String,
    pub peer_id: String,
    pub thread_id: String,
    pub author_id: String,
    pub content: String,
    pub kind: SessionKind,
    pub reply_to_message_id: String,
    pub direct: bool,
    pub mentioned: bool,
    pub received_at: DateTime<Utc>,
}

/// Assistant reply to send through a connector transport.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundReply {
    pub connector_id: String,
    pub channel_id: String,
    pub content: String,
    pub reply_to_external_message_id: String,
}

/// Edit of a previously sent external reply message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyEdit {
    pub connector_id: String,
    pub channel_id: String,
    pub external_message_id: String,
    pub content: String,
}

/// Signal that the assistant is thinking (e.g. typing indicator).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingSignal {
    pub connector_id: String,
    pub channel_id: String,
}

/// Result of a successfully sent [`OutboundReply`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentReply {
    pub external_message_id: String,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(sec, 0).single().expect("valid timestamp")
    }

    #[test]
    fn delivery_direction_serializes_like_go_constants() {
        assert_eq!(
            serde_json::to_string(&DeliveryDirection::Inbound).expect("serialize"),
            "\"inbound\""
        );
        assert_eq!(
            serde_json::to_string(&DeliveryDirection::Outbound).expect("serialize"),
            "\"outbound\""
        );
        let parsed: DeliveryDirection = serde_json::from_str("\"outbound\"").expect("deserialize");
        assert_eq!(parsed, DeliveryDirection::Outbound);
    }

    #[test]
    fn delivery_status_serializes_like_go_constants() {
        let cases = [
            (DeliveryStatus::Received, "received"),
            (DeliveryStatus::Thinking, "thinking"),
            (DeliveryStatus::Processing, "processing"),
            (DeliveryStatus::Streaming, "streaming"),
            (DeliveryStatus::Replied, "replied"),
            (DeliveryStatus::Partial, "partial"),
            (DeliveryStatus::Failed, "failed"),
        ];
        for (status, expected) in cases {
            assert_eq!(status.as_str(), expected);
            let json = serde_json::to_string(&status).expect("serialize");
            assert_eq!(json, format!("\"{expected}\""));
            let round_trip: DeliveryStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(round_trip, status);
        }
    }

    #[test]
    fn reply_capabilities_omit_zero_max_message_length() {
        let caps = ReplyCapabilities {
            supports_thinking: true,
            supports_streaming: false,
            max_message_length: 0,
        };
        let value = serde_json::to_value(&caps).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "supportsThinking": true,
                "supportsStreaming": false,
            })
        );
    }

    #[test]
    fn reply_capabilities_round_trip_with_limit() {
        let caps = ReplyCapabilities {
            supports_thinking: false,
            supports_streaming: true,
            max_message_length: 2000,
        };
        let json = serde_json::to_string(&caps).expect("serialize");
        assert!(json.contains("\"maxMessageLength\":2000"));
        let parsed: ReplyCapabilities = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, caps);
    }

    #[test]
    fn message_record_uses_go_json_tags_and_omitempty() {
        let record = MessageRecord {
            delivery_id: "dlv_1".to_string(),
            connector_id: "slack-main".to_string(),
            direction: DeliveryDirection::Inbound,
            channel_id: "C123".to_string(),
            content: "hello".to_string(),
            status: DeliveryStatus::Received,
            created_at: ts(1_700_000_000),
            updated_at: ts(1_700_000_001),
            ..Default::default()
        };
        let value = serde_json::to_value(&record).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "deliveryId": "dlv_1",
                "connectorId": "slack-main",
                "direction": "inbound",
                "channelId": "C123",
                "content": "hello",
                "status": "received",
                "createdAt": "2023-11-14T22:13:20Z",
                "updatedAt": "2023-11-14T22:13:21Z",
            })
        );
        let parsed: MessageRecord = serde_json::from_value(value).expect("deserialize");
        assert_eq!(parsed, record);
    }

    #[test]
    fn message_record_round_trip_with_all_optional_fields() {
        let record = MessageRecord {
            delivery_id: "dlv_2".to_string(),
            tenant_id: "ten_1".to_string(),
            connector_id: "discord-main".to_string(),
            direction: DeliveryDirection::Outbound,
            external_message_id: "msg_1".to_string(),
            connector_account_id: "acct_1".to_string(),
            channel_or_conversation_id: "conv_1".to_string(),
            provider_message_id: "prov_1".to_string(),
            equivalent_rule_id: "rule_1".to_string(),
            session_id: "sess_1".to_string(),
            thread_session_segment_id: "seg_1".to_string(),
            run_id: "run_1".to_string(),
            channel_id: "chan_1".to_string(),
            peer_id: "peer_1".to_string(),
            thread_id: "thread_1".to_string(),
            author_id: "author_1".to_string(),
            content: "reply body".to_string(),
            status: DeliveryStatus::Replied,
            error: "partial failure".to_string(),
            reply_to_external_message_id: "orig_1".to_string(),
            response_to_delivery_id: "dlv_1".to_string(),
            foreground_outcome_status: "succeeded".to_string(),
            background_delivery_id: "dlv_bg".to_string(),
            delivery_boundary_kind: "final".to_string(),
            created_at: ts(1_700_000_100),
            updated_at: ts(1_700_000_200),
        };
        let json = serde_json::to_string(&record).expect("serialize");
        for key in [
            "tenantId",
            "externalMessageId",
            "connectorAccountId",
            "channelOrConversationId",
            "providerMessageId",
            "equivalentRuleId",
            "sessionId",
            "threadSessionSegmentId",
            "runId",
            "peerId",
            "threadId",
            "authorId",
            "replyToExternalMessageId",
            "responseToDeliveryId",
            "foregroundOutcomeStatus",
            "backgroundDeliveryId",
            "deliveryBoundaryKind",
        ] {
            assert!(json.contains(&format!("\"{key}\"")), "missing key {key}");
        }
        let parsed: MessageRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, record);
    }

    #[test]
    fn inbound_message_defaults_to_direct_session_kind() {
        let message = InboundMessage {
            connector_id: "telegram-main".to_string(),
            connector_kind: "telegram".to_string(),
            channel_id: "chat_1".to_string(),
            content: "hi".to_string(),
            received_at: ts(1_700_000_000),
            ..Default::default()
        };
        assert_eq!(message.kind, SessionKind::Direct);
        assert!(!message.direct);
        assert!(!message.mentioned);
    }

    #[test]
    fn outbound_dtos_round_trip() {
        let reply = OutboundReply {
            connector_id: "matrix-main".to_string(),
            channel_id: "!room:example.org".to_string(),
            content: "assistant reply".to_string(),
            reply_to_external_message_id: "$event1".to_string(),
        };
        let json = serde_json::to_string(&reply).expect("serialize");
        let parsed: OutboundReply = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, reply);

        let edit = ReplyEdit {
            connector_id: "discord-main".to_string(),
            channel_id: "channel_1".to_string(),
            external_message_id: "reply_1".to_string(),
            content: "edited".to_string(),
        };
        let parsed: ReplyEdit =
            serde_json::from_str(&serde_json::to_string(&edit).expect("serialize"))
                .expect("deserialize");
        assert_eq!(parsed, edit);

        let signal = ThinkingSignal {
            connector_id: "slack-main".to_string(),
            channel_id: "C123".to_string(),
        };
        let parsed: ThinkingSignal =
            serde_json::from_str(&serde_json::to_string(&signal).expect("serialize"))
                .expect("deserialize");
        assert_eq!(parsed, signal);

        let sent = SentReply {
            external_message_id: "1699999999.000100".to_string(),
        };
        let parsed: SentReply =
            serde_json::from_str(&serde_json::to_string(&sent).expect("serialize"))
                .expect("deserialize");
        assert_eq!(parsed, sent);
    }

    #[test]
    fn message_record_missing_optional_fields_default_to_empty() {
        // Go unmarshals absent omitempty fields to the zero value; the port
        // must accept records written without them.
        let json = serde_json::json!({
            "deliveryId": "dlv_3",
            "connectorId": "slack-main",
            "direction": "outbound",
            "channelId": "C9",
            "content": "",
            "status": "failed",
            "createdAt": "2023-11-14T22:13:20Z",
            "updatedAt": "2023-11-14T22:13:20Z",
        });
        let record: MessageRecord = serde_json::from_value(json).expect("deserialize");
        assert_eq!(record.status, DeliveryStatus::Failed);
        assert!(record.tenant_id.is_empty());
        assert!(record.session_id.is_empty());
        assert!(record.error.is_empty());
    }
}
