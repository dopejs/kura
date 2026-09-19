//! Port of `daemon/internal/mcp/catalog.go`: the bundled MCP catalog entries, their
//! availability evaluation, the `mcp-secrets.json` file resolver, and the
//! install-spec fingerprinting helpers.

use std::collections::HashMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::types::{
    AvailabilityStatus, CatalogEntry, CatalogInstallInput, CatalogInstallSnapshot,
    CatalogInstallSupport, CatalogPrerequisite, CatalogSecretRequirement, CreateServerInput,
    Declaration, InstallMethod, OriginKind, TransportKind, normalize_declaration,
};
use crate::{clean_strings, first_non_empty};

/// Go `mcpSecretsFileName`.
pub const MCP_SECRETS_FILE_NAME: &str = "mcp-secrets.json";

/// Go `bundledCatalogEntries`: the five bundled catalog entries (filesystem, context7,
/// github, postgres, slack) with per-entry availability, sorted by id.
pub fn bundled_catalog_entries(cfg: &kura_config::Config) -> Vec<CatalogEntry> {
    let environment = crate::environment_scope(cfg.environment);
    let data_dir = cfg.data_dir.clone();

    let filesystem_spec = CreateServerInput {
        display_name: "Filesystem".to_string(),
        enabled: true,
        origin_kind: OriginKind::Catalog,
        catalog_entry_id: "filesystem".to_string(),
        environment_scope: environment.clone(),
        install_method: InstallMethod::Api,
        sandbox_profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
        declaration_id: "mcp_server:filesystem:lifecycle.start".to_string(),
        declaration: Some(Declaration {
            execution_mode: kura_sandbox::ExecutionMode::Subprocess,
            allowed_backend_kinds: vec![kura_sandbox::BackendKind::Subprocess],
            read_roots: vec![data_dir.clone()],
            write_roots: vec![data_dir.clone()],
            network_mode: kura_sandbox::NetworkMode::Deny,
            approval_mode: kura_sandbox::ApprovalMode::Allow,
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            ..Declaration::default()
        }),
        transport_kind: TransportKind::Stdio,
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-filesystem".to_string(),
            data_dir.clone(),
        ],
        working_dir: data_dir.clone(),
        auto_restart: true,
        ..CreateServerInput::default()
    };

    let github_spec = CreateServerInput {
        display_name: "GitHub".to_string(),
        enabled: true,
        origin_kind: OriginKind::Catalog,
        catalog_entry_id: "github".to_string(),
        environment_scope: environment.clone(),
        install_method: InstallMethod::Api,
        sandbox_profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
        declaration_id: "mcp_server:github:lifecycle.start".to_string(),
        declaration: filesystem_spec.declaration.clone(),
        transport_kind: TransportKind::Stdio,
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-github".to_string(),
        ],
        working_dir: data_dir.clone(),
        secret_refs: vec!["GITHUB_TOKEN".to_string()],
        auto_restart: true,
        ..CreateServerInput::default()
    };

    let postgres_spec = CreateServerInput {
        display_name: "Postgres".to_string(),
        enabled: true,
        origin_kind: OriginKind::Catalog,
        catalog_entry_id: "postgres".to_string(),
        environment_scope: environment.clone(),
        install_method: InstallMethod::Api,
        sandbox_profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
        declaration_id: "mcp_server:postgres:lifecycle.start".to_string(),
        declaration: Some(Declaration {
            execution_mode: kura_sandbox::ExecutionMode::Subprocess,
            allowed_backend_kinds: vec![kura_sandbox::BackendKind::Subprocess],
            read_roots: vec![data_dir.clone()],
            write_roots: vec![data_dir.clone()],
            network_mode: kura_sandbox::NetworkMode::AllowList,
            allow_loopback: true,
            allowed_hosts: vec!["localhost".to_string(), "127.0.0.1".to_string()],
            approval_mode: kura_sandbox::ApprovalMode::Allow,
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            ..Declaration::default()
        }),
        transport_kind: TransportKind::Stdio,
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-postgres".to_string(),
        ],
        working_dir: data_dir.clone(),
        secret_refs: vec!["POSTGRES_DSN".to_string()],
        auto_restart: true,
        ..CreateServerInput::default()
    };

    let slack_spec = CreateServerInput {
        display_name: "Slack".to_string(),
        enabled: true,
        origin_kind: OriginKind::Catalog,
        catalog_entry_id: "slack".to_string(),
        environment_scope: environment.clone(),
        install_method: InstallMethod::Api,
        sandbox_profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
        declaration_id: "mcp_server:slack:lifecycle.start".to_string(),
        declaration: Some(Declaration {
            execution_mode: kura_sandbox::ExecutionMode::Subprocess,
            allowed_backend_kinds: vec![kura_sandbox::BackendKind::Subprocess],
            read_roots: vec![data_dir.clone()],
            write_roots: vec![data_dir.clone()],
            network_mode: kura_sandbox::NetworkMode::AllowList,
            allowed_hosts: vec!["slack.com".to_string(), "api.slack.com".to_string()],
            allowed_ports: vec![443],
            approval_mode: kura_sandbox::ApprovalMode::Allow,
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            ..Declaration::default()
        }),
        transport_kind: TransportKind::Stdio,
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-slack".to_string(),
        ],
        working_dir: data_dir.clone(),
        secret_refs: vec!["SLACK_BOT_TOKEN".to_string()],
        auto_restart: true,
        ..CreateServerInput::default()
    };

    let context7_spec = CreateServerInput {
        display_name: "Context7".to_string(),
        enabled: true,
        origin_kind: OriginKind::Catalog,
        catalog_entry_id: "context7".to_string(),
        environment_scope: environment.clone(),
        install_method: InstallMethod::Api,
        sandbox_profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
        declaration_id: "mcp_server:context7:lifecycle.start".to_string(),
        declaration: Some(Declaration {
            execution_mode: kura_sandbox::ExecutionMode::DeclarationOnly,
            allowed_backend_kinds: vec![kura_sandbox::BackendKind::Subprocess],
            network_mode: kura_sandbox::NetworkMode::AllowList,
            allowed_hosts: vec!["mcp.context7.com".to_string()],
            allowed_ports: vec![443],
            approval_mode: kura_sandbox::ApprovalMode::Allow,
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            ..Declaration::default()
        }),
        transport_kind: TransportKind::StreamableHTTP,
        endpoint: "https://mcp.context7.com/mcp".to_string(),
        auto_restart: true,
        ..CreateServerInput::default()
    };

    let mut items = vec![
        CatalogEntry {
            id: "filesystem".to_string(),
            display_name: "Filesystem".to_string(),
            description: "Local project filesystem access for the active test workspace."
                .to_string(),
            transport_kind: TransportKind::Stdio,
            source_kind: "bundled".to_string(),
            tags: vec![
                "local".to_string(),
                "filesystem".to_string(),
                "starter".to_string(),
            ],
            immediate_use: false,
            prerequisites: vec![CatalogPrerequisite {
                kind: "binary".to_string(),
                name: "npx".to_string(),
                required: true,
                description: "Node.js with npx available on PATH".to_string(),
            }],
            environment_eligibility: vec!["test".to_string(), "prod".to_string()],
            install_support: CatalogInstallSupport {
                script_supported: true,
                script_args: vec!["filesystem".to_string()],
            },
            default_install_spec: filesystem_spec,
            ..CatalogEntry::default()
        },
        CatalogEntry {
            id: "context7".to_string(),
            display_name: "Context7".to_string(),
            description: "Remote docs and library context over streamable-http.".to_string(),
            transport_kind: TransportKind::StreamableHTTP,
            source_kind: "bundled".to_string(),
            tags: vec![
                "remote".to_string(),
                "docs".to_string(),
                "starter".to_string(),
            ],
            immediate_use: true,
            prerequisites: vec![CatalogPrerequisite {
                kind: "endpoint".to_string(),
                name: "streamable-http".to_string(),
                required: true,
                description: "Reachable streamable-http MCP endpoint".to_string(),
            }],
            environment_eligibility: vec!["test".to_string(), "prod".to_string()],
            install_support: CatalogInstallSupport {
                script_supported: true,
                script_args: vec!["context7".to_string()],
            },
            default_install_spec: context7_spec,
            ..CatalogEntry::default()
        },
        CatalogEntry {
            id: "github".to_string(),
            display_name: "GitHub".to_string(),
            description:
                "GitHub repository and issue access through a credential-backed MCP server."
                    .to_string(),
            transport_kind: TransportKind::Stdio,
            source_kind: "bundled".to_string(),
            tags: vec![
                "credentials".to_string(),
                "git".to_string(),
                "remote".to_string(),
            ],
            immediate_use: false,
            prerequisites: vec![CatalogPrerequisite {
                kind: "binary".to_string(),
                name: "npx".to_string(),
                required: true,
                description: "Node.js with npx available on PATH".to_string(),
            }],
            secret_requirements: vec![CatalogSecretRequirement {
                secret_ref: "GITHUB_TOKEN".to_string(),
                required: true,
                description: "GitHub personal access token".to_string(),
            }],
            environment_eligibility: vec!["test".to_string(), "prod".to_string()],
            install_support: CatalogInstallSupport {
                script_supported: true,
                script_args: vec!["github".to_string()],
            },
            default_install_spec: github_spec,
            ..CatalogEntry::default()
        },
        CatalogEntry {
            id: "postgres".to_string(),
            display_name: "Postgres".to_string(),
            description: "Database inspection and query access for a configured Postgres instance."
                .to_string(),
            transport_kind: TransportKind::Stdio,
            source_kind: "bundled".to_string(),
            tags: vec!["database".to_string(), "credentials".to_string()],
            immediate_use: false,
            prerequisites: vec![CatalogPrerequisite {
                kind: "binary".to_string(),
                name: "npx".to_string(),
                required: true,
                description: "Node.js with npx available on PATH".to_string(),
            }],
            secret_requirements: vec![CatalogSecretRequirement {
                secret_ref: "POSTGRES_DSN".to_string(),
                required: true,
                description: "Database connection string".to_string(),
            }],
            environment_eligibility: vec!["test".to_string(), "prod".to_string()],
            install_support: CatalogInstallSupport {
                script_supported: true,
                script_args: vec!["postgres".to_string()],
            },
            default_install_spec: postgres_spec,
            ..CatalogEntry::default()
        },
        CatalogEntry {
            id: "slack".to_string(),
            display_name: "Slack".to_string(),
            description: "Slack workspace access for channels, threads, and knowledge retrieval."
                .to_string(),
            transport_kind: TransportKind::Stdio,
            source_kind: "bundled".to_string(),
            tags: vec![
                "credentials".to_string(),
                "chat".to_string(),
                "remote".to_string(),
            ],
            immediate_use: false,
            prerequisites: vec![CatalogPrerequisite {
                kind: "binary".to_string(),
                name: "npx".to_string(),
                required: true,
                description: "Node.js with npx available on PATH".to_string(),
            }],
            secret_requirements: vec![CatalogSecretRequirement {
                secret_ref: "SLACK_BOT_TOKEN".to_string(),
                required: true,
                description: "Slack bot token".to_string(),
            }],
            environment_eligibility: vec!["test".to_string(), "prod".to_string()],
            install_support: CatalogInstallSupport {
                script_supported: true,
                script_args: vec!["slack".to_string()],
            },
            default_install_spec: slack_spec,
            ..CatalogEntry::default()
        },
    ];

    for entry in &mut items {
        let (status, reason) = evaluate_catalog_availability(cfg, entry);
        entry.availability_status = status;
        entry.availability_reason = reason;
    }
    items.sort_by(|a, b| a.id.cmp(&b.id));
    items
}

