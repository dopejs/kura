//! Anthropic's Messages API, spoken directly.
//!
//! A subscription is reached by holding its OAuth grant and calling this, not
//! by driving the vendor's coding agent from the command line. Driving that
//! agent means every request first pays for its system prompt and tool
//! definitions: measured against a ten-token question, twenty-seven thousand
//! tokens and eight seconds before a model saw anything. Loopforge is an agent
//! itself, so wrapping another one buys nothing.
//!
//! Nothing here forges a client attestation. Anthropic's own client sends one
//! so its servers can tell it apart from an impersonator; requests without it
//! are ordinary API requests made with a credential the user authorised, which
//! is what these are.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_stream::try_stream;
use futures::stream::BoxStream;
use futures::StreamExt;
use kura_protocol::{ResponseItem, Role};
use serde_json::{json, Value};

use crate::openai::{checked_bytes_stream, Credential};
use crate::provider::{ModelProvider, Prompt, ProviderError, ResponseEvent};

/// The version header the Messages API requires.
const API_VERSION: &str = "2023-06-01";

/// Betas an OAuth-authorised request is expected to declare.
///
/// `oauth-2025-04-20` is the one that matters: without it a grant minted for a
/// subscription is refused on this endpoint.
const OAUTH_BETAS: &str = "oauth-2025-04-20,claude-code-20250219";

/// Sent when the caller states no limit.
///
/// The Messages API requires `max_tokens`, unlike the chat-completions shape,
/// so there has to be a number here rather than an omission.
const DEFAULT_MAX_TOKENS: i64 = 8192;

/// Streaming client for Anthropic's `/v1/messages`.
pub struct AnthropicClient {
    base_url: String,
    model: String,
    credential: Credential,
    /// Extra headers. The credential and the API version cannot be replaced.
    headers: BTreeMap<String, String>,
    max_tokens: i64,
    http: reqwest::Client,
}

/// Headers a caller may not override.
///
/// The credential is supplied separately, and the version and beta headers are
/// what make the endpoint accept an OAuth grant at all -- a configuration file
/// able to replace them could only break the provider.
const RESERVED_HEADERS: [&str; 3] = ["authorization", "anthropic-version", "anthropic-beta"];

impl AnthropicClient {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        token: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            credential: Credential::new(token),
            headers: BTreeMap::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            http: reqwest::Client::new(),
        }
    }

    #[must_use]
    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers
            .into_iter()
            .filter(|(name, _)| !RESERVED_HEADERS.contains(&name.trim().to_lowercase().as_str()))
            .collect();
        self
    }

    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: i64) -> Self {
        if max_tokens > 0 {
            self.max_tokens = max_tokens;
        }
        self
    }

    /// A handle to this client's credential, for whatever keeps it fresh.
    #[must_use]
    pub fn credential(&self) -> Credential {
        self.credential.clone()
    }
}

/// A tool call arriving in fragments, by content-block index.
#[derive(Default)]
struct ToolUseAcc {
    id: String,
    name: String,
    arguments: String,
}

impl ModelProvider for AnthropicClient {
    fn stream<'a>(
        &'a self,
        prompt: &'a Prompt,
    ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
        let body = build_request(&self.model, prompt, self.max_tokens);
        let mut request = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", OAUTH_BETAS)
            .json(&body);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        // Read per request, not captured at construction: an access token
        // refreshed since the last dispatch has to be the one that goes out.
        let request = match self.credential.get() {
            Some(token) => request.bearer_auth(token),
            None => request,
        };

        Box::pin(try_stream! {
            let response = request.send().await?;
            let mut bytes = checked_bytes_stream(response).await?;

            let mut buffer = String::new();
            // Keyed by content-block index: a message interleaves text and
            // tool-use blocks, and their fragments arrive mixed together.
            let mut pending: BTreeMap<i64, ToolUseAcc> = BTreeMap::new();
            let mut done = false;

            while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim_end_matches('\r').to_string();
                    buffer.drain(..=pos);
                    // `event:` lines name what follows; the payload carries its
                    // own `type`, so only the data lines are read.
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data.is_empty() {
                        continue;
                    }
                    for event in accumulate_event(data, &mut pending, &mut done)? {
                        yield event;
                    }
                }
                if done {
                    break;
                }
            }

            // Whatever a `message_stop` did not close. Emitting them is better
            // than dropping a call the model asked for because the stream
            // ended untidily.
            for (index, call) in std::mem::take(&mut pending) {
                yield ResponseEvent::FunctionCall {
                    call_id: non_empty_or(call.id, format!("call_{index}")),
                    name: call.name,
                    arguments: non_empty_or(call.arguments, "{}".to_string()),
                };
            }
            yield ResponseEvent::Completed;
        })
    }
}

fn non_empty_or(value: String, fallback: String) -> String {
    if value.is_empty() { fallback } else { value }
}

