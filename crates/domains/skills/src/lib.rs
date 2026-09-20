//! Port of `daemon/internal/skills`: the file-system skill registry.
//!
//! Scans `<homeRoot>/skills` and `<dataRoot>/skills` for `SKILL.md` bundles,
//! parses YAML-ish frontmatter, inventories bundled files, computes
//! executable-skill availability from the `execution.*` frontmatter surface
//! (resolving entrypoints, working dirs and the data-dir
//! `skill-secrets.json` file), and projects a sandbox consumer declaration
//! for skill selection. Data-dir skills take precedence over home skills for
//! a shared skill id (insertion order into the index).
//!
//! The registry reads the filesystem only — there is no SQLite persistence,
//! matching the Go package (which is why there is no `kura-store`
//! dependency). The Go nil-receiver guards (`ErrSkillsRegistryMissing`,
//! empty results) are unrepresentable in Rust: construction always yields a
//! valid registry.
//!
//! Tenant-scoped secret resolution (`resolve_executable_skill_secrets_for_tenant`)
//! is the one async surface; it forwards to the async `kura-secrets` manager
//! and reads the tenant context from the `kura-identity` task-local carrier.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

string_enum!(Source {
    Home => "home",
    DataDir => "data_dir",
});

string_enum!(SkillAvailabilityStatus {
    NotExecutable => "not_executable",
    Available => "available",
    Unavailable => "unavailable",
});

