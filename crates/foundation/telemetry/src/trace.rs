//! Per-turn traces (agent-runtime-program.md 2.1).
//!
//! One span tree per agent turn: the turn, each model round, the dispatch a
//! round sends and the tool calls it asks for, with the request id, tenant,
//! token usage and outcome. Recording is always on and in process; a
//! [`TraceStore`] keeps the most recent turns and drops the oldest, so memory
//! is bounded by turn count and a per-turn span cap rather than by uptime.
//!
//! A trace records what ran and how it ended, never what was said: no message
//! text, tool arguments or tool output. It is read by operators and by the
//! tenant whose turn it was, and is not a second copy of the conversation.
//!
//! Spans are a flat list with parent ids, the shape OTLP export would need,
//! so an exporter can be added on top of a finished [`TurnTrace`] without
//! re-instrumenting the call sites.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Map, Value};

/// Turns kept when no capacity is configured.
pub const DEFAULT_TRACE_CAPACITY: usize = 512;

/// Spans kept per turn. The round cap bounds rounds, but not the tool calls a
/// round may ask for; a turn past this keeps its first spans and is marked
/// `truncated`.
pub const MAX_SPANS_PER_TURN: usize = 256;

/// How a span, or the whole turn, ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Running,
    Ok,
    Error,
    Cancelled,
}

/// A span's id, unique within its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanId(usize);

impl SpanId {
    /// Returned by [`TurnRecorder::open`] past the span cap: every operation
    /// on it is a no-op.
    const DROPPED: SpanId = SpanId(usize::MAX);
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub span_id: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<usize>,
    /// `turn`, `round`, `dispatch` or `tool_call`.
    pub kind: String,
    pub name: String,
    pub status: SpanStatus,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
    #[serde(skip)]
    started: Option<Instant>,
}

/// Token usage summed over every dispatch in the turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTrace {
    pub turn_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub request_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    pub status: SpanStatus,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub usage: TraceUsage,
    pub truncated: bool,
    pub spans: Vec<Span>,
}

/// Who a turn ran for and what started it.
#[derive(Debug, Clone, Default)]
pub struct TurnStart {
    pub tenant_id: String,
    pub request_id: String,
    pub thread_id: String,
    /// The root span's name, e.g. `chat.query`.
    pub name: String,
}

/// The most recent turns, newest last.
pub struct TraceStore {
    capacity: usize,
    turns: Mutex<VecDeque<Arc<Mutex<TurnTrace>>>>,
}

impl Default for TraceStore {
    fn default() -> Self {
        Self::new(DEFAULT_TRACE_CAPACITY)
    }
}

impl TraceStore {
    /// A store keeping `capacity` turns; zero is raised to one.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            turns: Mutex::new(VecDeque::with_capacity(capacity)),
        }
    }

    /// Records a new turn, evicting the oldest past capacity. The turn is
    /// readable at once, so a turn that hangs can be inspected while it hangs.
    pub fn start(&self, start: TurnStart) -> TurnRecorder {
        let now = Utc::now();
        let turn_id = format!("turn_{}", uuid::Uuid::now_v7().simple());
        let root = Span {
            span_id: 0,
            parent_span_id: None,
            kind: "turn".to_string(),
            name: start.name,
            status: SpanStatus::Running,
            error_code: String::new(),
            started_at: now,
            ended_at: None,
            duration_ms: None,
            attributes: Map::new(),
            started: Some(Instant::now()),
        };
        let trace = Arc::new(Mutex::new(TurnTrace {
            turn_id,
            tenant_id: start.tenant_id.trim().to_string(),
            request_id: start.request_id.trim().to_string(),
            thread_id: start.thread_id.trim().to_string(),
            status: SpanStatus::Running,
            error_code: String::new(),
            started_at: now,
            ended_at: None,
            duration_ms: None,
            usage: TraceUsage::default(),
            truncated: false,
            spans: vec![root],
        }));
        let mut turns = self.turns.lock();
        while turns.len() >= self.capacity {
            turns.pop_front();
        }
        turns.push_back(Arc::clone(&trace));
        TurnRecorder { trace }
    }

    /// A snapshot of one turn.
    #[must_use]
    pub fn get(&self, turn_id: &str) -> Option<TurnTrace> {
        let turns = self.turns.lock();
        turns
            .iter()
            .rev()
            .map(|trace| trace.lock())
            .find(|trace| trace.turn_id == turn_id)
            .map(|trace| trace.clone())
    }

    /// Snapshots of the newest turns that `keep` accepts, newest first.
    pub fn list(&self, limit: usize, keep: impl Fn(&TurnTrace) -> bool) -> Vec<TurnTrace> {
        let turns = self.turns.lock();
        turns
            .iter()
            .rev()
            .filter_map(|trace| {
                let trace = trace.lock();
                keep(&trace).then(|| trace.clone())
            })
            .take(limit)
            .collect()
    }
}

