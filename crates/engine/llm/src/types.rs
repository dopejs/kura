//! Serde domain types consumed by daemon packages, mirroring the JSON shapes
//! of `daemon/internal/llm/dispatcher.go`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Chat message role; wire values match the Go `MessageRole` constants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    #[default]
    System,
    User,
    Assistant,
    Tool,
}

/// A tool the model may call: name, description, and JSON Schema parameters
/// (Stage 9.0). Provider-agnostic; each provider encodes it in its own wire
/// shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
}

/// A tool invocation the model asked for. `arguments` is the raw JSON text
/// the model produced, kept verbatim so what was logged is what was run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    /// Assistant turns that requested tools carry the calls; absent otherwise.
    /// `serde(default)` keeps every persisted `messages_json` readable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For `role: tool` messages: which call this result answers.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
}

impl Message {
    #[must_use]
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        Message {
            role,
            content: content.into(),
            ..Message::default()
        }
    }
}

/// Token accounting for one dispatch. `total_tokens` is normalized to
/// `input + output` by the dispatcher when a provider leaves it zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

/// Dispatch lifecycle state; wire values match the Go `DispatchStatus`
/// constants (`partial_failed` etc.).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    #[default]
    Queued,
    Running,
    Completed,
    PartialFailed,
    Failed,
    Cancelled,
}

/// A prepared or settled dispatch record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    pub dispatch_id: String,
    pub provider: String,
    pub model: String,
    pub messages: Vec<Message>,
    /// Tools offered to the model on this dispatch (Stage 9.0).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    pub stream: bool,
    pub status: DispatchStatus,
    pub output: String,
    /// Tool calls the model answered with instead of (or alongside) text. A
    /// completed dispatch with calls and no output is not a failure: it is the
    /// model asking for work before it can answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub finish_reason: String,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub timeout_ms: i64,
    pub partial: bool,
    pub max_retries: i64,
    pub attempt_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Input accepted by [`crate::Dispatcher::prepare`].
// serde(default): Go decodes the create-dispatch request into the zero value,
// so absent fields (e.g. timeoutMs) must degrade to zero instead of rejecting
// the request at the API boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateDispatchInput {
    pub provider: String,
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    pub timeout_ms: i64,
    pub max_retries: i64,
}

/// One streamed delta forwarded to the caller's emitter. The dispatcher
/// backfills `output` with the aggregate text streamed so far.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamChunk {
    pub delta: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub finish_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_role_wire_values_match_go() {
        assert_eq!(
            serde_json::to_string(&MessageRole::System).unwrap(),
            "\"system\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::User).unwrap(),
            "\"user\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::Tool).unwrap(),
            "\"tool\""
        );
    }

    #[test]
    fn dispatch_status_wire_values_match_go() {
        assert_eq!(
            serde_json::to_string(&DispatchStatus::Queued).unwrap(),
            "\"queued\""
        );
        assert_eq!(
            serde_json::to_string(&DispatchStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&DispatchStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&DispatchStatus::PartialFailed).unwrap(),
            "\"partial_failed\""
        );
        assert_eq!(
            serde_json::to_string(&DispatchStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&DispatchStatus::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }

    #[test]
    fn usage_serializes_camel_case() {
        let usage = Usage {
            input_tokens: 3,
            output_tokens: 1,
            total_tokens: 4,
        };
        let json = serde_json::to_value(usage).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"inputTokens": 3, "outputTokens": 1, "totalTokens": 4})
        );
    }

    #[test]
    fn dispatch_omits_empty_optional_fields_like_go() {
        let now = Utc::now();
        let dispatch = Dispatch {
            dispatch_id: "d-1".into(),
            provider: "echo".into(),
            model: "m".into(),
            messages: vec![],
            tools: vec![],
            stream: false,
            status: DispatchStatus::Queued,
            output: String::new(),
            tool_calls: vec![],
            finish_reason: String::new(),
            usage: Usage::default(),
            error_code: String::new(),
            error: String::new(),
            timeout_ms: 30_000,
            partial: false,
            max_retries: 0,
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
        };
        let json = serde_json::to_value(&dispatch).unwrap();
        let object = json.as_object().unwrap();
        assert!(!object.contains_key("finishReason"));
        assert!(!object.contains_key("errorCode"));
        assert!(!object.contains_key("error"));
        assert!(!object.contains_key("startedAt"));
        assert!(!object.contains_key("completedAt"));
        assert!(object.contains_key("createdAt"));
        assert!(object.contains_key("timeoutMs"));
        // Round-trips despite the omitted fields.
        let back: Dispatch = serde_json::from_value(json).unwrap();
        assert_eq!(back, dispatch);
    }

    #[test]
    fn stream_chunk_omits_empty_optional_fields_like_go() {
        let chunk = StreamChunk {
            delta: "hi".into(),
            ..StreamChunk::default()
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json, serde_json::json!({"delta": "hi"}));
    }
}
