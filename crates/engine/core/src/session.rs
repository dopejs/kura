use std::sync::Arc;

use futures::StreamExt;
use kura_model_provider::ModelProvider;
use kura_model_provider::Prompt;
use kura_model_provider::ProviderError;
use kura_model_provider::ResponseEvent;
use kura_protocol::Event;
use kura_protocol::EventMsg;
use kura_protocol::ResponseItem;
use kura_protocol::Role;
use kura_protocol::ThreadId;

use crate::answer::AnswerCheck;
use crate::checkpoint::{
    CallState, CheckpointCall, CheckpointRound, INTERRUPTED_CALL_OUTPUT, ResumeStats,
    TurnCheckpoint, TurnJournal,
};
use crate::steer::TurnInbox;
use crate::tools::ToolInvocation;
use crate::tools::ToolOutput;
use crate::tools::ToolRegistry;

/// Hard cap on model→tool→model rounds inside a single user turn. Prevents
/// a misbehaving model or tool loop from burning unbounded provider quota.
const DEFAULT_MAX_TOOL_ROUNDS: usize = 16;

/// Calls of one round that may run at once (agent-runtime-program.md 2.6).
/// Bounded because each is a server process, a sandbox run or a provider
/// request somewhere; 1 restores strictly serial execution.
pub const DEFAULT_MAX_PARALLEL_TOOL_CALLS: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("exceeded {0} tool rounds in one turn")]
    MaxToolRounds(usize),
    /// The final answer failed the turn's [`AnswerCheck`], again after
    /// being asked to correct it.
    #[error("the answer was rejected: {0}")]
    AnswerRejected(String),
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
    max_parallel_tool_calls: usize,
    event_seq: u64,
    /// Where this turn stands; reported to `journal` on every change.
    checkpoint: TurnCheckpoint,
    journal: Option<Arc<dyn TurnJournal>>,
    resumed: ResumeStats,
    answer_check: Option<Arc<dyn AnswerCheck>>,
    inbox: Option<Arc<dyn TurnInbox>>,
    /// Messages taken from `inbox` this turn.
    steered: usize,
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
            max_parallel_tool_calls: DEFAULT_MAX_PARALLEL_TOOL_CALLS,
            event_seq: 0,
            checkpoint: TurnCheckpoint::default(),
            journal: None,
            resumed: ResumeStats::default(),
            answer_check: None,
            inbox: None,
            steered: 0,
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

    /// How many parallel-safe calls of one round may run at once; 0 and 1
    /// both mean serial.
    pub fn with_max_parallel_tool_calls(mut self, max: usize) -> Self {
        self.max_parallel_tool_calls = max.max(1);
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

    /// Report where the turn stands as it goes, so it can be resumed
    /// ([`Session::resume_turn`]) if the process running it dies.
    #[must_use]
    pub fn with_journal(mut self, journal: Arc<dyn TurnJournal>) -> Self {
        self.journal = Some(journal);
        self
    }

    /// Judge the final answer; one rejected answer is sent back with the
    /// check's feedback and answered again, as a round of the same turn.
    #[must_use]
    pub fn with_answer_check(mut self, check: Arc<dyn AnswerCheck>) -> Self {
        self.answer_check = Some(check);
        self
    }

    /// Read messages sent to the turn while it runs (see [`TurnInbox`]).
    #[must_use]
    pub fn with_inbox(mut self, inbox: Arc<dyn TurnInbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    /// Messages the last turn took from its inbox.
    #[must_use]
    pub fn steered(&self) -> usize {
        self.steered
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub fn history(&self) -> &[ResponseItem] {
        &self.history
    }

    /// What the last [`Session::resume_turn`] carried over; zero for a turn
    /// that started fresh.
    pub fn resumed(&self) -> ResumeStats {
        self.resumed
    }

    /// Run one user turn to completion: append the input, then loop
    /// model-stream → tool-dispatch until the model answers without new
    /// tool calls. Emits an ordered event stream through `emit`.
    pub async fn run_turn(
        &mut self,
        input: &str,
        emit: &mut dyn FnMut(Event),
    ) -> Result<TurnOutcome, CoreError> {
        self.resume_turn(input, TurnCheckpoint::default(), emit)
            .await
    }

    /// [`Session::run_turn`], continuing from where an earlier attempt at
    /// the same turn stood. Rounds in `checkpoint` count against the round
    /// cap. Settled calls are replayed, not run; calls that never started
    /// run; a call that started and never settled runs again only if it is
    /// parallel-safe -- changes nothing -- and is otherwise reported to the
    /// model as interrupted. Replayed and interrupted calls emit no events:
    /// they did not run in this attempt.
    ///
    /// With an inbox, messages sent while the turn runs join the
    /// conversation before the next model call, and one that arrives as the
    /// model answers makes that answer interim: the turn continues, and the
    /// extra rounds count against the cap. However the turn ends, the inbox
    /// is closed.
    pub async fn resume_turn(
        &mut self,
        input: &str,
        checkpoint: TurnCheckpoint,
        emit: &mut dyn FnMut(Event),
    ) -> Result<TurnOutcome, CoreError> {
        self.steered = 0;
        let outcome = self.turn(input, checkpoint, emit).await;
        if let Some(inbox) = &self.inbox {
            inbox.close();
        }
        outcome
    }

    async fn turn(
        &mut self,
        input: &str,
        checkpoint: TurnCheckpoint,
        emit: &mut dyn FnMut(Event),
    ) -> Result<TurnOutcome, CoreError> {
        self.history.push(ResponseItem::Message {
            role: Role::User,
            content: input.to_string(),
        });
        self.publish(emit, EventMsg::TurnStarted);

        self.checkpoint = TurnCheckpoint::default();
        self.resumed = ResumeStats::default();
        let mut rounds = 0;
        let mut repaired = false;
        for round in checkpoint.rounds {
            rounds += 1;
            self.resumed.rounds += 1;
            self.run_calls(round, emit).await;
        }
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

            if let Some(inbox) = self.inbox.clone() {
                self.steer(inbox.take(), emit);
            }
            let prompt = Prompt {
                instructions: self.instructions.clone(),
                input: self.history.clone(),
                tools: self.tools.specs(),
                // A turn's format is enforced by its answer check; a provider
                // that sends it natively is given it by its caller.
                response_format: None,
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
                    // Counted by the dispatcher, which owns the dispatch
                    // record; the loop has nothing to do with it.
                    ResponseEvent::Usage(_) | ResponseEvent::Completed => {}
                }
            }

            if pending_calls.is_empty() {
                if let Some(check) = self.answer_check.clone() {
                    if let Err(rejection) = check.check(&text) {
                        if repaired {
                            self.publish(
                                emit,
                                EventMsg::Error {
                                    message: rejection.reason.clone(),
                                },
                            );
                            break Err(CoreError::AnswerRejected(rejection.reason));
                        }
                        // Once. The model sees what it said and why it was
                        // not accepted; the retry is a round like any other
                        // and counts against the cap.
                        repaired = true;
                        if !text.is_empty() {
                            self.history.push(ResponseItem::Message {
                                role: Role::Assistant,
                                content: text,
                            });
                        }
                        self.history.push(ResponseItem::Message {
                            role: Role::User,
                            content: rejection.feedback,
                        });
                        continue;
                    }
                }
                if !text.is_empty() {
                    self.history.push(ResponseItem::Message {
                        role: Role::Assistant,
                        content: text.clone(),
                    });
                    self.publish(
                        emit,
                        EventMsg::AgentMessage {
                            message: text.clone(),
                        },
                    );
                }
                // Something arrived while the answer was being written: it
                // was an answer to a question that has since changed. The
                // model reads both and goes on.
                if let Some(inbox) = self.inbox.clone() {
                    let waiting = inbox.take_or_close();
                    if !waiting.is_empty() {
                        self.steer(waiting, emit);
                        continue;
                    }
                }
                self.publish(emit, EventMsg::TurnComplete);
                break Ok(TurnOutcome {
                    final_message: (!text.is_empty()).then_some(text),
                });
            }

            let round = CheckpointRound {
                calls: pending_calls
                    .into_iter()
                    .map(|(call_id, name, arguments)| CheckpointCall {
                        call_id,
                        name,
                        arguments,
                        state: CallState::Planned,
                    })
                    .collect(),
            };
            self.run_calls(round, emit).await;
        }
    }

    /// Appends messages sent to the running turn, as the user's.
    fn steer(&mut self, messages: Vec<String>, emit: &mut dyn FnMut(Event)) {
        for message in messages {
            self.steered += 1;
            self.history.push(ResponseItem::Message {
                role: Role::User,
                content: message.clone(),
            });
            self.publish(emit, EventMsg::SteerMessage { message });
        }
    }

    /// Runs one round's calls. Consecutive parallel-safe calls form a batch
    /// that runs concurrently (bounded); any other call is a barrier that
    /// runs alone, after everything before it and before everything after
    /// it, so calls with side effects keep the order the model asked for.
    /// Whatever ran concurrently, results enter the history in call order:
    /// the model reads them back against the calls it made.
    ///
    /// A round from a checkpoint arrives partly settled; only what has not
    /// settled runs (see [`Session::resume_turn`]). Every state change is
    /// journaled before the work it describes: a call is recorded as
    /// started before it starts, so a call that is not recorded as started
    /// certainly did not run.
    async fn run_calls(&mut self, mut round: CheckpointRound, emit: &mut dyn FnMut(Event)) {
        let invocations: Vec<ToolInvocation> = round
            .calls
            .iter()
            .map(|call| ToolInvocation {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect();
        // Decided for every call before any of them starts.
        let safe: Vec<bool> = invocations
            .iter()
            .map(|invocation| self.tools.parallel_safe(invocation))
            .collect();
        for (call, safe) in round.calls.iter_mut().zip(&safe) {
            match call.state {
                CallState::Settled { .. } => self.resumed.replayed_calls += 1,
                CallState::Started if *safe => {
                    self.resumed.rerun_calls += 1;
                    call.state = CallState::Planned;
                }
                CallState::Started => {
                    self.resumed.interrupted_calls += 1;
                    call.state = CallState::Settled {
                        output: INTERRUPTED_CALL_OUTPUT.to_string(),
                        success: false,
                    };
                }
                CallState::Planned => {}
            }
        }
        let parallel: Vec<bool> = safe
            .iter()
            .map(|safe| self.max_parallel_tool_calls > 1 && *safe)
            .collect();
        let pending = |round: &CheckpointRound, index: usize| {
            matches!(round.calls[index].state, CallState::Planned)
        };
        self.checkpoint.rounds.push(round);
        let at = self.checkpoint.rounds.len() - 1;
        self.journal();

        let mut start = 0;
        while start < invocations.len() {
            if !pending(&self.checkpoint.rounds[at], start) {
                start += 1;
                continue;
            }
            let end = if parallel[start] {
                start
                    + (start..invocations.len())
                        .take_while(|index| {
                            parallel[*index] && pending(&self.checkpoint.rounds[at], *index)
                        })
                        .count()
            } else {
                start + 1
            };
            for index in start..end {
                self.checkpoint.rounds[at].calls[index].state = CallState::Started;
            }
            self.journal();
            for invocation in &invocations[start..end] {
                self.publish(
                    emit,
                    EventMsg::ToolCallBegin {
                        call_id: invocation.call_id.clone(),
                        name: invocation.name.clone(),
                        arguments: invocation.arguments.clone(),
                    },
                );
            }
            let tools = self.tools.clone();
            let mut running = futures::stream::iter(start..end)
                .map(|index| {
                    let tools = tools.clone();
                    let invocation = &invocations[index];
                    async move { (index, tools.invoke(invocation).await) }
                })
                .buffer_unordered(self.max_parallel_tool_calls);
            // Each call's end is announced when it finishes, not when its
            // batch does, so its span measures the call.
            while let Some((index, result)) = running.next().await {
                // Tool failures are reported to the model as output so it
                // can recover; they do not abort the turn.
                let output = result.unwrap_or_else(|err| ToolOutput::failed(err.to_string()));
                self.publish(
                    emit,
                    EventMsg::ToolCallEnd {
                        call_id: invocations[index].call_id.clone(),
                        output: output.content.clone(),
                        success: output.success,
                    },
                );
                self.checkpoint.rounds[at].calls[index].state = CallState::Settled {
                    output: output.content,
                    success: output.success,
                };
                self.journal();
            }
            drop(running);
            start = end;
        }

        for call in &self.checkpoint.rounds[at].calls {
            let output = match &call.state {
                CallState::Settled { output, .. } => output.clone(),
                // Unreachable: every pending call ran above.
                CallState::Planned | CallState::Started => "tool call did not run".to_string(),
            };
            self.history.push(ResponseItem::FunctionCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            self.history.push(ResponseItem::FunctionCallOutput {
                call_id: call.call_id.clone(),
                output,
            });
        }
    }

    fn journal(&self) {
        if let Some(journal) = &self.journal {
            journal.record(&self.checkpoint);
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
    use futures::stream;
    use futures::stream::BoxStream;
    use kura_model_provider::ToolSpec;
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
        session
            .history()
            .iter()
            .filter(|item| matches!(item, ResponseItem::FunctionCall { .. }))
            .count()
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

    /// A tool that sleeps `arguments` milliseconds and logs when it starts
    /// and finishes, tracking how many of its calls overlap.
    struct TimedTool {
        name: &'static str,
        parallel: bool,
        log: Arc<Mutex<Vec<String>>>,
        running: Arc<std::sync::atomic::AtomicUsize>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::tools::Tool for TimedTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.into(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }

        fn call<'a>(
            &'a self,
            invocation: &'a ToolInvocation,
        ) -> Pin<Box<dyn Future<Output = Result<ToolOutput, crate::tools::ToolError>> + Send + 'a>>
        {
            use std::sync::atomic::Ordering;
            Box::pin(async move {
                self.log
                    .lock()
                    .push(format!("start {}", invocation.call_id));
                let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                let ms: u64 = invocation.arguments.parse().unwrap_or(0);
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                self.running.fetch_sub(1, Ordering::SeqCst);
                self.log.lock().push(format!("end {}", invocation.call_id));
                Ok(ToolOutput::ok(format!("out {}", invocation.call_id)))
            })
        }

        fn parallel_safe(&self, _invocation: &ToolInvocation) -> bool {
            self.parallel
        }
    }

    struct Timed {
        log: Arc<Mutex<Vec<String>>>,
        peak: Arc<std::sync::atomic::AtomicUsize>,
        registry: Arc<ToolRegistry>,
    }

    fn timed_registry() -> Timed {
        let log = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        for (name, parallel) in [("read", true), ("write", false)] {
            registry.register(Arc::new(TimedTool {
                name,
                parallel,
                log: log.clone(),
                running: running.clone(),
                peak: peak.clone(),
            }));
        }
        Timed {
            log,
            peak,
            registry: Arc::new(registry),
        }
    }

    fn call(call_id: &str, name: &str, ms: u64) -> ResponseEvent {
        ResponseEvent::FunctionCall {
            call_id: call_id.into(),
            name: name.into(),
            arguments: ms.to_string(),
        }
    }

    fn outputs_in_history(session: &Session) -> Vec<String> {
        session
            .history()
            .iter()
            .filter_map(|item| match item {
                ResponseItem::FunctionCallOutput { output, .. } => Some(output.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn parallel_safe_calls_overlap_and_results_keep_call_order() {
        let timed = timed_registry();
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![
                call("a", "read", 150),
                call("b", "read", 10),
                call("c", "read", 80),
            ],
            vec![ResponseEvent::OutputTextDelta("done".into())],
        ]));
        let mut session = Session::new(provider, timed.registry.clone());
        let (mut emit, seen) = collect_events();
        session.run_turn("go", &mut emit).await.unwrap();

        assert_eq!(timed.peak.load(std::sync::atomic::Ordering::SeqCst), 3);
        // b finished first, but the model reads results in the order it asked.
        assert_eq!(outputs_in_history(&session), ["out a", "out b", "out c"]);
        // Each end is announced when that call finishes.
        let ends: Vec<String> = seen
            .lock()
            .iter()
            .filter_map(|msg| match msg {
                EventMsg::ToolCallEnd { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ends, ["b", "c", "a"]);
    }

    #[tokio::test]
    async fn a_call_with_side_effects_is_a_barrier_that_runs_alone_and_in_order() {
        let timed = timed_registry();
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![
                call("r1", "read", 30),
                call("r2", "read", 5),
                call("w", "write", 5),
                call("r3", "read", 5),
                call("r4", "read", 5),
            ],
            vec![ResponseEvent::OutputTextDelta("done".into())],
        ]));
        let mut session = Session::new(provider, timed.registry.clone());
        let (mut emit, _) = collect_events();
        session.run_turn("go", &mut emit).await.unwrap();

        let log = timed.log.lock().clone();
        let at = |entry: &str| log.iter().position(|e| e == entry).expect(entry);
        assert!(
            at("start w") > at("end r1") && at("start w") > at("end r2"),
            "{log:?}"
        );
        assert!(
            at("start r3") > at("end w") && at("start r4") > at("end w"),
            "{log:?}"
        );
        assert!(
            at("start r2") < at("end r1"),
            "reads before the write overlap: {log:?}"
        );
        assert_eq!(
            outputs_in_history(&session),
            ["out r1", "out r2", "out w", "out r3", "out r4"]
        );
    }

    #[tokio::test]
    async fn the_batch_is_bounded_and_one_means_serial() {
        for (max, peak) in [(2, 2), (1, 1)] {
            let timed = timed_registry();
            let provider = Arc::new(FakeProvider::with_rounds(vec![
                (0..5).map(|i| call(&format!("c{i}"), "read", 10)).collect(),
                vec![ResponseEvent::OutputTextDelta("done".into())],
            ]));
            let mut session =
                Session::new(provider, timed.registry.clone()).with_max_parallel_tool_calls(max);
            let (mut emit, _) = collect_events();
            session.run_turn("go", &mut emit).await.unwrap();
            assert_eq!(
                timed.peak.load(std::sync::atomic::Ordering::SeqCst),
                peak,
                "max {max}"
            );
            assert_eq!(outputs_in_history(&session).len(), 5);
        }
    }

    /// Keeps every checkpoint the loop reported.
    #[derive(Default)]
    struct Recorded(Mutex<Vec<TurnCheckpoint>>);

    impl TurnJournal for Recorded {
        fn record(&self, checkpoint: &TurnCheckpoint) {
            self.0.lock().push(checkpoint.clone());
        }
    }

    fn checkpoint_call(call_id: &str, name: &str, state: CallState) -> CheckpointCall {
        CheckpointCall {
            call_id: call_id.into(),
            name: name.into(),
            arguments: "5".into(),
            state,
        }
    }

    fn settled(output: &str) -> CallState {
        CallState::Settled {
            output: output.into(),
            success: true,
        }
    }

    #[tokio::test]
    async fn a_call_is_journaled_as_started_before_it_runs_and_settled_after() {
        let timed = timed_registry();
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![call("r", "read", 5), call("w", "write", 5)],
            vec![ResponseEvent::OutputTextDelta("done".into())],
        ]));
        let journal = Arc::new(Recorded::default());
        let mut session =
            Session::new(provider, timed.registry.clone()).with_journal(journal.clone());
        let (mut emit, _) = collect_events();
        session.run_turn("go", &mut emit).await.unwrap();

        let states: Vec<Vec<CallState>> = journal
            .0
            .lock()
            .iter()
            .map(|checkpoint| {
                checkpoint.rounds[0]
                    .calls
                    .iter()
                    .map(|call| call.state.clone())
                    .collect()
            })
            .collect();
        assert_eq!(
            states,
            vec![
                vec![CallState::Planned, CallState::Planned],
                vec![CallState::Started, CallState::Planned],
                vec![settled("out r"), CallState::Planned],
                vec![settled("out r"), CallState::Started],
                vec![settled("out r"), settled("out w")],
            ]
        );
        assert_eq!(session.resumed(), ResumeStats::default());
    }

    #[tokio::test]
    async fn a_resumed_turn_replays_settled_calls_and_does_not_repeat_an_interrupted_write() {
        let timed = timed_registry();
        let provider = Arc::new(FakeProvider::with_rounds(vec![vec![
            ResponseEvent::OutputTextDelta("done".into()),
        ]]));
        let checkpoint = TurnCheckpoint {
            rounds: vec![CheckpointRound {
                calls: vec![
                    checkpoint_call("r1", "read", settled("earlier r1")),
                    checkpoint_call("w1", "write", settled("earlier w1")),
                    checkpoint_call("w2", "write", CallState::Started),
                ],
            }],
        };
        let journal = Arc::new(Recorded::default());
        let mut session =
            Session::new(provider.clone(), timed.registry.clone()).with_journal(journal.clone());
        let (mut emit, seen) = collect_events();
        let outcome = session
            .resume_turn("go", checkpoint, &mut emit)
            .await
            .unwrap();

        assert_eq!(outcome.final_message.as_deref(), Some("done"));
        assert!(timed.log.lock().is_empty(), "nothing ran again");
        assert_eq!(
            outputs_in_history(&session),
            ["earlier r1", "earlier w1", INTERRUPTED_CALL_OUTPUT]
        );
        assert_eq!(
            session.resumed(),
            ResumeStats {
                rounds: 1,
                replayed_calls: 2,
                interrupted_calls: 1,
                rerun_calls: 0,
            }
        );
        // The model's first round of this attempt reads all three.
        let prompts = provider.prompts.lock();
        assert_eq!(prompts.len(), 1);
        let replayed = prompts[0]
            .input
            .iter()
            .filter(|item| matches!(item, ResponseItem::FunctionCallOutput { .. }))
            .count();
        assert_eq!(replayed, 3);
        // Nothing that did not run in this attempt is announced as running.
        assert!(
            !seen
                .lock()
                .iter()
                .any(|msg| matches!(msg, EventMsg::ToolCallBegin { .. }))
        );
        // The interrupted call is settled in the journal, so a second
        // restart would not count it again.
        let last = journal.0.lock().last().cloned().unwrap();
        assert_eq!(
            last.rounds[0].calls[2].state,
            CallState::Settled {
                output: INTERRUPTED_CALL_OUTPUT.into(),
                success: false,
            }
        );
    }

    #[tokio::test]
    async fn a_resumed_round_runs_what_never_started_and_reruns_an_interrupted_read() {
        let timed = timed_registry();
        let provider = Arc::new(FakeProvider::with_rounds(vec![vec![
            ResponseEvent::OutputTextDelta("done".into()),
        ]]));
        let checkpoint = TurnCheckpoint {
            rounds: vec![CheckpointRound {
                calls: vec![
                    checkpoint_call("w1", "write", settled("earlier w1")),
                    checkpoint_call("r1", "read", CallState::Started),
                    checkpoint_call("w2", "write", CallState::Planned),
                ],
            }],
        };
        let mut session = Session::new(provider, timed.registry.clone());
        let (mut emit, _) = collect_events();
        session
            .resume_turn("go", checkpoint, &mut emit)
            .await
            .unwrap();

        let log = timed.log.lock().clone();
        assert_eq!(log, ["start r1", "end r1", "start w2", "end w2"]);
        assert_eq!(
            outputs_in_history(&session),
            ["earlier w1", "out r1", "out w2"]
        );
        assert_eq!(session.resumed().rerun_calls, 1);
        assert_eq!(session.resumed().interrupted_calls, 0);
    }

    #[tokio::test]
    async fn resumed_rounds_count_against_the_round_cap() {
        let timed = timed_registry();
        let provider = Arc::new(FakeProvider::with_rounds(vec![vec![call("r", "read", 1)]]));
        let checkpoint = TurnCheckpoint {
            rounds: (0..2)
                .map(|i| CheckpointRound {
                    calls: vec![checkpoint_call(&format!("c{i}"), "read", settled("x"))],
                })
                .collect(),
        };
        let mut session =
            Session::new(provider.clone(), timed.registry.clone()).with_max_tool_rounds(3);
        let (mut emit, _) = collect_events();
        let err = session
            .resume_turn("go", checkpoint, &mut emit)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::MaxToolRounds(3)));
        // Two rounds carried over, one asked for: the cap of three is met.
        assert_eq!(provider.prompts.lock().len(), 1);
    }

    #[test]
    fn a_checkpoint_round_trips_through_json() {
        let checkpoint = TurnCheckpoint {
            rounds: vec![CheckpointRound {
                calls: vec![
                    checkpoint_call("a", "read", CallState::Planned),
                    checkpoint_call("b", "write", CallState::Started),
                    checkpoint_call("c", "write", settled("ok")),
                ],
            }],
        };
        let json = serde_json::to_value(&checkpoint).unwrap();
        assert_eq!(
            json["rounds"][0]["calls"][2],
            serde_json::json!({
                "callId": "c", "name": "write", "arguments": "5",
                "state": {"state": "settled", "output": "ok", "success": true}
            })
        );
        let back: TurnCheckpoint = serde_json::from_value(json).unwrap();
        assert_eq!(back, checkpoint);
    }

    /// Accepts only answers that are a number.
    struct Numeric;

    impl AnswerCheck for Numeric {
        fn check(&self, answer: &str) -> Result<(), crate::answer::AnswerRejection> {
            answer
                .trim()
                .parse::<f64>()
                .map(|_| ())
                .map_err(|_| crate::answer::AnswerRejection {
                    reason: format!("not a number: {answer}"),
                    feedback: "answer with a number".into(),
                })
        }
    }

    #[tokio::test]
    async fn a_rejected_answer_is_sent_back_once_and_the_retry_is_the_answer() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![ResponseEvent::OutputTextDelta("about four".into())],
            vec![ResponseEvent::OutputTextDelta("4".into())],
        ]));
        let mut session = Session::new(provider.clone(), Arc::new(ToolRegistry::new()))
            .with_answer_check(Arc::new(Numeric));
        let (mut emit, _) = collect_events();
        let outcome = session.run_turn("2+2?", &mut emit).await.unwrap();

        assert_eq!(outcome.final_message.as_deref(), Some("4"));
        let prompts = provider.prompts.lock();
        assert_eq!(prompts.len(), 2);
        let retry = &prompts[1].input;
        assert_eq!(
            retry[retry.len() - 2..],
            [
                ResponseItem::Message {
                    role: Role::Assistant,
                    content: "about four".into(),
                },
                ResponseItem::Message {
                    role: Role::User,
                    content: "answer with a number".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn a_second_rejection_fails_the_turn() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![ResponseEvent::OutputTextDelta("four".into())],
            vec![ResponseEvent::OutputTextDelta("still four".into())],
            vec![ResponseEvent::OutputTextDelta("4".into())],
        ]));
        let mut session = Session::new(provider.clone(), Arc::new(ToolRegistry::new()))
            .with_answer_check(Arc::new(Numeric));
        let (mut emit, seen) = collect_events();
        let err = session.run_turn("2+2?", &mut emit).await.unwrap_err();

        assert!(
            matches!(err, CoreError::AnswerRejected(reason) if reason == "not a number: still four")
        );
        assert_eq!(provider.prompts.lock().len(), 2);
        assert!(
            seen.lock()
                .iter()
                .any(|msg| matches!(msg, EventMsg::Error { .. }))
        );
    }

    /// Hands out one scripted batch per take, whichever kind; records how
    /// it was asked and whether it ended up closed.
    #[derive(Default)]
    struct ScriptedInbox {
        batches: Mutex<VecDeque<Vec<String>>>,
        asked: Mutex<Vec<&'static str>>,
        closed: std::sync::atomic::AtomicBool,
    }

    impl ScriptedInbox {
        fn new(batches: Vec<Vec<&str>>) -> Arc<Self> {
            Arc::new(Self {
                batches: Mutex::new(
                    batches
                        .into_iter()
                        .map(|batch| batch.into_iter().map(String::from).collect())
                        .collect(),
                ),
                ..Self::default()
            })
        }

        fn closed(&self) -> bool {
            self.closed.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl TurnInbox for ScriptedInbox {
        fn take(&self) -> Vec<String> {
            self.asked.lock().push("take");
            self.batches.lock().pop_front().unwrap_or_default()
        }

        fn take_or_close(&self) -> Vec<String> {
            self.asked.lock().push("take_or_close");
            let batch = self.batches.lock().pop_front().unwrap_or_default();
            if batch.is_empty() {
                self.close();
            }
            batch
        }

        fn close(&self) {
            self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn user(text: &str) -> ResponseItem {
        ResponseItem::Message {
            role: Role::User,
            content: text.into(),
        }
    }

    fn echo_registry() -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        Arc::new(registry)
    }

    #[tokio::test]
    async fn a_message_sent_while_a_tool_ran_is_read_on_the_next_round() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![ResponseEvent::FunctionCall {
                call_id: "c1".into(),
                name: "echo".into(),
                arguments: "{}".into(),
            }],
            vec![ResponseEvent::OutputTextDelta("only the PDFs".into())],
        ]));
        // Round one: nothing yet. Round two: the message. Then the answer.
        let inbox = ScriptedInbox::new(vec![vec![], vec!["only the PDFs, please"]]);
        let mut session = Session::new(provider.clone(), echo_registry()).with_inbox(inbox.clone());
        let (mut emit, events) = collect_events();
        let outcome = session.run_turn("list the files", &mut emit).await.unwrap();

        assert_eq!(outcome.final_message.as_deref(), Some("only the PDFs"));
        assert_eq!(session.steered(), 1);
        let prompts = provider.prompts.lock();
        assert_eq!(prompts.len(), 2);
        // After the call and its result, which the model must still see
        // answered, not before them.
        let second = &prompts[1].input;
        assert!(matches!(
            second[second.len() - 2],
            ResponseItem::FunctionCallOutput { .. }
        ));
        assert_eq!(second.last(), Some(&user("only the PDFs, please")));
        assert!(events.lock().contains(&EventMsg::SteerMessage {
            message: "only the PDFs, please".into()
        }));
        assert_eq!(*inbox.asked.lock(), vec!["take", "take", "take_or_close"]);
        assert!(inbox.closed());
    }

    #[tokio::test]
    async fn a_message_that_arrives_with_the_answer_makes_it_interim() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![
            vec![ResponseEvent::OutputTextDelta("four".into())],
            vec![ResponseEvent::OutputTextDelta("quatre".into())],
        ]));
        let inbox = ScriptedInbox::new(vec![vec![], vec!["in French"]]);
        let mut session =
            Session::new(provider.clone(), Arc::new(ToolRegistry::new())).with_inbox(inbox.clone());
        let (mut emit, events) = collect_events();
        let outcome = session.run_turn("2+2?", &mut emit).await.unwrap();

        assert_eq!(outcome.final_message.as_deref(), Some("quatre"));
        let prompts = provider.prompts.lock();
        let second = &prompts[1].input;
        assert_eq!(
            second[second.len() - 2..],
            [
                ResponseItem::Message {
                    role: Role::Assistant,
                    content: "four".into(),
                },
                user("in French"),
            ]
        );
        // One turn, one completion.
        let completions = events
            .lock()
            .iter()
            .filter(|msg| matches!(msg, EventMsg::TurnComplete))
            .count();
        assert_eq!(completions, 1);
        assert!(inbox.closed());
    }

    #[tokio::test]
    async fn a_message_sent_before_the_first_round_follows_the_question() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![vec![
            ResponseEvent::OutputTextDelta("ok".into()),
        ]]));
        let inbox = ScriptedInbox::new(vec![vec!["and keep it short"]]);
        let mut session =
            Session::new(provider.clone(), Arc::new(ToolRegistry::new())).with_inbox(inbox);
        let (mut emit, _) = collect_events();
        session.run_turn("explain", &mut emit).await.unwrap();

        let prompts = provider.prompts.lock();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0].input[prompts[0].input.len() - 2..],
            [user("explain"), user("and keep it short")]
        );
    }

    #[tokio::test]
    async fn a_turn_that_fails_closes_its_inbox() {
        let provider = Arc::new(FakeProvider::with_rounds(vec![vec![
            ResponseEvent::FunctionCall {
                call_id: "c1".into(),
                name: "echo".into(),
                arguments: "{}".into(),
            },
        ]]));
        let inbox = ScriptedInbox::new(vec![]);
        let mut session = Session::new(provider, echo_registry())
            .with_max_tool_rounds(1)
            .with_inbox(inbox.clone());
        let (mut emit, _) = collect_events();
        let err = session.run_turn("go", &mut emit).await.unwrap_err();

        assert!(matches!(err, CoreError::MaxToolRounds(1)));
        // Never asked to take-or-close: the model never answered.
        assert!(!inbox.asked.lock().contains(&"take_or_close"));
        assert!(inbox.closed());
    }
}
