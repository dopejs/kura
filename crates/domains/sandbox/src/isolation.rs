//! Tenant isolation for container runs (Stage 3.1).
//!
//! A profile with [`IsolationMode::Tenant`] runs each execution in its own
//! container, built here rather than from the request: the request cannot
//! name mounts, a network, a user or limits. What one run can reach is fixed
//! by the tenant it runs for:
//!
//! - **Filesystem:** only that tenant's workspace, mounted at
//!   [`CONTAINER_WORKSPACE`]. The root filesystem is read-only; `/tmp` is a
//!   small tmpfs. Nothing of the daemon's data dir is mounted.
//! - **Identity:** a per-tenant uid ([`tenant_uid`]), never root, no
//!   capabilities, `no-new-privileges`.
//! - **Resources:** cgroup CPU, memory (swap included) and pids limits.
//! - **Network:** none. Tenant egress arrives with the policy-checking proxy
//!   (Stage 3.5); until then a request for network is refused, not quietly
//!   downgraded.
//!
//! Cross-tenant isolation rests on the mounts: another tenant's workspace is
//! not in the container's filesystem at all. The distinct uid is defence in
//! depth for files on the host.

use std::path::{Path, PathBuf};

/// Where the tenant workspace appears inside the container.
pub const CONTAINER_WORKSPACE: &str = "/workspace";

/// First uid handed to tenants: clear of system and ordinary user ranges.
const TENANT_UID_BASE: u32 = 100_000;
/// Width of the tenant uid range (2^30): collisions between tenant ids are
/// negligible at cell scale, and isolation does not depend on uniqueness.
const TENANT_UID_SPAN: u32 = 1 << 30;

/// Profile-level isolation contract.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum IsolationMode {
    /// Whatever the backend gives (the request shapes mounts and network).
    #[default]
    #[serde(rename = "none")]
    None,
    /// One container per run, scoped to the acting tenant (this module).
    #[serde(rename = "tenant")]
    Tenant,
}

impl IsolationMode {
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == IsolationMode::None
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            IsolationMode::None => "none",
            IsolationMode::Tenant => "tenant",
        }
    }
}

/// cgroup limits for one isolated run.
#[derive(Debug, Clone, PartialEq)]
pub struct IsolationLimits {
    pub cpus: f64,
    pub memory_mb: u64,
    pub pids: u64,
    pub tmpfs_mb: u64,
}

impl Default for IsolationLimits {
    fn default() -> Self {
        IsolationLimits {
            cpus: 1.0,
            memory_mb: 512,
            pids: 256,
            tmpfs_mb: 64,
        }
    }
}

impl IsolationLimits {
    /// Defaults overridden by `KURA_SANDBOX_TENANT_{CPUS,MEMORY_MB,PIDS,TMPFS_MB}`.
    /// An unparsable or non-positive value keeps the default (logged): a
    /// typo must not lift a limit.
    #[must_use]
    pub fn from_env() -> Self {
        let mut limits = IsolationLimits::default();
        if let Some(cpus) = env_number::<f64>("KURA_SANDBOX_TENANT_CPUS") {
            limits.cpus = cpus;
        }
        if let Some(memory) = env_number::<u64>("KURA_SANDBOX_TENANT_MEMORY_MB") {
            limits.memory_mb = memory;
        }
        if let Some(pids) = env_number::<u64>("KURA_SANDBOX_TENANT_PIDS") {
            limits.pids = pids;
        }
        if let Some(tmpfs) = env_number::<u64>("KURA_SANDBOX_TENANT_TMPFS_MB") {
            limits.tmpfs_mb = tmpfs;
        }
        limits
    }
}

fn env_number<T: std::str::FromStr + PartialOrd + Default>(key: &str) -> Option<T> {
    let raw = std::env::var(key).ok()?;
    match raw.trim().parse::<T>() {
        Ok(value) if value > T::default() => Some(value),
        _ => {
            eprintln!("[kura] sandbox: ignoring {key}={raw:?}; keeping the default limit");
            None
        }
    }
}