/// Go `evaluateCatalogAvailability`: prerequisite checks plus the install-spec
/// availability evaluation.
#[must_use]
pub fn evaluate_catalog_availability(
    cfg: &kura_config::Config,
    entry: &CatalogEntry,
) -> (AvailabilityStatus, String) {
    for requirement in &entry.prerequisites {
        if !requirement.required {
            continue;
        }
        match requirement.kind.as_str() {
            "binary" => {
                if look_path(&requirement.name).is_none() {
                    let message = format!("{} is unavailable", requirement.name);
                    return (
                        AvailabilityStatus::Unavailable,
                        first_non_empty(&[requirement.description.as_str(), &message]),
                    );
                }
            }
            "endpoint" => {
                if entry.transport_kind == TransportKind::StreamableHTTP
                    && entry.default_install_spec.endpoint.trim().is_empty()
                {
                    return (
                        AvailabilityStatus::Unsupported,
                        "streamable-http endpoint is not configured".to_string(),
                    );
                }
            }
            _ => {}
        }
    }
    evaluate_catalog_install_spec_availability(
        cfg,
        &entry.default_install_spec,
        &entry.secret_requirements,
    )
}

/// Go `secretRefsFromRequirements`.
#[must_use]
pub fn secret_refs_from_requirements(items: &[CatalogSecretRequirement]) -> Vec<String> {
    items.iter().map(|item| item.secret_ref.clone()).collect()
}

