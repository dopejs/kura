//! kura-plugin — the plugin kernel.
//!
//! Everything outside the trust-boundary kernel (store, event bus, identity,
//! auth, policy, secrets, audit) assembles as a plugin: a named unit that
//! declares what it provides and what it requires, and whose enablement is
//! resolved from a per-data-dir profile (`<data_dir>/plugins.json`) before
//! the daemon builds. Two tiers share this kernel: builtin plugins compiled
//! into the daemon (tier 1) and out-of-process providers reached over the
//! adapter/capability/MCP planes (tier 2, later phase).
//!
//! The kernel owns three mechanisms:
//! - **resolution** — profile + declared `requires` edges decide which
//!   plugins build; disabling a plugin transitively disables its dependents,
//!   and every decision is recorded in an [`AssemblyReport`] (nothing is
//!   silently dropped).
//! - **seams** — a typed registry ([`SeamMap`]) through which plugins share
//!   intermediates during assembly without depending on each other's crates.
//! - **hooks** — a waterfall [`HookBus`]: ordered interception points where a
//!   handler may mutate the payload or halt the action (the agent-loop hook
//!   points attach in the pluginization phase 2).

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Static identity + dependency declaration for one plugin.
#[derive(Debug, Clone, Copy)]
pub struct PluginDescriptor {
    /// Stable kebab-case id, unique across the assembly.
    pub id: &'static str,
    /// One-line human summary shown in the assembly report.
    pub summary: &'static str,
    /// Seam/service names this plugin provides (informational).
    pub provides: &'static [&'static str],
    /// Plugin ids that must be enabled for this plugin to build. Descriptor
    /// order must list dependencies before dependents.
    pub requires: &'static [&'static str],
}

/// Per-plugin entry in the profile file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginEntry {
    /// `Some(false)` disables the plugin; `None`/`Some(true)` inherits the
    /// default (enabled).
    pub enabled: Option<bool>,
    /// Free-form plugin-scoped configuration, opaque to the kernel.
    pub config: serde_json::Map<String, serde_json::Value>,
}

/// The on-disk plugin profile (`<data_dir>/plugins.json`). A missing file is
/// the default profile: every builtin enabled, no overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginProfile {
    /// Plugin ids disabled wholesale.
    pub disabled: Vec<String>,
    /// Per-plugin overrides keyed by plugin id (BTreeMap for stable dumps).
    pub entries: BTreeMap<String, PluginEntry>,
}

/// Profile load failure. A malformed profile fails the boot loudly instead of
/// silently assembling a different daemon than the operator configured.
#[derive(Debug)]
pub enum ProfileError {
    Read(std::io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::Read(err) => write!(f, "read plugins.json: {err}"),
            ProfileError::Parse(err) => write!(f, "parse plugins.json: {err}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// The profile file name inside the data dir.
pub const PROFILE_FILE_NAME: &str = "plugins.json";

impl PluginProfile {
    /// Loads `<data_dir>/plugins.json`; a missing file yields the default
    /// profile, a malformed one is an error.
    pub fn load(data_dir: &str) -> Result<Self, ProfileError> {
        let path = Path::new(data_dir).join(PROFILE_FILE_NAME);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => return Err(ProfileError::Read(err)),
        };
        serde_json::from_slice(&raw).map_err(ProfileError::Parse)
    }

    /// True when the profile explicitly disables `id`.
    fn explicitly_disabled(&self, id: &str) -> bool {
        self.disabled.iter().any(|d| d == id)
            || self
                .entries
                .get(id)
                .and_then(|entry| entry.enabled)
                .is_some_and(|enabled| !enabled)
    }

    /// The plugin-scoped config object for `id` (empty when absent).
    #[must_use]
    pub fn config_for(&self, id: &str) -> serde_json::Map<String, serde_json::Value> {
        self.entries
            .get(id)
            .map(|entry| entry.config.clone())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// External plugin manifests (tier 2, pluginization phase 3)
// ---------------------------------------------------------------------------

/// Error policy for one external hook registration: what happens when the
/// plugin process fails (spawn failure, protocol error, timeout).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookErrorPolicy {
    /// Failure is logged and the turn proceeds (availability first).
    #[default]
    Continue,
    /// Failure vetoes the action (fail closed — for policy plugins).
    Veto,
}

/// One hook registration requested by an external plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManifestHook {
    /// Hook point name (e.g. `chat/pre-dispatch`).
    pub point: String,
    pub on_error: HookErrorPolicy,
}

/// How the external plugin runs. `kind` is `process` (stdio line-JSON child)
/// for now; adapter-rpc/MCP transports arrive later.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManifestEntry {
    pub kind: String,
    pub command: String,
    pub args: Vec<String>,
    /// Per-hook-call timeout in milliseconds (default 2000).
    pub timeout_ms: u64,
}

/// The `manifest.json` of one external plugin under
/// `<data_dir>/plugins/<dir>/`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub summary: String,
    pub provides: Vec<String>,
    pub requires: Vec<String>,
    pub hooks: Vec<ManifestHook>,
    /// Seams this plugin serves over the process protocol (e.g.
    /// `context.embedder`). Seam calls ride the same line-JSON channel with
    /// point `seam:<name>:<op>`.
    pub seams: Vec<String>,
    pub entry: ManifestEntry,
}

