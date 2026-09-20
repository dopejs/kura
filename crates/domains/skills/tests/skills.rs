//! Port of daemon/internal/skills/registry_test.go: registry behavior over
//! real temp-dir fixture trees plus JSON round-trip coverage.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use kura_skills::{
    Registry, Skill, SkillAvailabilityStatus, SkillsError, Source,
    resolve_executable_skill_secrets, resolve_executable_skill_secrets_for_tenant,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn write_file(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("create dirs");
    std::fs::write(path, body).expect("write fixture");
}

fn write_skill_fixture(dir: &Path, body: &str) {
    write_file(&dir.join("SKILL.md"), body);
}

fn write_executable_skill_secrets(data_root: &Path, values: &[(&str, &str)]) {
    let map: HashMap<String, String> = values
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let payload = serde_json::to_string(&map).expect("encode secret values");
    write_file(&data_root.join("skill-secrets.json"), &payload);
}

#[derive(Default)]
struct ExecutableFixtureOptions {
    description: String,
    approval_mode: String,
    secret_refs: Vec<String>,
    args: Vec<String>,
    timeout_ms: i64,
    script_body: String,
    profile_id: String,
    required_enforcement_strength: String,
}

fn first_value(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn write_valid_executable_skill(data_root: &Path, skill_id: &str, opts: &ExecutableFixtureOptions) {
    let description = first_value(&opts.description, "executable");
    let approval_mode = first_value(&opts.approval_mode, "ask");
    let script_body = first_value(&opts.script_body, "#!/bin/sh\nprintf ok");
    let profile_id = first_value(&opts.profile_id, "subprocess_default");

    let mut lines = vec![
        "---".to_string(),
        format!("name: {skill_id}"),
        format!("description: \"{description}\""),
        "execution.entrypoint: scripts/run.sh".to_string(),
        "execution.working_dir: .".to_string(),
        format!("execution.profile_id: {profile_id}"),
        "execution.read_roots: .".to_string(),
        "execution.write_roots: .".to_string(),
        "execution.network_mode: deny".to_string(),
        format!("execution.approval_mode: {approval_mode}"),
    ];
    if !opts.args.is_empty() {
        lines.push(format!("execution.args: {}", opts.args.join(",")));
    }
    if !opts.secret_refs.is_empty() {
        lines.push(format!(
            "execution.secret_refs: {}",
            opts.secret_refs.join(",")
        ));
    }
    if opts.timeout_ms > 0 {
        lines.push(format!("execution.timeout_ms: {}", opts.timeout_ms));
    }
    if !opts.required_enforcement_strength.trim().is_empty() {
        lines.push(format!(
            "execution.required_enforcement_strength: {}",
            opts.required_enforcement_strength
        ));
    }
    lines.push("---".to_string());
    lines.push("run it".to_string());

    let skill_dir = data_root.join("skills").join(skill_id);
    write_skill_fixture(&skill_dir, &lines.join("\n"));
    write_file(&skill_dir.join("scripts").join("run.sh"), &script_body);
}

fn write_invalid_executable_skill(data_root: &Path, skill_id: &str, invalid_line: &str) {
    let content = format!(
        "---\nname: {skill_id}\ndescription: \"invalid executable\"\nexecution.entrypoint: scripts/run.sh\n{invalid_line}\n---\nrun it"
    );
    let skill_dir = data_root.join("skills").join(skill_id);
    write_skill_fixture(&skill_dir, &content);
    write_file(
        &skill_dir.join("scripts").join("run.sh"),
        "#!/bin/sh\nprintf nope",
    );
}

/// Registry whose home root is an empty temp dir (data dir carries the
/// fixtures), mirroring the Go tests that point HOME at a scratch dir.
fn registry_for(data_root: &Path) -> Registry {
    let home = tempfile::tempdir().expect("home temp dir");
    Registry::with_roots(home.path().to_str().unwrap(), data_root.to_str().unwrap())
        .expect("registry")
}

// ---------------------------------------------------------------------------
// Registry behavior
// ---------------------------------------------------------------------------

#[test]
fn loads_data_dir_and_home_skills_with_data_dir_precedence() {
    let home_root = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();

    write_skill_fixture(
        &home_root
            .path()
            .join(".agents")
            .join("skills")
            .join("shared-skill"),
        "---\nname: shared-skill\ndescription: \"home description\"\n---\nhome body",
    );
    write_skill_fixture(
        &home_root
            .path()
            .join(".agents")
            .join("skills")
            .join("home-only"),
        "---\nname: home-only\ndescription: \"home only\"\n---\nhome only body",
    );
    write_file(
        &home_root.path().join(".agents").join("AGENTS.md"),
        "home overlay",
    );

    write_skill_fixture(
        &data_root.path().join("skills").join("shared-skill"),
        "---\nname: shared-skill\ndescription: \"data description\"\n---\ndata body",
    );
    write_skill_fixture(
        &data_root.path().join("skills").join("data-only"),
        "---\nname: data-only\ndescription: \"data only\"\n---\ndata only body",
    );
    write_file(&data_root.path().join("AGENTS.md"), "data overlay");

    let home = home_root.path().join(".agents");
    let registry =
        Registry::with_roots(home.to_str().unwrap(), data_root.path().to_str().unwrap()).unwrap();

    let snapshot = registry.snapshot();
    assert_eq!(snapshot.skills.len(), 3, "expected 3 effective skills");
    assert_eq!(snapshot.overlays.len(), 2, "expected 2 overlays");

    let shared = registry.get("shared-skill").expect("shared-skill");
    assert_eq!(shared.source, Source::DataDir, "data-dir precedence");
    assert!(shared.body.contains("data body"));

    let selected = registry
        .resolve_selected(&[
            "home-only".to_string(),
            "shared-skill".to_string(),
            "HOME-ONLY".to_string(),
        ])
        .expect("resolve selected");
    assert_eq!(selected.len(), 2, "deduplicated selection");
    assert_eq!(selected[0].skill_id, "home-only");
    assert_eq!(selected[1].skill_id, "shared-skill");

    let overlays = registry.overlays();
    assert_eq!(overlays[0].overlay_id, "home_agents");
    assert_eq!(overlays[0].source, Source::Home);
    assert_eq!(overlays[0].body, "home overlay");
    assert_eq!(overlays[1].overlay_id, "data_dir_agents");
    assert_eq!(overlays[1].source, Source::DataDir);
    assert_eq!(overlays[1].body, "data overlay");
    assert!(overlays[0].size_bytes > 0);
}

#[test]
fn new_resolves_home_root_and_agents_skills_dir() {
    let home = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    write_skill_fixture(
        &home
            .path()
            .join(".agents")
            .join("skills")
            .join("home-skill"),
        "---\nname: home-skill\n---\nhome body",
    );

    let previous_home = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", home.path()) };
    let registry = Registry::new(data_root.path().to_str().unwrap()).expect("registry from HOME");
    match previous_home {
        Some(previous) => unsafe { std::env::set_var("HOME", previous) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    let skill = registry.get("home-skill").expect("home skill");
    assert_eq!(skill.source, Source::Home);
    assert_eq!(skill.name, "home-skill");
}

#[test]
fn rejects_unknown_skill() {
    let data_root = tempfile::tempdir().unwrap();
    let registry = registry_for(data_root.path());
    let err = registry
        .resolve_selected(&["missing".to_string()])
        .expect_err("missing skill must fail");
    assert_eq!(err, SkillsError::SkillNotFound("missing".to_string()));
}

#[test]
fn resolves_empty_selection() {
    let data_root = tempfile::tempdir().unwrap();
    let registry = registry_for(data_root.path());
    let selected = registry.resolve_selected(&[]).expect("empty selection");
    assert!(selected.is_empty());
}

#[test]
fn loads_bundled_files() {
    let data_root = tempfile::tempdir().unwrap();
    write_skill_fixture(
        &data_root.path().join("skills").join("bundle-skill"),
        "---\nname: bundle-skill\ndescription: \"has files\"\n---\nbundle body",
    );
    write_file(
        &data_root
            .path()
            .join("skills")
            .join("bundle-skill")
            .join("references")
            .join("guide.md"),
        "guide",
    );
    write_file(
        &data_root
            .path()
            .join("skills")
            .join("bundle-skill")
            .join("scripts")
            .join("run.sh"),
        "#!/bin/sh",
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("bundle-skill").expect("bundle-skill");
    assert_eq!(skill.files.len(), 2, "bundled file inventory");
    assert_eq!(skill.files[0].path, "references/guide.md");
    assert_eq!(skill.files[1].path, "scripts/run.sh");
    assert!(skill.files[0].size_bytes > 0);
}

#[test]
fn projects_sandbox_declaration_for_skill_selection() {
    let data_root = tempfile::tempdir().unwrap();
    write_skill_fixture(
        &data_root.path().join("skills").join("shared-skill"),
        "---\nname: shared-skill\ndescription: \"shared\"\n---\nshared body",
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("shared-skill").expect("shared-skill");
    let sandbox = skill.sandbox.as_ref().expect("sandbox declaration");
    let declaration = sandbox
        .get("declaration")
        .expect("declaration key")
        .as_object()
        .expect("declaration object");
    assert_eq!(
        declaration.get("consumerKind").and_then(|v| v.as_str()),
        Some("skill")
    );
    assert_eq!(
        declaration.get("consumerId").and_then(|v| v.as_str()),
        Some("shared-skill")
    );
    assert_eq!(
        declaration.get("operationKind").and_then(|v| v.as_str()),
        Some("skill_selection")
    );
    let read_roots = declaration
        .get("readRoots")
        .and_then(|v| v.as_array())
        .expect("readRoots array");
    assert_eq!(read_roots.len(), 1);
    let root = read_roots[0].as_str().expect("read root");
    assert!(root.ends_with("skills/shared-skill"), "read root {root}");
}

#[test]
fn parses_executable_manifest_and_defaults_approval_to_ask() {
    let data_root = tempfile::tempdir().unwrap();
    write_executable_skill_secrets(data_root.path(), &[("EXEC_SKILL_TOKEN", "available")]);
    write_valid_executable_skill(
        data_root.path(),
        "exec-skill",
        &ExecutableFixtureOptions {
            secret_refs: vec!["EXEC_SKILL_TOKEN".to_string()],
            args: vec!["alpha".to_string(), "beta".to_string()],
            timeout_ms: 500,
            ..ExecutableFixtureOptions::default()
        },
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("exec-skill").expect("exec-skill");
    let manifest = skill
        .execution_manifest
        .as_ref()
        .expect("executable manifest");
    assert_eq!(
        manifest.approval_mode,
        kura_sandbox::ApprovalMode::Ask,
        "default ask"
    );
    assert_eq!(
        skill.availability_status,
        SkillAvailabilityStatus::Available
    );
    assert_eq!(manifest.timeout_ms, 500);
    assert_eq!(manifest.args, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(manifest.secret_refs, vec!["EXEC_SKILL_TOKEN".to_string()]);

    let separator = std::path::MAIN_SEPARATOR;
    let expected_suffix = format!("skills{separator}exec-skill{separator}scripts{separator}run.sh");
    assert!(
        manifest.entrypoint.ends_with(&expected_suffix),
        "resolved entrypoint {}",
        manifest.entrypoint
    );
}

#[test]
fn marks_executable_skill_unavailable_when_secret_ref_missing_for_environment() {
    let data_root = tempfile::tempdir().unwrap();
    write_valid_executable_skill(
        data_root.path(),
        "secret-skill",
        &ExecutableFixtureOptions {
            description: "needs secret".to_string(),
            secret_refs: vec!["MISSING_SKILL_SECRET".to_string()],
            script_body: "#!/bin/sh\nprintf nope".to_string(),
            ..ExecutableFixtureOptions::default()
        },
    );

    let previous_env = std::env::var_os("KURA_ENV");
    unsafe { std::env::set_var("KURA_ENV", "test") };
    let registry = registry_for(data_root.path());
    match previous_env {
        Some(previous) => unsafe { std::env::set_var("KURA_ENV", previous) },
        None => unsafe { std::env::remove_var("KURA_ENV") },
    }

    let skill = registry.get("secret-skill").expect("secret-skill");
    assert_eq!(
        skill.availability_status,
        SkillAvailabilityStatus::Unavailable
    );
    assert!(
        skill.availability_reason.contains("MISSING_SKILL_SECRET"),
        "reason {}",
        skill.availability_reason
    );
    assert!(
        skill.availability_reason.contains("test"),
        "environment-scoped reason {}",
        skill.availability_reason
    );
}

#[test]
fn reads_executable_skill_secrets_from_data_dir() {
    let data_root = tempfile::tempdir().unwrap();
    write_executable_skill_secrets(data_root.path(), &[("EXEC_SKILL_TOKEN", "test-secret")]);
    write_valid_executable_skill(
        data_root.path(),
        "secret-skill",
        &ExecutableFixtureOptions {
            description: "needs secret".to_string(),
            secret_refs: vec!["EXEC_SKILL_TOKEN".to_string()],
            ..ExecutableFixtureOptions::default()
        },
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("secret-skill").expect("secret-skill");
    assert_eq!(
        skill.availability_status,
        SkillAvailabilityStatus::Available
    );
    let values = resolve_executable_skill_secrets(
        data_root.path().to_str().unwrap(),
        &skill
            .execution_manifest
            .as_ref()
            .expect("manifest")
            .secret_refs,
    )
    .expect("resolve secrets");
    assert_eq!(
        values.get("EXEC_SKILL_TOKEN").map(String::as_str),
        Some("test-secret")
    );
}

#[test]
fn keeps_explicit_approval_on_executable_skill() {
    let data_root = tempfile::tempdir().unwrap();
    write_executable_skill_secrets(data_root.path(), &[("EXEC_SKILL_TOKEN", "available")]);
    write_valid_executable_skill(
        data_root.path(),
        "approval-skill",
        &ExecutableFixtureOptions {
            secret_refs: vec!["EXEC_SKILL_TOKEN".to_string()],
            ..ExecutableFixtureOptions::default()
        },
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("approval-skill").expect("approval-skill");
    let manifest = skill.execution_manifest.as_ref().expect("manifest");
    assert_eq!(manifest.approval_mode, kura_sandbox::ApprovalMode::Ask);
}

#[test]
fn marks_invalid_executable_fixture_unavailable() {
    let data_root = tempfile::tempdir().unwrap();
    write_invalid_executable_skill(
        data_root.path(),
        "invalid-skill",
        "execution.network_mode: nope",
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("invalid-skill").expect("invalid-skill");
    assert_eq!(
        skill.availability_status,
        SkillAvailabilityStatus::Unavailable
    );
    assert!(skill.execution_manifest.is_some(), "manifest retained");
    assert!(
        skill.availability_reason.contains("network_mode"),
        "reason {}",
        skill.availability_reason
    );
}

#[test]
fn projects_explicit_docker_requirement_for_executable_skill() {
    let data_root = tempfile::tempdir().unwrap();
    write_valid_executable_skill(
        data_root.path(),
        "docker-skill",
        &ExecutableFixtureOptions {
            profile_id: "docker_default".to_string(),
            required_enforcement_strength: "containerized".to_string(),
            approval_mode: "allow".to_string(),
            ..ExecutableFixtureOptions::default()
        },
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("docker-skill").expect("docker-skill");
    assert_eq!(
        skill.availability_status,
        SkillAvailabilityStatus::Available
    );
    let manifest = skill.execution_manifest.as_ref().expect("manifest");
    assert_eq!(manifest.profile_id, "docker_default");
    assert_eq!(manifest.backend_kind, kura_sandbox::BackendKind::Docker);
    assert_eq!(manifest.required_enforcement_strength, "containerized");
}

#[test]
fn keeps_unmodified_executable_skills_on_baseline_backend() {
    let data_root = tempfile::tempdir().unwrap();
    write_valid_executable_skill(
        data_root.path(),
        "baseline-skill",
        &ExecutableFixtureOptions::default(),
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("baseline-skill").expect("baseline-skill");
    let manifest = skill.execution_manifest.as_ref().expect("manifest");
    assert_eq!(manifest.profile_id, "subprocess_default");
    assert_eq!(manifest.backend_kind, kura_sandbox::BackendKind::Subprocess);
    assert_eq!(manifest.required_enforcement_strength, "declared_only");
    assert_eq!(manifest.network_mode, kura_sandbox::NetworkMode::Deny);
}

#[test]
fn reload_refreshes_snapshot_and_index() {
    let data_root = tempfile::tempdir().unwrap();
    let registry = registry_for(data_root.path());
    assert!(registry.list().is_empty());

    write_skill_fixture(
        &data_root.path().join("skills").join("late-skill"),
        "---\nname: late-skill\n---\nlate body",
    );
    registry.reload().expect("reload");
    let skill = registry.get("late-skill").expect("late-skill after reload");
    assert_eq!(skill.skill_id, "late-skill");
    assert_eq!(registry.list().len(), 1);
}

// ---------------------------------------------------------------------------
// Tenant-scoped secret resolution
// ---------------------------------------------------------------------------

/// Minimal in-memory metadata store implementing `kura_secrets::Store`
/// (the Go test uses `SQLiteStore`; a fake is sufficient here).
#[derive(Default)]
struct FakeSecretStore {
    secrets: std::sync::Mutex<HashMap<String, kura_secrets::TenantSecret>>,
    versions: std::sync::Mutex<HashMap<String, kura_secrets::SecretVersion>>,
}

impl FakeSecretStore {
    fn key(tenant_id: &str, secret_ref: &str) -> String {
        format!("{tenant_id}\u{0}{secret_ref}")
    }
}

impl kura_secrets::Store for FakeSecretStore {
    fn create_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
        version: kura_secrets::SecretVersion,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            self.secrets
                .lock()
                .unwrap()
                .insert(Self::key(&secret.tenant_id, &secret.secret_ref), secret);
            self.versions
                .lock()
                .unwrap()
                .insert(version.secret_version_id.clone(), version);
            Ok(())
        })
    }

    fn update_secret_metadata<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            self.secrets
                .lock()
                .unwrap()
                .insert(Self::key(&secret.tenant_id, &secret.secret_ref), secret);
            Ok(())
        })
    }

    fn rotate_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
        previous_version_id: &'a str,
        version: kura_secrets::SecretVersion,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            if let Some(previous) = self.versions.lock().unwrap().get_mut(previous_version_id) {
                previous.status = kura_secrets::SecretVersionStatus::Superseded;
                previous.superseded_at = Some(chrono::Utc::now());
            }
            self.secrets
                .lock()
                .unwrap()
                .insert(Self::key(&secret.tenant_id, &secret.secret_ref), secret);
            self.versions
                .lock()
                .unwrap()
                .insert(version.secret_version_id.clone(), version);
            Ok(())
        })
    }

    fn disable_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            self.secrets
                .lock()
                .unwrap()
                .insert(Self::key(&secret.tenant_id, &secret.secret_ref), secret);
            Ok(())
        })
    }

    fn get_secret_by_ref<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_ref: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::TenantSecret>>> {
        Box::pin(async move {
            Ok(self
                .secrets
                .lock()
                .unwrap()
                .get(&Self::key(tenant_id, secret_ref))
                .cloned())
        })
    }

    fn get_secret_version<'a>(
        &'a self,
        _tenant_id: &'a str,
        secret_version_id: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::SecretVersion>>>
    {
        Box::pin(async move {
            Ok(self
                .versions
                .lock()
                .unwrap()
                .get(secret_version_id)
                .cloned())
        })
    }

    fn list_secrets<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Vec<kura_secrets::TenantSecret>>> {
        Box::pin(async move {
            Ok(self
                .secrets
                .lock()
                .unwrap()
                .values()
                .filter(|secret| secret.tenant_id == tenant_id)
                .cloned()
                .collect())
        })
    }
}

