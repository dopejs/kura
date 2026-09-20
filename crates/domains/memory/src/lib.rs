//! kura-memory: the memory plane foundation (Roadmap 78, spec 058).
//!
//! Layered memory following the TencentDB-Agent-Memory model adapted onto
//! Kura's planes: L0 is the existing conversation truth (referenced,
//! never duplicated), L1 atoms extract typed facts with mandatory source
//! links, L2 scenarios aggregate atoms, L3 distills the persona/core
//! profile — every layer carrying a deterministic drill-down path to the
//! layer below. All layers (and, later, skills/wiki/codegraph) share one
//! governed asset envelope: owner, tenant, visibility, version chain,
//! status, and agent bindings. Writes are policy-gated proposals;
//! ready assets are immutable and change by supersede; revocation
//! tombstones; retention expiry is a recorded transition.
//!
//! The manager is persistence-free (store DAOs live in kura-store per the
//! workspace's persistence-inversion rule); callers persist mutations and
//! restore state at boot.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    ($name:ident { $($v:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $s)] $v),+ }

        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $( $name::$v => $s ),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(AssetKind {
    ChatMemory => "chat_memory",
    Skill => "skill",
    Wiki => "wiki",
    CodeGraph => "code_graph",
});

string_enum!(MemoryLayer {
    L0Ref => "l0_ref",
    L1 => "l1",
    L2 => "l2",
    L3 => "l3",
});

string_enum!(Visibility {
    Private => "private",
    Team => "team",
    Restricted => "restricted",
    Agent => "agent",
});

string_enum!(AssetStatus {
    Pending => "pending",
    Ready => "ready",
    Superseded => "superseded",
    Revoked => "revoked",
    Expired => "expired",
});

string_enum!(ActorKind {
    Operator => "operator",
    Agent => "agent",
    System => "system",
});

string_enum!(AtomType {
    Fact => "fact",
    Preference => "preference",
    Constraint => "constraint",
    Event => "event",
    Decision => "decision",
    Reference => "reference",
});

string_enum!(SourceKind {
    Thread => "thread",
    Run => "run",
    Event => "event",
    Message => "message",
    Asset => "asset",
    External => "external",
});

impl Default for AssetKind {
    fn default() -> Self {
        AssetKind::ChatMemory
    }
}
impl Default for MemoryLayer {
    fn default() -> Self {
        MemoryLayer::L1
    }
}
impl Default for Visibility {
    fn default() -> Self {
        Visibility::Private
    }
}
impl Default for AssetStatus {
    fn default() -> Self {
        AssetStatus::Pending
    }
}
impl Default for ActorKind {
    fn default() -> Self {
        ActorKind::Operator
    }
}
impl Default for SourceKind {
    fn default() -> Self {
        SourceKind::Thread
    }
}

/// The acting identity behind a write (attribution is mandatory).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    pub kind: ActorKind,
    pub id: String,
}

/// A deterministic link to lower-layer evidence (the citation).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLink {
    pub kind: SourceKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub excerpt: String,
}

/// One governed memory asset in the uniform envelope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryAsset {
    pub asset_id: String,
    pub kind: AssetKind,
    pub layer: MemoryLayer,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub owner: Actor,
    pub visibility: Visibility,
    pub status: AssetStatus,
    pub version: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub supersedes_asset_id: String,
    /// Agent/persona ids this asset is equipped to (the loadout hook).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<String>,
    /// L1 only: the atom type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atom_type: Option<AtomType>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// L1 atom content / L2 scenario body / L3 profile body.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// L2/L3 drill-down: the member asset ids one layer below.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub member_asset_ids: Vec<String>,
    /// L1/L0-ref drill-down: links into the conversation truth.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_links: Vec<SourceLink>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub retention_class: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status_reason: String,
}

/// Input to create/capture an asset.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateAssetInput {
    pub kind: AssetKind,
    pub layer: MemoryLayer,
    pub tenant_id: String,
    pub owner: Actor,
    pub visibility: Visibility,
    pub atom_type: Option<AtomType>,
    pub title: String,
    pub content: String,
    pub member_asset_ids: Vec<String>,
    pub source_links: Vec<SourceLink>,
    pub retention_class: String,
    pub bindings: Vec<String>,
    pub supersedes_asset_id: String,
}

/// The write-policy verdict over a proposed mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteDecision {
    Accept,
    RequireApproval { reason: String },
    Reject { reason: String },
}

