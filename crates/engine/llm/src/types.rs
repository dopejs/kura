//! Serde domain types consumed by daemon packages, mirroring the JSON shapes
//! of `daemon/internal/llm/dispatcher.go`.

use chrono::{DateTime, Utc};
use crate::provider::ToolCall;
use kura_protocol::ToolSpec;
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

/// One turn of conversation as the dispatcher carries it.
///
/// The two tool fields are what let a turn that called something be replayed
/// on the next round. Without them an assistant message could only say what
/// the model wrote, not what it asked to call, so the round after a tool ran
/// showed the model a result with nothing to attach it to.
///
/// The shape mirrors the chat-completions wire every provider already speaks:
/// an assistant message carries `tool_calls`, and each result comes back as a
/// `Tool` message naming the call it answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    /// Calls this assistant turn asked for. Empty on every other role.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// The call a `Tool` message is the result of. Empty on every other role.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
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
    /// The tools the model was offered on this round.
    ///
    /// Persisted with the rest of the dispatch for the same reason the
    /// messages are: what the model could see is what the record has to show,
    /// or a turn that called a tool cannot be explained afterwards.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    pub stream: bool,
    pub status: DispatchStatus,
    pub output: String,
    /// Calls the model asked for on this round, if any.
    ///
    /// A caller running a tool loop reads these, runs them, appends the
    /// results to `messages`, and dispatches again. Empty means the model
    /// answered, which is how a loop knows the turn is over.
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
// Not `Eq`: a tool carries a JSON Schema, and `serde_json::Value` is only
// `PartialEq`. Nothing compares dispatch inputs for equality.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateDispatchInput {
    pub provider: String,
    pub model: String,
    pub messages: Vec<Message>,
    /// Tools the caller is offering the model for this dispatch.
    ///
    /// A dispatch is one model round, not a whole turn: a caller running a
    /// tool loop offers the same tools each round and appends what the tools
    /// answered to `messages`. Keeping the loop above this layer means every
    /// round is prepared, persisted and hooked like any other dispatch.
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
        assert_eq!(serde_json::to_string(&MessageRole::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&MessageRole::User).unwrap(), "\"user\"");
        assert_eq!(serde_json::to_string(&MessageRole::Assistant).unwrap(), "\"assistant\"");
        assert_eq!(serde_json::to_string(&MessageRole::Tool).unwrap(), "\"tool\"");
    }

    #[test]
    fn dispatch_status_wire_values_match_go() {
        assert_eq!(serde_json::to_string(&DispatchStatus::Queued).unwrap(), "\"queued\"");
        assert_eq!(serde_json::to_string(&DispatchStatus::Running).unwrap(), "\"running\"");
        assert_eq!(serde_json::to_string(&DispatchStatus::Completed).unwrap(), "\"completed\"");
        assert_eq!(
            serde_json::to_string(&DispatchStatus::PartialFailed).unwrap(),
            "\"partial_failed\""
        );
        assert_eq!(serde_json::to_string(&DispatchStatus::Failed).unwrap(), "\"failed\"");
        assert_eq!(serde_json::to_string(&DispatchStatus::Cancelled).unwrap(), "\"cancelled\"");
    }

    #[test]
    fn usage_serializes_camel_case() {
        let usage = Usage { input_tokens: 3, output_tokens: 1, total_tokens: 4 };
        let json = serde_json::to_value(usage).unwrap();
        assert_eq!(json, serde_json::json!({"inputTokens": 3, "outputTokens": 1, "totalTokens": 4}));
    }

    #[test]
    fn dispatch_omits_empty_optional_fields_like_go() {
        let now = Utc::now();
        let dispatch = Dispatch {
            tools: Vec::new(),
            tool_calls: Vec::new(),
            dispatch_id: "d-1".into(),
            provider: "echo".into(),
            model: "m".into(),
            messages: vec![],
            stream: false,
            status: DispatchStatus::Queued,
            output: String::new(),
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
        let chunk = StreamChunk { delta: "hi".into(), ..StreamChunk::default() };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json, serde_json::json!({"delta": "hi"}));
    }
}