#[tokio::test]
async fn resolves_executable_skill_secrets_for_tenant_uses_active_tenant() {
    let backend_dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FakeSecretStore::default());
    let backend =
        Arc::new(kura_secrets::LocalBackend::new(backend_dir.path()).expect("local backend"));
    let manager = kura_secrets::Manager::new(store.clone(), backend);

    manager
        .create(kura_secrets::CreateInput {
            tenant_id: "ten_a".to_string(),
            secret_ref: "EXEC_SKILL_TOKEN".to_string(),
            value: "tenant-a".to_string(),
            ..kura_secrets::CreateInput::default()
        })
        .await
        .expect("create tenant A secret");
    manager
        .create(kura_secrets::CreateInput {
            tenant_id: "ten_b".to_string(),
            secret_ref: "EXEC_SKILL_TOKEN".to_string(),
            value: "tenant-b".to_string(),
            ..kura_secrets::CreateInput::default()
        })
        .await
        .expect("create tenant B secret");

    let context = kura_identity::TenantContext {
        principal_id: "prn_b".to_string(),
        tenant_id: "ten_b".to_string(),
        token_id: String::new(),
        ..kura_identity::TenantContext::default()
    };
    let refs = ["EXEC_SKILL_TOKEN".to_string()];
    let values = kura_identity::tenantctx::scope(context, async {
        resolve_executable_skill_secrets_for_tenant(Some(&manager), &refs)
            .await
            .expect("resolve tenant secrets")
    })
    .await;
    assert_eq!(
        values.get("EXEC_SKILL_TOKEN").map(String::as_str),
        Some("tenant-b"),
        "active tenant secret wins"
    );

    // Fails closed without a tenant context.
    let err = resolve_executable_skill_secrets_for_tenant(Some(&manager), &refs)
        .await
        .expect_err("missing tenant context must fail");
    assert_eq!(err, SkillsError::TenantContextRequired);

    // Fails closed without a secret manager.
    let err = resolve_executable_skill_secrets_for_tenant(None, &refs)
        .await
        .expect_err("missing manager must fail");
    assert_eq!(err, SkillsError::SecretManagerRequired);

    // Empty refs resolve without any context or manager.
    let empty: Vec<String> = Vec::new();
    let values = resolve_executable_skill_secrets_for_tenant(None, &empty)
        .await
        .expect("empty refs");
    assert!(values.is_empty());
}