/// Everything an isolated container run needs beyond the launch itself.
#[derive(Debug, Clone, PartialEq)]
pub struct IsolatedRun {
    pub tenant_id: String,
    pub uid: u32,
    /// The workspace as the daemon sees it, created by [`ensure_workspace`].
    pub workspace_local: PathBuf,
    /// The workspace as the docker daemon sees it (bind mount source).
    pub workspace_source: String,
    pub limits: IsolationLimits,
}

/// Why a tenant cannot run isolated work.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IsolationError {
    #[error("isolated execution requires a tenant")]
    TenantRequired,
    #[error("tenant id {0:?} cannot name a workspace")]
    InvalidTenantId(String),
    #[error("creating the tenant workspace failed: {0}")]
    Workspace(String),
}

/// A per-tenant uid in `[TENANT_UID_BASE, TENANT_UID_BASE + 2^30)`, stable
/// across restarts and hosts (FNV-1a of the tenant id).
#[must_use]
pub fn tenant_uid(tenant_id: &str) -> u32 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in tenant_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    TENANT_UID_BASE + (hash % u64::from(TENANT_UID_SPAN)) as u32
}

/// Tenant ids become a path component, so only a conservative charset is
/// accepted: anything else could traverse (`..`) or alias another tenant.
fn validate_tenant_id(tenant_id: &str) -> Result<&str, IsolationError> {
    let trimmed = tenant_id.trim();
    if trimmed.is_empty() {
        return Err(IsolationError::TenantRequired);
    }
    let valid = trimmed.len() <= 128
        && trimmed != "."
        && trimmed != ".."
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !valid || trimmed.starts_with('.') {
        return Err(IsolationError::InvalidTenantId(tenant_id.to_string()));
    }
    Ok(trimmed)
}

/// Where tenant workspaces live: `local_root` is the daemon's view
/// (`<data_dir>/tenants`), `host_root` the docker daemon's view of the same
/// directory — they differ when the daemon itself runs in a container with
/// the docker socket mounted (`KURA_SANDBOX_TENANT_HOST_ROOT`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoots {
    pub local_root: PathBuf,
    pub host_root: PathBuf,
}

impl WorkspaceRoots {
    #[must_use]
    pub fn for_data_dir(data_dir: &str) -> Self {
        let local_root = Path::new(data_dir).join("tenants");
        let host_root = std::env::var("KURA_SANDBOX_TENANT_HOST_ROOT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map_or_else(|| local_root.clone(), PathBuf::from);
        WorkspaceRoots {
            local_root,
            host_root,
        }
    }

    /// Describe `tenant_id`'s run. No filesystem effect, so a dry-run
    /// explain can call it; [`ensure_workspace`] creates the directory when
    /// the run actually starts.
    pub fn resolve(
        &self,
        tenant_id: &str,
        limits: IsolationLimits,
    ) -> Result<IsolatedRun, IsolationError> {
        let tenant = validate_tenant_id(tenant_id)?;
        Ok(IsolatedRun {
            tenant_id: tenant.to_string(),
            uid: tenant_uid(tenant),
            workspace_local: self.local_root.join(tenant).join("workspace"),
            workspace_source: self
                .host_root
                .join(tenant)
                .join("workspace")
                .to_string_lossy()
                .into_owned(),
            limits,
        })
    }
}

/// Create the run's workspace if needed.
///
/// The tenant directory is 0700 (owned by the daemon), the workspace inside
/// it 0777: the container's per-tenant uid must write there, and the daemon
/// may not be root to chown it. Host users cannot traverse the 0700 parent;
/// a container sees only its own workspace, mounted directly.
pub fn ensure_workspace(run: &IsolatedRun) -> Result<(), IsolationError> {
    let workspace = &run.workspace_local;
    std::fs::create_dir_all(workspace).map_err(|err| IsolationError::Workspace(err.to_string()))?;
    if let Some(tenant_dir) = workspace.parent() {
        set_mode(tenant_dir, 0o700)?;
    }
    set_mode(workspace, 0o777)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), IsolationError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|err| IsolationError::Workspace(format!("{}: {err}", path.display())))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), IsolationError> {
    Ok(())
}