/// A discovered external plugin: its manifest plus the directory it lives in
/// (the working directory for its process).
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalPlugin {
    pub manifest: PluginManifest,
    pub dir: std::path::PathBuf,
}

/// The external plugins directory name inside the data dir.
pub const EXTERNAL_PLUGINS_DIR: &str = "plugins";

/// Scans `<data_dir>/plugins/*/manifest.json`. Third-party content must not
/// brick the boot: malformed or invalid manifests are skipped with a warning
/// instead of failing (unlike the operator-owned profile). Results are
/// sorted by directory name for a stable assembly order.
#[must_use]
pub fn discover_external(data_dir: &str) -> (Vec<ExternalPlugin>, Vec<String>) {
    let root = Path::new(data_dir).join(EXTERNAL_PLUGINS_DIR);
    let mut plugins = Vec::new();
    let mut warnings = Vec::new();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (plugins, warnings),
        Err(err) => {
            warnings.push(format!("read {}: {err}", root.display()));
            return (plugins, warnings);
        }
    };
    let mut dirs: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let manifest_path = dir.join("manifest.json");
        let raw = match std::fs::read(&manifest_path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                warnings.push(format!("{}: no manifest.json", dir.display()));
                continue;
            }
            Err(err) => {
                warnings.push(format!("{}: {err}", manifest_path.display()));
                continue;
            }
        };
        let manifest: PluginManifest = match serde_json::from_slice(&raw) {
            Ok(manifest) => manifest,
            Err(err) => {
                warnings.push(format!("{}: {err}", manifest_path.display()));
                continue;
            }
        };
        if manifest.id.trim().is_empty() {
            warnings.push(format!(
                "{}: manifest id is required",
                manifest_path.display()
            ));
            continue;
        }
        if manifest.entry.kind != "process" || manifest.entry.command.trim().is_empty() {
            warnings.push(format!(
                "{}: entry must be kind=process with a command",
                manifest_path.display()
            ));
            continue;
        }
        plugins.push(ExternalPlugin { manifest, dir });
    }
    (plugins, warnings)
}

/// Resolved status of one plugin in the assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatus {
    pub id: String,
    pub summary: String,
    /// Where the plugin comes from; `builtin` until tier-2 providers ship.
    pub source: String,
    pub enabled: bool,
    /// Why the plugin is disabled; absent when enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub provides: Vec<String>,
    pub requires: Vec<String>,
}

/// The full assembly decision record: one entry per known plugin in build
/// order, plus warnings for profile entries that matched nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssemblyReport {
    pub plugins: Vec<PluginStatus>,
    pub warnings: Vec<String>,
}

impl AssemblyReport {
    /// True when `id` resolved enabled.
    #[must_use]
    pub fn enabled(&self, id: &str) -> bool {
        self.plugins.iter().any(|p| p.id == id && p.enabled)
    }
}

/// A plugin to resolve: the owned, source-tagged form shared by builtin
/// descriptors and discovered external manifests.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSpec {
    pub id: String,
    pub summary: String,
    /// `builtin` or `external`.
    pub source: String,
    pub provides: Vec<String>,
    pub requires: Vec<String>,
}

impl PluginSpec {
    #[must_use]
    pub fn from_descriptor(desc: &PluginDescriptor) -> Self {
        PluginSpec {
            id: desc.id.to_string(),
            summary: desc.summary.to_string(),
            source: "builtin".to_string(),
            provides: desc.provides.iter().map(ToString::to_string).collect(),
            requires: desc.requires.iter().map(ToString::to_string).collect(),
        }
    }

