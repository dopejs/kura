//! One model round, as a [`ModelProvider`].
//!
//! The agent loop lives in `kura-core`: it streams a model, runs whatever
//! calls come back, and goes around until the model answers without asking for
//! more. What it drives is a `ModelProvider` -- a bare model client, with no
//! idea that this daemon persists dispatches, runs hooks, publishes events or
//! resolves providers.
//!
//! This is that `ModelProvider`, backed by the dispatcher. Every round the
//! loop takes is prepared, hooked, persisted and evented exactly like the
//! single dispatch a turn used to be, so using tools costs none of it.
//!
//! Written this way round on purpose. Chat briefly carried a loop of its own,
//! which meant the codebase had two: this one and the one `kura-core` already
//! had. Looping is the same problem in both places, so there is one loop; what
//! differs between an embedded agent and this daemon is what a *round* is, and
//! that is exactly what this file supplies.

use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
use kura_llm::{CreateDispatchInput, Dispatch, Message, MessageRole, ToolCall};
use kura_model_provider::{ModelProvider, Prompt, ProviderError, ResponseEvent};
use kura_protocol::{ResponseItem, Role};
use parking_lot::Mutex;

use crate::types::{QueryInput, Service};

/// Everything a round needs that a bare model client does not have.
pub(crate) struct RoundContext {
    pub service: Service,
    pub input: QueryInput,
    pub agent_profile_id: String,
    pub selected_skills: Vec<kura_skills::Skill>,
    /// Provider and model are resolved once for the turn. A round does not
    /// re-resolve, or a turn could change provider halfway through answering.
    pub provider: String,
    pub model: String,
    pub timeout_ms: i64,
    pub max_retries: i64,
    /// Linked to the caller's token for the whole turn. Every round shares it,
    /// so cancelling stops the turn wherever it has got to -- a token created
    /// per round would leave the caller unable to stop anything.
    pub cancel: kura_llm::CancelToken,
}

/// What the turn produced, collected as its rounds ran.
#[derive(Default)]
pub(crate) struct RoundLog {
    /// The last dispatch, which is the one that answered.
    pub last: Option<Dispatch>,
    /// The first, whose id identifies the turn in continuity: the question was
    /// asked once however many rounds answering it takes.
    pub first_id: String,
    /// Set when a round settled as failed. The loop stops on the next poll and
    /// the caller reports it.
    pub failure: Option<String>,
    pub rounds: usize,
}

/// Work that belongs to the first round of a turn only -- profile projection,
/// binding evidence, the continuity request -- run once the first dispatch
/// exists to attach it to.
pub(crate) type OnFirstRound =
    Box<dyn Fn(&Dispatch) -> Result<(), crate::error::ChatError> + Send + Sync>;

pub(crate) struct DispatcherProvider {
    context: RoundContext,
    log: Arc<Mutex<RoundLog>>,
    on_first_round: OnFirstRound,
}

impl DispatcherProvider {
    pub fn new(context: RoundContext, log: Arc<Mutex<RoundLog>>, on_first_round: OnFirstRound) -> Self {
        Self { context, log, on_first_round }
    }
}

/// The protocol's roles map one-to-one onto the dispatcher's.
fn to_message_role(role: Role) -> MessageRole {
    match role {
        Role::System => MessageRole::System,
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
    }
}