// ---------------------------------------------------------------------------
// JSON round-trip
// ---------------------------------------------------------------------------

#[test]
fn skill_json_round_trip_preserves_camel_case_wire_shape() {
    let data_root = tempfile::tempdir().unwrap();
    write_executable_skill_secrets(data_root.path(), &[("EXEC_SKILL_TOKEN", "available")]);
    write_valid_executable_skill(
        data_root.path(),
        "exec-skill",
        &ExecutableFixtureOptions {
            secret_refs: vec!["EXEC_SKILL_TOKEN".to_string()],
            args: vec!["alpha".to_string(), "beta".to_string()],
            timeout_ms: 500,
            ..ExecutableFixtureOptions::default()
        },
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("exec-skill").expect("exec-skill");

    let json = serde_json::to_string(&skill).expect("serialize skill");
    for key in [
        "skillId",
        "name",
        "description",
        "source",
        "rootPath",
        "skillPath",
        "instructionPath",
        "frontmatter",
        "frontmatterRaw",
        "body",
        "files",
        "executionManifest",
        "availabilityStatus",
        "sandbox",
    ] {
        assert!(
            json.contains(&format!("\"{key}\"")),
            "missing camelCase key {key} in {json}"
        );
    }
    assert!(json.contains("\"backendKind\":\"subprocess\""), "{json}");
    assert!(json.contains("\"approvalMode\":\"ask\""), "{json}");
    assert!(json.contains("\"networkMode\":\"deny\""), "{json}");
    assert!(
        json.contains("\"availabilityStatus\":\"available\""),
        "{json}"
    );
    assert!(json.contains("\"source\":\"data_dir\""), "{json}");

    let decoded: Skill = serde_json::from_str(&json).expect("deserialize skill");
    assert_eq!(decoded, skill, "round-trip equality");
}

#[test]
fn non_executable_skill_round_trip() {
    let data_root = tempfile::tempdir().unwrap();
    write_skill_fixture(
        &data_root.path().join("skills").join("plain-skill"),
        "---\nname: plain-skill\ndescription: \"plain\"\n---\nplain body",
    );

    let registry = registry_for(data_root.path());
    let skill = registry.get("plain-skill").expect("plain-skill");
    assert!(skill.execution_manifest.is_none());
    assert_eq!(
        skill.availability_status,
        SkillAvailabilityStatus::NotExecutable
    );

    let json = serde_json::to_string(&skill).expect("serialize");
    assert!(
        json.contains("\"availabilityStatus\":\"not_executable\""),
        "{json}"
    );
    assert!(!json.contains("executionManifest"), "{json}");

    let decoded: Skill = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, skill);
}