/// Go `ResolveMCPSecrets`: reads `mcp-secrets.json` under the secret root. Missing
/// file resolves to an empty map (Go os.IsNotExist path).
pub fn resolve_mcp_secrets(
    secret_root: &str,
    secret_refs: &[String],
) -> Result<HashMap<String, String>, String> {
    let refs = clean_strings(secret_refs);
    if refs.is_empty() {
        return Ok(HashMap::new());
    }
    let path = Path::new(secret_root.trim()).join(MCP_SECRETS_FILE_NAME);
    let payload = match std::fs::read(&path) {
        Ok(payload) => payload,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(format!("read mcp secrets: {err}")),
    };
    let values: HashMap<String, String> =
        serde_json::from_slice(&payload).map_err(|e| format!("decode mcp secrets: {e}"))?;
    let mut resolved = HashMap::with_capacity(refs.len());
    for secret_ref in &refs {
        if let Some(value) = values.get(secret_ref) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                resolved.insert(secret_ref.clone(), trimmed.to_string());
            }
        }
    }
    Ok(resolved)
}

/// Go `evaluateCatalogInstallSpecAvailability`.
#[must_use]
pub fn evaluate_catalog_install_spec_availability(
    cfg: &kura_config::Config,
    spec: &CreateServerInput,
    requirements: &[CatalogSecretRequirement],
) -> (AvailabilityStatus, String) {
    match spec.transport_kind {
        TransportKind::Stdio => {
            if spec.command.trim().is_empty() {
                return (
                    AvailabilityStatus::Unavailable,
                    "stdio command is not configured".to_string(),
                );
            }
        }
        TransportKind::StreamableHTTP => {
            if spec.endpoint.trim().is_empty() {
                return (
                    AvailabilityStatus::Unsupported,
                    "streamable-http endpoint is not configured".to_string(),
                );
            }
        }
        _ => {
            return (
                AvailabilityStatus::Unsupported,
                "transport kind is unsupported".to_string(),
            );
        }
    }
    if requires_offline_verified_local_command(spec) {
        return (
            AvailabilityStatus::Unavailable,
            "default bundled stdio command requires a local command override because sandbox network is denied".to_string(),
        );
    }
    if requirements.is_empty() {
        return (AvailabilityStatus::Ready, String::new());
    }
    match resolve_mcp_secrets(&cfg.data_dir, &secret_refs_from_requirements(requirements)) {
        Err(err) => (AvailabilityStatus::Blocked, err),
        Ok(resolved) => {
            for requirement in requirements {
                if !requirement.required {
                    continue;
                }
                if !resolved.contains_key(&requirement.secret_ref) {
                    let message = format!("{} is required", requirement.secret_ref);
                    return (
                        AvailabilityStatus::Blocked,
                        first_non_empty(&[requirement.description.as_str(), &message]),
                    );
                }
            }
            (AvailabilityStatus::Ready, String::new())
        }
    }
}