/// Name of the data-dir JSON file holding executable-skill secret values
/// (Go `executableSkillSecretsFileName`).
pub const EXECUTABLE_SKILL_SECRETS_FILE_NAME: &str = "skill-secrets.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub path: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub source: Source,
    pub root_path: String,
    pub skill_path: String,
    pub instruction_path: String,
    #[serde(default)]
    pub frontmatter: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub frontmatter_raw: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default)]
    pub files: Vec<File>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_manifest: Option<ExecutableManifest>,
    #[serde(default)]
    pub availability_status: SkillAvailabilityStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableManifest {
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    pub profile_id: String,
    #[serde(default)]
    pub backend_kind: kura_sandbox::BackendKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write_roots: Vec<String>,
    #[serde(default)]
    pub network_mode: kura_sandbox::NetworkMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_ports: Vec<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<String>,
    pub approval_mode: kura_sandbox::ApprovalMode,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub timeout_ms: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_enforcement_strength: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub overlay_id: String,
    pub source: Source,
    pub path: String,
    pub size_bytes: i64,
    pub modified_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub loaded_at: DateTime<Utc>,
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub overlays: Vec<Overlay>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SkillsError {
    #[error("skill not found: {0}")]
    SkillNotFound(String),
    /// Go `ErrSkillsRegistryMissing`. Only produced by Go's nil-receiver
    /// guards, which are unrepresentable in Rust; kept for surface parity.
    #[error("skills registry is not configured")]
    RegistryMissing,
    #[error("resolve user home: {0}")]
    HomeResolution(String),
    #[error("read skills root {0}: {1}")]
    ReadRoot(String, String),
    #[error("read skill {0}: {1}")]
    ReadSkill(String, String),
    #[error("read overlay {0}: {1}")]
    ReadOverlay(String, String),
    #[error("walk skill files {0}: {1}")]
    WalkSkillFiles(String, String),
    #[error("read {0}: {1}")]
    ReadFile(String, String),
    #[error("decode {0}: {1}")]
    Decode(String, String),
    #[error("resolve executable skill secret {0}: {1}")]
    ResolveSecret(String, String),
    #[error("resolve executable skill secret {0}: empty value")]
    EmptySecretValue(String),
    #[error("tenant secret manager is required")]
    SecretManagerRequired,
    #[error("tenant context required")]
    TenantContextRequired,
}

#[derive(Default)]
struct RegistryInner {
    snapshot: Snapshot,
    index: HashMap<String, Skill>,
}

/// File-system skill registry (Go `skills.Registry`).
///
/// The snapshot and normalized-id index are guarded by a
/// `parking_lot::RwLock` (workspace convention). Read methods return
/// owned clones, matching the Go copy-on-read semantics.
pub struct Registry {
    home_root: String,
    data_root: String,
    inner: parking_lot::RwLock<RegistryInner>,
}

impl Registry {
    /// Go `NewRegistry`: resolves the user home directory and uses
    /// `<home>/.agents` as the home root. Requires `HOME` to be set.
    #[must_use]
    pub fn new(data_root: &str) -> Result<Self, SkillsError> {
        let home_dir =
            std::env::var("HOME").map_err(|err| SkillsError::HomeResolution(err.to_string()))?;
        let home_root = Path::new(&home_dir).join(".agents");
        Self::with_roots(&home_root.to_string_lossy(), data_root)
    }

    /// Go `NewRegistryWithRoots`: both roots are trimmed; the registry is
    /// loaded immediately (an error here fails construction).
    #[must_use]
    pub fn with_roots(home_root: &str, data_root: &str) -> Result<Self, SkillsError> {
        let registry = Registry {
            home_root: home_root.trim().to_string(),
            data_root: data_root.trim().to_string(),
            inner: parking_lot::RwLock::new(RegistryInner::default()),
        };
        registry.reload()?;
        Ok(registry)
    }

    /// Go `Reload`: rescans both skill roots and both `AGENTS.md`
    /// overlays and replaces the snapshot and index atomically.
    pub fn reload(&self) -> Result<(), SkillsError> {
        let home_skills_root = Path::new(&self.home_root).join("skills");
        let data_skills_root = Path::new(&self.data_root).join("skills");
        let home_skills = scan_skills(
            &home_skills_root.to_string_lossy(),
            Source::Home,
            &self.data_root,
        )?;
        let data_skills = scan_skills(
            &data_skills_root.to_string_lossy(),
            Source::DataDir,
            &self.data_root,
        )?;
        let home_overlay =
            load_overlay(&Path::new(&self.home_root).join("AGENTS.md"), Source::Home)?;
        let data_overlay = load_overlay(
            &Path::new(&self.data_root).join("AGENTS.md"),
            Source::DataDir,
        )?;

        let mut index: HashMap<String, Skill> = HashMap::new();
        for skill in &home_skills {
            index.insert(normalize_skill_id(&skill.skill_id), skill.clone());
        }
        for skill in &data_skills {
            index.insert(normalize_skill_id(&skill.skill_id), skill.clone());
        }

        let mut skills_list: Vec<Skill> = index.values().cloned().collect();
        skills_list.sort_by(|a, b| a.skill_id.cmp(&b.skill_id));

        let mut overlays = Vec::with_capacity(2);
        if let Some(overlay) = home_overlay {
            overlays.push(overlay);
        }
        if let Some(overlay) = data_overlay {
            overlays.push(overlay);
        }

        let mut inner = self.inner.write();
        inner.index = index;
        inner.snapshot = Snapshot {
            loaded_at: Utc::now(),
            skills: skills_list,
            overlays,
        };
        Ok(())
    }

    /// Go `Snapshot`: owned clone of the current snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.inner.read().snapshot.clone()
    }

    /// Go `List`: the snapshot's skills (sorted by skill id).
    #[must_use]
    pub fn list(&self) -> Vec<Skill> {
        self.snapshot().skills
    }

    /// Go `Overlays`: the snapshot's overlays (home, then data dir).
    #[must_use]
    pub fn overlays(&self) -> Vec<Overlay> {
        self.snapshot().overlays
    }

    /// Go `Get`: returns the skill by id (matched case-insensitively and
    /// trimmed), or `None`.
    #[must_use]
    pub fn get(&self, skill_id: &str) -> Option<Skill> {
        self.inner
            .read()
            .index
            .get(&normalize_skill_id(skill_id))
            .cloned()
    }

    /// Go `ResolveSelected`: resolves the given skill ids in input order,
    /// deduplicating by normalized id and skipping empty ids. Fails with
    /// `SkillsError::SkillNotFound` on the first unknown id.
    pub fn resolve_selected(&self, skill_ids: &[String]) -> Result<Vec<Skill>, SkillsError> {
        let inner = self.inner.read();
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for item in skill_ids {
            let normalized = normalize_skill_id(item);
            if normalized.is_empty() {
                continue;
            }
            if seen.contains(&normalized) {
                continue;
            }
            let skill = inner
                .index
                .get(&normalized)
                .cloned()
                .ok_or_else(|| SkillsError::SkillNotFound(item.trim().to_string()))?;
            selected.push(skill);
            seen.insert(normalized);
        }
        Ok(selected)
    }
}