/// Writes one turn's spans. Cheap to clone; every clone writes the same turn.
#[derive(Clone)]
pub struct TurnRecorder {
    trace: Arc<Mutex<TurnTrace>>,
}

impl TurnRecorder {
    #[must_use]
    pub fn turn_id(&self) -> String {
        self.trace.lock().turn_id.clone()
    }

    /// The turn's own span, parent of every round.
    #[must_use]
    pub fn root(&self) -> SpanId {
        SpanId(0)
    }

    /// Opens a span under `parent`. Past [`MAX_SPANS_PER_TURN`] the span is
    /// dropped and the turn marked truncated.
    pub fn open(&self, parent: SpanId, kind: &str, name: &str) -> SpanId {
        let mut trace = self.trace.lock();
        if trace.spans.len() >= MAX_SPANS_PER_TURN || parent == SpanId::DROPPED {
            trace.truncated = true;
            return SpanId::DROPPED;
        }
        let span_id = trace.spans.len();
        trace.spans.push(Span {
            span_id,
            parent_span_id: Some(parent.0),
            kind: kind.to_string(),
            name: name.to_string(),
            status: SpanStatus::Running,
            error_code: String::new(),
            started_at: Utc::now(),
            ended_at: None,
            duration_ms: None,
            attributes: Map::new(),
            started: Some(Instant::now()),
        });
        SpanId(span_id)
    }

    pub fn set_attribute(&self, span: SpanId, key: &str, value: impl Into<Value>) {
        let mut trace = self.trace.lock();
        if let Some(span) = trace.spans.get_mut(span.0) {
            span.attributes.insert(key.to_string(), value.into());
        }
    }

    /// Closes a span. Closing one already closed is a no-op, so the first
    /// outcome recorded is the one kept.
    pub fn close(&self, span: SpanId, status: SpanStatus, error_code: &str) {
        let mut trace = self.trace.lock();
        if let Some(span) = trace.spans.get_mut(span.0) {
            close_span(span, status, error_code);
        }
    }

    pub fn add_usage(&self, input_tokens: i64, output_tokens: i64, total_tokens: i64) {
        let mut trace = self.trace.lock();
        trace.usage.input_tokens += input_tokens;
        trace.usage.output_tokens += output_tokens;
        trace.usage.total_tokens += total_tokens;
    }

    /// Ends the turn. Spans still running end with the turn's status: they
    /// were cut short by however the turn ended.
    pub fn finish(&self, status: SpanStatus, error_code: &str) {
        let mut trace = self.trace.lock();
        if trace.status != SpanStatus::Running {
            return;
        }
        for span in trace.spans.iter_mut().rev() {
            close_span(span, status, error_code);
        }
        let root = &trace.spans[0];
        let (ended_at, duration_ms) = (root.ended_at, root.duration_ms);
        trace.status = status;
        trace.error_code = error_code.to_string();
        trace.ended_at = ended_at;
        trace.duration_ms = duration_ms;
    }
}

