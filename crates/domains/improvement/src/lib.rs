//! kura-improvement — audited self-improvement proposals.
//!
//! The closed loop the operator can audit and veto: the agent proposes a
//! **bounded** change (slice 1 target: one plugin-profile config value),
//! carrying motivating evidence, the concrete current→proposed diff, the
//! predicted effect, and — recorded at apply time — the full prior profile
//! as the rollback path. Nothing applies without operator approval; nothing
//! applies without a recorded rollback; proposals are rate-bounded per
//! target per window, and the bound is operator configuration the agent
//! cannot adjust.
//!
//! Proposals persist as white-box JSON files under
//! `<data_dir>/improvement/` (one file per proposal, restored at build), so
//! the audit chain survives restarts and stays operator-readable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// One evidence citation (kind + id mirror the memory plane's source-link
/// vocabulary without depending on it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvidenceLink {
    pub kind: String,
    pub id: String,
}

/// A measured result backing a proposal (Stage 6.2). "Predicted effect" is
/// an assertion; this is the observation it rests on: a replay attempt or
/// campaign in the evaluation plane, the metric read from it, and the
/// baseline it is compared against.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvaluationEvidence {
    /// `replay_attempt` or `campaign`.
    pub kind: String,
    /// The attempt/campaign id in the evaluation plane.
    pub id: String,
    pub metric: String,
    pub baseline: f64,
    pub observed: f64,
}

impl EvaluationEvidence {
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.kind.trim().is_empty() && !self.id.trim().is_empty() && !self.metric.trim().is_empty()
    }
}

/// The parameters a proposal may target (Stage 6.1): every tunable this
/// program introduced, and nothing else. An allowlist rather than "any
/// profile key" because `apply` rewrites `plugins.json` for the whole daemon
/// — an agent proposing a change to a key it invented would otherwise land a
/// silent no-op or, worse, a typo that disables a plugin.
pub const KNOWN_TARGETS: &[(&str, &str)] = &[
    ("context", "memoryBudgetChars"),
    ("context", "retrievalBudgetChars"),
    ("context", "refThresholdChars"),
    ("context", "retrievalMaxCorpus"),
    ("context", "vectorMinSimilarity"),
    ("session-strategy", "personalBudgetChars"),
    ("session-strategy", "threadBudgetChars"),
    ("session-strategy", "keepRecent"),
];

#[must_use]
pub fn is_known_target(plugin: &str, key: &str) -> bool {
    KNOWN_TARGETS
        .iter()
        .any(|(p, k)| *p == plugin.trim() && *k == key.trim())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Applied,
    Rejected,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImprovementProposal {
    pub proposal_id: String,
    /// The plugin whose profile entry changes (e.g. `session-strategy`).
    pub target_plugin: String,
    /// The config key inside the entry (e.g. `personalBudgetChars`).
    pub config_key: String,
    pub current_value: serde_json::Value,
    pub proposed_value: serde_json::Value,
    pub predicted_effect: String,
    pub evidence_links: Vec<EvidenceLink>,
    /// The measurement the predicted effect rests on (Stage 6.2).
    #[serde(default)]
    pub evaluation: EvaluationEvidence,
    pub proposed_by: String,
    pub status: ProposalStatus,
    /// Decision reason (rejection) or rollback reason.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The full plugin profile before apply — the recorded rollback path.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub prior_profile: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input to [`Manager::propose`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProposeInput {
    pub target_plugin: String,
    pub config_key: String,
    pub current_value: serde_json::Value,
    pub proposed_value: serde_json::Value,
    pub predicted_effect: String,
    pub evidence_links: Vec<EvidenceLink>,
    pub proposed_by: String,
    pub evaluation: EvaluationEvidence,
}

/// Operator configuration (the `config` object of the `self-improve`
/// profile entry). The rate bound is deliberately NOT agent-adjustable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImprovementConfig {
    /// Max proposals per (plugin, key) target per 24h window; 0 = default.
    pub max_per_target_per_day: usize,
}

pub const DEFAULT_MAX_PER_TARGET_PER_DAY: usize = 3;