/// Resolves executable-skill secret refs against the data-dir
/// `skill-secrets.json` file (Go `ResolveExecutableSkillSecrets`).
/// Empty refs are skipped; refs without a non-empty value are omitted.
pub fn resolve_executable_skill_secrets(
    secret_root: &str,
    secret_refs: &[String],
) -> Result<HashMap<String, String>, SkillsError> {
    let mut items = HashMap::new();
    if secret_refs.is_empty() {
        return Ok(items);
    }
    let values = load_executable_skill_secret_file(secret_root)?;
    for secret_ref in secret_refs {
        let trimmed = secret_ref.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(value) = values.get(trimmed) {
            if !value.trim().is_empty() {
                items.insert(trimmed.to_string(), value.clone());
            }
        }
    }
    Ok(items)
}

/// Resolves executable-skill secret refs through the tenant secret manager
/// for the active tenant context (Go `ResolveExecutableSkillSecretsForTenant`).
/// This is the only async surface: it forwards to the async `kura-secrets`
/// manager. Fails closed when no tenant context is installed or when the
/// manager is absent.
pub async fn resolve_executable_skill_secrets_for_tenant(
    secret_manager: Option<&kura_secrets::Manager>,
    secret_refs: &[String],
) -> Result<HashMap<String, String>, SkillsError> {
    let refs = clean_strings(secret_refs);
    let mut items = HashMap::new();
    if refs.is_empty() {
        return Ok(items);
    }
    let Some(secret_manager) = secret_manager else {
        return Err(SkillsError::SecretManagerRequired);
    };
    let tenant_id =
        kura_identity::tenantctx::require().map_err(|_| SkillsError::TenantContextRequired)?;
    for secret_ref in &refs {
        let secret = secret_manager
            .resolve(kura_secrets::ResolveInput {
                tenant_id: tenant_id.clone(),
                secret_ref: secret_ref.clone(),
            })
            .await
            .map_err(|err| SkillsError::ResolveSecret(secret_ref.clone(), err.to_string()))?;
        if secret.value.trim().is_empty() {
            return Err(SkillsError::EmptySecretValue(secret_ref.clone()));
        }
        items.insert(secret_ref.clone(), secret.value);
    }
    Ok(items)
}

// ---------------------------------------------------------------------------
// Filesystem scanning
// ---------------------------------------------------------------------------

fn scan_skills(root: &str, source: Source, secret_root: &str) -> Result<Vec<Skill>, SkillsError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(SkillsError::ReadRoot(root.to_string(), err.to_string())),
    };
    let mut entries: Vec<_> = match entries.collect::<Result<Vec<_>, _>>() {
        Ok(entries) => entries,
        Err(err) => return Err(SkillsError::ReadRoot(root.to_string(), err.to_string())),
    };
    // Go os.ReadDir returns entries sorted by name.
    entries.sort_by_key(|entry| entry.file_name());

    let mut skills_list = Vec::new();
    for entry in entries {
        let is_dir = match entry.file_type() {
            Ok(file_type) => file_type.is_dir(),
            Err(err) => return Err(SkillsError::ReadRoot(root.to_string(), err.to_string())),
        };
        if !is_dir {
            continue;
        }
        let skill_root = entry.path().to_string_lossy().into_owned();
        let skill = load_skill(&skill_root, source, secret_root)?;
        if skill.skill_id.is_empty() {
            continue;
        }
        skills_list.push(skill);
    }
    Ok(skills_list)
}