/// The operation classes the policy evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOperation {
    Create,
    Consolidate,
    WidenVisibility,
}

/// Policy hook over memory writes. Fail closed: hook errors reject.
pub trait WritePolicy: Send + Sync {
    fn evaluate(&self, operation: WriteOperation, asset: &MemoryAsset) -> WriteDecision;
}

/// Spec 058 default policy: operator-actor preference/reference atoms
/// auto-accept; agent-actor writes and every visibility widening require
/// approval; everything else from operators auto-accepts.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultWritePolicy;

impl WritePolicy for DefaultWritePolicy {
    fn evaluate(&self, operation: WriteOperation, asset: &MemoryAsset) -> WriteDecision {
        if matches!(operation, WriteOperation::WidenVisibility) {
            return WriteDecision::RequireApproval {
                reason: "visibility widening requires review".to_string(),
            };
        }
        match asset.owner.kind {
            ActorKind::Agent => WriteDecision::RequireApproval {
                reason: "agent-authored memory requires review".to_string(),
            },
            ActorKind::Operator | ActorKind::System => WriteDecision::Accept,
        }
    }
}

/// A draft atom produced by the consolidation extractor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AtomDraft {
    pub atom_type: Option<AtomType>,
    pub title: String,
    pub content: String,
    pub source_links: Vec<SourceLink>,
}

/// One L0 item handed to the extractor (a reference into conversation truth).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct L0Item {
    pub source: SourceLink,
    pub role: String,
    pub text: String,
    pub occurred_at: DateTime<Utc>,
}

/// The async refinement seam (L0→L1 extraction, L1→L2 aggregation, L2→L3
/// distillation). The model-backed implementation lands with spec 059; the
/// default produces nothing and records the run.
pub trait Consolidator: Send + Sync {
    fn extract_l1(&self, tenant_id: &str, window: &[L0Item]) -> Result<Vec<AtomDraft>, String>;
    fn aggregate_l2(
        &self,
        tenant_id: &str,
        atoms: &[MemoryAsset],
    ) -> Result<Vec<AtomDraft>, String>;
    fn distill_l3(
        &self,
        tenant_id: &str,
        scenarios: &[MemoryAsset],
    ) -> Result<Option<AtomDraft>, String>;
}

/// Default no-op consolidator (recorded runs, zero drafts).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopConsolidator;

impl Consolidator for NoopConsolidator {
    fn extract_l1(&self, _tenant_id: &str, _window: &[L0Item]) -> Result<Vec<AtomDraft>, String> {
        Ok(Vec::new())
    }
    fn aggregate_l2(
        &self,
        _tenant_id: &str,
        _atoms: &[MemoryAsset],
    ) -> Result<Vec<AtomDraft>, String> {
        Ok(Vec::new())
    }
    fn distill_l3(
        &self,
        _tenant_id: &str,
        _scenarios: &[MemoryAsset],
    ) -> Result<Option<AtomDraft>, String> {
        Ok(None)
    }
}

/// Consolidation trigger configuration (TencentDB-Agent-Memory defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConsolidationConfig {
    /// L1 extraction every N captured turns.
    pub extract_every_turns: i64,
    /// L1 extraction after this idle window (seconds).
    pub extract_idle_seconds: i64,
    /// Minimum seconds between L2 passes.
    pub scenario_min_interval_seconds: i64,
    /// L3 distillation every N new atoms.
    pub persona_every_atoms: i64,
    /// Warm-up: first-session extraction at turns 1, 2, 4, 8, ...
    pub warmup_doubling: bool,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        ConsolidationConfig {
            extract_every_turns: 5,
            extract_idle_seconds: 600,
            scenario_min_interval_seconds: 900,
            persona_every_atoms: 50,
            warmup_doubling: true,
        }
    }
}

