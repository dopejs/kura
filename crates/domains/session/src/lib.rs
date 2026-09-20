//! kura-session — session-strategy policies for the plugin plane.
//!
//! The user-facing thesis: for a personal agent, context management is
//! session management, and it belongs to plugins. This crate is the policy
//! engine behind the `session-strategy` builtin plugin, which attaches at
//! the `chat/pre-dispatch` hook and shapes the assembled message window
//! **deterministically** before the dispatch is prepared (and therefore
//! before it is persisted — the shaped window is exactly what is logged and
//! what the model sees).
//!
//! Two strategies share one mechanism and differ by budget:
//! - **personal** (default): long-session window for direct chat/shell work.
//! - **thread**: tighter window for channel-origin turns (one context per
//!   thread; thread scoping itself comes from the continuity plane).
//!
//! The mechanism is frame-preserving elision:
//! - system messages are the **frame** (persona, skills, safety posture) —
//!   never elided;
//! - the most recent `keep_recent` non-system messages are always kept
//!   (the current query is the last of them);
//! - when the total content length exceeds the strategy's budget, oldest
//!   non-system messages are elided and replaced with a single marker line.
//!
//! Eviction is safe by construction: every chat turn is captured to the
//! memory plane (L0) at turn settle independently of the window, so elided
//! turns remain reachable through thread continuity and memory — the marker
//! says so to the model.

use serde::{Deserialize, Serialize};

/// The role/content shape of one message in the hook payload (a structural
/// subset of `kura_llm::Message`; this crate stays decoupled from the LLM
/// crate on purpose).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowMessage {
    pub role: String,
    pub content: String,
}

/// Plugin configuration (the `config` object of the `session-strategy`
/// entry in `plugins.json`). Zero values fall back to defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionStrategyConfig {
    /// Budget for personal (non-channel) turns, in content chars.
    pub personal_budget_chars: usize,
    /// Budget for channel-origin turns, in content chars.
    pub thread_budget_chars: usize,
    /// Minimum number of most-recent non-system messages always kept.
    pub keep_recent: usize,
    /// Segmentation policy for channel-origin threads, keyed by connector id
    /// (the prefix of `channelScopeRef`), with `"*"` as the default for any
    /// connector not listed. Personal threads are never auto-segmented.
    pub channel_segmentation: std::collections::BTreeMap<String, ChannelSegmentationPolicy>,
}

/// Channel-native thread segmentation policy (Stage 5.2).
///
/// A thread's continuity is scoped to its current *session segment*, and the
/// threads engine already knows how to open a new one (`reset_thread`). What
/// was missing is a rule for *when* an IM conversation should start a fresh
/// segment on its own: a group channel that goes quiet for a day is a new
/// conversation when it resumes, and carrying the old one in is exactly the
/// "context drift" the product outline names. This policy is evaluated at
/// `chat/turn-start`, before continuity is assembled, so the new segment is
/// what the turn sees.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelSegmentationPolicy {
    /// Open a new segment when the thread has been idle this long. 0 = never
    /// segment by idleness.
    pub idle_gap_seconds: u64,
}

/// Personal-session default budget (~a long working window).
pub const DEFAULT_PERSONAL_BUDGET_CHARS: usize = 48_000;
/// Thread default budget (IM threads stay tight and pure).
pub const DEFAULT_THREAD_BUDGET_CHARS: usize = 16_000;
/// Default floor of recent non-system messages.
pub const DEFAULT_KEEP_RECENT: usize = 4;

impl SessionStrategyConfig {
    /// Effective budget for a turn given its source kind (`channel` uses the
    /// thread budget; everything else is personal).
    #[must_use]
    pub fn budget_for(&self, source_kind: Option<&str>) -> usize {
        let is_channel = source_kind == Some("channel");
        if is_channel {
            if self.thread_budget_chars > 0 {
                self.thread_budget_chars
            } else {
                DEFAULT_THREAD_BUDGET_CHARS
            }
        } else if self.personal_budget_chars > 0 {
            self.personal_budget_chars
        } else {
            DEFAULT_PERSONAL_BUDGET_CHARS
        }
    }

    /// The segmentation policy for a channel scope ref (`connector:channel`),
    /// falling back to the `"*"` entry. `None` when nothing applies.
    #[must_use]
    pub fn segmentation_for(&self, channel_scope_ref: &str) -> Option<&ChannelSegmentationPolicy> {
        let connector = channel_scope_ref
            .split(':')
            .next()
            .unwrap_or_default()
            .trim();
        if !connector.is_empty() {
            if let Some(policy) = self.channel_segmentation.get(connector) {
                return Some(policy);
            }
        }
        self.channel_segmentation.get("*")
    }