fn load_skill(skill_root: &str, source: Source, secret_root: &str) -> Result<Skill, SkillsError> {
    let instruction_path = Path::new(skill_root).join("SKILL.md");
    let (content, _stat) = match read_file_with_stat(&instruction_path) {
        Ok(value) => value,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Skill::default()),
        Err(err) => {
            return Err(SkillsError::ReadSkill(
                instruction_path.to_string_lossy().into_owned(),
                err.to_string(),
            ));
        }
    };

    let (frontmatter_raw, frontmatter, body) = parse_skill_frontmatter(&content);
    let frontmatter_name = frontmatter
        .get("name")
        .map(|name| name.trim())
        .unwrap_or("");
    let skill_name = if frontmatter_name.is_empty() {
        Path::new(skill_root)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        frontmatter_name.to_string()
    };
    let skill_id = normalize_skill_id(&skill_name);
    if skill_id.is_empty() {
        return Ok(Skill::default());
    }

    let files = bundled_files(skill_root)?;
    let (execution_manifest, availability_status, availability_reason) =
        parse_executable_manifest(skill_root, secret_root, &frontmatter);

    Ok(Skill {
        skill_id: skill_id.clone(),
        name: skill_name.trim().to_string(),
        description: frontmatter
            .get("description")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        source,
        root_path: Path::new(skill_root)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .filter(|parent| !parent.is_empty())
            .unwrap_or_else(|| ".".to_string()),
        skill_path: skill_root.to_string(),
        instruction_path: instruction_path.to_string_lossy().into_owned(),
        frontmatter,
        frontmatter_raw,
        body: body.trim().to_string(),
        files,
        execution_manifest,
        availability_status,
        availability_reason,
        sandbox: build_skill_sandbox_view(&skill_id, skill_root),
    })
}

fn load_overlay(path: &Path, source: Source) -> Result<Option<Overlay>, SkillsError> {
    let (content, stat) = match read_file_with_stat(path) {
        Ok(value) => value,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(SkillsError::ReadOverlay(
                path.to_string_lossy().into_owned(),
                err.to_string(),
            ));
        }
    };
    let modified_at = match stat.modified() {
        Ok(system_time) => DateTime::<Utc>::from(system_time),
        Err(err) => {
            return Err(SkillsError::ReadOverlay(
                path.to_string_lossy().into_owned(),
                err.to_string(),
            ));
        }
    };
    Ok(Some(Overlay {
        overlay_id: format!("{}_agents", source.as_str()),
        source,
        path: path.to_string_lossy().into_owned(),
        size_bytes: stat.len() as i64,
        modified_at,
        body: content.trim().to_string(),
    }))
}

fn read_file_with_stat(path: &Path) -> std::io::Result<(String, std::fs::Metadata)> {
    let info = std::fs::metadata(path)?;
    let content = std::fs::read_to_string(path)?;
    Ok((content, info))
}