/// The recorded outcome of one consolidation pass.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConsolidationRun {
    pub run_id: String,
    pub tenant_id: String,
    pub trigger: String,
    pub extracted_l1: i64,
    pub aggregated_l2: i64,
    pub distilled_l3: i64,
    pub pending_approval: i64,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Per-tenant consolidation bookkeeping.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TenantConsolidationState {
    pub turns_since_extract: i64,
    pub total_turns: i64,
    pub atoms_since_persona: i64,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub last_extract_at: Option<DateTime<Utc>>,
    pub last_scenario_at: Option<DateTime<Utc>>,
    pub next_warmup_turn: i64,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MemoryError {
    #[error("memory asset not found")]
    AssetNotFound,
    #[error("memory asset content is required")]
    ContentRequired,
    #[error("memory asset attribution (owner) is required")]
    AttributionRequired,
    #[error("memory writes require source links (recalled memory must cite evidence)")]
    SourceLinksRequired,
    #[error("l2/l3 assets require member asset ids (the drill-down path)")]
    MembersRequired,
    #[error("memory asset is not pending approval")]
    NotPending,
    #[error("memory asset is not active")]
    NotActive,
    #[error("memory write rejected: {0}")]
    Rejected(String),
    #[error("visibility can only widen through the policy gate")]
    InvalidVisibilityChange,
}

// The full v7 hex keeps ids time-sortable AND unique. Truncating to the
// first 16 chars (as this once did) leaves ~12 random bits after the
// millisecond timestamp, so assets created in the same millisecond — the
// consolidator's batch writes, every test — collided and silently
// overwrote each other.
fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::now_v7().simple())
}

fn visibility_rank(v: Visibility) -> u8 {
    match v {
        Visibility::Private => 0,
        Visibility::Agent => 1,
        Visibility::Restricted => 2,
        Visibility::Team => 3,
    }
}

#[derive(Default)]
struct ManagerInner {
    by_id: HashMap<String, MemoryAsset>,
    ids: Vec<String>,
    consolidation: HashMap<String, TenantConsolidationState>,
}

/// The memory plane manager: in-memory truth with restore; callers persist.
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
    env: String,
    policy: Arc<dyn WritePolicy>,
    consolidator: Arc<dyn Consolidator>,
    config: ConsolidationConfig,
}

impl Manager {
    #[must_use]
    pub fn new(
        environment_scope: &str,
        policy: Option<Arc<dyn WritePolicy>>,
        consolidator: Option<Arc<dyn Consolidator>>,
        config: Option<ConsolidationConfig>,
    ) -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
            env: environment_scope.trim().to_string(),
            policy: policy.unwrap_or_else(|| Arc::new(DefaultWritePolicy)),
            consolidator: consolidator.unwrap_or_else(|| Arc::new(NoopConsolidator)),
            config: config.unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn environment_scope(&self) -> &str {
        &self.env
    }

    #[must_use]
    pub fn config(&self) -> &ConsolidationConfig {
        &self.config
    }

    /// Boot restore (oldest-first so supersede chains replay in order).
    /// Duplicate ids are collapsed (last write wins) — data written under
    /// the truncated-id bug held colliding ids, and replaying them verbatim
    /// would surface one asset multiple times in every listing.
    pub fn restore(&self, assets: Vec<MemoryAsset>) {
        let mut inner = self.inner.write();
        inner.by_id.clear();
        inner.ids.clear();
        for asset in assets {
            if !inner.by_id.contains_key(&asset.asset_id) {
                inner.ids.push(asset.asset_id.clone());
            }
            inner.by_id.insert(asset.asset_id.clone(), asset);
        }
    }

    fn validate(input: &CreateAssetInput) -> Result<(), MemoryError> {
        if input.owner.id.trim().is_empty() {
            return Err(MemoryError::AttributionRequired);
        }
        if input.content.trim().is_empty() && input.layer != MemoryLayer::L0Ref {
            return Err(MemoryError::ContentRequired);
        }
        // L0 refs may carry a bounded excerpt (the extraction window's text
        // cache); truth stays in the conversation stores the links point at.
        match input.layer {
            MemoryLayer::L0Ref | MemoryLayer::L1 => {
                if input.source_links.is_empty() {
                    return Err(MemoryError::SourceLinksRequired);
                }
            }
            MemoryLayer::L2 | MemoryLayer::L3 => {
                if input.member_asset_ids.is_empty() {
                    return Err(MemoryError::MembersRequired);
                }
            }
        }
        Ok(())
    }

