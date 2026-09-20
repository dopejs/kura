//! Port of daemon/internal/computeruse: browser-first computer-use session/action/artifact
//! types and the in-memory driver. The manager (which wires runtime/policy/billing/store) is
//! the next increment.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

mod manager;
mod sqlite_artifact_recorder;
pub mod subprocess_driver;
pub use manager::*;
pub use sqlite_artifact_recorder::SqliteArtifactRecorder;
pub use subprocess_driver::{SubprocessDriver, SubprocessDriverConfig, WorkerSupervisor};

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(SessionStatus {
    Starting => "starting",
    Active => "active",
    Blocked => "blocked",
    Closing => "closing",
    Closed => "closed",
    Failed => "failed",
    Interrupted => "interrupted",
});

string_enum!(ActionStatus {
    Requested => "requested",
    WaitingApproval => "waiting_approval",
    Running => "running",
    Completed => "completed",
    Denied => "denied",
    Failed => "failed",
    Interrupted => "interrupted",
});

string_enum!(ActionKind {
    Navigate => "navigate",
    Back => "back",
    Forward => "forward",
    Wait => "wait",
    Screenshot => "screenshot",
    Snapshot => "snapshot",
    Click => "click",
    Input => "input",
    Select => "select",
    Download => "download",
    CloseSession => "close_session",
});

string_enum!(PageTarget {
    ActivePage => "active_page",
    NewTab => "new_tab",
    NewWindow => "new_window",
});

string_enum!(RiskLevel {
    Low => "low",
    High => "high",
});

string_enum!(MatchResult {
    Matched => "matched",
    Mismatched => "mismatched",
});

string_enum!(ArtifactKind {
    Screenshot => "screenshot",
    PageSnapshot => "page_snapshot",
    Download => "download",
});

string_enum!(ArtifactStatus {
    Capturing => "capturing",
    Available => "available",
    CaptureFailed => "capture_failed",
});

