use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::error::ThreadsError;
use crate::redaction::RedactionStatus;
use crate::redaction::safe_continuity_content;
use crate::redaction::safe_summary;
use crate::source::SourceKind;
use crate::utc_now_or;

pub const DEFAULT_CONTINUITY_MAX_PRIOR_TURNS: i32 = 12;
pub const DEFAULT_CONTINUITY_ACTIVE_DAYS: i64 = 30;
pub const DEFAULT_CONTINUITY_WINDOW_POLICY_ID: &str = "default_recent_12_30d";
pub const CONTINUITY_ORDER_DAEMON_SEQUENCE: &str = "daemon_acceptance_sequence";

/// Go: `ContinuityMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityMode {
    Auto,
    Disabled,
}

/// Go: `ContinuityRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityRole {
    User,
    Assistant,
}

/// Go: `ContinuityStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityStatus {
    Applied,
    Empty,
    Disabled,
    Blocked,
    Partial,
    Failed,
}

/// Go: `ContinuityDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityDecision {
    Included,
    Excluded,
}

/// Go: `ContinuityItemKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityItemKind {
    Turn,
    ArtifactExcerpt,
    HandoffSourceReference,
}

/// Go: `ContinuityReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityReason {
    IncludedRecent,
    NoEligibleTurns,
    OverLimit,
    TooOld,
    ResetBoundary,
    LifecycleBlocked,
    SourceMismatch,
    PermissionDenied,
    RedactionFailed,
    RetentionExpired,
    DuplicateSourceEvent,
    IncompleteEvidence,
    UnsupportedSource,
    ArtifactReferenceOnly,
    ContinuityDisabled,
    ContinuityUnavailable,
}

/// Go: `RuntimeArtifactExcerpt`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeArtifactExcerpt {
    pub artifact_excerpt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_turn_id: String,
    pub resource_kind: String,
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub excerpt_text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub excerpt_source: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// One recorded conversation turn eligible for continuity. Go: `ContinuityTurn`.
///
/// `retention_expires_at: None` maps to Go's zero `time.Time`; note the two
/// call sites treat zero differently (see `eligible_continuity_turns` and
/// `build_handoff_source_references`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityTurn {
    pub continuity_turn_id: String,
    pub tenant_id: String,
    pub thread_id: String,
    pub session_segment_id: String,
    pub acceptance_sequence: i64,
    pub role: ContinuityRole,
    pub source_kind: SourceKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_linkage_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_timestamp: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dispatch_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_to_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub safe_content: String,
    pub content_redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_excerpt_refs: Vec<RuntimeArtifactExcerpt>,
    pub recorded_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_event_key: String,
}

/// Go: `ContinuityWindowPolicy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityWindowPolicy {
    pub window_policy_id: String,
    pub max_prior_turns: i32,
    pub active_window_days: i64,
    pub ordered_by: String,
}

/// Go: `ContinuityPreview`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityPreview {
    pub continuity_preview_id: String,
    pub tenant_id: String,
    pub thread_id: String,
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dispatch_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_turn_id: String,
    pub window_policy_id: String,
    pub max_prior_turns: i32,
    pub active_window_days: i64,
    pub included_count: i32,
    pub excluded_count: i32,
    pub continuity_applied: bool,
    pub status: ContinuityStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    pub assembly_started_at: DateTime<Utc>,
    pub assembly_completed_at: DateTime<Utc>,
    pub assembly_duration_ms: i64,
    pub retention_expires_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

/// Go: `ContinuityPreviewItem`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityPreviewItem {
    pub preview_item_id: String,
    pub continuity_preview_id: String,
    pub tenant_id: String,
    pub thread_id: String,
    pub item_kind: ContinuityItemKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<ContinuityRole>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artifact_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artifact_excerpt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handoff_source_reference_id: String,
    pub decision: ContinuityDecision,
    pub reason_code: ContinuityReason,
    #[serde(default)]
    pub acceptance_sequence: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_timestamp: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
    pub redaction_status: RedactionStatus,
    pub item_order: i32,
}