    /// Creates an asset through the write policy: Accept -> Ready,
    /// RequireApproval -> Pending, Reject -> error. Returns the stored
    /// asset and the policy decision.
    pub fn create(
        &self,
        input: CreateAssetInput,
    ) -> Result<(MemoryAsset, WriteDecision), MemoryError> {
        Self::validate(&input)?;
        let now = Utc::now();
        let mut asset = MemoryAsset {
            asset_id: new_id("mem"),
            kind: input.kind,
            layer: input.layer,
            tenant_id: input.tenant_id.trim().to_string(),
            owner: input.owner,
            visibility: input.visibility,
            status: AssetStatus::Pending,
            version: 1,
            supersedes_asset_id: input.supersedes_asset_id.trim().to_string(),
            bindings: input.bindings,
            atom_type: input.atom_type,
            title: input.title.trim().to_string(),
            content: input.content,
            member_asset_ids: input.member_asset_ids,
            source_links: input.source_links,
            retention_class: input.retention_class,
            created_at: now,
            updated_at: now,
            ..MemoryAsset::default()
        };
        let operation = if asset.supersedes_asset_id.is_empty() {
            WriteOperation::Create
        } else {
            WriteOperation::Consolidate
        };
        let decision = self.policy.evaluate(operation, &asset);
        match &decision {
            WriteDecision::Reject { reason } => {
                return Err(MemoryError::Rejected(reason.clone()));
            }
            WriteDecision::RequireApproval { reason } => {
                asset.status = AssetStatus::Pending;
                asset.status_reason = reason.clone();
            }
            WriteDecision::Accept => {
                asset.status = AssetStatus::Ready;
                asset.ready_at = Some(now);
            }
        }

        let mut inner = self.inner.write();
        // Supersede chain: version follows the superseded asset; the old
        // asset flips to Superseded once the new one is Ready.
        if !asset.supersedes_asset_id.is_empty() {
            if let Some(previous) = inner.by_id.get_mut(&asset.supersedes_asset_id) {
                asset.version = previous.version + 1;
                if asset.status == AssetStatus::Ready {
                    previous.status = AssetStatus::Superseded;
                    previous.updated_at = now;
                }
            }
        }
        inner.ids.push(asset.asset_id.clone());
        inner.by_id.insert(asset.asset_id.clone(), asset.clone());
        Ok((asset, decision))
    }

    /// Approves a pending asset (the review side of RequireApproval).
    /// Returns the updated asset plus the superseded predecessor when the
    /// approval completes a supersede chain.
    pub fn approve(
        &self,
        asset_id: &str,
        reviewer: &Actor,
    ) -> Result<(MemoryAsset, Option<MemoryAsset>), MemoryError> {
        let mut inner = self.inner.write();
        let now = Utc::now();
        let supersedes = {
            let asset = inner
                .by_id
                .get_mut(asset_id.trim())
                .ok_or(MemoryError::AssetNotFound)?;
            if asset.status != AssetStatus::Pending {
                return Err(MemoryError::NotPending);
            }
            asset.status = AssetStatus::Ready;
            asset.ready_at = Some(now);
            asset.updated_at = now;
            asset.status_reason = format!("approved by {}:{}", reviewer.kind, reviewer.id);
            asset.supersedes_asset_id.clone()
        };
        let mut previous = None;
        if !supersedes.is_empty() {
            if let Some(prior) = inner.by_id.get_mut(&supersedes) {
                prior.status = AssetStatus::Superseded;
                prior.updated_at = now;
                previous = Some(prior.clone());
            }
        }
        let asset = inner
            .by_id
            .get(asset_id.trim())
            .cloned()
            .ok_or(MemoryError::AssetNotFound)?;
        Ok((asset, previous))
    }

    /// Rejects a pending asset (tombstoned as revoked with the reason).
    pub fn reject(&self, asset_id: &str, reason: &str) -> Result<MemoryAsset, MemoryError> {
        let mut inner = self.inner.write();
        let asset = inner
            .by_id
            .get_mut(asset_id.trim())
            .ok_or(MemoryError::AssetNotFound)?;
        if asset.status != AssetStatus::Pending {
            return Err(MemoryError::NotPending);
        }
        let now = Utc::now();
        asset.status = AssetStatus::Revoked;
        asset.revoked_at = Some(now);
        asset.updated_at = now;
        asset.status_reason = reason.trim().to_string();
        Ok(asset.clone())
    }

    /// Revokes an active asset (reversibility: tombstone, never delete).
    pub fn revoke(&self, asset_id: &str, reason: &str) -> Result<MemoryAsset, MemoryError> {
        let mut inner = self.inner.write();
        let asset = inner
            .by_id
            .get_mut(asset_id.trim())
            .ok_or(MemoryError::AssetNotFound)?;
        if asset.status != AssetStatus::Ready && asset.status != AssetStatus::Pending {
            return Err(MemoryError::NotActive);
        }
        let now = Utc::now();
        asset.status = AssetStatus::Revoked;
        asset.revoked_at = Some(now);
        asset.updated_at = now;
        asset.status_reason = reason.trim().to_string();
        Ok(asset.clone())
    }

