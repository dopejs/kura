//! Port of daemon/internal/router. See rs/MIGRATION.md for conventions.
//!
//! Message routing decisions: maps inbound channel/peer/thread coordinates to
//! stable sessions, reusing existing sessions by routing key and isolating
//! group sessions per thread.
//!
//! Follow-up: `Session::active_profile_projection` is a JSON pass-through
//! (`serde_json::Value`) because `profiles.RuntimeProjection` has not been
//! ported to `kura-profiles` yet. Once it lands, swap the field type to the
//! real projection type without changing the wire format.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RouterError {
    #[error("session not found")]
    SessionNotFound,
    #[error("channel is required")]
    ChannelRequired,
    #[error("peer is required")]
    PeerRequired,
    #[error("thread is required for group sessions")]
    ThreadRequired,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Direct,
    Group,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Direct => "direct",
            SessionKind::Group => "group",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub session_id: String,
    pub kind: SessionKind,
    pub status: SessionStatus,
    pub channel: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_id: String,
    pub peer_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    pub routing_key: String,
    pub generation: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reset_at: Option<DateTime<Utc>>,
    /// Placeholder for `profiles.RuntimeProjection` (unported crate).
    /// Kept as raw JSON to preserve wire compatibility; see module docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_projection: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteInput {
    #[serde(default)]
    pub kind: SessionKind,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub peer_id: String,
    #[serde(default)]
    pub thread_id: String,
}

#[derive(Default)]
struct RouterState {
    sessions_by_id: HashMap<String, Session>,
    /// Insertion order, preserving Go's `sessionIDs` slice semantics.
    session_ids: Vec<String>,
    by_routing_key: HashMap<String, String>,
}

#[derive(Default)]
pub struct SessionRouter {
    state: RwLock<RouterState>,
}

impl SessionRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Route an inbound message to a session. Returns `(session, created)`:
    /// an existing session is reused (and touched) when the routing key
    /// matches, otherwise a new generation-1 session is created.
    pub fn route(&self, input: RouteInput) -> Result<(Session, bool), RouterError> {
        let routing_key = make_routing_key(&input)?;
        let mut state = self.state.write();

        if let Some(session_id) = state.by_routing_key.get(&routing_key).cloned() {
            // The routing-key index and the session map are mutated under the
            // same write lock, so a mapped key always resolves to a session.
            if let Some(session) = state.sessions_by_id.get_mut(&session_id) {
                let now = Utc::now();
                session.updated_at = now;
                session.last_active_at = now;
                return Ok((session.clone(), false));
            }
        }

        let now = Utc::now();
        let session = Session {
            session_id: new_session_id(),
            kind: input.kind,
            status: SessionStatus::Active,
            channel: input.channel,
            account_id: input.account_id,
            peer_id: input.peer_id,
            thread_id: input.thread_id,
            routing_key: routing_key.clone(),
            generation: 1,
            created_at: now,
            updated_at: now,
            last_active_at: now,
            last_reset_at: None,
            active_profile_projection: None,
        };

        state
            .sessions_by_id
            .insert(session.session_id.clone(), session.clone());
        state.session_ids.push(session.session_id.clone());
        state
            .by_routing_key
            .insert(routing_key, session.session_id.clone());

        Ok((session, true))
    }

    /// List sessions in insertion order.
    pub fn list_sessions(&self) -> Vec<Session> {
        let state = self.state.read();
        state
            .session_ids
            .iter()
            .filter_map(|id| state.sessions_by_id.get(id).cloned())
            .collect()
    }

    pub fn get_session(&self, session_id: &str) -> Option<Session> {
        self.state.read().sessions_by_id.get(session_id).cloned()
    }

    pub fn touch_session(&self, session_id: &str) -> Result<Session, RouterError> {
        let mut state = self.state.write();
        let session = state
            .sessions_by_id
            .get_mut(session_id)
            .ok_or(RouterError::SessionNotFound)?;

        let now = Utc::now();
        session.updated_at = now;
        session.last_active_at = now;

        Ok(session.clone())
    }

    /// Reset a session: bump generation, refresh activity timestamps, and
    /// record the reset time.
    pub fn reset_session(&self, session_id: &str) -> Result<Session, RouterError> {
        let mut state = self.state.write();
        let session = state
            .sessions_by_id
            .get_mut(session_id)
            .ok_or(RouterError::SessionNotFound)?;

        let now = Utc::now();
        session.generation += 1;
        session.updated_at = now;
        session.last_active_at = now;
        session.last_reset_at = Some(now);

        Ok(session.clone())
    }

    /// Replace all sessions with a restored snapshot (e.g. after restart),
    /// rebuilding the routing-key index so subsequent routes reuse them.
    pub fn restore_sessions(&self, sessions: Vec<Session>) {
        let mut state = self.state.write();
        state.sessions_by_id = HashMap::with_capacity(sessions.len());
        state.session_ids = Vec::with_capacity(sessions.len());
        state.by_routing_key = HashMap::with_capacity(sessions.len());

        for session in sessions {
            state
                .by_routing_key
                .insert(session.routing_key.clone(), session.session_id.clone());
            state.session_ids.push(session.session_id.clone());
            state
                .sessions_by_id
                .insert(session.session_id.clone(), session);
        }
    }
}