    #[must_use]
    pub fn from_manifest(manifest: &PluginManifest) -> Self {
        PluginSpec {
            id: manifest.id.trim().to_string(),
            summary: manifest.summary.clone(),
            source: "external".to_string(),
            provides: manifest.provides.clone(),
            requires: manifest.requires.clone(),
        }
    }
}

/// Resolves the effective enablement of `descriptors` (in declared order)
/// under `profile`. See [`resolve_specs`].
#[must_use]
pub fn resolve(descriptors: &[PluginDescriptor], profile: &PluginProfile) -> AssemblyReport {
    let specs: Vec<PluginSpec> = descriptors
        .iter()
        .map(PluginSpec::from_descriptor)
        .collect();
    resolve_specs(&specs, profile)
}

/// Resolves the effective enablement of `specs` (in declared order — builtins
/// first, externals after, dependencies before dependents) under `profile`.
/// A plugin is disabled either explicitly by the profile, transitively when
/// any of its `requires` resolved disabled, or when its id duplicates an
/// earlier spec; the reason records which. Profile ids that match no spec
/// become warnings.
#[must_use]
pub fn resolve_specs(specs: &[PluginSpec], profile: &PluginProfile) -> AssemblyReport {
    let known: HashSet<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    let mut warnings = Vec::new();
    for id in &profile.disabled {
        if !known.contains(id.as_str()) {
            warnings.push(format!("profile disables unknown plugin `{id}`"));
        }
    }
    for id in profile.entries.keys() {
        if !known.contains(id.as_str()) {
            warnings.push(format!("profile configures unknown plugin `{id}`"));
        }
    }

    let mut enabled: HashMap<&str, bool> = HashMap::new();
    let mut plugins = Vec::with_capacity(specs.len());
    for spec in specs {
        let mut reason = None;
        if enabled.contains_key(spec.id.as_str()) {
            reason = Some("duplicate plugin id".to_string());
        } else if profile.explicitly_disabled(&spec.id) {
            reason = Some("disabled by profile".to_string());
        } else {
            for req in &spec.requires {
                // Dependencies are declared before dependents; a forward or
                // unknown reference is surfaced as disabled.
                match enabled.get(req.as_str()) {
                    Some(true) => {}
                    Some(false) => {
                        reason = Some(format!("requires disabled plugin `{req}`"));
                        break;
                    }
                    None => {
                        reason = Some(format!("requires unknown plugin `{req}`"));
                        break;
                    }
                }
            }
        }
        let is_enabled = reason.is_none();
        if !enabled.contains_key(spec.id.as_str()) {
            enabled.insert(spec.id.as_str(), is_enabled);
        }
        plugins.push(PluginStatus {
            id: spec.id.clone(),
            summary: spec.summary.clone(),
            source: spec.source.clone(),
            enabled: is_enabled,
            reason,
            provides: spec.provides.clone(),
            requires: spec.requires.clone(),
        });
    }
    AssemblyReport { plugins, warnings }
}

/// Typed seam registry: assembly-time sharing of intermediates between
/// plugins, keyed by type. Values are stored as-is (no Send/Sync bound —
/// assembly is single-threaded); consumers get clones.
#[derive(Default)]
pub struct SeamMap {
    map: HashMap<TypeId, Box<dyn Any>>,
}

impl SeamMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `value` under its type, replacing any previous value.
    pub fn put<T: 'static>(&mut self, value: T) {
        self.map.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// A clone of the registered `T`, if any plugin provided one.
    #[must_use]
    pub fn get<T: Clone + 'static>(&self) -> Option<T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
            .cloned()
    }

    #[must_use]
    pub fn contains<T: 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }
}

/// Canonical hook point names. Points are plain strings so plugins can add
/// their own; these constants are the daemon-defined seams.
pub mod points {
    /// Start of a chat turn, before prompt assembly. Payload:
    /// `{tenantId, threadId, query, sourceKind}`; hooks may rewrite `query`
    /// or halt to veto the turn.
    pub const CHAT_TURN_START: &str = "chat/turn-start";
    /// After full context assembly (skills, profile, continuity), before the
    /// dispatch is prepared/persisted. Payload:
    /// `{tenantId, threadId, sourceKind, provider, model,
    /// messages: [{role, content}]}`; hooks may rewrite
    /// any of the three or halt to veto. Because the dispatch record is
    /// created after this point, whatever the hooks leave here is exactly
    /// what is logged and what the model sees ("model-visible = logged").
    pub const CHAT_PRE_DISPATCH: &str = "chat/pre-dispatch";
    /// End of a chat turn, after the dispatch settled and continuity was
    /// persisted. Payload: `{dispatchId, tenantId, threadId, query, output,
    /// status, sourceKind, requestTurnId, responseTurnId}`. Observational:
    /// a halt only stops later handlers, never the turn.
    pub const CHAT_TURN_END: &str = "chat/turn-end";
    /// Stage 9.0: before a tool the model asked for is executed. Payload:
    /// `{tenantId, threadId, dispatchId, callId, name, arguments}`; hooks may
    /// rewrite `arguments` or halt to veto the call — a veto is reported to
    /// the model as a tool error, never silently dropped.
    pub const CHAT_TOOL_CALL: &str = "chat/tool-call";
}