    /// Changes visibility. Narrowing applies immediately; widening runs
    /// through the policy gate (RequireApproval leaves the asset unchanged
    /// and returns the decision for the caller's approval flow).
    pub fn set_visibility(
        &self,
        asset_id: &str,
        visibility: Visibility,
    ) -> Result<(MemoryAsset, WriteDecision), MemoryError> {
        let mut inner = self.inner.write();
        let asset = inner
            .by_id
            .get_mut(asset_id.trim())
            .ok_or(MemoryError::AssetNotFound)?;
        if asset.status != AssetStatus::Ready {
            return Err(MemoryError::NotActive);
        }
        if visibility_rank(visibility) <= visibility_rank(asset.visibility) {
            asset.visibility = visibility;
            asset.updated_at = Utc::now();
            return Ok((asset.clone(), WriteDecision::Accept));
        }
        let decision = self.policy.evaluate(WriteOperation::WidenVisibility, asset);
        match &decision {
            WriteDecision::Accept => {
                asset.visibility = visibility;
                asset.updated_at = Utc::now();
            }
            WriteDecision::RequireApproval { .. } => {}
            WriteDecision::Reject { reason } => {
                return Err(MemoryError::Rejected(reason.clone()));
            }
        }
        Ok((asset.clone(), decision))
    }

    #[must_use]
    pub fn get(&self, asset_id: &str) -> Option<MemoryAsset> {
        self.inner.read().by_id.get(asset_id.trim()).cloned()
    }

    /// Lists assets for a tenant (empty tenant = the local operator scope),
    /// optionally filtered by layer/status.
    #[must_use]
    pub fn list(
        &self,
        tenant_id: &str,
        layer: Option<MemoryLayer>,
        status: Option<AssetStatus>,
    ) -> Vec<MemoryAsset> {
        let inner = self.inner.read();
        inner
            .ids
            .iter()
            .filter_map(|id| inner.by_id.get(id))
            .filter(|asset| asset.tenant_id == tenant_id.trim())
            .filter(|asset| layer.map_or(true, |l| asset.layer == l))
            .filter(|asset| status.map_or(true, |s| asset.status == s))
            .cloned()
            .collect()
    }

    /// The deterministic drill-down path: the asset plus, recursively, the
    /// member assets below it, ending in L1 source links (the L0 citation).
    pub fn drilldown(&self, asset_id: &str) -> Result<DrilldownNode, MemoryError> {
        let inner = self.inner.read();
        fn build(
            inner: &ManagerInner,
            asset_id: &str,
            depth: usize,
        ) -> Result<DrilldownNode, MemoryError> {
            let asset = inner
                .by_id
                .get(asset_id)
                .ok_or(MemoryError::AssetNotFound)?;
            let mut members = Vec::new();
            if depth < 4 {
                for member_id in &asset.member_asset_ids {
                    members.push(build(inner, member_id, depth + 1)?);
                }
            }
            Ok(DrilldownNode {
                asset: asset.clone(),
                members,
            })
        }
        build(&inner, asset_id.trim(), 0)
    }

    /// Records a captured L0 turn for trigger bookkeeping and returns
    /// whether an extraction pass is now due.
    pub fn record_turn(&self, tenant_id: &str, at: DateTime<Utc>) -> bool {
        let mut inner = self.inner.write();
        let state = inner
            .consolidation
            .entry(tenant_id.trim().to_string())
            .or_default();
        state.turns_since_extract += 1;
        state.total_turns += 1;
        state.last_activity_at = Some(at);
        if state.next_warmup_turn == 0 {
            state.next_warmup_turn = 1;
        }
        if self.config.warmup_doubling && state.total_turns >= state.next_warmup_turn {
            return true;
        }
        state.turns_since_extract >= self.config.extract_every_turns
    }

    /// Whether the idle trigger fired for the tenant at `now`.
    #[must_use]
    pub fn idle_due(&self, tenant_id: &str, now: DateTime<Utc>) -> bool {
        let inner = self.inner.read();
        let Some(state) = inner.consolidation.get(tenant_id.trim()) else {
            return false;
        };
        if state.turns_since_extract == 0 {
            return false;
        }
        state
            .last_activity_at
            .is_some_and(|last| now - last >= Duration::seconds(self.config.extract_idle_seconds))
    }