/// Go: `ContinuityPreviewDetail`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuityPreviewDetail {
    pub preview: ContinuityPreview,
    pub items: Vec<ContinuityPreviewItem>,
}

/// Go: `DefaultContinuityPolicy`.
pub fn default_continuity_policy() -> ContinuityWindowPolicy {
    ContinuityWindowPolicy {
        window_policy_id: DEFAULT_CONTINUITY_WINDOW_POLICY_ID.to_string(),
        max_prior_turns: DEFAULT_CONTINUITY_MAX_PRIOR_TURNS,
        active_window_days: DEFAULT_CONTINUITY_ACTIVE_DAYS,
        ordered_by: CONTINUITY_ORDER_DAEMON_SEQUENCE.to_string(),
    }
}

/// Go: `NormalizeContinuityMode` — blank defaults to auto, only the explicit
/// disabled value disables continuity.
pub fn normalize_continuity_mode(mode: Option<ContinuityMode>) -> ContinuityMode {
    match mode {
        Some(ContinuityMode::Disabled) => ContinuityMode::Disabled,
        _ => ContinuityMode::Auto,
    }
}

/// Go: `ValidateContinuityTurn`. Role and redaction-status validity are
/// enforced by the Rust types, so only the identity fields need checking.
pub fn validate_continuity_turn(turn: &ContinuityTurn) -> Result<(), ThreadsError> {
    if turn.tenant_id.trim().is_empty()
        || turn.thread_id.trim().is_empty()
        || turn.session_segment_id.trim().is_empty()
    {
        return Err(ThreadsError::ContinuityTurnMissingIdentity);
    }
    Ok(())
}

/// Go: `EligibleContinuityTurns`. Returns the included window (sorted by
/// acceptance sequence, newest-biased to `max_prior_turns`) plus exclusion
/// preview items for every rejected turn.
///
/// A turn without `retention_expires_at` (Go zero time) compares before `now`
/// and is excluded as `retention_expired`, matching Go semantics.
pub fn eligible_continuity_turns(
    turns: &[ContinuityTurn],
    policy: &ContinuityWindowPolicy,
    now: Option<DateTime<Utc>>,
) -> (Vec<ContinuityTurn>, Vec<ContinuityPreviewItem>) {
    let default_policy;
    let policy = if policy.max_prior_turns <= 0 {
        default_policy = default_continuity_policy();
        &default_policy
    } else {
        policy
    };
    let now = utc_now_or(now);
    let cutoff = now - Duration::days(policy.active_window_days);
    let mut eligible: Vec<&ContinuityTurn> = Vec::with_capacity(turns.len());
    let mut excluded: Vec<ContinuityPreviewItem> = Vec::new();
    for turn in turns {
        let reason = match turn.retention_expires_at {
            // Go: zero time is Before(now) → retention_expired.
            None => Some(ContinuityReason::RetentionExpired),
            Some(expires) if expires < now => Some(ContinuityReason::RetentionExpired),
            _ if turn.recorded_at < cutoff => Some(ContinuityReason::TooOld),
            _ if matches!(
                turn.content_redaction_status,
                RedactionStatus::RedactionFailed | RedactionStatus::Suppressed
            ) =>
            {
                Some(ContinuityReason::RedactionFailed)
            }
            _ => None,
        };
        if let Some(reason) = reason {
            excluded.push(preview_item_for_turn(
                turn,
                ContinuityDecision::Excluded,
                reason,
                excluded.len() as i32,
            ));
            continue;
        }
        eligible.push(turn);
    }
    eligible.sort_by_key(|turn| turn.acceptance_sequence);
    if eligible.len() as i32 > policy.max_prior_turns {
        let over = eligible.len() - policy.max_prior_turns as usize;
        for turn in &eligible[..over] {
            excluded.push(preview_item_for_turn(
                turn,
                ContinuityDecision::Excluded,
                ContinuityReason::OverLimit,
                excluded.len() as i32,
            ));
        }
        eligible = eligible.split_off(over);
    }
    (eligible.into_iter().cloned().collect(), excluded)
}