/// Outcome of one hook handler in a waterfall run.
pub enum HookOutcome {
    /// Pass the (possibly mutated) payload to the next handler.
    Continue,
    /// Stop the waterfall and veto the action with a reason.
    Halt(String),
}

/// One interception handler. Handlers run in registration order and may
/// mutate the payload before the next handler sees it.
pub trait Hook: Send + Sync {
    fn handle(&self, payload: &mut serde_json::Value) -> HookOutcome;
}

/// Result of running a hook point.
#[derive(Debug, Clone, PartialEq)]
pub struct HookRunResult {
    /// `Some((plugin_id, reason))` when a handler halted the waterfall.
    pub halted: Option<(String, String)>,
    /// Number of handlers that ran (including the halting one).
    pub ran: usize,
}

impl HookRunResult {
    #[must_use]
    pub fn allowed(&self) -> bool {
        self.halted.is_none()
    }
}

/// Waterfall hook bus. Points are string-named (`agent/pre-step`,
/// `tools/pre-execute`, ...); registration order per point is run order.
#[derive(Default)]
pub struct HookBus {
    handlers: parking_lot::RwLock<HashMap<String, Vec<(String, Arc<dyn Hook>)>>>,
}

impl HookBus {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `hook` for `point`, attributed to `plugin_id`.
    pub fn register(&self, point: &str, plugin_id: &str, hook: Arc<dyn Hook>) {
        self.handlers
            .write()
            .entry(point.to_string())
            .or_default()
            .push((plugin_id.to_string(), hook));
    }

    /// Every registration as `(point, plugin_id)` pairs, points sorted for a
    /// stable dump (registration order preserved within a point).
    #[must_use]
    pub fn registrations(&self) -> Vec<(String, String)> {
        let guard = self.handlers.read();
        let mut points: Vec<&String> = guard.keys().collect();
        points.sort();
        let mut out = Vec::new();
        for point in points {
            for (plugin_id, _) in &guard[point] {
                out.push((point.clone(), plugin_id.clone()));
            }
        }
        out
    }

    /// Runs the waterfall for `point` over `payload`. Handlers mutate the
    /// payload in order; a [`HookOutcome::Halt`] stops the run.
    pub fn run(&self, point: &str, payload: &mut serde_json::Value) -> HookRunResult {
        let handlers = {
            let guard = self.handlers.read();
            guard.get(point).cloned().unwrap_or_default()
        };
        // Stage 10.3: the waterfall's duration by point, so a slow plugin
        // shows up as a slow hook point rather than as slow chat.
        let clock = std::time::Instant::now();
        let result = Self::run_handlers(handlers, payload);
        if result.ran > 0 {
            kura_telemetry::metrics::registry().observe(
                kura_telemetry::metrics::HOOK_DURATION_SECONDS,
                &[("point", point)],
                clock.elapsed().as_secs_f64(),
            );
        }
        result
    }

