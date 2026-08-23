//! OpenAI's Responses API, as Codex speaks it.
//!
//! The second of the two wires a subscription needs; everything else a user is
//! likely to hold -- xAI, Kimi, Z.ai, MiniMax -- is OpenAI-compatible and needs
//! no protocol of its own, only its token.
//!
//! This shape differs from chat completions in more than its path. Input is a
//! list of typed items rather than messages, instructions are a top-level
//! field, tools are flat objects rather than nested under `function`, and the
//! stream is a sequence of named events rather than choice deltas.
//!
//! Nothing here forges a client attestation. OpenAI's own client sends one so
//! its servers can tell it apart from an impersonator; a request without it is
//! an ordinary API request made with a credential the user authorised.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_stream::try_stream;
use futures::stream::BoxStream;
use futures::StreamExt;
use kura_protocol::{ResponseItem, Role};
use serde_json::{json, Value};

use crate::openai::{checked_bytes_stream, Credential};
use crate::provider::{ModelProvider, Prompt, ProviderError, ResponseEvent};

/// Streaming client for `/responses`.
pub struct ResponsesClient {
    base_url: String,
    model: String,
    credential: Credential,
    headers: BTreeMap<String, String>,
    http: reqwest::Client,
}

/// The credential is supplied separately and cannot be replaced by a header map.
const RESERVED_HEADERS: [&str; 1] = ["authorization"];

impl ResponsesClient {
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
    pub fn credential(&self) -> Credential {
        self.credential.clone()
    }
}

/// A tool call arriving in fragments, by output index.
#[derive(Default)]
struct CallAcc {
    call_id: String,
    name: String,
    arguments: String,
}

impl ModelProvider for ResponsesClient {
    fn stream<'a>(
        &'a self,
        prompt: &'a Prompt,
    ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
        let body = build_request(&self.model, prompt);
        let mut request = self
            .http
            .post(format!("{}/responses", self.base_url))
            .json(&body);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        // Read per request: a token refreshed since the last dispatch has to
        // be the one that goes out.
        let request = match self.credential.get() {
            Some(token) => request.bearer_auth(token),
            None => request,
        };

