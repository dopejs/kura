use std::sync::Arc;

use kura_model_provider::ModelProvider;
use kura_model_provider::Prompt;
use kura_model_provider::ProviderError;
use kura_model_provider::ResponseEvent;
use kura_protocol::Event;
use kura_protocol::EventMsg;
use kura_protocol::ResponseItem;
use kura_protocol::Role;
use kura_protocol::ThreadId;
use futures::StreamExt;

use crate::tools::ToolInvocation;
use crate::tools::ToolOutput;
use crate::tools::ToolRegistry;

/// Hard cap on model→tool→model rounds inside a single user turn. Prevents
/// a misbehaving model or tool loop from burning unbounded provider quota.
const DEFAULT_MAX_TOOL_ROUNDS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("exceeded {0} tool rounds in one turn")]
    MaxToolRounds(usize),
}

#[derive(Debug)]
pub struct TurnOutcome {
    /// Final assistant text for the turn, if the model produced any.
    pub final_message: Option<String>,
}

pub struct Session {
    thread_id: ThreadId,
    instructions: Option<String>,
    history: Vec<ResponseItem>,
    provider: Arc<dyn ModelProvider>,
    tools: Arc<ToolRegistry>,
    max_tool_rounds: usize,
    event_seq: u64,
}

impl Session {
    pub fn new(provider: Arc<dyn ModelProvider>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            thread_id: ThreadId::new(),
            instructions: None,
            history: Vec::new(),
            provider,
            tools,
            max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
            event_seq: 0,
        }
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn with_max_tool_rounds(mut self, max_tool_rounds: usize) -> Self {
        self.max_tool_rounds = max_tool_rounds;
        self
    }

    /// Seed the conversation this turn continues.
    ///
    /// A caller that assembles its own history -- skills, continuity, an
    /// operator overlay -- hands it over here rather than replaying it one
    /// `run_turn` at a time, which would dispatch once per prior message.
    #[must_use]
    pub fn with_history(mut self, history: Vec<ResponseItem>) -> Self {
        self.history = history;
        self
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub fn history(&self) -> &[ResponseItem] {
        &self.history
    }

    /// Run one user turn to completion: append the input, then loop
    /// model-stream → tool-dispatch until the model answers without new
    /// tool calls. Emits an ordered event stream through `emit`.
    pub async fn run_turn(
        &mut self,
        input: &str,
        emit: &mut dyn FnMut(Event),
    ) -> Result<TurnOutcome, CoreError> {
        self.history.push(ResponseItem::Message {
            role: Role::User,
            content: input.to_string(),        });
        self.publish(emit, EventMsg::TurnStarted);

        let mut rounds = 0;
        loop {
            // `>=`, checked before the round it would allow. With `>` the cap
            // permitted one round more than it named -- a cap of three ran
            // four. Never exercised until the loop became load-bearing.
            if rounds >= self.max_tool_rounds {
                let message = CoreError::MaxToolRounds(self.max_tool_rounds).to_string();
                self.publish(
                    emit,
                    EventMsg::Error {
                        message: message.clone(),
                    },
                );
                break Err(CoreError::MaxToolRounds(self.max_tool_rounds));
            }
            rounds += 1;

            let prompt = Prompt {
                instructions: self.instructions.clone(),
                input: self.history.clone(),
                tools: self.tools.specs(),
            };
            // Clone the Arc so the stream borrows a local, not `self`.
            let provider = self.provider.clone();
            let mut stream = provider.stream(&prompt);

            let mut text = String::new();
            let mut pending_calls = Vec::new();
            while let Some(event) = stream.next().await {
                match event? {
                    ResponseEvent::OutputTextDelta(delta) => {
                        text.push_str(&delta);
                        self.publish(emit, EventMsg::AgentMessageDelta { delta });
                    }
                    ResponseEvent::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    } => pending_calls.push((call_id, name, arguments)),
                    ResponseEvent::Completed => {}
                }
            }

            if pending_calls.is_empty() {
                if !text.is_empty() {
                    self.history.push(ResponseItem::Message {
                        role: Role::Assistant,
                        content: text.clone(),                    });
                    self.publish(
                        emit,
                        EventMsg::AgentMessage {
                            message: text.clone(),
                        },
                    );
                }
                self.publish(emit, EventMsg::TurnComplete);
                break Ok(TurnOutcome {
                    final_message: (!text.is_empty()).then_some(text),
                });
            }

            for (call_id, name, arguments) in pending_calls {
                self.publish(
                    emit,
                    EventMsg::ToolCallBegin {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    },
                );
                let invocation = ToolInvocation {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                };
                // Tool failures are reported to the model as output so it can
                // recover; they do not abort the turn.
                let output = match self.tools.invoke(&invocation).await {
                    Ok(output) => output,
                    Err(err) => ToolOutput::failed(err.to_string()),
                };
                self.publish(
                    emit,
                    EventMsg::ToolCallEnd {
                        call_id: call_id.clone(),
                        output: output.content.clone(),
                        success: output.success,
                    },
                );
                self.history.push(ResponseItem::FunctionCall {
                    call_id: call_id.clone(),
                    name,
                    arguments,
                });
                self.history.push(ResponseItem::FunctionCallOutput {
                    call_id,
                    output: output.content,
                });
            }
        }
    }