/// The `docker run` arguments for an isolated run, up to and including the
/// image; the caller appends the command and its args. Pure, so every flag
/// that carries the isolation is unit-tested without a docker daemon.
#[must_use]
pub fn isolated_docker_args(
    run: &IsolatedRun,
    container_name: &str,
    image: &str,
    env: &[String],
) -> Vec<String> {
    let user = format!("{uid}:{uid}", uid = run.uid);
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-i".into(),
        "--name".into(),
        container_name.into(),
        "--label".into(),
        "kura.sandbox=isolated".into(),
        "--label".into(),
        format!("kura.tenant={}", run.tenant_id),
        "--user".into(),
        user,
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--read-only".into(),
        "--tmpfs".into(),
        format!("/tmp:rw,noexec,nosuid,nodev,size={}m", run.limits.tmpfs_mb),
        "--network".into(),
        "none".into(),
        "--ipc".into(),
        "private".into(),
        "--init".into(),
        "--cpus".into(),
        format!("{}", run.limits.cpus),
        "--memory".into(),
        format!("{}m", run.limits.memory_mb),
        // Equal to --memory: no swap on top of the limit.
        "--memory-swap".into(),
        format!("{}m", run.limits.memory_mb),
        "--pids-limit".into(),
        run.limits.pids.to_string(),
        "--mount".into(),
        format!(
            "type=bind,src={},dst={CONTAINER_WORKSPACE}",
            run.workspace_source
        ),
        "--workdir".into(),
        CONTAINER_WORKSPACE.into(),
        "-e".into(),
        format!("HOME={CONTAINER_WORKSPACE}"),
    ];
    args.extend(container_env_args(env));
    args.push(image.into());
    args
}

/// Variables the `docker` client itself reads. A container variable with one
/// of these names must not reach the client's environment (a tenant-supplied
/// `DOCKER_HOST` would redirect the client), so it is passed inline instead.
fn affects_docker_client(key: &str) -> bool {
    matches!(key, "PATH" | "HOME" | "XDG_RUNTIME_DIR") || key.starts_with("DOCKER_")
}

/// `-e` flags for the container's environment (`KEY=VALUE` items).
///
/// Values are **not** on the command line: `-e KEY` makes docker copy the
/// value from the client's own environment ([`docker_client_env`]). An argv
/// is readable by every user on the host through `ps`, and these variables
/// carry the tenant's resolved secrets. Only client-affecting names go
/// inline, so they can never steer the client.
#[must_use]
pub fn container_env_args(env: &[String]) -> Vec<String> {
    let mut args = Vec::with_capacity(env.len() * 2);
    for item in env {
        let Some((key, _)) = item.split_once('=') else {
            continue;
        };
        args.push("-e".to_string());
        if affects_docker_client(key) {
            args.push(item.clone());
        } else {
            args.push(key.to_string());
        }
    }
    args
}