        Box::pin(try_stream! {
            let response = request.send().await?;
            let mut bytes = checked_bytes_stream(response).await?;

            let mut buffer = String::new();
            let mut pending: BTreeMap<i64, CallAcc> = BTreeMap::new();
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
                    if data.is_empty() || data == "[DONE]" {
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

            for (index, call) in std::mem::take(&mut pending) {
                yield ResponseEvent::FunctionCall {
                    call_id: non_empty_or(call.call_id, format!("call_{index}")),
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

fn accumulate_event(
    data: &str,
    pending: &mut BTreeMap<i64, CallAcc>,
    done: &mut bool,
) -> Result<Vec<ResponseEvent>, ProviderError> {
    let value: Value = serde_json::from_str(data)
        .map_err(|error| ProviderError::Malformed(error.to_string()))?;
    let mut events = Vec::new();
    let index = value["output_index"].as_i64().unwrap_or(0);

    match value["type"].as_str().unwrap_or_default() {
        "response.output_text.delta" => {
            if let Some(text) = value["delta"].as_str() {
                if !text.is_empty() {
                    events.push(ResponseEvent::OutputTextDelta(text.to_string()));
                }
            }
        }
        // A tool call is announced before its arguments arrive.
        "response.output_item.added" => {
            let item = &value["item"];
            if item["type"].as_str() == Some("function_call") {
                pending.insert(
                    index,
                    CallAcc {
                        call_id: item["call_id"]
                            .as_str()
                            .or_else(|| item["id"].as_str())
                            .unwrap_or_default()
                            .to_string(),
                        name: item["name"].as_str().unwrap_or_default().to_string(),
                        arguments: String::new(),
                    },
                );
            }
        }
        // Arguments arrive as JSON fragments that parse as nothing alone.
        "response.function_call_arguments.delta" => {
            if let Some(fragment) = value["delta"].as_str() {
                pending.entry(index).or_default().arguments.push_str(fragment);
            }
        }
        "response.function_call_arguments.done" => {
            if let Some(call) = pending.get_mut(&index) {
                // The final event carries the whole string; preferring it
                // avoids a call assembled from fragments that went missing.
                if let Some(arguments) = value["arguments"].as_str() {
                    if !arguments.is_empty() {
                        call.arguments = arguments.to_string();
                    }
                }
            }
        }
        "response.output_item.done" => {
            if let Some(call) = pending.remove(&index) {
                events.push(ResponseEvent::FunctionCall {
                    call_id: non_empty_or(call.call_id, format!("call_{index}")),
                    name: call.name,
                    arguments: non_empty_or(call.arguments, "{}".to_string()),
                });
            }
        }
        "response.completed" | "response.done" => *done = true,
        // Reported inside a 200 stream rather than as a status, so ignoring it
        // would end the turn as though the model had simply stopped.
        "response.failed" | "error" => {
            let message = value["response"]["error"]["message"]
                .as_str()
                .or_else(|| value["error"]["message"].as_str())
                .unwrap_or("the provider reported an error");
            return Err(ProviderError::Malformed(message.to_string()));
        }
        _ => {}
    }
    Ok(events)
}

/// Build the `/responses` body from a prompt.
fn build_request(model: &str, prompt: &Prompt) -> Value {
    let mut input: Vec<Value> = Vec::new();

    for item in &prompt.input {
        match item {
            ResponseItem::Message { role, content } => {
                let role = match role {
                    Role::Assistant => "assistant",
                    Role::System => "system",
                    _ => "user",
                };
                // Input text is `input_text`; the assistant's own output is
                // `output_text`, and swapping them is rejected.
                let part = if role == "assistant" { "output_text" } else { "input_text" };
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": [{"type": part, "text": content}],
                }));
            }
            ResponseItem::FunctionCall { call_id, name, arguments } => {
                input.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    // Arguments stay a string here, unlike Anthropic's object.
                    "arguments": arguments,
                }));
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
    }

    let mut body = json!({
        "model": model,
        "input": input,
        "stream": true,
    });
    if let Some(instructions) = &prompt.instructions {
        if !instructions.trim().is_empty() {
            body["instructions"] = json!(instructions);
        }
    }
    if !prompt.tools.is_empty() {
        body["tools"] = Value::Array(
            prompt
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        // Flat, not nested under `function` as chat
                        // completions requires.
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
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

    fn spawn(body: &'static str) -> (String, std::sync::Arc<Mutex<String>>) {
        let seen = std::sync::Arc::new(Mutex::new(String::new()));
        let recorded = std::sync::Arc::clone(&seen);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                // Until the whole body has arrived: headers and payload reach
                // the socket separately, so one read asserts nothing about it.
                let mut received: Vec<u8> = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 { break; }
                    received.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&received);
                    let Some(head_end) = text.find("\r\n\r\n") else { continue };
                    let length: usize = text
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: ")
                            .or_else(|| line.strip_prefix("Content-Length: ")))
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    if received.len() >= head_end + 4 + length { break; }
                }
                *recorded.lock().expect("lock") = String::from_utf8_lossy(&received).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{addr}"), seen)
    }

    async fn collect(client: &ResponsesClient, prompt: &Prompt) -> Vec<ResponseEvent> {
        let mut events = Vec::new();
        let mut stream = client.stream(prompt);
        while let Some(event) = stream.next().await {
            events.push(event.expect("stream event"));
        }
        events
    }

    fn user(text: &str) -> Prompt {
        Prompt {
            input: vec![ResponseItem::Message { role: Role::User, content: text.to_string() }],
            ..Prompt::default()
        }
    }

    #[tokio::test]
    async fn text_deltas_are_relayed_in_order() {
        let sse = "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Hel\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"lo\"}\n\ndata: {\"type\":\"response.completed\"}\n\n";
        let (base, _) = spawn(sse);
        let client = ResponsesClient::new(base, "gpt-5-codex", Some("t".into()));

        let events = collect(&client, &user("hi")).await;

        assert_eq!(events, vec![
            ResponseEvent::OutputTextDelta("Hel".into()),
            ResponseEvent::OutputTextDelta("lo".into()),
            ResponseEvent::Completed,
        ]);
    }

    #[tokio::test]
    async fn a_tool_call_is_emitted_once_its_arguments_are_whole() {
        let sse = "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"fc_1\",\"name\":\"read_file\"}}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"path\\\":\"}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"\\\"a.rs\\\"}\"}\n\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\ndata: {\"type\":\"response.completed\"}\n\n";
        let (base, _) = spawn(sse);
        let client = ResponsesClient::new(base, "m", Some("t".into()));

        let events = collect(&client, &user("hi")).await;

        assert_eq!(events, vec![
            ResponseEvent::FunctionCall {
                call_id: "fc_1".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"a.rs\"}".into(),
            },
            ResponseEvent::Completed,
        ]);
    }

    #[tokio::test]
    async fn the_final_argument_string_wins_over_the_fragments() {
        // The endpoint repeats the whole string when it is done. Trusting the
        // fragments alone loses a call whose deltas were dropped in transit.
        let sse = "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"fc_1\",\"name\":\"f\"}}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"partial\\\"\"}\n\ndata: {\"type\":\"response.function_call_arguments.done\",\"output_index\":0,\"arguments\":\"{\\\"whole\\\":true}\"}\n\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\ndata: {\"type\":\"response.completed\"}\n\n";
        let (base, _) = spawn(sse);
        let client = ResponsesClient::new(base, "m", Some("t".into()));

        let events = collect(&client, &user("hi")).await;

        assert_eq!(events[0], ResponseEvent::FunctionCall {
            call_id: "fc_1".into(),
            name: "f".into(),
            arguments: "{\"whole\":true}".into(),
        });
    }

    #[tokio::test]
    async fn two_calls_in_one_response_do_not_mix_their_fragments() {
        // A response carries several output items and their argument deltas
        // arrive interleaved. Keyed by anything but the output index, two
        // calls collapse into one carrying the other's arguments.
        let sse = "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"a\",\"name\":\"one\"}}\n\ndata: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"call_id\":\"b\",\"name\":\"two\"}}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"{\\\"x\\\":2}\"}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"x\\\":1}\"}\n\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\ndata: {\"type\":\"response.output_item.done\",\"output_index\":1}\n\ndata: {\"type\":\"response.completed\"}\n\n";
        let (base, _) = spawn(sse);
        let client = ResponsesClient::new(base, "m", Some("t".into()));

        let events = collect(&client, &user("hi")).await;

        assert_eq!(events, vec![
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
        ]);
    }

    #[tokio::test]
    async fn a_failure_inside_the_stream_ends_the_turn() {
        let sse = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"rate limited\"}}}\n\n";
        let (base, _) = spawn(sse);
        let client = ResponsesClient::new(base, "m", Some("t".into()));

        let prompt = user("hi");
        let mut stream = client.stream(&prompt);
        match stream.next().await.expect("an event") {
            Err(ProviderError::Malformed(message)) => assert!(message.contains("rate limited")),
            other => panic!("expected the failure to surface, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_request_uses_this_api_rather_than_chat_completions() {
        let (base, seen) = spawn("data: {\"type\":\"response.completed\"}\n\n");
        let client = ResponsesClient::new(base, "gpt-5-codex", Some("oauth-token".into()));
        let prompt = Prompt {
            instructions: Some("be brief".into()),
            input: vec![ResponseItem::Message { role: Role::User, content: "hi".into() }],
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "read".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
        };

        collect(&client, &prompt).await;

        let request = seen.lock().expect("lock").clone();
        assert!(request.contains("POST /responses"), "{request}");
        assert!(request.contains("Bearer oauth-token"), "{request}");
        // Input items, not messages; instructions top-level, not a message.
        assert!(request.contains("\"input\""), "{request}");
        assert!(request.contains("\"instructions\""), "{request}");
        assert!(!request.contains("\"messages\""), "{request}");
        // A tool is flat here. Chat completions nests the name and schema
        // under a `function` object; sending that shape is rejected. Key order
        // is not part of the contract, so the shape is what is asserted.
        assert!(request.contains("\"name\":\"read_file\""), "{request}");
        assert!(!request.contains("\"function\":{"), "{request}");
    }

    #[tokio::test]
    async fn a_tool_result_is_its_own_input_item() {
        let (base, seen) = spawn("data: {\"type\":\"response.completed\"}\n\n");
        let client = ResponsesClient::new(base, "m", Some("t".into()));
        let prompt = Prompt {
            input: vec![ResponseItem::FunctionCallOutput {
                call_id: "fc_1".into(),
                output: "contents".into(),
            }],
            ..Prompt::default()
        };

        collect(&client, &prompt).await;

        let request = seen.lock().expect("lock").clone();
        assert!(request.contains("\"function_call_output\""), "{request}");
        assert!(request.contains("\"call_id\":\"fc_1\""), "{request}");
    }

    #[tokio::test]
    async fn a_refreshed_token_reaches_the_next_request() {
        let (base, seen) = spawn("data: {\"type\":\"response.completed\"}\n\n");
        let client = ResponsesClient::new(base, "m", Some("first".into()));

        client.credential().set(Some("second".into()));
        collect(&client, &user("hi")).await;

        assert!(seen.lock().expect("lock").contains("Bearer second"));
    }
}