/// Go `requiresOfflineVerifiedLocalCommand`.
#[must_use]
pub fn requires_offline_verified_local_command(spec: &CreateServerInput) -> bool {
    if spec.transport_kind != TransportKind::Stdio {
        return false;
    }
    let Some(declaration) = &spec.declaration else {
        return false;
    };
    if declaration.network_mode != kura_sandbox::NetworkMode::Deny {
        return false;
    }
    let command = spec.command.trim();
    if command != "npx" && command != "npm" {
        return false;
    }
    for arg in clean_strings(&spec.args) {
        if arg.starts_with('-') {
            continue;
        }
        return arg.contains("@modelcontextprotocol/");
    }
    false
}

/// Go `installSnapshotFromCreateSpec`.
#[must_use]
pub fn install_snapshot_from_create_spec(spec: &CreateServerInput) -> CatalogInstallSnapshot {
    CatalogInstallSnapshot {
        server_id: spec.server_id.trim().to_string(),
        display_name: spec.display_name.trim().to_string(),
        enabled: Some(spec.enabled),
        sandbox_profile_id: spec.sandbox_profile_id.trim().to_string(),
        command: spec.command.trim().to_string(),
        args: spec.args.clone(),
        endpoint: spec.endpoint.trim().to_string(),
        working_dir: spec.working_dir.trim().to_string(),
        secret_refs: clean_strings(&spec.secret_refs),
        install_method: spec.install_method,
    }
}