fn bundled_files(skill_root: &str) -> Result<Vec<File>, SkillsError> {
    let root = Path::new(skill_root);
    let mut files = Vec::new();
    walk_bundled_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Recursive equivalent of Go's `filepath.WalkDir` (lexical order at each
/// level, symlinks not followed, `SKILL.md` excluded).
fn walk_bundled_files(dir: &Path, root: &Path, files: &mut Vec<File>) -> Result<(), SkillsError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            return Err(SkillsError::WalkSkillFiles(
                dir.to_string_lossy().into_owned(),
                err.to_string(),
            ));
        }
    };
    let mut entries: Vec<_> = match entries.collect::<Result<Vec<_>, _>>() {
        Ok(entries) => entries,
        Err(err) => {
            return Err(SkillsError::WalkSkillFiles(
                dir.to_string_lossy().into_owned(),
                err.to_string(),
            ));
        }
    };
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                return Err(SkillsError::WalkSkillFiles(
                    path.to_string_lossy().into_owned(),
                    err.to_string(),
                ));
            }
        };
        if file_type.is_dir() {
            walk_bundled_files(&path, root, files)?;
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
            continue;
        }
        let size_bytes = match entry.metadata() {
            Ok(meta) => meta.len() as i64,
            Err(err) => {
                return Err(SkillsError::WalkSkillFiles(
                    path.to_string_lossy().into_owned(),
                    err.to_string(),
                ));
            }
        };
        let relative = match path.strip_prefix(root) {
            Ok(relative) => relative,
            Err(err) => {
                return Err(SkillsError::WalkSkillFiles(
                    path.to_string_lossy().into_owned(),
                    err.to_string(),
                ));
            }
        };
        files.push(File {
            path: relative.to_string_lossy().replace('\\', "/"),
            size_bytes,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Executable manifest parsing and availability
// ---------------------------------------------------------------------------

const EXECUTION_PREFIX: &str = "execution.";

fn parse_executable_manifest(
    skill_root: &str,
    secret_root: &str,
    frontmatter: &HashMap<String, String>,
) -> (Option<ExecutableManifest>, SkillAvailabilityStatus, String) {
    let has_execution_keys = frontmatter
        .keys()
        .any(|key| key.trim().starts_with(EXECUTION_PREFIX));
    if !has_execution_keys {
        return (None, SkillAvailabilityStatus::NotExecutable, String::new());
    }

    let field = |key: &str| -> &str { frontmatter.get(key).map(String::as_str).unwrap_or("") };

    let network_mode_raw = first_non_empty(&[field("execution.network_mode"), "deny"]);
    let approval_mode_raw = first_non_empty(&[field("execution.approval_mode"), "ask"]);

    let mut manifest = ExecutableManifest {
        entrypoint: field("execution.entrypoint").trim().to_string(),
        args: split_csv(field("execution.args")),
        profile_id: field("execution.profile_id").trim().to_string(),
        read_roots: resolve_skill_paths(skill_root, &split_csv(field("execution.read_roots"))),
        write_roots: resolve_skill_paths(skill_root, &split_csv(field("execution.write_roots"))),
        network_mode: parse_network_mode(&network_mode_raw),
        allowed_hosts: split_csv(field("execution.allowed_hosts")),
        allowed_ports: split_csv_ints(field("execution.allowed_ports")),
        secret_refs: split_csv(field("execution.secret_refs")),
        approval_mode: parse_approval_mode(&approval_mode_raw),
        required_enforcement_strength: first_non_empty(&[
            field("execution.required_enforcement_strength"),
            "declared_only",
        ]),
        ..ExecutableManifest::default()
    };
    let working_dir = field("execution.working_dir").trim();
    if !working_dir.is_empty() {
        manifest.working_dir = resolve_skill_path(skill_root, working_dir);
    }
    let timeout_value = field("execution.timeout_ms").trim();
    if !timeout_value.is_empty() {
        match timeout_value.parse::<i64>() {
            Ok(timeout_ms) if timeout_ms > 0 => manifest.timeout_ms = timeout_ms,
            _ => {
                return (
                    Some(manifest),
                    SkillAvailabilityStatus::Unavailable,
                    "execution.timeout_ms must be a positive integer".to_string(),
                );
            }
        }
    }

    match network_mode_raw.trim() {
        "deny" | "allow_list" | "full" => {}
        _ => {
            return (
                Some(manifest),
                SkillAvailabilityStatus::Unavailable,
                "execution.network_mode is invalid".to_string(),
            );
        }
    }
    match approval_mode_raw.trim() {
        "allow" | "ask" | "deny" => {}
        _ => {
            return (
                Some(manifest),
                SkillAvailabilityStatus::Unavailable,
                "execution.approval_mode is invalid".to_string(),
            );
        }
    }
    if manifest.entrypoint.is_empty() {
        return (
            Some(manifest),
            SkillAvailabilityStatus::Unavailable,
            "execution.entrypoint is required".to_string(),
        );
    }
    if manifest.profile_id.is_empty() {
        return (
            Some(manifest),
            SkillAvailabilityStatus::Unavailable,
            "execution.profile_id is required".to_string(),
        );
    }
    manifest.backend_kind = backend_kind_for_profile(&manifest.profile_id);
    if !supports_required_strength(&manifest.required_enforcement_strength) {
        return (
            Some(manifest),
            SkillAvailabilityStatus::Unavailable,
            "execution.required_enforcement_strength exceeds current backend support".to_string(),
        );
    }
    if let Some(resolved) = resolve_manifest_entrypoint(skill_root, &manifest.entrypoint) {
        manifest.entrypoint = resolved;
    } else {
        return (
            Some(manifest),
            SkillAvailabilityStatus::Unavailable,
            "execution.entrypoint does not resolve to an executable target".to_string(),
        );
    }
    if !manifest.working_dir.is_empty() {
        let is_dir = std::fs::metadata(&manifest.working_dir)
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        if !is_dir {
            return (
                Some(manifest),
                SkillAvailabilityStatus::Unavailable,
                "execution.working_dir does not exist".to_string(),
            );
        }
    }
    match resolve_executable_skill_secrets(secret_root, &manifest.secret_refs) {
        Ok(resolved_secrets) => {
            for secret_ref in manifest.secret_refs.clone() {
                if secret_ref.trim().is_empty() {
                    return (
                        Some(manifest),
                        SkillAvailabilityStatus::Unavailable,
                        "execution.secret_refs contains an empty entry".to_string(),
                    );
                }
                if !resolved_secrets.contains_key(secret_ref.trim()) {
                    return (
                        Some(manifest),
                        SkillAvailabilityStatus::Unavailable,
                        format!(
                            "secret ref {} is unavailable in {}",
                            secret_ref,
                            effective_environment()
                        ),
                    );
                }
            }
        }
        Err(err) => {
            return (
                Some(manifest),
                SkillAvailabilityStatus::Unavailable,
                format!("executable skill secrets are unavailable: {err}"),
            );
        }
    }
    (
        Some(manifest),
        SkillAvailabilityStatus::Available,
        String::new(),
    )
}

fn load_executable_skill_secret_file(
    secret_root: &str,
) -> Result<HashMap<String, String>, SkillsError> {
    let path = executable_skill_secrets_path(secret_root);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(SkillsError::ReadFile(path.clone(), err.to_string())),
    };
    let mut values: HashMap<String, String> = serde_json::from_str(&content)
        .map_err(|err| SkillsError::Decode(path.clone(), err.to_string()))?;
    // Go: keys are trimmed; empty keys are dropped, re-keyed values win.
    let keys: Vec<String> = values.keys().cloned().collect();
    for key in keys {
        let trimmed_key = key.trim().to_string();
        if trimmed_key.is_empty() {
            values.remove(&key);
            continue;
        }
        if trimmed_key != key {
            if let Some(value) = values.remove(&key) {
                values.insert(trimmed_key, value);
            }
        }
    }
    Ok(values)
}

fn executable_skill_secrets_path(secret_root: &str) -> String {
    Path::new(secret_root.trim())
        .join(EXECUTABLE_SKILL_SECRETS_FILE_NAME)
        .to_string_lossy()
        .into_owned()
}

/// Go `resolveManifestEntrypoint`: absolute paths must exist; relative
/// paths containing `/` or starting with `.` resolve against the skill
/// root and must exist; bare command names pass through.
fn resolve_manifest_entrypoint(skill_root: &str, value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return std::fs::metadata(path).ok().map(|_| trimmed.to_string());
    }
    if trimmed.contains('/') || trimmed.starts_with('.') {
        let resolved = resolve_skill_path(skill_root, trimmed);
        return std::fs::metadata(&resolved).ok().map(|_| resolved);
    }
    Some(trimmed.to_string())
}