impl ImprovementConfig {
    #[must_use]
    pub fn bound(&self) -> usize {
        if self.max_per_target_per_day > 0 {
            self.max_per_target_per_day
        } else {
            DEFAULT_MAX_PER_TARGET_PER_DAY
        }
    }
}

#[derive(Debug)]
pub enum ImprovementError {
    NotFound,
    EvidenceRequired,
    EvaluationRequired,
    UnknownTarget(String),
    NonNumericValue(String),
    TargetRequired,
    RateBounded(String),
    InvalidTransition(String),
    Io(String),
}
impl std::fmt::Display for ImprovementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImprovementError::NotFound => write!(f, "improvement proposal not found"),
            ImprovementError::EvidenceRequired => {
                write!(f, "improvement proposals require motivating evidence links")
            }
            ImprovementError::EvaluationRequired => write!(
                f,
                "improvement proposals require an evaluation result (kind, id, metric) — a predicted effect must be measured, not asserted"
            ),
            ImprovementError::UnknownTarget(t) => {
                write!(f, "{t} is not a known improvement target")
            }
            ImprovementError::NonNumericValue(t) => write!(f, "{t} takes a numeric value"),
            ImprovementError::TargetRequired => {
                write!(f, "targetPlugin and configKey are required")
            }
            ImprovementError::RateBounded(target) => write!(
                f,
                "rate bound reached for target {target}; the bound is operator configuration"
            ),
            ImprovementError::InvalidTransition(msg) => write!(f, "{msg}"),
            ImprovementError::Io(msg) => write!(f, "improvement persistence: {msg}"),
        }
    }
}
impl std::error::Error for ImprovementError {}

/// Proposal registry with white-box JSON persistence.
pub struct Manager {
    dir: PathBuf,
    config: ImprovementConfig,
    inner: parking_lot::RwLock<HashMap<String, ImprovementProposal>>,
}