    #[must_use]
    pub fn keep_recent_floor(&self) -> usize {
        if self.keep_recent > 0 {
            self.keep_recent
        } else {
            DEFAULT_KEEP_RECENT
        }
    }
}

/// Result of one window-shaping pass.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedWindow {
    pub messages: Vec<WindowMessage>,
    /// Number of non-system messages elided (0 = untouched).
    pub elided: usize,
    /// The elided messages, oldest first — the compression-to-memory input
    /// (the caller captures them into the memory plane so eviction never
    /// discards content).
    pub elided_messages: Vec<WindowMessage>,
}

/// The marker inserted where messages were elided.
#[must_use]
pub fn elision_marker(elided: usize) -> String {
    format!(
        "[session window: {elided} earlier message(s) elided by the session-strategy plugin; \
         the full history remains in thread continuity and the memory plane]"
    )
}

/// Shapes `messages` to fit `budget_chars` of total content, preserving the
/// frame (system messages) and at least `keep_recent` most-recent
/// non-system messages. Deterministic: same input → same output. When the
/// input fits the budget the messages come back untouched.
#[must_use]
pub fn shape_window(
    messages: &[WindowMessage],
    budget_chars: usize,
    keep_recent: usize,
) -> ShapedWindow {
    let total: usize = messages.iter().map(|m| m.content.len()).sum();
    if total <= budget_chars {
        return ShapedWindow {
            messages: messages.to_vec(),
            elided: 0,
            elided_messages: Vec::new(),
        };
    }

    // Frame chars are mandatory; history fits in what remains.
    let frame_chars: usize = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.len())
        .sum();
    let history_budget = budget_chars.saturating_sub(frame_chars);

    // Walk non-system messages from newest to oldest, keeping while within
    // the history budget; `keep_recent` newest are kept unconditionally.
    let history_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role != "system")
        .map(|(i, _)| i)
        .collect();
    let mut kept: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut used = 0usize;
    for (rank, &idx) in history_indices.iter().rev().enumerate() {
        let len = messages[idx].content.len();
        if rank < keep_recent || used + len <= history_budget {
            kept.insert(idx);
            used += len;
        }
    }

    let elided = history_indices.len() - kept.len();
    if elided == 0 {
        return ShapedWindow {
            messages: messages.to_vec(),
            elided: 0,
            elided_messages: Vec::new(),
        };
    }

    // Rebuild: frame stays in place; the marker replaces the first elided
    // position so the model sees where history was cut.
    let mut out: Vec<WindowMessage> = Vec::with_capacity(messages.len() - elided + 1);
    let mut elided_messages: Vec<WindowMessage> = Vec::with_capacity(elided);
    let mut marker_placed = false;
    for (idx, message) in messages.iter().enumerate() {
        if message.role == "system" || kept.contains(&idx) {
            out.push(message.clone());
        } else {
            elided_messages.push(message.clone());
            if !marker_placed {
                out.push(WindowMessage {
                    role: "system".to_string(),
                    content: elision_marker(elided),
                });
                marker_placed = true;
            }
        }
    }
    ShapedWindow {
        messages: out,
        elided,
        elided_messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> WindowMessage {
        WindowMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn under_budget_is_untouched() {
        let messages = vec![msg("system", "frame"), msg("user", "hello")];
        let shaped = shape_window(&messages, 1000, 4);
        assert_eq!(shaped.elided, 0);
        assert_eq!(shaped.messages, messages);
    }

    #[test]
    fn over_budget_elides_oldest_history_and_keeps_frame() {
        let mut messages = vec![msg("system", "persona frame")];
        for i in 0..10 {
            messages.push(msg("user", &format!("question {i} {}", "x".repeat(100))));
            messages.push(msg("assistant", &format!("answer {i} {}", "y".repeat(100))));
        }
        messages.push(msg("user", "current query"));
        let shaped = shape_window(&messages, 500, 2);
        assert!(shaped.elided > 0);
        // Frame preserved.
        assert!(shaped.messages.iter().any(|m| m.content == "persona frame"));
        // Current query preserved (keep_recent floor).
        assert_eq!(shaped.messages.last().unwrap().content, "current query");
        // Marker present exactly once, positioned before the kept history.
        let markers: Vec<usize> = shaped
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.content.contains("elided by the session-strategy"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(markers.len(), 1);
        // Oldest history gone, newest kept.
        assert!(
            !shaped
                .messages
                .iter()
                .any(|m| m.content.starts_with("question 0"))
        );
        // Deterministic.
        let again = shape_window(&messages, 500, 2);
        assert_eq!(again, shaped);
    }

    #[test]
    fn keep_recent_floor_wins_over_budget() {
        let messages = vec![
            msg("user", &"a".repeat(300)),
            msg("assistant", &"b".repeat(300)),
            msg("user", &"c".repeat(300)),
        ];
        // Budget of zero still keeps the floor.
        let shaped = shape_window(&messages, 0, 2);
        assert_eq!(shaped.elided, 1);
        let non_system: Vec<&WindowMessage> = shaped
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .collect();
        assert_eq!(non_system.len(), 2);
        assert!(non_system[1].content.starts_with('c'));
    }

    #[test]
    fn config_budgets_key_off_source_kind() {
        let config = SessionStrategyConfig::default();
        assert_eq!(config.budget_for(None), DEFAULT_PERSONAL_BUDGET_CHARS);
        assert_eq!(
            config.budget_for(Some("chat")),
            DEFAULT_PERSONAL_BUDGET_CHARS
        );
        assert_eq!(
            config.budget_for(Some("channel")),
            DEFAULT_THREAD_BUDGET_CHARS
        );
        let custom = SessionStrategyConfig {
            personal_budget_chars: 100,
            thread_budget_chars: 50,
            keep_recent: 1,
            ..Default::default()
        };
        assert_eq!(custom.budget_for(None), 100);
        assert_eq!(custom.budget_for(Some("channel")), 50);
        assert_eq!(custom.keep_recent_floor(), 1);

        // Parses from a plugins.json config object.
        let parsed: SessionStrategyConfig = serde_json::from_value(serde_json::json!({
            "personalBudgetChars": 200,
            "threadBudgetChars": 80,
            "keepRecent": 3
        }))
        .expect("parse");
        assert_eq!(parsed.budget_for(None), 200);
    }
}

// ---------------------------------------------------------------------------
// Session frames (Stage 5.1)
// ---------------------------------------------------------------------------

/// Manager-document kind under which a thread's frame is persisted.
pub const DOC_KIND_SESSION_FRAME: &str = "session_frame";

/// An explicit, durable statement of what a thread is for. It is rendered as
/// a system message on every turn, and system messages are the part of the
/// window elision never touches — so the goal survives no matter how much
/// history is evicted, instead of depending on where in the transcript it
/// was last said.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionFrame {
    pub thread_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub goal: String,
    pub constraints: Vec<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SessionFrame {
    /// True when there is nothing to inject.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.goal.trim().is_empty() && self.constraints.iter().all(|c| c.trim().is_empty())
    }
}

/// Prefix of the injected frame message; the strategy uses it to replace a
/// stale frame rather than stack one per turn.
pub const FRAME_MESSAGE_PREFIX: &str = "[session frame]";

/// Renders the frame as the system message the model sees.
#[must_use]
pub fn render_frame_message(frame: &SessionFrame) -> String {
    let mut out = format!("{FRAME_MESSAGE_PREFIX} goal: {}", frame.goal.trim());
    let constraints: Vec<&str> = frame
        .constraints
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect();
    if !constraints.is_empty() {
        out.push_str("; constraints: ");
        out.push_str(&constraints.join("; "));
    }
    out
}

/// Inserts (or replaces) the frame message at the front of the window. The
/// frame is a system message, so `shape_window` will keep it under any
/// budget; placing it first keeps it ahead of persona/skills so the goal is
/// the first thing the model reads.
pub fn apply_frame(messages: &mut Vec<WindowMessage>, frame: &SessionFrame) {
    if frame.is_empty() {
        return;
    }
    let rendered = render_frame_message(frame);
    if let Some(existing) = messages
        .iter_mut()
        .find(|m| m.role == "system" && m.content.starts_with(FRAME_MESSAGE_PREFIX))
    {
        existing.content = rendered;
        return;
    }
    messages.insert(
        0,
        WindowMessage {
            role: "system".to_string(),
            content: rendered,
        },
    );
}

/// Whether a channel thread's next turn should open a new session segment.
/// Pure so the rule can be tested without a store: the caller supplies the
/// last accepted turn's timestamp in the current segment (or `None` for a
/// segment with no turns yet, which never triggers).
#[must_use]
pub fn segment_boundary_due(
    policy: &ChannelSegmentationPolicy,
    last_turn_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if policy.idle_gap_seconds == 0 {
        return false;
    }
    let Some(last) = last_turn_at else {
        return false;
    };
    now.signed_duration_since(last).num_seconds() >= policy.idle_gap_seconds as i64
}

#[cfg(test)]
mod frame_and_segmentation_tests {
    use super::*;

    fn msg(role: &str, content: &str) -> WindowMessage {
        WindowMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn frame() -> SessionFrame {
        SessionFrame {
            thread_id: "thr_1".into(),
            goal: "ship the Q4 report".into(),
            constraints: vec!["no external sends".into(), "".into(), "cite sources".into()],
            ..SessionFrame::default()
        }
    }

    /// The point of a frame: it survives elision that would drop everything
    /// else, because it is a system message and system messages are the
    /// frame `shape_window` never touches.
    #[test]
    fn the_frame_survives_a_window_that_elides_all_history() {
        let mut messages: Vec<WindowMessage> = (0..40)
            .map(|i| {
                msg(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &"history ".repeat(30),
                )
            })
            .collect();
        apply_frame(&mut messages, &frame());
        assert!(messages[0].content.starts_with(FRAME_MESSAGE_PREFIX));
        assert_eq!(
            messages[0].content,
            "[session frame] goal: ship the Q4 report; constraints: no external sends; cite sources"
        );

        let shaped = shape_window(&messages, 400, 1);
        assert!(shaped.elided > 30, "history was elided: {}", shaped.elided);
        assert!(
            shaped
                .messages
                .iter()
                .any(|m| m.content.starts_with(FRAME_MESSAGE_PREFIX)),
            "the frame is still in the window"
        );
    }

    /// Re-applying replaces the frame rather than stacking one per turn.
    #[test]
    fn re_applying_replaces_rather_than_stacks() {
        let mut messages = vec![msg("system", "persona"), msg("user", "hi")];
        apply_frame(&mut messages, &frame());
        let mut updated = frame();
        updated.goal = "ship the Q4 report by Friday".into();
        apply_frame(&mut messages, &updated);
        let frames: Vec<&WindowMessage> = messages
            .iter()
            .filter(|m| m.content.starts_with(FRAME_MESSAGE_PREFIX))
            .collect();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].content.contains("by Friday"));
        assert!(!apply_frame_noop_marker(&messages));
    }

    fn apply_frame_noop_marker(_m: &[WindowMessage]) -> bool {
        false
    }

    #[test]
    fn an_empty_frame_injects_nothing() {
        let mut messages = vec![msg("user", "hi")];
        apply_frame(&mut messages, &SessionFrame::default());
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn segmentation_triggers_only_past_the_idle_gap_and_never_on_a_fresh_segment() {
        let policy = ChannelSegmentationPolicy {
            idle_gap_seconds: 3600,
        };
        let now = chrono::Utc::now();
        assert!(
            !segment_boundary_due(&policy, None, now),
            "no turns yet: never"
        );
        assert!(!segment_boundary_due(
            &policy,
            Some(now - chrono::Duration::seconds(3599)),
            now
        ));
        assert!(segment_boundary_due(
            &policy,
            Some(now - chrono::Duration::seconds(3600)),
            now
        ));
        let off = ChannelSegmentationPolicy {
            idle_gap_seconds: 0,
        };
        assert!(
            !segment_boundary_due(&off, Some(now - chrono::Duration::days(30)), now),
            "0 = never"
        );
    }

    #[test]
    fn segmentation_policy_resolves_by_connector_then_wildcard() {
        let mut cfg = SessionStrategyConfig::default();
        cfg.channel_segmentation.insert(
            "*".into(),
            ChannelSegmentationPolicy {
                idle_gap_seconds: 100,
            },
        );
        cfg.channel_segmentation.insert(
            "discord-main".into(),
            ChannelSegmentationPolicy {
                idle_gap_seconds: 7,
            },
        );
        assert_eq!(
            cfg.segmentation_for("discord-main:general")
                .unwrap()
                .idle_gap_seconds,
            7
        );
        assert_eq!(
            cfg.segmentation_for("telegram-main:chat1")
                .unwrap()
                .idle_gap_seconds,
            100
        );
        assert_eq!(cfg.segmentation_for("").unwrap().idle_gap_seconds, 100);
        let none = SessionStrategyConfig::default();
        assert!(none.segmentation_for("discord-main:general").is_none());
    }
}
