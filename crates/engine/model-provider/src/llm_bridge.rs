//! Makes an HTTP model provider usable by the dispatcher.
//!
//! `ModelProvider` streams protocol events; `kura_llm::Provider` is what the
//! dispatcher registers and speaks in messages and chunks. Without this bridge
//! the OpenAI-compatible client existed but was never registered, so the
//! provider appeared in the inventory, accepted configuration, and then could
//! not answer a single query.

use futures::StreamExt;
use futures::future::BoxFuture;
use kura_llm::{
    Message, MessageRole, Provider, ProviderError as LlmError, ProviderRequest, ProviderResponse,
    StreamChunk, StreamEmitter, ToolCall, Usage,
};
use kura_protocol::{ResponseItem, Role, ToolSpec};

use crate::openai::OpenAiCompatibleClient;
use crate::provider::{ModelProvider, Prompt, ProviderError, ResponseEvent};

/// Registers an OpenAI-compatible endpoint with the dispatcher.
/// Adapts any [`ModelProvider`] to the dispatcher's `Provider`.
///
/// Generic over the client because the adaptation -- accumulate the stream,
/// relay chunks, report the reason it ended -- is the same whatever wire
/// produced the events. Anthropic's Messages API and OpenAI's Responses API
/// differ in how a request is built and a stream is read, and in nothing
/// downstream of that.
pub struct ModelProviderBridge<M: ModelProvider> {
    name: String,
    client: M,
}

/// The original name, kept because it is what callers and tests already use.
pub type OpenAiCompatibleProvider = ModelProviderBridge<OpenAiCompatibleClient>;

impl<M: ModelProvider> ModelProviderBridge<M> {
    #[must_use]
    pub fn new(name: impl Into<String>, client: M) -> Self {
        Self { name: name.into(), client }
    }
}

/// The dispatcher's roles map one-to-one onto the protocol's.
fn to_protocol_role(role: MessageRole) -> Role {
    match role {
        MessageRole::System => Role::System,
        MessageRole::User => Role::User,
        MessageRole::Assistant => Role::Assistant,
        MessageRole::Tool => Role::Tool,
    }
}

/// Build a prompt from dispatch messages.
///
/// A leading system message becomes `instructions` because that is where the
/// OpenAI request builder puts it; sending it twice would duplicate the system
/// prompt in the request body.
fn to_prompt(messages: &[Message], tools: &[ToolSpec]) -> Prompt {
    let mut instructions = None;
    let mut input = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        if index == 0 && message.role == MessageRole::System {
            instructions = Some(message.content.clone());
            continue;
        }
        // A tool result answers a specific call, and an assistant turn may
        // have asked for several. Flattening either into plain text would hand
        // the model a result with nothing to attach it to, and it would ask
        // for the same call again.
        if !message.tool_call_id.is_empty() {
            input.push(ResponseItem::FunctionCallOutput {
                call_id: message.tool_call_id.clone(),
                output: message.content.clone(),
            });
            continue;
        }
        if !message.content.is_empty() || message.tool_calls.is_empty() {
            input.push(ResponseItem::Message {
                role: to_protocol_role(message.role),
                content: message.content.clone(),            });
        }
        for call in &message.tool_calls {
            input.push(ResponseItem::FunctionCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }
    }
    // Carried, not dropped. This was `Vec::new()`, so every provider reaching
    // the dispatcher was told about no tools whatever the caller offered --
    // the clients could serialize a tool list and never received one.
    Prompt { instructions, input, tools: tools.to_vec() }
}

/// Map a transport failure onto the dispatcher's error model.
///
/// Retryability is decided here rather than left to the caller: a 429 or 5xx is
/// worth another attempt, a 4xx is not, and getting that wrong either wastes a
/// quota or gives up on a transient blip.
fn to_llm_error(error: ProviderError) -> LlmError {
    match error {
        ProviderError::Status { status, body } => LlmError::Provider {
            code: format!("http_{status}"),
            message: body,
            retryable: status == 429 || (500..600).contains(&status),
        },
        ProviderError::Http(inner) => LlmError::Provider {
            code: "transport".to_string(),
            message: inner.to_string(),
            retryable: true,
        },
        ProviderError::Malformed(message) => LlmError::Provider {
            code: "malformed".to_string(),
            message,
            retryable: false,
        },
    }
}