/// Build the routing key for an input, applying the same validation rules as
/// the Go implementation: channel and peer are always required; group
/// sessions additionally require a thread.
fn make_routing_key(input: &RouteInput) -> Result<String, RouterError> {
    if input.channel.is_empty() {
        return Err(RouterError::ChannelRequired);
    }
    if input.peer_id.is_empty() {
        return Err(RouterError::PeerRequired);
    }

    match input.kind {
        SessionKind::Group => {
            if input.thread_id.is_empty() {
                return Err(RouterError::ThreadRequired);
            }
            Ok(format!(
                "{}:{}:{}:{}:{}",
                input.kind.as_str(),
                input.channel,
                input.account_id,
                input.peer_id,
                input.thread_id
            ))
        }
        SessionKind::Direct => Ok(format!(
            "{}:{}:{}:{}",
            SessionKind::Direct.as_str(),
            input.channel,
            input.account_id,
            input.peer_id
        )),
    }
}

/// Generate a session ID with the Go format: `sess_` + 16 random hex chars.
fn new_session_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("sess_{}", &raw[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_reuses_direct_session_and_isolates_group_sessions() {
        let r = SessionRouter::new();

        let (direct, created) = r
            .route(RouteInput {
                kind: SessionKind::Direct,
                channel: "telegram".into(),
                account_id: "acct_1".into(),
                peer_id: "user_1".into(),
                thread_id: String::new(),
            })
            .expect("route direct");
        assert!(created, "expected first direct session to be created");
        assert_eq!(direct.kind, SessionKind::Direct);
        assert_eq!(direct.status, SessionStatus::Active);
        assert_eq!(direct.generation, 1);

        let (same_direct, created) = r
            .route(RouteInput {
                kind: SessionKind::Direct,
                channel: "telegram".into(),
                account_id: "acct_1".into(),
                peer_id: "user_1".into(),
                thread_id: String::new(),
            })
            .expect("route direct again");
        assert!(!created, "expected direct route to reuse session");
        assert_eq!(same_direct.session_id, direct.session_id);

        let (group_a, created) = r
            .route(RouteInput {
                kind: SessionKind::Group,
                channel: "telegram".into(),
                account_id: "acct_1".into(),
                peer_id: "group_1".into(),
                thread_id: "thread_a".into(),
            })
            .expect("route group A");
        assert!(created, "expected group session A to be created");

        let (group_b, created) = r
            .route(RouteInput {
                kind: SessionKind::Group,
                channel: "telegram".into(),
                account_id: "acct_1".into(),
                peer_id: "group_1".into(),
                thread_id: "thread_b".into(),
            })
            .expect("route group B");
        assert!(created, "expected group session B to be created");
        assert_ne!(
            group_a.session_id, group_b.session_id,
            "expected different group sessions for different thread IDs"
        );
    }

    #[test]
    fn reset_session_increments_generation() {
        let r = SessionRouter::new();

        let (session, _) = r
            .route(RouteInput {
                kind: SessionKind::Direct,
                channel: "local".into(),
                account_id: String::new(),
                peer_id: "chat".into(),
                thread_id: String::new(),
            })
            .expect("route");

        let reset = r.reset_session(&session.session_id).expect("reset");
        assert_eq!(reset.generation, 2, "expected generation 2 after reset");
        assert!(reset.last_reset_at.is_some(), "expected last_reset_at set");
        assert!(reset.last_reset_at.expect("checked above") >= session.created_at);
    }

    #[test]
    fn restore_sessions_preserves_routing_compatibility() {
        let r = SessionRouter::new();
        let (original, _) = r
            .route(RouteInput {
                kind: SessionKind::Group,
                channel: "slack".into(),
                account_id: "workspace_1".into(),
                peer_id: "channel_1".into(),
                thread_id: "thread_1".into(),
            })
            .expect("route group");

        let restored = SessionRouter::new();
        restored.restore_sessions(vec![original.clone()]);
        let (after_restart, created) = restored
            .route(RouteInput {
                kind: SessionKind::Group,
                channel: "slack".into(),
                account_id: "workspace_1".into(),
                peer_id: "channel_1".into(),
                thread_id: "thread_1".into(),
            })
            .expect("route after restore");
        assert!(
            !created && after_restart.session_id == original.session_id,
            "expected restored route to reuse {}",
            original.session_id
        );
    }

    #[test]
    fn route_validates_required_fields() {
        let r = SessionRouter::new();

        let err = r
            .route(RouteInput {
                kind: SessionKind::Direct,
                channel: String::new(),
                account_id: String::new(),
                peer_id: "user_1".into(),
                thread_id: String::new(),
            })
            .expect_err("channel required");
        assert_eq!(err, RouterError::ChannelRequired);
        assert_eq!(err.to_string(), "channel is required");

        let err = r
            .route(RouteInput {
                kind: SessionKind::Direct,
                channel: "telegram".into(),
                account_id: String::new(),
                peer_id: String::new(),
                thread_id: String::new(),
            })
            .expect_err("peer required");
        assert_eq!(err, RouterError::PeerRequired);

        let err = r
            .route(RouteInput {
                kind: SessionKind::Group,
                channel: "telegram".into(),
                account_id: String::new(),
                peer_id: "group_1".into(),
                thread_id: String::new(),
            })
            .expect_err("thread required for group");
        assert_eq!(err, RouterError::ThreadRequired);

        // Validation failures must not create sessions.
        assert!(r.list_sessions().is_empty());
    }

    #[test]
    fn routing_key_format_matches_go() {
        let direct = RouteInput {
            kind: SessionKind::Direct,
            channel: "telegram".into(),
            account_id: "acct_1".into(),
            peer_id: "user_1".into(),
            thread_id: String::new(),
        };
        assert_eq!(
            make_routing_key(&direct).expect("direct key"),
            "direct:telegram:acct_1:user_1"
        );

        let group = RouteInput {
            kind: SessionKind::Group,
            channel: "slack".into(),
            account_id: "ws".into(),
            peer_id: "chan".into(),
            thread_id: "t1".into(),
        };
        assert_eq!(
            make_routing_key(&group).expect("group key"),
            "group:slack:ws:chan:t1"
        );
    }

    #[test]
    fn touch_and_get_session() {
        let r = SessionRouter::new();
        let (session, _) = r
            .route(RouteInput {
                kind: SessionKind::Direct,
                channel: "local".into(),
                account_id: String::new(),
                peer_id: "chat".into(),
                thread_id: String::new(),
            })
            .expect("route");

        let fetched = r.get_session(&session.session_id).expect("get");
        assert_eq!(fetched.session_id, session.session_id);
        assert!(r.get_session("sess_missing").is_none());

        let touched = r.touch_session(&session.session_id).expect("touch");
        assert_eq!(touched.generation, 1, "touch must not bump generation");
        assert!(touched.last_active_at >= session.last_active_at);

        let err = r
            .touch_session("sess_missing")
            .expect_err("unknown session");
        assert_eq!(err, RouterError::SessionNotFound);
        let err = r
            .reset_session("sess_missing")
            .expect_err("unknown session");
        assert_eq!(err, RouterError::SessionNotFound);
    }

    #[test]
    fn session_json_shape_matches_go_tags() {
        let r = SessionRouter::new();
        let (session, _) = r
            .route(RouteInput {
                kind: SessionKind::Direct,
                channel: "local".into(),
                account_id: String::new(),
                peer_id: "chat".into(),
                thread_id: String::new(),
            })
            .expect("route");

        let json = serde_json::to_value(&session).expect("serialize");
        let obj = json.as_object().expect("session object");
        for key in [
            "sessionId",
            "kind",
            "status",
            "channel",
            "peerId",
            "routingKey",
            "generation",
            "createdAt",
            "updatedAt",
            "lastActiveAt",
        ] {
            assert!(obj.contains_key(key), "missing key {key}");
        }
        // Go `omitempty` fields are absent when empty.
        for key in [
            "accountId",
            "threadId",
            "lastResetAt",
            "activeProfileProjection",
        ] {
            assert!(!obj.contains_key(key), "unexpected key {key}");
        }
        assert_eq!(json["kind"], "direct");
        assert_eq!(json["status"], "active");
        assert!(session.session_id.starts_with("sess_"));
        assert_eq!(session.session_id.len(), "sess_".len() + 16);

        // Round-trip through restore keeps the router compatible with its
        // own serialized form.
        let restored: Session = serde_json::from_value(json).expect("deserialize");
        assert_eq!(restored, session);
    }
}