    /// Runs one consolidation pass through the Consolidator seam: L1
    /// extraction over the supplied L0 window, L2 aggregation when due, L3
    /// distillation when the atom volume trigger fires. Drafts run through
    /// the write policy like any other write. Returns the run record plus
    /// every asset written (for the caller to persist).
    pub fn consolidate(
        &self,
        tenant_id: &str,
        trigger: &str,
        window: &[L0Item],
    ) -> (ConsolidationRun, Vec<MemoryAsset>) {
        let tenant_id = tenant_id.trim();
        let started_at = Utc::now();
        let mut run = ConsolidationRun {
            run_id: new_id("memrun"),
            tenant_id: tenant_id.to_string(),
            trigger: trigger.to_string(),
            started_at,
            completed_at: started_at,
            ..ConsolidationRun::default()
        };
        let mut written = Vec::new();

        let extractor_owner = Actor {
            kind: ActorKind::System,
            id: "consolidator".to_string(),
        };

        match self.consolidator.extract_l1(tenant_id, window) {
            Ok(drafts) => {
                for draft in drafts {
                    let input = CreateAssetInput {
                        kind: AssetKind::ChatMemory,
                        layer: MemoryLayer::L1,
                        tenant_id: tenant_id.to_string(),
                        owner: extractor_owner.clone(),
                        visibility: Visibility::Private,
                        atom_type: draft.atom_type,
                        title: draft.title,
                        content: draft.content,
                        source_links: draft.source_links,
                        ..CreateAssetInput::default()
                    };
                    match self.create(input) {
                        Ok((asset, decision)) => {
                            if matches!(decision, WriteDecision::RequireApproval { .. }) {
                                run.pending_approval += 1;
                            }
                            run.extracted_l1 += 1;
                            written.push(asset);
                        }
                        Err(err) => run.error = err.to_string(),
                    }
                }
            }
            Err(err) => run.error = err,
        }

        // Trigger bookkeeping reset + L3 volume accounting.
        let l3_due = {
            let mut inner = self.inner.write();
            let state = inner
                .consolidation
                .entry(tenant_id.to_string())
                .or_default();
            state.turns_since_extract = 0;
            state.last_extract_at = Some(started_at);
            if self.config.warmup_doubling && state.total_turns >= state.next_warmup_turn {
                state.next_warmup_turn = (state.next_warmup_turn.max(1)) * 2;
            }
            state.atoms_since_persona += run.extracted_l1;
            state.atoms_since_persona >= self.config.persona_every_atoms
        };

        // L2 aggregation over ready atoms when the interval allows.
        let scenario_due = {
            let inner = self.inner.read();
            inner
                .consolidation
                .get(tenant_id)
                .and_then(|s| s.last_scenario_at)
                .map_or(true, |last| {
                    started_at - last
                        >= Duration::seconds(self.config.scenario_min_interval_seconds)
                })
        };
        if scenario_due {
            let atoms = self.list(tenant_id, Some(MemoryLayer::L1), Some(AssetStatus::Ready));
            if !atoms.is_empty() {
                match self.consolidator.aggregate_l2(tenant_id, &atoms) {
                    Ok(drafts) => {
                        for draft in drafts {
                            let member_ids = atoms.iter().map(|a| a.asset_id.clone()).collect();
                            let input = CreateAssetInput {
                                kind: AssetKind::ChatMemory,
                                layer: MemoryLayer::L2,
                                tenant_id: tenant_id.to_string(),
                                owner: extractor_owner.clone(),
                                visibility: Visibility::Private,
                                title: draft.title,
                                content: draft.content,
                                member_asset_ids: member_ids,
                                ..CreateAssetInput::default()
                            };
                            if let Ok((asset, _)) = self.create(input) {
                                run.aggregated_l2 += 1;
                                written.push(asset);
                            }
                        }
                        let mut inner = self.inner.write();
                        if let Some(state) = inner.consolidation.get_mut(tenant_id) {
                            state.last_scenario_at = Some(started_at);
                        }
                    }
                    Err(err) => run.error = err,
                }
            }
        }

        if l3_due {
            let scenarios = self.list(tenant_id, Some(MemoryLayer::L2), Some(AssetStatus::Ready));
            if !scenarios.is_empty() {
                if let Ok(Some(draft)) = self.consolidator.distill_l3(tenant_id, &scenarios) {
                    let member_ids = scenarios.iter().map(|a| a.asset_id.clone()).collect();
                    let previous_l3 = self
                        .list(tenant_id, Some(MemoryLayer::L3), Some(AssetStatus::Ready))
                        .into_iter()
                        .next()
                        .map(|a| a.asset_id)
                        .unwrap_or_default();
                    let input = CreateAssetInput {
                        kind: AssetKind::ChatMemory,
                        layer: MemoryLayer::L3,
                        tenant_id: tenant_id.to_string(),
                        owner: extractor_owner.clone(),
                        visibility: Visibility::Private,
                        title: draft.title,
                        content: draft.content,
                        member_asset_ids: member_ids,
                        supersedes_asset_id: previous_l3,
                        ..CreateAssetInput::default()
                    };
                    if let Ok((asset, _)) = self.create(input) {
                        run.distilled_l3 += 1;
                        written.push(asset);
                    }
                }
                let mut inner = self.inner.write();
                if let Some(state) = inner.consolidation.get_mut(tenant_id) {
                    state.atoms_since_persona = 0;
                }
            }
        }

        run.completed_at = Utc::now();
        (run, written)
    }