impl<M: ModelProvider> Provider for ModelProviderBridge<M> {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
        // Completion is the streaming path with the chunks accumulated: the
        // endpoint is the same, and duplicating the request builder would let
        // the two drift.
        Box::pin(async move {
            let mut noop = |_: StreamChunk| Ok(());
            self.run(request, &mut noop).await
        })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
        Box::pin(async move { self.run(request, emit).await })
    }
}

impl<M: ModelProvider> ModelProviderBridge<M> {
    async fn run(
        &self,
        request: ProviderRequest,
        emit: StreamEmitter<'_>,
    ) -> Result<ProviderResponse, LlmError> {
        let prompt = to_prompt(&request.messages, &request.tools);
        let mut events = self.client.stream(&prompt);
        let mut output = String::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = String::new();

        while let Some(event) = events.next().await {
            // Cancellation is checked between chunks so a cancelled dispatch
            // stops consuming the response rather than running to completion.
            if request.cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            match event.map_err(to_llm_error)? {
                ResponseEvent::OutputTextDelta(delta) => {
                    output.push_str(&delta);
                    emit(StreamChunk {
                        delta,
                        output: String::new(),
                        finish_reason: String::new(),
                        usage: None,
                    })?;
                }
                ResponseEvent::FunctionCall { call_id, name, arguments } => {
                    // Reported, not swallowed. Discarding these is what made a
                    // tool-capable model produce prose about what it would do:
                    // it asked to call something, the answer never left this
                    // loop, and the caller saw an empty reply or a narration.
                    tool_calls.push(ToolCall { call_id, name, arguments });
                }
                ResponseEvent::Completed => {
                    finish_reason = "stop".to_string();
                }
            }
        }

        Ok(ProviderResponse {
            output,
            tool_calls,
            finish_reason,
            // The endpoint reports usage only in a trailing frame this client
            // does not surface; leaving it zero lets the dispatcher normalize
            // rather than inventing counts.
            usage: Usage::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_llm::CancelToken;

    fn request(messages: Vec<Message>) -> ProviderRequest {
        ProviderRequest {
            dispatch_id: "d1".into(),
            tools: Vec::new(),
            provider: "openai_compatible".into(),
            model: "m".into(),
            messages,
            attempt: 1,
            timeout_ms: 30_000,
            stream_first_chunk_timeout_ms: 30_000,
            stream_idle_timeout_ms: 30_000,
            stream_max_duration_ms: 60_000,
            cancel: CancelToken::new(),
        }
    }

    #[test]
    fn a_leading_system_message_becomes_instructions() {
        // The request builder already emits `instructions` as a system
        // message; keeping it in `input` too would send it twice.
        let prompt = to_prompt(&[
            Message { role: MessageRole::System, content: "be brief".into(), ..Default::default() },
            Message { role: MessageRole::User, content: "hi".into(), ..Default::default() },
        ], &[]);
        assert_eq!(prompt.instructions.as_deref(), Some("be brief"));
        assert_eq!(prompt.input.len(), 1);
    }

    #[test]
    fn a_later_system_message_stays_in_the_conversation() {
        let prompt = to_prompt(&[
            Message { role: MessageRole::User, content: "hi".into(), ..Default::default() },
            Message { role: MessageRole::System, content: "now be terse".into(), ..Default::default() },
        ], &[]);
        assert!(prompt.instructions.is_none());
        assert_eq!(prompt.input.len(), 2);
    }

    /// A provider that replays a fixed event sequence.
    struct ScriptedProvider(Vec<ResponseEvent>);

    impl ModelProvider for ScriptedProvider {
        fn stream<'a>(
            &'a self,
            _prompt: &'a Prompt,
        ) -> futures::stream::BoxStream<'a, Result<ResponseEvent, ProviderError>> {
            Box::pin(futures::stream::iter(self.0.clone().into_iter().map(Ok)))
        }
    }

    #[tokio::test]
    async fn a_tool_call_survives_the_adapter() {
        // These were matched and discarded, with a comment saying so. A model
        // that asked to call something produced a dispatch carrying an empty
        // reply, and a caller had no way to learn a call had been requested --
        // which is why a tool-capable model could only narrate.
        let bridge = ModelProviderBridge::new(
            "scripted",
            ScriptedProvider(vec![
                ResponseEvent::FunctionCall {
                    call_id: "call_1".into(),
                    name: "loopforge_status".into(),
                    arguments: "{}".into(),
                },
                ResponseEvent::Completed,
            ]),
        );

        let response = bridge.complete(request(vec![Message {
            role: MessageRole::User,
            content: "where am i".into(),
            ..Default::default()
        }]))
        .await
        .expect("dispatch");

        assert_eq!(
            response.tool_calls,
            vec![ToolCall {
                call_id: "call_1".into(),
                name: "loopforge_status".into(),
                arguments: "{}".into(),
            }]
        );
    }

    #[tokio::test]
    async fn text_and_calls_are_reported_together() {
        // A model may say something and ask for a call in the same round;
        // keeping only one of the two loses half the turn.
        let bridge = ModelProviderBridge::new(
            "scripted",
            ScriptedProvider(vec![
                ResponseEvent::OutputTextDelta("checking".into()),
                ResponseEvent::FunctionCall {
                    call_id: "call_1".into(),
                    name: "loopforge_status".into(),
                    arguments: "{}".into(),
                },
                ResponseEvent::Completed,
            ]),
        );

        let response = bridge.complete(request(vec![Message {
            role: MessageRole::User,
            content: "where am i".into(),
            ..Default::default()
        }]))
        .await
        .expect("dispatch");

        assert_eq!(response.output, "checking");
        assert_eq!(response.tool_calls.len(), 1);
    }

    #[tokio::test]
    async fn a_plain_answer_reports_no_calls() {
        let bridge = ModelProviderBridge::new(
            "scripted",
            ScriptedProvider(vec![
                ResponseEvent::OutputTextDelta("hello".into()),
                ResponseEvent::Completed,
            ]),
        );

        let response = bridge.complete(request(vec![Message {
            role: MessageRole::User,
            content: "hi".into(),
            ..Default::default()
        }]))
        .await
        .expect("dispatch");

        assert_eq!(response.output, "hello");
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn a_tool_round_replays_as_a_call_and_its_result() {
        // The round after a tool ran has to show the model what it asked for
        // and what came back. A `Message` could only carry text, so the call
        // vanished and the result arrived attached to nothing -- the model
        // would ask for the same call again.
        let prompt = to_prompt(
            &[
                Message { role: MessageRole::User, content: "where am i".into(), ..Default::default() },
                Message {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        call_id: "call_1".into(),
                        name: "loopforge_status".into(),
                        arguments: "{}".into(),
                    }],
                    ..Default::default()
                },
                Message {
                    role: MessageRole::Tool,
                    content: "{\"stage\":\"DISCOVERY\"}".into(),
                    tool_call_id: "call_1".into(),
                    ..Default::default()
                },
            ],
            &[],
        );

        assert_eq!(
            prompt.input,
            vec![
                ResponseItem::Message { role: Role::User, content: "where am i".into() },
                ResponseItem::FunctionCall {
                    call_id: "call_1".into(),
                    name: "loopforge_status".into(),
                    arguments: "{}".into(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".into(),
                    output: "{\"stage\":\"DISCOVERY\"}".into(),
                },
            ]
        );
    }

    #[test]
    fn an_assistant_that_spoke_and_called_keeps_both() {
        // A model may answer and ask for a call in the same turn.
        let prompt = to_prompt(
            &[Message {
                role: MessageRole::Assistant,
                content: "checking".into(),
                tool_calls: vec![ToolCall {
                    call_id: "call_1".into(),
                    name: "loopforge_status".into(),
                    arguments: "{}".into(),
                }],
                ..Default::default()
            }],
            &[],
        );

        assert_eq!(prompt.input.len(), 2);
        assert!(matches!(prompt.input[0], ResponseItem::Message { .. }));
        assert!(matches!(prompt.input[1], ResponseItem::FunctionCall { .. }));
    }

    #[test]
    fn an_ordinary_conversation_is_unchanged() {
        // The common path must not acquire empty items.
        let prompt = to_prompt(
            &[
                Message { role: MessageRole::User, content: "hi".into(), ..Default::default() },
                Message { role: MessageRole::Assistant, content: "hello".into(), ..Default::default() },
            ],
            &[],
        );
        assert_eq!(prompt.input.len(), 2);
    }

    #[test]
    fn the_tools_a_caller_offers_reach_the_provider() {
        // This built the prompt with `tools: Vec::new()` regardless. Every
        // provider behind the dispatcher was therefore told about no tools
        // whatever the caller offered -- the wire clients could serialize a
        // tool list and never received one to serialize.
        let tools = vec![ToolSpec {
            name: "loopforge_status".into(),
            description: "read project state".into(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let prompt = to_prompt(
            &[Message { role: MessageRole::User, content: "where am i".into(), ..Default::default() }],
            &tools,
        );
        assert_eq!(prompt.tools, tools);
    }

    #[test]
    fn a_request_offering_nothing_still_offers_nothing() {
        // Plain chat is the ordinary case and must not acquire a tool list.
        let prompt = to_prompt(
            &[Message { role: MessageRole::User, content: "hi".into(), ..Default::default() }],
            &[],
        );
        assert!(prompt.tools.is_empty());
    }

    #[test]
    fn transient_failures_are_retryable_and_client_errors_are_not() {
        // Retrying a 401 wastes an attempt and never succeeds; not retrying a
        // 429 gives up on a transient limit.
        let retryable = |status: u16| match to_llm_error(ProviderError::Status {
            status,
            body: String::new(),
        }) {
            LlmError::Provider { retryable, .. } => retryable,
            other => panic!("unexpected error: {other:?}"),
        };
        assert!(retryable(429));
        assert!(retryable(503));
        assert!(!retryable(401));
        assert!(!retryable(400));

        match to_llm_error(ProviderError::Malformed("bad".into())) {
            LlmError::Provider { retryable, .. } => assert!(!retryable),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn a_status_failure_keeps_the_response_body() {
        // The body is where a provider explains the refusal; dropping it
        // leaves the operator with only a number.
        match to_llm_error(ProviderError::Status {
            status: 400,
            body: "model not found".into(),
        }) {
            LlmError::Provider { message, code, .. } => {
                assert_eq!(message, "model not found");
                assert_eq!(code, "http_400");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    /// A minimal OpenAI-compatible SSE endpoint, so the bridge is exercised
    /// over real HTTP rather than a stubbed client. Serves one connection.
    fn spawn_endpoint(body: &'static str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_streamed_reply_is_relayed_and_accumulated() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n\n";
        let provider = OpenAiCompatibleProvider::new(
            "openai_compatible",
            OpenAiCompatibleClient::new(spawn_endpoint(sse), "m", None),
        );

        let mut chunks: Vec<String> = Vec::new();
        let mut emit = |chunk: StreamChunk| {
            chunks.push(chunk.delta);
            Ok(())
        };
        let response = provider
            .run(
                request(vec![Message { role: MessageRole::User, content: "hi".into(), ..Default::default() }]),
                &mut emit,
            )
            .await
            .expect("dispatch succeeds");

        // Both halves matter: the caller saw incremental chunks, and the
        // dispatcher received the whole reply.
        assert_eq!(chunks, vec!["Hel".to_string(), "lo".to_string()]);
        assert_eq!(response.output, "Hello");
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_surfaces_as_a_provider_error() {
        let provider = OpenAiCompatibleProvider::new(
            "openai_compatible",
            // Port 9 is discard: nothing will answer.
            OpenAiCompatibleClient::new("http://127.0.0.1:9/v1", "m", None),
        );
        let mut sink = |_: StreamChunk| Ok(());
        let result = provider
            .run(
                request(vec![Message { role: MessageRole::User, content: "hi".into(), ..Default::default() }]),
                &mut sink,
            )
            .await;
        assert!(matches!(result, Err(LlmError::Provider { .. })));
    }
}
