use async_stream::try_stream;
use futures::StreamExt;
use futures::stream::BoxStream;
use kura_protocol::ResponseItem;
use kura_protocol::Role;
use serde_json::Value;
use serde_json::json;

use crate::provider::ModelProvider;
use crate::provider::Prompt;
use crate::provider::ProviderError;
use crate::provider::ResponseEvent;

/// Streaming client for OpenAI-compatible `/chat/completions` endpoints
/// (OpenAI, local vLLM/Ollama gateways, openai-compatible providers in the
/// Go daemon's provider manager).
pub struct OpenAiCompatibleClient {
    base_url: String,
    model: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl OpenAiCompatibleClient {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key,
            http: reqwest::Client::new(),
        }
    }
}

/// Partially received tool call; `arguments` arrive as JSON fragments that
/// must be concatenated before the call is complete.
#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
}

impl ModelProvider for OpenAiCompatibleClient {
    fn stream<'a>(
        &'a self,
        prompt: &'a Prompt,
    ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
        let body = build_request(&self.model, prompt);
        let request = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);
        let request = match &self.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        };
        Box::pin(try_stream! {
            let response = request.send().await?;
            let mut bytes = checked_bytes_stream(response).await?;

            let mut buffer = String::new();
            let mut pending_calls: Vec<ToolCallAcc> = Vec::new();
            let mut done = false;
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim_end_matches('\r').to_string();
                    buffer.drain(..=pos);
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        done = true;
                        break;
                    }
                    if data.is_empty() {
                        continue;
                    }
                    for event in accumulate_chunk(data, &mut pending_calls)? {
                        yield event;
                    }
                }
                if done {
                    break;
                }
            }

            for (index, call) in pending_calls.into_iter().enumerate() {
                yield ResponseEvent::FunctionCall {
                    call_id: non_empty_or(call.id, format!("call_{index}")),
                    name: call.name,
                    arguments: call.arguments,
                };
            }
            yield ResponseEvent::Completed;
        })
    }
}

fn non_empty_or(value: String, fallback: String) -> String {
    if value.is_empty() { fallback } else { value }
}

/// Return the response byte stream, or a `Status` error carrying the body.
async fn checked_bytes_stream(
    response: reqwest::Response,
) -> Result<impl futures::Stream<Item = reqwest::Result<bytes::Bytes>>, ProviderError> {
    if response.status().is_success() {
        Ok(response.bytes_stream())
    } else {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        Err(ProviderError::Status { status, body })
    }
}

/// Parse one SSE `data:` payload into events, accumulating tool-call
/// fragments in `pending_calls`. Returns only immediately emittable events
/// (text deltas); completed calls are flushed by the caller at stream end.
fn accumulate_chunk(
    data: &str,
    pending_calls: &mut Vec<ToolCallAcc>,
) -> Result<Vec<ResponseEvent>, ProviderError> {
    let chunk: Value = serde_json::from_str(data)
        .map_err(|err| ProviderError::Malformed(format!("invalid chunk json: {err}")))?;
    let mut events = Vec::new();
    let Some(choices) = chunk["choices"].as_array() else {
        return Ok(events);
    };
    for choice in choices {
        let delta = &choice["delta"];
        if let Some(content) = delta["content"].as_str()
            && !content.is_empty()
        {
            events.push(ResponseEvent::OutputTextDelta(content.to_string()));
        }
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for (position, call) in tool_calls.iter().enumerate() {
                let index = call["index"].as_u64().unwrap_or(position as u64) as usize;
                if pending_calls.len() <= index {
                    pending_calls.resize_with(index + 1, ToolCallAcc::default);
                }
                let acc = &mut pending_calls[index];
                if let Some(id) = call["id"].as_str() {
                    acc.id.push_str(id);
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    acc.name.push_str(name);
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    acc.arguments.push_str(arguments);
                }
            }
        }
    }
    Ok(events)
}

/// Build the `/chat/completions` request body, mapping conversation history
/// into OpenAI message shapes.
fn build_request(model: &str, prompt: &Prompt) -> Value {
    let mut messages = Vec::new();
    if let Some(instructions) = &prompt.instructions {
        messages.push(json!({"role": "system", "content": instructions}));
    }
    for item in &prompt.input {
        match item {
            ResponseItem::Message { role, content } => {
                let role = match role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                };
                messages.push(json!({"role": role, "content": content}));
            }
            ResponseItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                messages.push(json!({
                    "role": "assistant",
                    "tool_calls": [{
                        "id": call_id,
                        "type": "function",
                        "function": {"name": name, "arguments": arguments},
                    }],
                }));
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": output,
                }));
            }
        }
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });
    if !prompt.tools.is_empty() {
        let tools: Vec<Value> = prompt
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect();
        body["tools"] = Value::Array(tools);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolSpec;

    #[test]
    fn request_maps_history_to_openai_messages() {
        let prompt = Prompt {
            instructions: Some("be brief".into()),
            input: vec![
                ResponseItem::Message {
                    role: Role::User,
                    content: "hi".into(),
                },
                ResponseItem::FunctionCall {
                    call_id: "call_1".into(),
                    name: "shell".into(),
                    arguments: "{\"cmd\":\"ls\"}".into(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".into(),
                    output: "file.txt".into(),
                },
            ],
            tools: vec![ToolSpec {
                name: "shell".into(),
                description: "run a command".into(),
                parameters: json!({"type": "object"}),
            }],
        };
        let body = build_request("test-model", &prompt);
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], true);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages[0],
            json!({"role": "system", "content": "be brief"})
        );
        assert_eq!(messages[1], json!({"role": "user", "content": "hi"}));
        assert_eq!(messages[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(
            messages[3],
            json!({"role": "tool", "tool_call_id": "call_1", "content": "file.txt"})
        );
        assert_eq!(body["tools"][0]["function"]["name"], "shell");
    }

    #[test]
    fn chunk_without_tools_yields_text_deltas() {
        let mut pending = Vec::new();
        let events = accumulate_chunk(
            r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
            &mut pending,
        )
        .unwrap();
        assert_eq!(events, vec![ResponseEvent::OutputTextDelta("hello".into())]);
        assert!(pending.is_empty());
    }

    #[test]
    fn tool_call_fragments_accumulate_across_chunks() {
        let mut pending = Vec::new();
        accumulate_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"name":"shell","arguments":"{\"cm"}}]}}]}"#,
            &mut pending,
        )
        .unwrap();
        let events = accumulate_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"d\":\"ls\"}"}}]}}]}"#,
            &mut pending,
        )
        .unwrap();
        assert!(events.is_empty());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "call_9");
        assert_eq!(pending[0].name, "shell");
        assert_eq!(pending[0].arguments, "{\"cmd\":\"ls\"}");
    }

    #[test]
    fn usage_only_chunks_are_ignored() {
        let mut pending = Vec::new();
        let events =
            accumulate_chunk(r#"{"choices":[],"usage":{"total_tokens":3}}"#, &mut pending).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn invalid_json_is_malformed_error() {
        let mut pending = Vec::new();
        let err = accumulate_chunk("{oops", &mut pending).unwrap_err();
        assert!(matches!(err, ProviderError::Malformed(_)));
    }
}