/// Flatten the loop's history into the messages a dispatch carries.
///
/// The two are the same conversation in different vocabularies: the loop
/// thinks in items, a dispatch is persisted as messages. A call stays attached
/// to the assistant turn that asked for it, because the round after a tool ran
/// has to show the model what it asked as well as what came back.
pub(crate) fn to_messages(prompt: &Prompt) -> Vec<Message> {
    let mut messages = Vec::with_capacity(prompt.input.len() + 1);
    if let Some(instructions) = &prompt.instructions {
        messages.push(Message {
            role: MessageRole::System,
            content: instructions.clone(),
            ..Default::default()
        });
    }
    for item in &prompt.input {
        match item {
            ResponseItem::Message { role, content } => messages.push(Message {
                role: to_message_role(*role),
                content: content.clone(),
                ..Default::default()
            }),
            ResponseItem::FunctionCall { call_id, name, arguments } => {
                let call = ToolCall {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                };
                match messages.last_mut() {
                    Some(last) if last.role == MessageRole::Assistant => {
                        last.tool_calls.push(call)
                    }
                    _ => messages.push(Message {
                        role: MessageRole::Assistant,
                        content: String::new(),
                        tool_calls: vec![call],
                        ..Default::default()
                    }),
                }
            }
            ResponseItem::FunctionCallOutput { call_id, output } => messages.push(Message {
                role: MessageRole::Tool,
                content: output.clone(),
                tool_call_id: call_id.clone(),
                ..Default::default()
            }),
        }
    }
    messages
}

impl ModelProvider for DispatcherProvider {
    fn stream<'a>(
        &'a self,
        prompt: &'a Prompt,
    ) -> BoxStream<'a, Result<ResponseEvent, ProviderError>> {
        futures::stream::once(async move { self.run_round(prompt).await })
            .flat_map(|round| match round {
                Ok(events) => futures::stream::iter(events).map(Ok).boxed(),
                Err(error) => futures::stream::once(async move { Err(error) }).boxed(),
            })
            .boxed()
    }
}

impl DispatcherProvider {
    async fn run_round(&self, prompt: &Prompt) -> Result<Vec<ResponseEvent>, ProviderError> {
        let context = &self.context;
        let mut dispatch_input = CreateDispatchInput {
            provider: context.provider.clone(),
            model: context.model.clone(),
            messages: to_messages(prompt),
            tools: prompt.tools.clone(),
            timeout_ms: context.timeout_ms,
            max_retries: context.max_retries,
        };

        // Per round, not per turn. The invariant this hook exists for is that
        // the persisted dispatch is byte-identical to what the provider
        // receives, and each round is its own dispatch; running it once a turn
        // would persist every later round unhooked.
        context
            .service
            .run_pre_dispatch_hooks(&context.input, &context.agent_profile_id, &mut dispatch_input)
            .map_err(round_error)?;

        let dispatch = context
            .service
            .dispatcher
            .prepare(dispatch_input, false)
            .map_err(|error| round_error(crate::error::ChatError::Prepare(error)))?;

        let first = {
            let mut log = self.log.lock();
            log.rounds += 1;
            let first = log.first_id.is_empty();
            if first {
                log.first_id = dispatch.dispatch_id.clone();
            }
            first
        };

        context.service.record_round_requested(context, &dispatch).map_err(round_error)?;
        if first {
            (self.on_first_round)(&dispatch).map_err(round_error)?;
        }

        let (settled, failure) = match context
            .service
            .dispatcher
            .dispatch(dispatch, &context.cancel)
            .await
        {
            Ok(settled) => (settled, None),
            Err(failed) => {
                let message = failed.error.to_string();
                (failed.dispatch, Some(message))
            }
        };
        context.service.record_round_settled(context, &settled).map_err(round_error)?;

        // The loop reads these: text it accumulates, calls it runs, and
        // `Completed` to end the round. A settled-but-failed round yields no
        // calls, so the loop stops and the caller reports the failure.
        let mut events = Vec::new();
        if !settled.output.is_empty() {
            events.push(ResponseEvent::OutputTextDelta(settled.output.clone()));
        }
        if failure.is_none() {
            for call in &settled.tool_calls {
                events.push(ResponseEvent::FunctionCall {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
            }
        }
        events.push(ResponseEvent::Completed);

        let mut log = self.log.lock();
        log.failure = failure;
        log.last = Some(settled);
        Ok(events)
    }
}

/// A round that could not run at all.
///
/// `ProviderError` is the only failure the loop understands, so a hook veto or
/// a rejected dispatch travels as one and is unwrapped by the caller. It is
/// not a transport failure and must not read as one, which is why the message
/// is carried verbatim rather than flattened.
fn round_error(error: crate::error::ChatError) -> ProviderError {
    ProviderError::Malformed(error.to_string())
}