fn close_span(span: &mut Span, status: SpanStatus, error_code: &str) {
    if span.status != SpanStatus::Running {
        return;
    }
    span.status = status;
    span.error_code = error_code.to_string();
    span.ended_at = Some(Utc::now());
    span.duration_ms = span
        .started
        .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(store: &TraceStore, tenant: &str) -> TurnRecorder {
        store.start(TurnStart {
            tenant_id: tenant.to_string(),
            request_id: "req_1".to_string(),
            name: "chat.query".to_string(),
            ..TurnStart::default()
        })
    }

    #[test]
    fn a_turn_is_a_tree_readable_while_it_runs_and_after() {
        let store = TraceStore::new(4);
        let turn = start(&store, "ten_a");
        let round = turn.open(turn.root(), "round", "round 1");
        let dispatch = turn.open(round, "dispatch", "openai_compatible");
        turn.set_attribute(dispatch, "attemptCount", 1);
        turn.add_usage(2, 3, 5);
        turn.close(dispatch, SpanStatus::Ok, "");

        let running = store.get(&turn.turn_id()).expect("running turn");
        assert_eq!(running.status, SpanStatus::Running);
        assert_eq!(running.spans[2].parent_span_id, Some(1));
        assert_eq!(running.spans[2].status, SpanStatus::Ok);

        turn.finish(SpanStatus::Ok, "");
        let done = store.get(&turn.turn_id()).expect("finished turn");
        assert_eq!(done.status, SpanStatus::Ok);
        assert!(done.duration_ms.is_some());
        assert_eq!(done.usage.total_tokens, 5);
        assert!(done.spans.iter().all(|span| span.status == SpanStatus::Ok));
        assert_eq!(done.request_id, "req_1");
    }

    #[test]
    fn finishing_cuts_running_spans_short_but_keeps_settled_ones() {
        let store = TraceStore::default();
        let turn = start(&store, "ten_a");
        let first = turn.open(turn.root(), "round", "round 1");
        turn.close(first, SpanStatus::Ok, "");
        let second = turn.open(turn.root(), "round", "round 2");

        turn.finish(SpanStatus::Error, "max_tool_rounds");
        // A second finish does not overwrite the first.
        turn.finish(SpanStatus::Ok, "");

        let trace = store.get(&turn.turn_id()).expect("trace");
        assert_eq!(trace.status, SpanStatus::Error);
        assert_eq!(trace.error_code, "max_tool_rounds");
        assert_eq!(trace.spans[first.0].status, SpanStatus::Ok);
        assert_eq!(trace.spans[second.0].status, SpanStatus::Error);
        assert_eq!(trace.spans[second.0].error_code, "max_tool_rounds");
    }

    #[test]
    fn the_store_keeps_only_the_newest_turns() {
        let store = TraceStore::new(2);
        let ids: Vec<String> = (0..3).map(|_| start(&store, "ten_a").turn_id()).collect();

        assert!(store.get(&ids[0]).is_none(), "oldest evicted");
        let listed: Vec<String> = store
            .list(10, |_| true)
            .into_iter()
            .map(|t| t.turn_id)
            .collect();
        assert_eq!(listed, vec![ids[2].clone(), ids[1].clone()]);
    }

    #[test]
    fn listing_filters_and_limits() {
        let store = TraceStore::new(8);
        for tenant in ["ten_a", "ten_b", "ten_a", "ten_a"] {
            start(&store, tenant);
        }
        let mine = store.list(2, |t| t.tenant_id == "ten_a");
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|t| t.tenant_id == "ten_a"));
    }

    #[test]
    fn a_turn_past_the_span_cap_is_truncated_not_unbounded() {
        let store = TraceStore::default();
        let turn = start(&store, "ten_a");
        let mut last = turn.root();
        for _ in 0..MAX_SPANS_PER_TURN + 10 {
            last = turn.open(turn.root(), "tool_call", "t");
        }
        assert_eq!(last, SpanId::DROPPED);
        // Operations on a dropped span are no-ops, and children of one too.
        turn.set_attribute(last, "k", 1);
        turn.close(last, SpanStatus::Ok, "");
        assert_eq!(turn.open(last, "x", "y"), SpanId::DROPPED);

        let trace = store.get(&turn.turn_id()).expect("trace");
        assert_eq!(trace.spans.len(), MAX_SPANS_PER_TURN);
        assert!(trace.truncated);
    }

    #[test]
    fn the_wire_shape_is_camel_case_without_instants() {
        let store = TraceStore::default();
        let turn = start(&store, "ten_a");
        turn.finish(SpanStatus::Cancelled, "cancelled");
        let json = serde_json::to_value(store.get(&turn.turn_id()).unwrap()).unwrap();
        assert_eq!(json["status"], "cancelled");
        assert_eq!(json["errorCode"], "cancelled");
        assert_eq!(json["spans"][0]["kind"], "turn");
        assert!(json["spans"][0].get("started").is_none());
        assert!(json["turnId"].as_str().unwrap().starts_with("turn_"));
    }
}