/// The `docker` client's environment for an isolated run: the daemon's own
/// client settings (`PATH`, `HOME`, `DOCKER_*`), then the container values
/// that [`container_env_args`] names. Nothing else of the daemon's
/// environment is passed.
#[must_use]
pub fn docker_client_env(env: &[String]) -> Vec<String> {
    let mut out: Vec<String> = std::env::vars()
        .filter(|(key, _)| affects_docker_client(key))
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    out.extend(
        env.iter()
            .filter(|item| {
                item.split_once('=')
                    .is_some_and(|(key, _)| !affects_docker_client(key))
            })
            .cloned(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> IsolatedRun {
        IsolatedRun {
            tenant_id: "ten_a".into(),
            uid: tenant_uid("ten_a"),
            workspace_local: PathBuf::from("/data/tenants/ten_a/workspace"),
            workspace_source: "/srv/kura/tenants/ten_a/workspace".into(),
            limits: IsolationLimits::default(),
        }
    }

    fn flag_value<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
            .collect()
    }

    #[test]
    fn the_container_gets_only_the_tenant_workspace_no_network_and_no_root() {
        let args = isolated_docker_args(&run(), "kura-exec-1", "alpine:3.20", &["A=1".into()]);
        assert_eq!(
            flag_value(&args, "--mount"),
            ["type=bind,src=/srv/kura/tenants/ten_a/workspace,dst=/workspace"],
            "exactly one mount"
        );
        assert!(!args.iter().any(|a| a == "-v" || a == "--volume"));
        assert_eq!(flag_value(&args, "--network"), ["none"]);
        assert!(!args.iter().any(|a| a.contains("host")), "{args:?}");
        let uid = tenant_uid("ten_a");
        assert_eq!(
            flag_value(&args, "--user"),
            [format!("{uid}:{uid}").as_str()]
        );
        assert!(uid >= TENANT_UID_BASE);
        assert_eq!(flag_value(&args, "--cap-drop"), ["ALL"]);
        assert_eq!(flag_value(&args, "--security-opt"), ["no-new-privileges"]);
        assert!(args.iter().any(|a| a == "--read-only"));
        assert!(!args.iter().any(|a| a == "--privileged" || a == "--cap-add"));
        assert_eq!(flag_value(&args, "--workdir"), ["/workspace"]);
        assert_eq!(flag_value(&args, "--name"), ["kura-exec-1"]);
        assert_eq!(args.last().map(String::as_str), Some("alpine:3.20"));
        assert!(flag_value(&args, "-e").contains(&"A"), "{args:?}");
    }

    #[test]
    fn limits_reach_the_cgroup_flags_with_no_swap_headroom() {
        let mut run = run();
        run.limits = IsolationLimits {
            cpus: 0.5,
            memory_mb: 256,
            pids: 64,
            tmpfs_mb: 16,
        };
        let args = isolated_docker_args(&run, "n", "img", &[]);
        assert_eq!(flag_value(&args, "--cpus"), ["0.5"]);
        assert_eq!(flag_value(&args, "--memory"), ["256m"]);
        assert_eq!(flag_value(&args, "--memory-swap"), ["256m"]);
        assert_eq!(flag_value(&args, "--pids-limit"), ["64"]);
        assert_eq!(
            flag_value(&args, "--tmpfs"),
            ["/tmp:rw,noexec,nosuid,nodev,size=16m"]
        );
    }

    #[test]
    fn tenant_uids_are_stable_and_distinct() {
        assert_eq!(tenant_uid("ten_a"), tenant_uid("ten_a"));
        assert_ne!(tenant_uid("ten_a"), tenant_uid("ten_b"));
        assert!(tenant_uid("") >= TENANT_UID_BASE);
    }

    #[test]
    fn a_tenant_id_cannot_escape_the_tenants_root() {
        let dir = std::env::temp_dir().join(format!("kura-iso-{}", std::process::id()));
        let roots = WorkspaceRoots {
            local_root: dir.join("tenants"),
            host_root: PathBuf::from("/host/tenants"),
        };
        for bad in ["", "  ", "..", ".", "../ten_b", "ten/a", ".hidden", "ten a"] {
            assert!(
                roots.resolve(bad, IsolationLimits::default()).is_err(),
                "{bad:?} accepted"
            );
        }
        let run = roots
            .resolve("ten_a", IsolationLimits::default())
            .expect("resolved");
        assert_eq!(run.workspace_source, "/host/tenants/ten_a/workspace");
        assert!(!dir.join("tenants").exists(), "resolve touches nothing");
        ensure_workspace(&run).expect("created");
        assert!(dir.join("tenants/ten_a/workspace").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: PathBuf| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode(dir.join("tenants/ten_a")), 0o700);
            assert_eq!(mode(dir.join("tenants/ten_a/workspace")), 0o777);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secret_values_stay_off_the_command_line() {
        let env = vec![
            "API_TOKEN=sk-secret-value".to_string(),
            "DOCKER_HOST=tcp://attacker:2375".to_string(),
            "PATH=/opt/tool/bin:/usr/bin".to_string(),
        ];
        let args = isolated_docker_args(&run(), "n", "img", &env);
        assert!(
            !args.iter().any(|a| a.contains("sk-secret-value")),
            "{args:?}"
        );
        let e = flag_value(&args, "-e");
        assert!(e.contains(&"API_TOKEN"));
        // Client-affecting names go inline, to the container only.
        assert!(e.contains(&"DOCKER_HOST=tcp://attacker:2375"));
        assert!(e.contains(&"PATH=/opt/tool/bin:/usr/bin"));

        let client = docker_client_env(&env);
        assert!(client.contains(&"API_TOKEN=sk-secret-value".to_string()));
        assert!(
            !client
                .iter()
                .any(|item| item.contains("attacker") || item.contains("/opt/tool")),
            "the tenant cannot steer the docker client: {client:?}"
        );
    }
}