impl Manager {
    /// Builds the manager, restoring persisted proposals from
    /// `<data_dir>/improvement/`.
    pub fn new(data_dir: &str, config: ImprovementConfig) -> Self {
        let dir = Path::new(data_dir).join("improvement");
        let mut map = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(raw) = std::fs::read(entry.path()) {
                    if let Ok(proposal) = serde_json::from_slice::<ImprovementProposal>(&raw) {
                        map.insert(proposal.proposal_id.clone(), proposal);
                    }
                }
            }
        }
        Manager {
            dir,
            config,
            inner: parking_lot::RwLock::new(map),
        }
    }

    fn persist(&self, proposal: &ImprovementProposal) -> Result<(), ImprovementError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| ImprovementError::Io(e.to_string()))?;
        let path = self.dir.join(format!("{}.json", proposal.proposal_id));
        let tmp = self.dir.join(format!("{}.json.tmp", proposal.proposal_id));
        let encoded =
            serde_json::to_vec_pretty(proposal).map_err(|e| ImprovementError::Io(e.to_string()))?;
        std::fs::write(&tmp, encoded)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|e| ImprovementError::Io(e.to_string()))
    }

    /// Creates a pending proposal, enforcing evidence and the per-target
    /// rate bound over the trailing 24h window.
    pub fn propose(&self, input: ProposeInput) -> Result<ImprovementProposal, ImprovementError> {
        if input.target_plugin.trim().is_empty() || input.config_key.trim().is_empty() {
            return Err(ImprovementError::TargetRequired);
        }
        if input.evidence_links.iter().all(|l| l.id.trim().is_empty()) {
            return Err(ImprovementError::EvidenceRequired);
        }
        let target_label = format!("{}/{}", input.target_plugin.trim(), input.config_key.trim());
        if !is_known_target(&input.target_plugin, &input.config_key) {
            return Err(ImprovementError::UnknownTarget(target_label));
        }
        if !input.proposed_value.is_number() {
            return Err(ImprovementError::NonNumericValue(target_label));
        }
        if !input.evaluation.is_usable() {
            return Err(ImprovementError::EvaluationRequired);
        }
        let now = Utc::now();
        let target = format!("{}/{}", input.target_plugin.trim(), input.config_key.trim());
        {
            let inner = self.inner.read();
            let window_start = now - Duration::hours(24);
            let recent = inner
                .values()
                .filter(|p| {
                    p.target_plugin == input.target_plugin.trim()
                        && p.config_key == input.config_key.trim()
                        && p.created_at >= window_start
                })
                .count();
            if recent >= self.config.bound() {
                return Err(ImprovementError::RateBounded(target));
            }
        }
        let proposal = ImprovementProposal {
            proposal_id: format!("imp_{}", uuid::Uuid::now_v7().simple()),
            target_plugin: input.target_plugin.trim().to_string(),
            config_key: input.config_key.trim().to_string(),
            current_value: input.current_value,
            proposed_value: input.proposed_value,
            predicted_effect: input.predicted_effect.trim().to_string(),
            evidence_links: input.evidence_links,
            evaluation: input.evaluation,
            proposed_by: input.proposed_by.trim().to_string(),
            status: ProposalStatus::Pending,
            reason: String::new(),
            prior_profile: serde_json::Value::Null,
            created_at: now,
            updated_at: now,
        };
        self.persist(&proposal)?;
        self.inner
            .write()
            .insert(proposal.proposal_id.clone(), proposal.clone());
        Ok(proposal)
    }

    #[must_use]
    pub fn get(&self, proposal_id: &str) -> Option<ImprovementProposal> {
        self.inner.read().get(proposal_id.trim()).cloned()
    }

    /// Newest first.
    #[must_use]
    pub fn list(&self) -> Vec<ImprovementProposal> {
        let mut items: Vec<_> = self.inner.read().values().cloned().collect();
        items.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then(a.proposal_id.cmp(&b.proposal_id))
        });
        items
    }

    fn transition(
        &self,
        proposal_id: &str,
        expected: ProposalStatus,
        mutate: impl FnOnce(&mut ImprovementProposal),
    ) -> Result<ImprovementProposal, ImprovementError> {
        let mut inner = self.inner.write();
        let proposal = inner
            .get_mut(proposal_id.trim())
            .ok_or(ImprovementError::NotFound)?;
        if proposal.status != expected {
            return Err(ImprovementError::InvalidTransition(format!(
                "proposal is {:?}; expected {:?}",
                proposal.status, expected
            )));
        }
        mutate(proposal);
        proposal.updated_at = Utc::now();
        let snapshot = proposal.clone();
        drop(inner);
        self.persist(&snapshot)?;
        Ok(snapshot)
    }

    /// Records an applied change with its rollback snapshot (the full prior
    /// profile). No change applies without one.
    pub fn mark_applied(
        &self,
        proposal_id: &str,
        prior_profile: serde_json::Value,
    ) -> Result<ImprovementProposal, ImprovementError> {
        if prior_profile.is_null() {
            return Err(ImprovementError::InvalidTransition(
                "apply requires the prior-profile rollback snapshot".to_string(),
            ));
        }
        self.transition(proposal_id, ProposalStatus::Pending, |p| {
            p.status = ProposalStatus::Applied;
            p.prior_profile = prior_profile;
        })
    }

    pub fn reject(
        &self,
        proposal_id: &str,
        reason: &str,
    ) -> Result<ImprovementProposal, ImprovementError> {
        self.transition(proposal_id, ProposalStatus::Pending, |p| {
            p.status = ProposalStatus::Rejected;
            p.reason = reason.trim().to_string();
        })
    }

    pub fn mark_rolled_back(
        &self,
        proposal_id: &str,
        reason: &str,
    ) -> Result<ImprovementProposal, ImprovementError> {
        self.transition(proposal_id, ProposalStatus::Applied, |p| {
            p.status = ProposalStatus::RolledBack;
            p.reason = reason.trim().to_string();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kura-improvement-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.to_string_lossy().into_owned()
    }

    fn input(key: &str) -> ProposeInput {
        ProposeInput {
            target_plugin: "session-strategy".to_string(),
            config_key: key.to_string(),
            current_value: serde_json::json!(48000),
            proposed_value: serde_json::json!(64000),
            predicted_effect: "fewer elisions in long sessions".to_string(),
            evidence_links: vec![EvidenceLink {
                kind: "event".to_string(),
                id: "evt_1".to_string(),
            }],
            proposed_by: "agent".to_string(),
            evaluation: EvaluationEvidence {
                kind: "replay_attempt".into(),
                id: "att_1".into(),
                metric: "elisions_per_turn".into(),
                baseline: 3.0,
                observed: 1.0,
            },
        }
    }

    #[test]
    fn unknown_targets_and_unmeasured_proposals_are_refused() {
        let dir = std::env::temp_dir().join(format!("kura-improve-{}", uuid::Uuid::now_v7()));
        let m = Manager::new(dir.to_str().unwrap(), ImprovementConfig::default());
        let mut bad = input("personalBudgetChars");
        bad.config_key = "personalBudgetChras".into();
        assert!(matches!(
            m.propose(bad).unwrap_err(),
            ImprovementError::UnknownTarget(_)
        ));
        let mut unmeasured = input("personalBudgetChars");
        unmeasured.evaluation = EvaluationEvidence::default();
        assert!(matches!(
            m.propose(unmeasured).unwrap_err(),
            ImprovementError::EvaluationRequired
        ));
        let mut text = input("personalBudgetChars");
        text.proposed_value = serde_json::json!("lots");
        assert!(matches!(
            m.propose(text).unwrap_err(),
            ImprovementError::NonNumericValue(_)
        ));
        assert!(m.propose(input("keepRecent")).is_ok());
    }

    #[test]
    fn lifecycle_apply_requires_snapshot_and_persists_across_restart() {
        let dir = tempdir();
        let manager = Manager::new(&dir, ImprovementConfig::default());
        let proposal = manager
            .propose(input("personalBudgetChars"))
            .expect("propose");
        assert_eq!(proposal.status, ProposalStatus::Pending);

        // Apply without a snapshot is refused (no change without rollback).
        assert!(
            manager
                .mark_applied(&proposal.proposal_id, serde_json::Value::Null)
                .is_err()
        );
        let applied = manager
            .mark_applied(&proposal.proposal_id, serde_json::json!({ "entries": {} }))
            .expect("apply with snapshot");
        assert_eq!(applied.status, ProposalStatus::Applied);

        let rolled = manager
            .mark_rolled_back(&proposal.proposal_id, "regression")
            .expect("rollback");
        assert_eq!(rolled.status, ProposalStatus::RolledBack);
        assert_eq!(rolled.reason, "regression");

        // Restart: the audit chain survives via the white-box files.
        let reloaded = Manager::new(&dir, ImprovementConfig::default());
        let restored = reloaded.get(&proposal.proposal_id).expect("restored");
        assert_eq!(restored.status, ProposalStatus::RolledBack);
        assert_eq!(restored.prior_profile, serde_json::json!({ "entries": {} }));
    }

    #[test]
    fn evidence_and_rate_bound_enforced() {
        let manager = Manager::new(
            &tempdir(),
            ImprovementConfig {
                max_per_target_per_day: 2,
            },
        );
        let mut no_evidence = input("keepRecent");
        no_evidence.evidence_links.clear();
        assert!(matches!(
            manager.propose(no_evidence),
            Err(ImprovementError::EvidenceRequired)
        ));

        manager.propose(input("keepRecent")).expect("first");
        manager.propose(input("keepRecent")).expect("second");
        assert!(matches!(
            manager.propose(input("keepRecent")),
            Err(ImprovementError::RateBounded(_))
        ));
        // A different target is unaffected.
        manager
            .propose(input("threadBudgetChars"))
            .expect("other target");
    }

    #[test]
    fn transitions_are_guarded() {
        let manager = Manager::new(&tempdir(), ImprovementConfig::default());
        let proposal = manager
            .propose(input("personalBudgetChars"))
            .expect("propose");
        manager
            .reject(&proposal.proposal_id, "not now")
            .expect("reject");
        assert!(manager.reject(&proposal.proposal_id, "again").is_err());
        assert!(
            manager
                .mark_applied(&proposal.proposal_id, serde_json::json!({}))
                .is_err()
        );
    }
}