/// Go: `PreviewItemForTurn`.
pub fn preview_item_for_turn(
    turn: &ContinuityTurn,
    decision: ContinuityDecision,
    reason: ContinuityReason,
    order: i32,
) -> ContinuityPreviewItem {
    let summary = safe_summary(
        &turn.safe_content,
        turn.content_redaction_status == RedactionStatus::Redacted,
    );
    ContinuityPreviewItem {
        preview_item_id: String::new(),
        continuity_preview_id: String::new(),
        tenant_id: turn.tenant_id.clone(),
        thread_id: turn.thread_id.clone(),
        item_kind: ContinuityItemKind::Turn,
        continuity_turn_id: turn.continuity_turn_id.clone(),
        role: Some(turn.role),
        artifact_ref: String::new(),
        artifact_excerpt_id: String::new(),
        handoff_source_reference_id: String::new(),
        decision,
        reason_code: reason,
        acceptance_sequence: turn.acceptance_sequence,
        source_timestamp: turn.source_timestamp,
        safe_summary: summary.text,
        redaction_status: summary.status,
        item_order: order,
    }
}

/// Go: `ResetBoundaryPreviewItems` — pre-reset turns are always excluded at
/// the reset boundary.
pub fn reset_boundary_preview_items(
    turns: &[ContinuityTurn],
    start_order: i32,
) -> Vec<ContinuityPreviewItem> {
    turns
        .iter()
        .enumerate()
        .map(|(index, turn)| {
            preview_item_for_turn(
                turn,
                ContinuityDecision::Excluded,
                ContinuityReason::ResetBoundary,
                start_order + index as i32,
            )
        })
        .collect()
}