    /// Tenants with consolidation bookkeeping (for the scheduler tick).
    #[must_use]
    pub fn tenants_with_bookkeeping(&self) -> Vec<String> {
        self.inner.read().consolidation.keys().cloned().collect()
    }

    /// Builds the L0 window for a tenant: the captured L0 refs since the
    /// last extraction pass, mapped to extractor items (each item's text is
    /// the ref's bounded excerpt; its source link points at conversation
    /// truth).
    #[must_use]
    pub fn pending_l0_window(&self, tenant_id: &str) -> Vec<L0Item> {
        let since = self
            .inner
            .read()
            .consolidation
            .get(tenant_id.trim())
            .and_then(|s| s.last_extract_at);
        self.list(
            tenant_id,
            Some(MemoryLayer::L0Ref),
            Some(AssetStatus::Ready),
        )
        .into_iter()
        .filter(|asset| since.map_or(true, |t| asset.created_at > t))
        .map(|asset| L0Item {
            source: asset.source_links.first().cloned().unwrap_or_default(),
            role: asset.title.clone(),
            text: asset.content.clone(),
            occurred_at: asset.created_at,
        })
        .collect()
    }

    /// Applies retention expiry at `now`; returns the expired assets (for
    /// the caller to persist + publish).
    pub fn sweep_retention(&self, now: DateTime<Utc>) -> Vec<MemoryAsset> {
        let mut inner = self.inner.write();
        let mut expired = Vec::new();
        let ids: Vec<String> = inner.ids.clone();
        for id in ids {
            if let Some(asset) = inner.by_id.get_mut(&id) {
                if asset.status == AssetStatus::Ready {
                    if let Some(expires_at) = asset.expires_at {
                        if expires_at <= now {
                            asset.status = AssetStatus::Expired;
                            asset.updated_at = now;
                            expired.push(asset.clone());
                        }
                    }
                }
            }
        }
        expired
    }

    /// Renders the white-box Markdown projection for an L2/L3 asset.
    #[must_use]
    pub fn render_markdown(&self, asset: &MemoryAsset) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# {}\n\n",
            if asset.title.is_empty() {
                &asset.asset_id
            } else {
                &asset.title
            }
        ));
        out.push_str(&format!(
            "- asset: `{}` (layer {}, v{}, {})\n- owner: {}:{}\n- visibility: {}\n- updated: {}\n\n",
            asset.asset_id,
            asset.layer,
            asset.version,
            asset.status,
            asset.owner.kind,
            asset.owner.id,
            asset.visibility,
            asset.updated_at.to_rfc3339(),
        ));
        out.push_str(&asset.content);
        out.push_str("\n\n## Drill-down\n\n");
        for member in &asset.member_asset_ids {
            out.push_str(&format!("- [[{member}]]\n"));
        }
        for link in &asset.source_links {
            out.push_str(&format!("- {}:{}\n", link.kind, link.id));
        }
        out
    }
}

/// One node of the deterministic drill-down tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrilldownNode {
    pub asset: MemoryAsset,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<DrilldownNode>,
}
