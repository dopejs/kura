use serde::Deserialize;
use serde::Serialize;

/// Client → core submission. Mirrors codex's `Op`: every way a caller can
/// drive or interrupt the agent goes through this enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    UserInput { text: String },
    Interrupt,
    Shutdown,
}

/// A tool the model may call, as the provider is told about it.
///
/// Lives here rather than beside the HTTP clients that serialize it: the
/// dispatcher has to carry a tool list from the caller to the provider, and it
/// sits below those clients in the dependency graph. Keeping the type up there
/// is why the bridge could only ever hand the provider an empty list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments. Opaque here; each wire shapes it.
    pub parameters: serde_json::Value,
}

/// One item of conversation history fed back to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    Message {
        role: Role,
        content: String,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
        /// Images the tool returned beside its text, such as a screenshot.
        /// Omitted when empty, so a text-only result serializes as before.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImagePart>,
    },
}

/// An image a tool returned, such as a screenshot.
///
/// `data` is held in memory only and never serialized. Every place that
/// writes history down (turn checkpoints, dispatch records) would otherwise
/// store each screenshot again on every round, and the screen may show what
/// the user would not want kept there. The image itself lives in the
/// artifact store; a stored copy keeps the reference. A part read back from
/// storage has no data, and the wires show the model a note in its place:
/// after a resume the screen has moved on anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePart {
    /// `image/png`, `image/jpeg`, …
    pub media_type: String,
    /// Base64, without a `data:` prefix. Empty once the image left memory.
    #[serde(skip)]
    pub data: String,
    /// The artifact holding the image, when the tool saved one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

impl ImagePart {
    /// What the model reads where an image is no longer available.
    pub const OMITTED: &'static str = "[image omitted]";

    /// Whether the bytes are still here to send.
    #[must_use]
    pub fn is_present(&self) -> bool {
        !self.data.is_empty()
    }

    /// The part without its bytes, as storage keeps it.
    #[must_use]
    pub fn stored(&self) -> Self {
        Self {
            media_type: self.media_type.clone(),
            data: String::new(),
            artifact: self.artifact.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// core → client event. `id` is a per-turn sequence anchor so clients can
/// order and deduplicate deltas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub msg: EventMsg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventMsg {
    TurnStarted,
    AgentMessageDelta {
        delta: String,
    },
    ToolCallBegin {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolCallEnd {
        call_id: String,
        output: String,
        success: bool,
    },
    AgentMessage {
        message: String,
    },
    /// A message the user sent to the turn while it ran, now part of the
    /// conversation the model reads next (agent-runtime-program.md 2.8, G8).
    SteerMessage {
        message: String,
    },
    TurnComplete,
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_serializes_with_snake_case_tag() {
        let op = Op::UserInput { text: "hi".into() };
        let json = serde_json::to_value(&op).unwrap();
        assert_eq!(json, serde_json::json!({"op": "user_input", "text": "hi"}));
    }

    #[test]
    fn event_msg_round_trips() {
        let event = Event {
            id: "t-1".into(),
            msg: EventMsg::ToolCallEnd {
                call_id: "call_1".into(),
                output: "ok".into(),
                success: true,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn response_item_tool_cycle_shape() {
        let item = ResponseItem::FunctionCall {
            call_id: "call_1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["type"], "function_call");
        assert_eq!(json["name"], "shell");
    }

    #[test]
    fn a_tool_result_stores_its_images_by_reference_only() {
        // Text only: the same JSON as before images existed, both ways.
        let plain = r#"{"type":"function_call_output","call_id":"c","output":"o"}"#;
        let item: ResponseItem = serde_json::from_str(plain).unwrap();
        assert_eq!(
            item,
            ResponseItem::FunctionCallOutput {
                call_id: "c".into(),
                output: "o".into(),
                images: Vec::new(),
            }
        );
        assert_eq!(serde_json::to_string(&item).unwrap(), plain);

        let shot = ResponseItem::FunctionCallOutput {
            call_id: "c".into(),
            output: "o".into(),
            images: vec![ImagePart {
                media_type: "image/png".into(),
                data: "BYTES".into(),
                artifact: Some("art_1".into()),
            }],
        };
        let json = serde_json::to_string(&shot).unwrap();
        assert_eq!(
            json,
            r#"{"type":"function_call_output","call_id":"c","output":"o","images":[{"mediaType":"image/png","artifact":"art_1"}]}"#
        );
        let ResponseItem::FunctionCallOutput { images, .. } = serde_json::from_str(&json).unwrap()
        else {
            unreachable!()
        };
        assert!(!images[0].is_present());
        assert_eq!(images[0].artifact.as_deref(), Some("art_1"));
    }
}