/// Read one server-sent payload, emitting whatever it completes.
fn accumulate_event(
    data: &str,
    pending: &mut BTreeMap<i64, ToolUseAcc>,
    done: &mut bool,
) -> Result<Vec<ResponseEvent>, ProviderError> {
    let value: Value = serde_json::from_str(data)
        .map_err(|error| ProviderError::Malformed(error.to_string()))?;
    let mut events = Vec::new();
    let index = value["index"].as_i64().unwrap_or(0);

    match value["type"].as_str().unwrap_or_default() {
        "content_block_start" => {
            let block = &value["content_block"];
            if block["type"].as_str() == Some("tool_use") {
                pending.insert(
                    index,
                    ToolUseAcc {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        arguments: String::new(),
                    },
                );
            }
        }
        "content_block_delta" => {
            let delta = &value["delta"];
            match delta["type"].as_str().unwrap_or_default() {
                "text_delta" => {
                    if let Some(text) = delta["text"].as_str() {
                        if !text.is_empty() {
                            events.push(ResponseEvent::OutputTextDelta(text.to_string()));
                        }
                    }
                }
                // Tool arguments arrive as JSON fragments that mean nothing
                // until concatenated, so they are held rather than emitted.
                "input_json_delta" => {
                    if let Some(fragment) = delta["partial_json"].as_str() {
                        pending.entry(index).or_default().arguments.push_str(fragment);
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            if let Some(call) = pending.remove(&index) {
                events.push(ResponseEvent::FunctionCall {
                    call_id: non_empty_or(call.id, format!("call_{index}")),
                    name: call.name,
                    // An argument-less call still needs a body the caller can
                    // parse.
                    arguments: non_empty_or(call.arguments, "{}".to_string()),
                });
            }
        }
        "message_stop" => *done = true,
        // The stream carries its own failures rather than a status code, so a
        // mid-stream error has to end the turn instead of being skipped.
        "error" => {
            let message = value["error"]["message"]
                .as_str()
                .unwrap_or("the provider reported an error");
            return Err(ProviderError::Malformed(message.to_string()));
        }
        _ => {}
    }
    Ok(events)
}

/// Build the `/v1/messages` body from a prompt.
///
/// Anthropic differs from the chat-completions shape in three ways that matter:
/// instructions are a top-level `system` rather than a message, `max_tokens` is
/// required, and a tool result is a block inside a *user* message rather than a
/// message with its own role.
fn build_request(model: &str, prompt: &Prompt, max_tokens: i64) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    for item in &prompt.input {
        match item {
            ResponseItem::Message { role, content } => {
                let role = match role {
                    // Anthropic accepts only `user` and `assistant`. A system
                    // message arriving mid-conversation is folded into the
                    // user turn rather than dropped.
                    Role::Assistant => "assistant",
                    _ => "user",
                };
                messages.push(json!({
                    "role": role,
                    "content": [{"type": "text", "text": content}],
                }));
            }
            ResponseItem::FunctionCall { call_id, name, arguments } => {
                // `input` must be an object; the model's fragments are text
                // until parsed, and a malformed one becomes an empty object
                // rather than a body the endpoint rejects outright.
                let input: Value =
                    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
                messages.push(json!({
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": input,
                    }],
                }));
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": output,
                    }],
                }));
            }
        }
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
    });
    if let Some(instructions) = &prompt.instructions {
        if !instructions.trim().is_empty() {
            body["system"] = json!([{"type": "text", "text": instructions}]);
        }
    }
    if !prompt.tools.is_empty() {
        body["tools"] = Value::Array(
            prompt
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        // Named `input_schema` here, not `parameters`.
                        "input_schema": tool.parameters,
                    })
                })
                .collect(),
        );
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolSpec;
    use futures::StreamExt;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    /// An endpoint that records the request and replays a scripted stream.
    fn spawn(body: &'static str) -> (String, std::sync::Arc<Mutex<String>>) {
        let seen = std::sync::Arc::new(Mutex::new(String::new()));
        let recorded = std::sync::Arc::clone(&seen);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                // Read until the whole body has arrived. Headers and body
                // reach the socket in separate segments, so a single read
                // captures the headers and asserts nothing about the payload.
                let mut received: Vec<u8> = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    received.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&received);
                    let Some(head_end) = text.find("\r\n\r\n") else { continue };
                    let length: usize = text
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length: ")
                                .or_else(|| line.strip_prefix("Content-Length: "))
                        })
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    if received.len() >= head_end + 4 + length {
                        break;
                    }
                }
                *recorded.lock().expect("lock") =
                    String::from_utf8_lossy(&received).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{addr}"), seen)
    }

    async fn collect(client: &AnthropicClient, prompt: &Prompt) -> Vec<ResponseEvent> {
        let mut events = Vec::new();
        let mut stream = client.stream(prompt);
        while let Some(event) = stream.next().await {
            events.push(event.expect("stream event"));
        }
        events
    }

    fn user(text: &str) -> Prompt {
        Prompt {
            input: vec![ResponseItem::Message {
                role: Role::User,
                content: text.to_string(),
            }],
            ..Prompt::default()
        }
    }

    #[tokio::test]
    async fn text_deltas_are_relayed_in_order() {
        let sse = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n";
        let (base, _) = spawn(sse);
        let client = AnthropicClient::new(base, "claude-sonnet-4-5", Some("t".into()));

        let events = collect(&client, &user("hi")).await;

        assert_eq!(
            events,
            vec![
                ResponseEvent::OutputTextDelta("Hel".into()),
                ResponseEvent::OutputTextDelta("lo".into()),
                ResponseEvent::Completed,
            ]
        );
    }

    #[tokio::test]
    async fn a_tool_call_is_emitted_once_its_arguments_are_whole() {
        // Arguments arrive as JSON fragments that parse as nothing on their
        // own, so emitting per fragment would hand the caller broken input.
        let sse = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.rs\\\"}\"}}\n\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\ndata: {\"type\":\"message_stop\"}\n\n";
        let (base, _) = spawn(sse);
        let client = AnthropicClient::new(base, "m", Some("t".into()));

        let events = collect(&client, &user("hi")).await;

        assert_eq!(
            events,
            vec![
                ResponseEvent::FunctionCall {
                    call_id: "toolu_1".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a.rs\"}".into(),
                },
                ResponseEvent::Completed,
            ]
        );
    }

    #[tokio::test]
    async fn interleaved_blocks_do_not_mix_their_fragments() {
        // A message carries several blocks at once and their deltas arrive
        // mixed; keyed by anything but the index, two calls become one.
        let sse = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"a\",\"name\":\"one\"}}\n\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"b\",\"name\":\"two\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"x\\\":2}\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"x\\\":1}\"}}\n\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\ndata: {\"type\":\"message_stop\"}\n\n";
        let (base, _) = spawn(sse);
        let client = AnthropicClient::new(base, "m", Some("t".into()));

        let events = collect(&client, &user("hi")).await;

        assert_eq!(
            events,
            vec![
                ResponseEvent::FunctionCall {
                    call_id: "a".into(),
                    name: "one".into(),
                    arguments: "{\"x\":1}".into(),
                },
                ResponseEvent::FunctionCall {
                    call_id: "b".into(),
                    name: "two".into(),
                    arguments: "{\"x\":2}".into(),
                },
                ResponseEvent::Completed,
            ]
        );
    }

    #[tokio::test]
    async fn a_mid_stream_error_ends_the_turn() {
        // This endpoint reports failures inside a 200 stream, so skipping the
        // event would end the turn as though the model had simply stopped.
        let sse = "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let (base, _) = spawn(sse);
        let client = AnthropicClient::new(base, "m", Some("t".into()));

        let prompt = user("hi");
        let mut stream = client.stream(&prompt);
        let first = stream.next().await.expect("an event");

        match first {
            Err(ProviderError::Malformed(message)) => assert!(message.contains("Overloaded")),
            other => panic!("expected the error to surface, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_request_carries_what_this_endpoint_requires() {
        let (base, seen) = spawn("data: {\"type\":\"message_stop\"}\n\n");
        let client = AnthropicClient::new(base, "claude-sonnet-4-5", Some("oauth-token".into()));
        let prompt = Prompt {
            instructions: Some("be brief".to_string()),
            input: vec![ResponseItem::Message { role: Role::User, content: "hi".into() }],
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "read".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
        };

        collect(&client, &prompt).await;

        let request = seen.lock().expect("lock").clone();
        // The beta is what makes a subscription grant acceptable here.
        assert!(request.contains("anthropic-beta: oauth-2025-04-20"), "{request}");
        assert!(request.contains("anthropic-version: 2023-06-01"), "{request}");
        assert!(request.contains("Bearer oauth-token"), "{request}");
        // Required by this endpoint, unlike the chat-completions shape.
        assert!(request.contains("\"max_tokens\""), "{request}");
        // Instructions are top-level `system`, not a message.
        assert!(request.contains("\"system\""), "{request}");
        // And tool schemas are `input_schema`.
        assert!(request.contains("\"input_schema\""), "{request}");
        assert!(!request.contains("\"parameters\""), "{request}");
    }

    #[tokio::test]
    async fn a_tool_result_is_sent_as_a_user_block() {
        // Anthropic has no `tool` role: a result is a block inside a user
        // message, and sending it as its own role is rejected.
        let (base, seen) = spawn("data: {\"type\":\"message_stop\"}\n\n");
        let client = AnthropicClient::new(base, "m", Some("t".into()));
        let prompt = Prompt {
            input: vec![ResponseItem::FunctionCallOutput {
                call_id: "toolu_1".into(),
                output: "contents".into(),
            }],
            ..Prompt::default()
        };

        collect(&client, &prompt).await;

        let request = seen.lock().expect("lock").clone();
        assert!(request.contains("\"tool_result\""), "{request}");
        assert!(request.contains("\"tool_use_id\":\"toolu_1\""), "{request}");
        assert!(!request.contains("\"role\":\"tool\""), "{request}");
    }

    #[tokio::test]
    async fn a_refreshed_token_reaches_the_next_request() {
        let (base, seen) = spawn("data: {\"type\":\"message_stop\"}\n\n");
        let client = AnthropicClient::new(base, "m", Some("first".into()));

        client.credential().set(Some("second".into()));
        collect(&client, &user("hi")).await;

        assert!(seen.lock().expect("lock").contains("Bearer second"));
    }
}