    fn publish(&mut self, emit: &mut dyn FnMut(Event), msg: EventMsg) {
        self.event_seq += 1;
        emit(Event {
            id: format!("{}-{}", self.thread_id, self.event_seq),
            msg,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_model_provider::ToolSpec;
    use futures::stream;
    use futures::stream::BoxStream;
    use parking_lot::Mutex;
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;

    /// Scripted provider: each `stream` call pops the next scripted round.
    struct FakeProvider {
        rounds: Mutex<VecDeque<Vec<ResponseEvent>>>,
        prompts: Mutex<Vec<Prompt>>,
    }

    impl FakeProvider {
        fn with_rounds(rounds: Vec<Vec<ResponseEvent>>) -> Self {
            Self {
                rounds: Mutex::new(rounds.into()),
                prompts: Mutex::new(Vec::new()),
            }
        }
    }

    impl ModelProvider for FakeProvider {
        fn stream<'a>(
            &'a self,
            prompt: &'a Prompt,
        ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
            self.prompts.lock().push(prompt.clone());
            let events = self.rounds.lock().pop_front().unwrap_or_default();
            Box::pin(stream::iter(events.into_iter().map(Ok)))
        }
    }

    struct EchoTool;

    impl crate::tools::Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "echo back the arguments".into(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }

        fn call<'a>(
            &'a self,
            invocation: &'a ToolInvocation,
        ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, crate::tools::ToolError>> + Send + 'a>>
        {
            Box::pin(async move { Ok(ToolOutput::ok(invocation.arguments.clone())) })
        }
    }

    fn collect_events() -> (impl FnMut(Event), Arc<Mutex<Vec<EventMsg>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let emit = move |event: Event| sink.lock().push(event.msg);
        (emit, seen)
    }

    #[tokio::test]
    async fn text_only_turn_completes_with_history() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![vec![
            ResponseEvent::OutputTextDelta("hello".into()),
            ResponseEvent::OutputTextDelta(" world".into()),
            ResponseEvent::Completed,
        ]]));
        let mut session = Session::new(provider, Arc::new(ToolRegistry::new()));
        let (mut emit, seen) = collect_events();

        let outcome = session.run_turn("hi", &mut emit).await.unwrap();

        assert_eq!(outcome.final_message.as_deref(), Some("hello world"));
        let msgs = seen.lock().clone();
        assert_eq!(
            msgs,
            vec![
                EventMsg::TurnStarted,
                EventMsg::AgentMessageDelta {
                    delta: "hello".into()
                },
                EventMsg::AgentMessageDelta {
                    delta: " world".into()
                },
                EventMsg::AgentMessage {
                    message: "hello world".into()
                },
                EventMsg::TurnComplete,
            ]
        );
        assert_eq!(session.history().len(), 2);
    }

    #[tokio::test]
    async fn tool_round_trip_feeds_output_back_to_model() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![
                ResponseEvent::FunctionCall {
                    call_id: "call_1".into(),
                    name: "echo".into(),
                    arguments: "{\"text\":\"ping\"}".into(),
                },
                ResponseEvent::Completed,
            ],
            vec![
                ResponseEvent::OutputTextDelta("done".into()),
                ResponseEvent::Completed,
            ],
        ]));
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let mut session = Session::new(provider.clone(), Arc::new(registry));
        let (mut emit, seen) = collect_events();

        let outcome = session.run_turn("use the tool", &mut emit).await.unwrap();

        assert_eq!(outcome.final_message.as_deref(), Some("done"));
        let msgs = seen.lock().clone();
        assert!(msgs.contains(&EventMsg::ToolCallBegin {
            call_id: "call_1".into(),
            name: "echo".into(),
            arguments: "{\"text\":\"ping\"}".into(),
        }));
        assert!(msgs.contains(&EventMsg::ToolCallEnd {
            call_id: "call_1".into(),
            output: "{\"text\":\"ping\"}".into(),
            success: true,
        }));
        // History: user, function call, call output, assistant.
        assert_eq!(session.history().len(), 4);
        // Second model round must have received the tool output.
        let prompts = provider.prompts.lock();
        assert_eq!(prompts.len(), 2);
        assert!(prompts[1].input.iter().any(|item| matches!(
            item,
            ResponseItem::FunctionCallOutput { call_id, .. } if call_id == "call_1"
        )));
    }

    #[tokio::test]
    async fn unknown_tool_is_reported_to_model_not_fatal() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![
                ResponseEvent::FunctionCall {
                    call_id: "call_1".into(),
                    name: "missing".into(),
                    arguments: "{}".into(),
                },
                ResponseEvent::Completed,
            ],
            vec![
                ResponseEvent::OutputTextDelta("recovered".into()),
                ResponseEvent::Completed,
            ],
        ]));
        let mut session = Session::new(provider, Arc::new(ToolRegistry::new()));
        let (mut emit, seen) = collect_events();

        let outcome = session.run_turn("hi", &mut emit).await.unwrap();

        assert_eq!(outcome.final_message.as_deref(), Some("recovered"));
        let msgs = seen.lock().clone();
        assert!(msgs.iter().any(|msg| matches!(
            msg,
            EventMsg::ToolCallEnd { success: false, output, .. }
                if output.contains("tool not found")
        )));
    }

    #[tokio::test]
    async fn endless_tool_calls_hit_round_cap() {
        let call_round = vec![
            ResponseEvent::FunctionCall {
                call_id: "call_1".into(),
                name: "echo".into(),
                arguments: "{}".into(),
            },
            ResponseEvent::Completed,
        ];
        let provider = Arc::new(FakeProvider::with_rounds(vec![call_round; 4]));
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let mut session = Session::new(provider, Arc::new(registry)).with_max_tool_rounds(2);
        let (mut emit, seen) = collect_events();

        let err = session.run_turn("hi", &mut emit).await.unwrap_err();

        assert!(matches!(err, CoreError::MaxToolRounds(2)));
        let msgs = seen.lock().clone();
        assert!(msgs.iter().any(|msg| matches!(msg, EventMsg::Error { .. })));
        // The cap has to bound what it names. Asserting only that the error
        // fired let it run one round more than allowed for as long as this
        // loop was unused: a cap of two dispatched three times.
        assert_eq!(provider_rounds(&session), 2);
    }

    /// How many rounds the provider was actually asked for.
    fn provider_rounds(session: &Session) -> usize {
        session.history().iter().filter(|item| {
            matches!(item, ResponseItem::FunctionCall { .. })
        }).count()
    }

    #[tokio::test]
    async fn provider_error_aborts_turn() {
        struct FailingProvider;

        impl ModelProvider for FailingProvider {
            fn stream<'a>(
                &'a self,
                _prompt: &'a Prompt,
            ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
                Box::pin(stream::once(async {
                    Err(ProviderError::Malformed("boom".into()))
                }))
            }
        }

        let mut session = Session::new(Arc::new(FailingProvider), Arc::new(ToolRegistry::new()));
        let (mut emit, _seen) = collect_events();

        let err = session.run_turn("hi", &mut emit).await.unwrap_err();

        assert!(matches!(
            err,
            CoreError::Provider(ProviderError::Malformed(_))
        ));
    }
}