    fn run_handlers(
        handlers: Vec<(String, Arc<dyn Hook>)>,
        payload: &mut serde_json::Value,
    ) -> HookRunResult {
        let mut ran = 0;
        for (plugin_id, hook) in handlers {
            ran += 1;
            if let HookOutcome::Halt(reason) = hook.handle(payload) {
                return HookRunResult {
                    halted: Some((plugin_id, reason)),
                    ran,
                };
            }
        }
        HookRunResult { halted: None, ran }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PluginDescriptor = PluginDescriptor {
        id: "a",
        summary: "base",
        provides: &["a-svc"],
        requires: &[],
    };
    const B: PluginDescriptor = PluginDescriptor {
        id: "b",
        summary: "depends on a",
        provides: &[],
        requires: &["a"],
    };
    const C: PluginDescriptor = PluginDescriptor {
        id: "c",
        summary: "depends on b",
        provides: &[],
        requires: &["b"],
    };

    #[test]
    fn default_profile_enables_everything() {
        let report = resolve(&[A, B, C], &PluginProfile::default());
        assert!(report.plugins.iter().all(|p| p.enabled));
        assert!(report.warnings.is_empty());
        assert!(report.enabled("a") && report.enabled("b") && report.enabled("c"));
    }

    #[test]
    fn explicit_disable_cascades_to_dependents() {
        let profile = PluginProfile {
            disabled: vec!["a".to_string()],
            ..Default::default()
        };
        let report = resolve(&[A, B, C], &profile);
        assert!(!report.enabled("a"));
        assert_eq!(
            report.plugins[0].reason.as_deref(),
            Some("disabled by profile")
        );
        assert!(!report.enabled("b"));
        assert_eq!(
            report.plugins[1].reason.as_deref(),
            Some("requires disabled plugin `a`")
        );
        assert!(!report.enabled("c"));
        assert_eq!(
            report.plugins[2].reason.as_deref(),
            Some("requires disabled plugin `b`")
        );
    }

    #[test]
    fn entry_enabled_false_disables_and_unknown_ids_warn() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "b".to_string(),
            PluginEntry {
                enabled: Some(false),
                config: serde_json::Map::new(),
            },
        );
        entries.insert("nope".to_string(), PluginEntry::default());
        let profile = PluginProfile {
            disabled: vec!["ghost".to_string()],
            entries,
        };
        let report = resolve(&[A, B, C], &profile);
        assert!(report.enabled("a"));
        assert!(!report.enabled("b"));
        assert!(!report.enabled("c"));
        assert_eq!(
            report.warnings.len(),
            2,
            "ghost + nope warned: {:?}",
            report.warnings
        );
    }

    #[test]
    fn unknown_requirement_is_disabled_not_a_panic() {
        const BROKEN: PluginDescriptor = PluginDescriptor {
            id: "broken",
            summary: "forward dep",
            provides: &[],
            requires: &["later"],
        };
        let report = resolve(&[BROKEN], &PluginProfile::default());
        assert!(!report.enabled("broken"));
        assert_eq!(
            report.plugins[0].reason.as_deref(),
            Some("requires unknown plugin `later`")
        );
    }

    #[test]
    fn profile_load_missing_default_and_malformed_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().to_string_lossy().into_owned();
        let profile = PluginProfile::load(&data_dir).expect("missing file is default");
        assert_eq!(profile, PluginProfile::default());

        std::fs::write(dir.path().join(PROFILE_FILE_NAME), b"{not json").expect("write");
        assert!(matches!(
            PluginProfile::load(&data_dir),
            Err(ProfileError::Parse(_))
        ));

        std::fs::write(
            dir.path().join(PROFILE_FILE_NAME),
            serde_json::json!({
                "disabled": ["channel-discord"],
                "entries": { "memory": { "enabled": true, "config": { "k": 1 } } }
            })
            .to_string(),
        )
        .expect("write");
        let profile = PluginProfile::load(&data_dir).expect("well-formed profile");
        assert!(profile.explicitly_disabled("channel-discord"));
        assert!(!profile.explicitly_disabled("memory"));
        assert_eq!(
            profile.config_for("memory").get("k"),
            Some(&serde_json::json!(1))
        );
    }

    #[test]
    fn discover_external_reads_valid_manifests_and_warns_on_bad_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().to_string_lossy().into_owned();

        // No plugins dir at all: empty, no warnings.
        let (plugins, warnings) = discover_external(&data_dir);
        assert!(plugins.is_empty() && warnings.is_empty());

        let root = dir.path().join(EXTERNAL_PLUGINS_DIR);
        // Valid plugin.
        std::fs::create_dir_all(root.join("good")).expect("mkdir");
        std::fs::write(
            root.join("good/manifest.json"),
            serde_json::json!({
                "id": "session-strategy",
                "summary": "external session window",
                "requires": ["chat"],
                "hooks": [{ "point": "chat/pre-dispatch", "onError": "veto" }],
                "entry": { "kind": "process", "command": "/bin/sh", "args": ["run.sh"], "timeoutMs": 500 }
            })
            .to_string(),
        )
        .expect("write");
        // Malformed JSON.
        std::fs::create_dir_all(root.join("broken")).expect("mkdir");
        std::fs::write(root.join("broken/manifest.json"), b"{nope").expect("write");
        // Missing manifest.
        std::fs::create_dir_all(root.join("empty")).expect("mkdir");
        // Missing entry command.
        std::fs::create_dir_all(root.join("no-entry")).expect("mkdir");
        std::fs::write(
            root.join("no-entry/manifest.json"),
            serde_json::json!({ "id": "x", "entry": { "kind": "process" } }).to_string(),
        )
        .expect("write");

        let (plugins, warnings) = discover_external(&data_dir);
        assert_eq!(
            plugins.len(),
            1,
            "only the valid manifest loads: {warnings:?}"
        );
        let plugin = &plugins[0];
        assert_eq!(plugin.manifest.id, "session-strategy");
        assert_eq!(plugin.manifest.hooks[0].on_error, HookErrorPolicy::Veto);
        assert_eq!(plugin.manifest.entry.timeout_ms, 500);
        assert_eq!(plugin.dir, root.join("good"));
        assert_eq!(
            warnings.len(),
            3,
            "broken + empty + no-entry warned: {warnings:?}"
        );
    }

    #[test]
    fn resolve_specs_handles_externals_and_duplicates() {
        let specs = vec![
            PluginSpec::from_descriptor(&A),
            PluginSpec {
                id: "ext".to_string(),
                summary: "external".to_string(),
                source: "external".to_string(),
                provides: vec![],
                requires: vec!["a".to_string()],
            },
            // Duplicate of a builtin id: disabled, builtin wins.
            PluginSpec {
                id: "a".to_string(),
                summary: "impostor".to_string(),
                source: "external".to_string(),
                provides: vec![],
                requires: vec![],
            },
        ];
        let report = resolve_specs(&specs, &PluginProfile::default());
        assert!(report.enabled("a") && report.enabled("ext"));
        assert_eq!(report.plugins[1].source, "external");
        assert!(!report.plugins[2].enabled);
        assert_eq!(
            report.plugins[2].reason.as_deref(),
            Some("duplicate plugin id")
        );

        // Disabling the builtin dependency cascades into the external.
        let profile = PluginProfile {
            disabled: vec!["a".to_string()],
            ..Default::default()
        };
        let report = resolve_specs(&specs, &profile);
        assert!(!report.enabled("ext"));
    }

    #[test]
    fn seam_map_round_trip() {
        #[derive(Clone, PartialEq, Debug)]
        struct Marker(u32);
        let mut seams = SeamMap::new();
        assert!(!seams.contains::<Marker>());
        seams.put(Marker(7));
        assert!(seams.contains::<Marker>());
        assert_eq!(seams.get::<Marker>(), Some(Marker(7)));
        assert_eq!(seams.get::<String>(), None);
    }

    #[test]
    fn hook_bus_waterfall_mutates_in_order_and_halts() {
        struct Add(&'static str);
        impl Hook for Add {
            fn handle(&self, payload: &mut serde_json::Value) -> HookOutcome {
                let list = payload.as_array_mut().expect("array payload");
                list.push(serde_json::json!(self.0));
                HookOutcome::Continue
            }
        }
        struct Veto;
        impl Hook for Veto {
            fn handle(&self, _payload: &mut serde_json::Value) -> HookOutcome {
                HookOutcome::Halt("policy says no".to_string())
            }
        }

        let bus = HookBus::new();
        bus.register("tools/pre-execute", "p1", Arc::new(Add("first")));
        bus.register("tools/pre-execute", "p2", Arc::new(Add("second")));
        let mut payload = serde_json::json!([]);
        let result = bus.run("tools/pre-execute", &mut payload);
        assert!(result.allowed());
        assert_eq!(result.ran, 2);
        assert_eq!(payload, serde_json::json!(["first", "second"]));

        bus.register("tools/pre-execute", "p3", Arc::new(Veto));
        bus.register("tools/pre-execute", "p4", Arc::new(Add("never")));
        let mut payload = serde_json::json!([]);
        let result = bus.run("tools/pre-execute", &mut payload);
        assert_eq!(
            result.halted,
            Some(("p3".to_string(), "policy says no".to_string()))
        );
        assert_eq!(result.ran, 3, "halt stops the waterfall before p4");
        assert_eq!(payload, serde_json::json!(["first", "second"]));

        // Unregistered point: no handlers, allowed.
        let mut payload = serde_json::json!({});
        let result = bus.run("agent/pre-step", &mut payload);
        assert!(result.allowed());
        assert_eq!(result.ran, 0);
    }
}