fn supports_required_strength(value: &str) -> bool {
    matches!(
        value.trim(),
        "" | "declared_only" | "subprocess" | "containerized" | "docker"
    )
}

fn backend_kind_for_profile(profile_id: &str) -> kura_sandbox::BackendKind {
    if profile_id.trim() == kura_sandbox::PROFILE_ID_DOCKER_DEFAULT {
        kura_sandbox::BackendKind::Docker
    } else {
        kura_sandbox::BackendKind::Subprocess
    }
}

/// Cleans and, for relative values, joins against the skill root
/// (Go `resolveSkillPath` / `filepath.Clean`).
fn resolve_skill_path(skill_root: &str, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let path = Path::new(trimmed);
    let resolved = if path.is_absolute() {
        clean_path(path)
    } else {
        clean_path(&Path::new(skill_root).join(trimmed))
    };
    resolved.to_string_lossy().into_owned()
}

fn resolve_skill_paths(skill_root: &str, values: &[String]) -> Vec<String> {
    let mut resolved = Vec::new();
    for value in values {
        let item = resolve_skill_path(skill_root, value);
        if !item.is_empty() {
            resolved.push(item);
        }
    }
    resolved
}

/// `filepath.Clean` equivalent: rebuilds the path from its components,
/// collapsing `.` and resolving `..`.
fn clean_path(path: &Path) -> std::path::PathBuf {
    path.components().collect()
}

fn split_csv(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn clean_strings(values: &[String]) -> Vec<String> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for value in values {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() || seen.contains(&trimmed) {
            continue;
        }
        seen.insert(trimmed.clone());
        items.push(trimmed);
    }
    items
}

fn split_csv_ints(value: &str) -> Vec<i64> {
    let items = split_csv(value);
    let mut ports = Vec::new();
    for item in items {
        match item.parse::<i64>() {
            Ok(parsed) => ports.push(parsed),
            // Go returns nil on the first non-numeric entry.
            Err(_) => return Vec::new(),
        }
    }
    ports
}