/// Go: `PreviewItemsForArtifactExcerpts` — artifact excerpts are reference-only
/// unless they carry fresh, redacted, non-empty text.
pub fn preview_items_for_artifact_excerpts(
    turn: &ContinuityTurn,
    start_order: i32,
    now: Option<DateTime<Utc>>,
) -> Vec<ContinuityPreviewItem> {
    let now = utc_now_or(now);
    let mut items = Vec::with_capacity(turn.artifact_excerpt_refs.len());
    for excerpt in &turn.artifact_excerpt_refs {
        let mut item = ContinuityPreviewItem {
            preview_item_id: String::new(),
            continuity_preview_id: String::new(),
            tenant_id: turn.tenant_id.clone(),
            thread_id: turn.thread_id.clone(),
            item_kind: ContinuityItemKind::ArtifactExcerpt,
            continuity_turn_id: turn.continuity_turn_id.clone(),
            role: None,
            artifact_ref: format!(
                "{}/{}",
                excerpt.resource_kind.trim(),
                excerpt.resource_id.trim()
            )
            .trim()
            .to_string(),
            artifact_excerpt_id: excerpt.artifact_excerpt_id.clone(),
            handoff_source_reference_id: String::new(),
            decision: ContinuityDecision::Excluded,
            reason_code: ContinuityReason::ArtifactReferenceOnly,
            acceptance_sequence: turn.acceptance_sequence,
            source_timestamp: turn.source_timestamp,
            safe_summary: "suppressed".to_string(),
            redaction_status: RedactionStatus::Suppressed,
            item_order: start_order + items.len() as i32,
        };
        if let Some(expires) = excerpt.retention_expires_at {
            if expires < now {
                item.reason_code = ContinuityReason::RetentionExpired;
                items.push(item);
                continue;
            }
        }
        if excerpt.redaction_status != RedactionStatus::Redacted {
            item.reason_code = ContinuityReason::RedactionFailed;
        } else if excerpt.excerpt_text.trim().is_empty() {
            item.reason_code = ContinuityReason::IncompleteEvidence;
        } else {
            let summary = safe_continuity_content(&excerpt.excerpt_text);
            if summary.status != RedactionStatus::Redacted {
                item.reason_code = ContinuityReason::RedactionFailed;
            } else {
                item.decision = ContinuityDecision::Included;
                item.reason_code = ContinuityReason::IncludedRecent;
            }
            item.safe_summary = summary.text;
            item.redaction_status = summary.status;
        }
        items.push(item);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;

    fn test_turn(
        id: &str,
        seq: i64,
        recorded_at: DateTime<Utc>,
        retention_expires_at: Option<DateTime<Utc>>,
        status: RedactionStatus,
    ) -> ContinuityTurn {
        ContinuityTurn {
            continuity_turn_id: id.to_string(),
            tenant_id: "ten_1".to_string(),
            thread_id: "thr_1".to_string(),
            session_segment_id: "seg_1".to_string(),
            acceptance_sequence: seq,
            role: ContinuityRole::User,
            source_kind: SourceKind::Chat,
            source_linkage_id: String::new(),
            source_message_id: String::new(),
            source_timestamp: None,
            dispatch_id: String::new(),
            response_to_turn_id: String::new(),
            safe_content: id.to_string(),
            content_redaction_status: status,
            artifact_excerpt_refs: Vec::new(),
            recorded_at,
            retention_expires_at,
            source_event_key: String::new(),
        }
    }

    // Port of TestContinuityPolicySelectsDefaultWindowByAcceptanceSequence.
    #[test]
    fn policy_selects_default_window_by_acceptance_sequence() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let turns: Vec<ContinuityTurn> = (1..=14_i64)
            .map(|i| {
                test_turn(
                    "turn",
                    i,
                    now + Duration::minutes(i),
                    Some(now + Duration::days(90)),
                    RedactionStatus::Redacted,
                )
            })
            .collect();
        let (included, excluded) =
            eligible_continuity_turns(&turns, &default_continuity_policy(), Some(now));
        assert_eq!(included.len() as i32, DEFAULT_CONTINUITY_MAX_PRIOR_TURNS);
        assert_eq!(included.first().map(|t| t.acceptance_sequence), Some(3));
        assert_eq!(included.last().map(|t| t.acceptance_sequence), Some(14));
        assert_eq!(excluded.len(), 2);
        assert_eq!(excluded[0].reason_code, ContinuityReason::OverLimit);
    }

    // Port of TestContinuityPolicyExcludesAgeRetentionAndUnsafeRedaction.
    #[test]
    fn policy_excludes_age_retention_and_unsafe_redaction() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let turns = vec![
            test_turn(
                "old",
                1,
                now - Duration::days(31),
                Some(now + Duration::days(90)),
                RedactionStatus::Redacted,
            ),
            test_turn(
                "expired",
                2,
                now,
                Some(now - Duration::hours(1)),
                RedactionStatus::Redacted,
            ),
            test_turn(
                "unsafe",
                3,
                now,
                Some(now + Duration::days(90)),
                RedactionStatus::RedactionFailed,
            ),
            test_turn(
                "ok",
                4,
                now,
                Some(now + Duration::days(90)),
                RedactionStatus::Redacted,
            ),
        ];
        let (included, excluded) =
            eligible_continuity_turns(&turns, &default_continuity_policy(), Some(now));
        assert_eq!(included.len(), 1);
        assert_eq!(included[0].continuity_turn_id, "ok");
        for reason in [
            ContinuityReason::TooOld,
            ContinuityReason::RetentionExpired,
            ContinuityReason::RedactionFailed,
        ] {
            assert!(
                excluded.iter().any(|item| item.reason_code == reason),
                "missing exclusion reason {reason:?} in {excluded:?}"
            );
        }
    }

    // Port of TestResetBoundaryPreviewItemsExcludePreResetTurns.
    #[test]
    fn reset_boundary_preview_items_exclude_pre_reset_turns() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let items = reset_boundary_preview_items(
            &[
                test_turn(
                    "pre_reset_1",
                    1,
                    now,
                    Some(now + Duration::days(90)),
                    RedactionStatus::Redacted,
                ),
                test_turn(
                    "pre_reset_2",
                    2,
                    now,
                    Some(now + Duration::days(90)),
                    RedactionStatus::Redacted,
                ),
            ],
            3,
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].decision, ContinuityDecision::Excluded);
        assert_eq!(items[0].reason_code, ContinuityReason::ResetBoundary);
        assert_eq!(items[0].item_order, 3);
        assert_eq!(items[1].continuity_turn_id, "pre_reset_2");
        assert_eq!(items[1].item_order, 4);
    }
}