string_enum!(FailureClass {
    PolicyDenied => "policy_denial",
    UnavailableConsumer => "unavailable_consumer",
    NavigationFailure => "navigation_failure",
    TargetMismatch => "target_mismatch",
    UnsupportedAction => "unsupported_action",
    Interrupted => "interrupted",
});

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSummary {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedPageScope {
    pub scope_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub page_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub scope_revision: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub derived_from_action_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetMatchContext {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub match_strategy: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expected_page_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expected_selector: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expected_text: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub trusted_scope_revision: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub observed_page_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub observed_selector_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_result: Option<MatchResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub computer_use_action_id: String,
    pub run_id: String,
    pub kind: ArtifactKind,
    pub status: ArtifactStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_name: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub byte_size: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub storage_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub capture_failure_reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub computer_use_action_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    pub computer_use_session_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    pub action_kind: ActionKind,
    pub status: ActionStatus,
    pub risk_level: RiskLevel,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_match_context: Option<TargetMatchContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_before: Option<PageSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_after: Option<PageSummary>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason: String,
    pub requested_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub input: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub computer_use_session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    pub status: SessionStatus,
    pub driver_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_page_scope: Option<TrustedPageScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_page: Option<PageSummary>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_action_id: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub initial_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateActionInput {
    pub action_kind: ActionKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_value: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub wait_ms: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub page_target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_match_context: Option<TargetMatchContext>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequestResult {
    pub action: Action,
    pub pending: bool,
    pub approved: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtifactCaptureRequest {
    pub run_id: String,
    pub computer_use_session_id: String,
    pub computer_use_action_id: String,
    pub kind: ArtifactKind,
    pub mime_type: String,
    pub file_name: String,
    pub content: Vec<u8>,
    pub estimated_byte_size: i64,
}

#[must_use]
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

// ---- driver ----

pub trait Driver: Send + Sync {
    fn start_session(&self, session: Session, input: CreateSessionInput)
    -> Result<Session, String>;
    fn execute_action(
        &self,
        session: Session,
        action: Action,
    ) -> Result<(Session, Action, Vec<ArtifactCaptureRequest>), String>;
    fn close_session(&self, session: Session) -> Result<Session, String>;
}

pub struct MemoryDriver;

impl Default for MemoryDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryDriver {
    pub fn new() -> Self {
        MemoryDriver
    }
}

impl Driver for MemoryDriver {
    fn start_session(
        &self,
        mut session: Session,
        input: CreateSessionInput,
    ) -> Result<Session, String> {
        session.status = SessionStatus::Active;
        session.driver_kind = first_non_empty(&[input.driver_kind.trim(), "browser"]);
        if !input.initial_url.trim().is_empty() {
            let page = PageSummary {
                url: input.initial_url.trim().to_string(),
                title: title_from_url(&input.initial_url),
            };
            session.current_page = Some(page.clone());
            session.trusted_page_scope = Some(next_trusted_scope(&session, "", &page));
        }
        Ok(session)
    }

    fn close_session(&self, mut session: Session) -> Result<Session, String> {
        let now = Utc::now();
        session.status = SessionStatus::Closed;
        session.closed_at = Some(now);
        session.updated_at = now;
        Ok(session)
    }

    fn execute_action(
        &self,
        mut session: Session,
        mut action: Action,
    ) -> Result<(Session, Action, Vec<ArtifactCaptureRequest>), String> {
        session.current_page = navigation_current_page(&session);
        let now = Utc::now();
        action.status = ActionStatus::Running;
        action.updated_at = now;
        action.page_before = clone_page(session.current_page.as_ref());

        let mut captures: Vec<ArtifactCaptureRequest> = Vec::new();
        match action.action_kind {
            ActionKind::Navigate => {
                let raw_url = input_string(&action.input, "url");
                if raw_url.is_empty() {
                    return Ok((
                        session,
                        fail_action(
                            action,
                            FailureClass::NavigationFailure,
                            "navigate action requires url",
                        ),
                        Vec::new(),
                    ));
                }
                let page = PageSummary {
                    url: raw_url.clone(),
                    title: title_from_url(&raw_url),
                };
                action.page_after = Some(page.clone());
                session.current_page = Some(page.clone());
                session.trusted_page_scope = Some(next_trusted_scope(
                    &session,
                    &action.computer_use_action_id,
                    &page,
                ));
            }
            ActionKind::Wait => {
                action.page_after = clone_page(session.current_page.as_ref());
            }
            ActionKind::Screenshot | ActionKind::Snapshot => {
                action.page_after = clone_page(session.current_page.as_ref());
                captures.push(build_page_evidence_capture(
                    &session,
                    &action,
                    action.action_kind,
                ));
            }
            ActionKind::Click | ActionKind::Input | ActionKind::Select | ActionKind::Download => {
                if mismatched(action.target_match_context.as_ref()) {
                    let evidence =
                        build_page_evidence_capture(&session, &action, ActionKind::Snapshot);
                    let failed = fail_action(
                        action,
                        FailureClass::TargetMismatch,
                        "approved target no longer matches current page",
                    );
                    return Ok((session, failed, vec![evidence]));
                }
                action.page_after = clone_page(session.current_page.as_ref());
                if action.action_kind == ActionKind::Select {
                    action.page_after = apply_selection_state(action.page_after.as_ref(), &action);
                    session.current_page = clone_page(action.page_after.as_ref());
                }
                captures.push(build_page_evidence_capture(
                    &session,
                    &action,
                    ActionKind::Snapshot,
                ));
                if action.action_kind == ActionKind::Download {
                    captures.push(ArtifactCaptureRequest {
                        run_id: session.run_id.clone(),
                        computer_use_session_id: session.computer_use_session_id.clone(),
                        computer_use_action_id: action.computer_use_action_id.clone(),
                        kind: ArtifactKind::Download,
                        mime_type: "text/plain".to_string(),
                        file_name: download_file_name(&action),
                        content: download_artifact_content(&action).into_bytes(),
                        estimated_byte_size: 0,
                    });
                }
            }
            ActionKind::Back | ActionKind::Forward => {
                let (back, forward) = navigation_history(&session);
                let page = match action.action_kind {
                    ActionKind::Back => {
                        if back.is_empty() {
                            return Ok((
                                session,
                                fail_action(
                                    action,
                                    FailureClass::NavigationFailure,
                                    "back action requires prior page history",
                                ),
                                Vec::new(),
                            ));
                        }
                        back.last().cloned()
                    }
                    _ => {
                        if forward.is_empty() {
                            return Ok((
                                session,
                                fail_action(
                                    action,
                                    FailureClass::NavigationFailure,
                                    "forward action requires forward page history",
                                ),
                                Vec::new(),
                            ));
                        }
                        forward.last().cloned()
                    }
                };
                action.page_after = page.clone();
                session.current_page = page.clone();
                session.trusted_page_scope = page
                    .as_ref()
                    .map(|p| next_trusted_scope(&session, &action.computer_use_action_id, p));
            }
            ActionKind::CloseSession => {
                let closed = self.close_session(session.clone())?;
                session = closed;
                action.page_after = clone_page(session.current_page.as_ref());
            }
        }

        let completed_at = Utc::now();
        action.status = ActionStatus::Completed;
        action.updated_at = completed_at;
        action.completed_at = Some(completed_at);
        session.last_action_id = action.computer_use_action_id.clone();
        if action.action_kind != ActionKind::CloseSession {
            session.status = SessionStatus::Active;
        }
        session.updated_at = completed_at;
        Ok((session, action, captures))
    }
}

pub(crate) fn build_page_evidence_capture(
    session: &Session,
    action: &Action,
    kind: ActionKind,
) -> ArtifactCaptureRequest {
    let mut artifact_kind = ArtifactKind::PageSnapshot;
    let mut mime_type = "application/json";
    let mut file_name = "page-snapshot.json";
    if kind == ActionKind::Screenshot {
        artifact_kind = ArtifactKind::Screenshot;
        mime_type = "text/plain";
        file_name = "screenshot.txt";
    }
    let content = if artifact_kind == ArtifactKind::Screenshot {
        format!(
            "screenshot placeholder for {} ({})",
            first_page_field(
                action.page_after.as_ref(),
                action.page_before.as_ref(),
                "url"
            ),
            action.action_kind.as_str()
        )
    } else {
        let json = serde_json::json!({
            "sessionId": &session.computer_use_session_id,
            "actionId": &action.computer_use_action_id,
            "actionKind": action.action_kind.as_str(),
            "url": first_page_field(action.page_after.as_ref(), action.page_before.as_ref(), "url"),
            "title": first_page_field(action.page_after.as_ref(), action.page_before.as_ref(), "title"),
            "value": input_string(&action.input, "value"),
            "selectedValue": input_string(&action.input, "selectedValue"),
        });
        let mut s = serde_json::to_string(&json).unwrap_or_default();
        s.push(char::from(10));
        s
    };
    ArtifactCaptureRequest {
        run_id: session.run_id.clone(),
        computer_use_session_id: session.computer_use_session_id.clone(),
        computer_use_action_id: action.computer_use_action_id.clone(),
        kind: artifact_kind,
        mime_type: mime_type.to_string(),
        file_name: file_name.to_string(),
        content: content.into_bytes(),
        estimated_byte_size: 0,
    }
}

fn navigation_current_page(session: &Session) -> Option<PageSummary> {
    let (current, _, _) = navigation_state(session);
    current
}

fn navigation_history(session: &Session) -> (Vec<PageSummary>, Vec<PageSummary>) {
    let (_, back, forward) = navigation_state(session);
    (back, forward)
}

fn navigation_state(
    session: &Session,
) -> (Option<PageSummary>, Vec<PageSummary>, Vec<PageSummary>) {
    let mut current: Option<PageSummary> =
        session.actions.iter().find_map(|a| a.page_before.clone());
    if current.is_none() {
        current = clone_page(session.current_page.as_ref());
    }
    let mut back: Vec<PageSummary> = Vec::new();
    let mut forward: Vec<PageSummary> = Vec::new();
    for prior in &session.actions {
        if prior.status != ActionStatus::Completed {
            continue;
        }
        match prior.action_kind {
            ActionKind::Navigate => {
                if let Some(after) = &prior.page_after {
                    if let Some(cur) = &current {
                        if !same_page(Some(cur), Some(after)) {
                            back.push(cur.clone());
                        }
                    }
                    current = Some(after.clone());
                    forward.clear();
                }
            }
            ActionKind::Back => {
                if let Some(prev) = back.pop() {
                    if let Some(cur) = &current {
                        forward.push(cur.clone());
                    }
                    current = Some(prev);
                }
            }
            ActionKind::Forward => {
                if let Some(next) = forward.pop() {
                    if let Some(cur) = &current {
                        back.push(cur.clone());
                    }
                    current = Some(next);
                }
            }
            ActionKind::Select => {
                if let Some(after) = &prior.page_after {
                    current = Some(after.clone());
                }
            }
            _ => {}
        }
    }
    (current, back, forward)
}

fn same_page(left: Option<&PageSummary>, right: Option<&PageSummary>) -> bool {
    match (left, right) {
        (Some(l), Some(r)) => l.url == r.url && l.title == r.title,
        (None, None) => true,
        _ => false,
    }
}

fn apply_selection_state(page: Option<&PageSummary>, action: &Action) -> Option<PageSummary> {
    let mut next = page.cloned().unwrap_or_default();
    let mut label = input_string(&action.input, "selectedValue");
    if label.is_empty() {
        label = "selected".to_string();
    }
    let selector = selector_from_target(action.target_match_context.as_ref());
    if !selector.is_empty() {
        next.title = format!(
            "{} [{selector}={label}]",
            first_non_empty(&[&next.title, &next.url, "page"])
        )
        .trim()
        .to_string();
    } else {
        next.title = format!(
            "{} [selected={label}]",
            first_non_empty(&[&next.title, &next.url, "page"])
        )
        .trim()
        .to_string();
    }
    Some(next)
}

fn selector_from_target(target: Option<&TargetMatchContext>) -> String {
    target
        .map(|t| t.expected_selector.trim().to_string())
        .unwrap_or_default()
}

fn input_string(input: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn download_file_name(action: &Action) -> String {
    let mut base = first_page_field(
        action.page_after.as_ref(),
        action.page_before.as_ref(),
        "title",
    );
    if base.is_empty() {
        base = "computer-use-download".to_string();
    }
    base = base.to_lowercase().replace(' ', "-");
    base = base.trim_matches('-').to_string();
    if base.is_empty() {
        base = "computer-use-download".to_string();
    }
    let base_name = base.rsplit('/').next().unwrap_or(&base).to_string();
    format!("{base_name}.txt")
}

fn download_artifact_content(action: &Action) -> String {
    format!(
        "download artifact
url={}
title={}
action={}
selector={}
",
        first_page_field(
            action.page_after.as_ref(),
            action.page_before.as_ref(),
            "url"
        ),
        first_page_field(
            action.page_after.as_ref(),
            action.page_before.as_ref(),
            "title"
        ),
        action.action_kind.as_str(),
        selector_from_target(action.target_match_context.as_ref()),
    )
}

fn next_trusted_scope(session: &Session, action_id: &str, page: &PageSummary) -> TrustedPageScope {
    let revision = session
        .trusted_page_scope
        .as_ref()
        .map(|s| s.scope_revision + 1)
        .unwrap_or(1);
    let now = Utc::now();
    TrustedPageScope {
        scope_id: format!("cuscope_{}", now.timestamp_nanos_opt().unwrap_or(0)),
        computer_use_session_id: session.computer_use_session_id.clone(),
        origin: origin_from_url(&page.url),
        page_url: page.url.clone(),
        title: page.title.clone(),
        scope_revision: revision,
        derived_from_action_id: action_id.to_string(),
        created_at: now,
        ..TrustedPageScope::default()
    }
}

fn mismatched(target: Option<&TargetMatchContext>) -> bool {
    let Some(target) = target else { return false };
    let expected_selector = target.expected_selector.trim().to_lowercase();
    let expected_text = target.expected_text.trim().to_lowercase();
    let expected_url = target.expected_page_url.trim().to_lowercase();
    expected_selector.contains("missing")
        || expected_text.contains("missing")
        || expected_url.contains("missing")
}

fn fail_action(mut action: Action, class: FailureClass, reason: &str) -> Action {
    let now = Utc::now();
    action.status = ActionStatus::Failed;
    action.failure_class = class.as_str().to_string();
    action.failure_reason = reason.to_string();
    action.updated_at = now;
    action.completed_at = Some(now);
    if action.target_match_context.is_some() && class == FailureClass::TargetMismatch {
        if let Some(ctx) = action.target_match_context.as_mut() {
            ctx.match_result = Some(MatchResult::Mismatched);
            ctx.evaluated_at = Some(now);
        }
    }
    action
}

fn title_from_url(raw: &str) -> String {
    match url::Url::parse(raw.trim()) {
        Ok(parsed) => {
            if let Some(host) = parsed.host_str() {
                if !host.is_empty() {
                    return host.to_string();
                }
            }
            if !parsed.path().is_empty() {
                return parsed.path().to_string();
            }
            raw.trim().to_string()
        }
        Err(_) => raw.trim().to_string(),
    }
}

fn origin_from_url(raw: &str) -> String {
    match url::Url::parse(raw.trim()) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            if let Some(host) = parsed.host_str() {
                if !scheme.is_empty() && !host.is_empty() {
                    return format!("{scheme}://{host}");
                }
            }
            String::new()
        }
        Err(_) => String::new(),
    }
}

fn clone_page(page: Option<&PageSummary>) -> Option<PageSummary> {
    page.cloned()
}

fn first_page_field(
    after: Option<&PageSummary>,
    before: Option<&PageSummary>,
    field: &str,
) -> String {
    match field {
        "url" => {
            if let Some(a) = after {
                if !a.url.is_empty() {
                    return a.url.clone();
                }
            }
            if let Some(b) = before {
                return b.url.clone();
            }
        }
        "title" => {
            if let Some(a) = after {
                if !a.title.is_empty() {
                    return a.title.clone();
                }
            }
            if let Some(b) = before {
                return b.title.clone();
            }
        }
        _ => {}
    }
    String::new()
}

#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}