fn effective_environment() -> String {
    match std::env::var("KURA_ENV") {
        Ok(value) if value.trim() == "prod" => "prod".to_string(),
        _ => "test".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Sandbox declaration projection
// ---------------------------------------------------------------------------

/// Go `buildSkillSandboxView`: projects a `ConsumerContractView` for the
/// skill-selection operation and returns it as a JSON object (the Go code
/// round-trips the view through `json.Marshal`/`json.Unmarshal`).
fn build_skill_sandbox_view(
    skill_id: &str,
    skill_root: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let normalized = normalize_skill_id(skill_id);
    let view = kura_sandbox::ConsumerContractView {
        declaration: Some(kura_sandbox::ConsumerRequirementDeclaration {
            declaration_id: format!("skill:{normalized}:selection"),
            consumer_kind: kura_sandbox::ConsumerKind::Skill,
            consumer_id: normalized,
            operation_kind: "skill_selection".to_string(),
            profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
            execution_mode: kura_sandbox::ExecutionMode::DeclarationOnly,
            allowed_backend_kinds: vec![kura_sandbox::BackendKind::Subprocess],
            read_roots: vec![skill_root.to_string()],
            write_roots: Vec::new(),
            network_mode: Some(kura_sandbox::NetworkMode::Deny),
            allowed_hosts: Vec::new(),
            allowed_ports: Vec::new(),
            allow_loopback: false,
            secret_refs: Vec::new(),
            approval_mode: Some(kura_sandbox::ApprovalMode::Allow),
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            source: kura_sandbox::Source::Builtin,
        }),
        ..kura_sandbox::ConsumerContractView::default()
    };
    serde_json::to_value(&view)
        .ok()
        .and_then(|value| value.as_object().cloned())
}

// ---------------------------------------------------------------------------
// Frontmatter parsing helpers
// ---------------------------------------------------------------------------

fn parse_skill_frontmatter(raw: &str) -> (String, HashMap<String, String>, String) {
    let trimmed = raw.replace("\r\n", "\n");
    if !trimmed.starts_with("---\n") {
        return (String::new(), HashMap::new(), trimmed);
    }
    let rest = trimmed.strip_prefix("---\n").unwrap_or(&trimmed);
    let Some(end) = rest.find("\n---\n") else {
        return (String::new(), HashMap::new(), trimmed);
    };
    let header = &rest[..end];
    let body = rest[end..].strip_prefix("\n---\n").unwrap_or(&rest[end..]);
    let mut fields = HashMap::new();
    for line in header.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        fields.insert(key.trim().to_string(), unquote_yaml_scalar(value.trim()));
    }
    (header.to_string(), fields, body.to_string())
}

fn unquote_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let mut chars = trimmed.chars();
        let first = chars.next().unwrap();
        let last = chars.next_back().unwrap();
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return chars.as_str().trim().to_string();
        }
    }
    trimmed.to_string()
}

fn normalize_skill_id(value: &str) -> String {
    value.trim().to_lowercase()
}

fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

/// Unknown network-mode values fall back to Deny as a placeholder; validity
/// is enforced separately against the raw frontmatter value in
/// `parse_executable_manifest` (Go can carry the raw string in its
/// untyped enum, Rust cannot).
fn parse_network_mode(value: &str) -> kura_sandbox::NetworkMode {
    match value.trim() {
        "deny" => kura_sandbox::NetworkMode::Deny,
        "allow_list" => kura_sandbox::NetworkMode::AllowList,
        "full" => kura_sandbox::NetworkMode::Full,
        _ => kura_sandbox::NetworkMode::Deny,
    }
}

/// Unknown approval-mode values fall back to Ask as a placeholder; validity
/// is enforced separately (see `parse_network_mode`).
fn parse_approval_mode(value: &str) -> kura_sandbox::ApprovalMode {
    match value.trim() {
        "allow" => kura_sandbox::ApprovalMode::Allow,
        "ask" => kura_sandbox::ApprovalMode::Ask,
        "deny" => kura_sandbox::ApprovalMode::Deny,
        _ => kura_sandbox::ApprovalMode::Ask,
    }
}

#[must_use]
fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