/// Go `catalogInstallInputFromSnapshot`.
#[must_use]
pub fn catalog_install_input_from_snapshot(
    snapshot: &CatalogInstallSnapshot,
) -> CatalogInstallInput {
    CatalogInstallInput {
        server_id: snapshot.server_id.trim().to_string(),
        display_name: snapshot.display_name.trim().to_string(),
        enabled: snapshot.enabled,
        sandbox_profile_id: snapshot.sandbox_profile_id.trim().to_string(),
        command: snapshot.command.trim().to_string(),
        args: snapshot.args.clone(),
        endpoint: snapshot.endpoint.trim().to_string(),
        working_dir: snapshot.working_dir.trim().to_string(),
        secret_refs: clean_strings(&snapshot.secret_refs),
    }
}

/// Go `fingerprintCreateServerSpec`: sha256 of the normalized create spec, hex
/// prefixed with `sha256:`.
#[must_use]
pub fn fingerprint_create_server_spec(spec: &CreateServerInput) -> String {
    let mut spec = spec.clone();
    spec.args = spec.args.clone();
    spec.secret_refs = clean_strings(&spec.secret_refs);
    if let Some(declaration) = &spec.declaration {
        let normalized = normalize_declaration(declaration.clone());
        spec.declaration = Some(normalized);
    }
    let Ok(payload) = serde_json::to_string(&spec) else {
        return String::new();
    };
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    format!("sha256:{digest:x}")
}

/// Go `exec.LookPath` approximation for the catalog "binary" prerequisite check.
#[must_use]
pub fn look_path(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if name.contains('/') || name.contains('\\') {
        return if Path::new(name).is_file() {
            Some(name.to_string())
        } else {
            None
        };
    }
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}
