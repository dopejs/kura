//! Stage 3.1: tenant-isolated container runs.
//!
//! The refusals run everywhere. The container tests need a docker daemon with
//! the sandbox image pulled and run only when `KURA_DOCKER_TESTS=1` (CI's
//! Linux job sets it); elsewhere they return early and say so.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kura_events::Bus;
use kura_policy::Engine;
use kura_sandbox::{
    AccessRequest, DecisionResolution, ExecutionRequest, ExecutionStatus, Manager, NetworkMode,
    PROFILE_ID_TENANT_ISOLATED, is_terminal, tenant_uid,
};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_isolation_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.to_string_lossy().to_string()
}

fn manager(data_dir: &str) -> Manager {
    let config = kura_config::Config {
        project_root: String::new(),
        environment: kura_config::Environment::Test,
        bind_addr: "127.0.0.1:19192".to_string(),
        data_dir: data_dir.to_string(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        llm: kura_config::LlmConfig::default(),
        connectors: kura_config::ConnectorConfig::default(),
        egress: Default::default(),
        store: Default::default(),
    };
    let store = Arc::new(Mutex::new(SQLiteStore::new(data_dir).expect("open store")));
    Manager::new(config, Some(store), Bus::new(), Engine::new())
}

fn as_tenant<R>(tenant_id: &str, f: impl FnOnce() -> R) -> R {
    as_member(tenant_id, None, f)
}

fn as_member<R>(tenant_id: &str, role: Option<kura_identity::Role>, f: impl FnOnce() -> R) -> R {
    kura_identity::tenantctx::with_context(
        kura_identity::TenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: format!("prn_{tenant_id}"),
            role,
            ..kura_identity::TenantContext::default()
        },
        f,
    )
}

fn request(script: &str) -> ExecutionRequest {
    ExecutionRequest {
        profile_id: PROFILE_ID_TENANT_ISOLATED.to_string(),
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        requested_by: "test".to_string(),
        ..ExecutionRequest::default()
    }
}

fn denial(decision: &kura_sandbox::Decision) -> Option<&str> {
    decision
        .matched_rules
        .iter()
        .find(|rule| rule.starts_with("isolation:"))
        .map(String::as_str)
}

#[test]
fn an_isolated_run_needs_a_tenant() {
    let data_dir = temp_dir("no_tenant");
    let decision = manager(&data_dir)
        .explain(request("true"))
        .expect("explain");
    assert_eq!(decision.resolution, DecisionResolution::Deny);
    assert_eq!(denial(&decision), Some("isolation:tenant_required"));
}

#[test]
fn an_isolated_run_cannot_widen_its_container() {
    let data_dir = temp_dir("widen");
    let manager = manager(&data_dir);
    let cases: Vec<(&str, ExecutionRequest)> = vec![
        (
            "isolation:network",
            ExecutionRequest {
                access: AccessRequest {
                    network_mode: Some(NetworkMode::Full),
                    ..AccessRequest::default()
                },
                ..request("true")
            },
        ),
        (
            "isolation:network",
            ExecutionRequest {
                access: AccessRequest {
                    allow_loopback: true,
                    ..AccessRequest::default()
                },
                ..request("true")
            },
        ),
        (
            "isolation:mounts",
            ExecutionRequest {
                access: AccessRequest {
                    read_roots: vec![data_dir.clone()],
                    ..AccessRequest::default()
                },
                ..request("true")
            },
        ),
        (
            "isolation:cwd",
            ExecutionRequest {
                cwd: "/etc".to_string(),
                ..request("true")
            },
        ),
        (
            "isolation:cwd",
            ExecutionRequest {
                cwd: "/workspace/../etc".to_string(),
                ..request("true")
            },
        ),
    ];
    for (rule, request) in cases {
        let decision = as_tenant("ten_a", || manager.explain(request.clone())).expect("explain");
        assert_eq!(decision.resolution, DecisionResolution::Deny, "{request:?}");
        assert_eq!(denial(&decision), Some(rule), "{request:?}");
    }
    let decision = as_tenant("../ten_b", || manager.explain(request("true"))).expect("explain");
    assert_eq!(denial(&decision), Some("isolation:tenant_invalid"));
}

/// A well-formed request passes the isolation gate (whether docker is
/// present decides the rest), and explaining it creates nothing on disk.
#[test]
fn a_valid_isolated_request_passes_the_gate_without_side_effects() {
    let data_dir = temp_dir("valid");
    let manager = manager(&data_dir);
    let decision = as_tenant("ten_a", || {
        manager.explain(ExecutionRequest {
            cwd: "/workspace/project".to_string(),
            ..request("true")
        })
    })
    .expect("explain");
    assert_eq!(denial(&decision), None, "{:?}", decision.matched_rules);
    assert!(
        !std::path::Path::new(&data_dir).join("tenants").exists(),
        "explain is a dry run"
    );
    let profile = manager
        .get_profile(PROFILE_ID_TENANT_ISOLATED)
        .expect("builtin profile");
    assert_eq!(profile.isolation, kura_sandbox::IsolationMode::Tenant);
    assert!(
        profile.env_policy.injected_vars.is_empty(),
        "nothing of the daemon leaks in"
    );
}

// --- Stage 3.2: the shared-hosting floor -----------------------------------

const OPERATOR: &str = "ten_operator";

fn shared_manager(name: &str) -> Manager {
    let manager = manager(&temp_dir(name));
    manager.require_tenant_isolation(OPERATOR);
    manager
}

fn subprocess(script: &str) -> ExecutionRequest {
    ExecutionRequest {
        profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
        ..request(script)
    }
}

fn floor_rule(decision: &kura_sandbox::Decision) -> Option<&str> {
    decision
        .matched_rules
        .iter()
        .find(|rule| rule.starts_with("hosting:") || rule.starts_with("tenant:"))
        .map(String::as_str)
}

#[test]
fn a_shared_daemon_refuses_tenant_work_on_a_backend_that_does_not_isolate() {
    let manager = shared_manager("floor_tenant");
    let decision = as_tenant("ten_a", || manager.explain(subprocess("true"))).expect("explain");
    assert_eq!(decision.resolution, DecisionResolution::Deny);
    assert_eq!(floor_rule(&decision), Some("hosting:isolation_required"));
    assert!(decision.explanation.contains(PROFILE_ID_TENANT_ISOLATED));

    // A member of the operator tenant who is not its owner is a tenant too.
    let decision = as_member(OPERATOR, Some(kura_identity::Role::Admin), || {
        manager.explain(subprocess("true"))
    })
    .expect("explain");
    assert_eq!(floor_rule(&decision), Some("hosting:isolation_required"));

    // The tenant's way in is the isolated profile: past the floor.
    let decision = as_tenant("ten_a", || manager.explain(request("true"))).expect("explain");
    assert_eq!(floor_rule(&decision), None, "{:?}", decision.matched_rules);
    assert_eq!(denial(&decision), None);
}

/// Work nobody can attribute is not the operator's: it gets the tenant
/// rule, which is the fail-closed direction.
#[test]
fn a_shared_daemon_treats_unowned_work_as_a_tenants() {
    let manager = shared_manager("floor_unowned");
    let decision = manager.explain(subprocess("true")).expect("explain");
    assert_eq!(floor_rule(&decision), Some("hosting:isolation_required"));
}

#[test]
fn the_operator_keeps_host_execution_on_a_shared_daemon() {
    let manager = shared_manager("floor_operator");
    let decision = as_member(OPERATOR, Some(kura_identity::Role::Owner), || {
        manager.explain(subprocess("true"))
    })
    .expect("explain");
    assert_eq!(floor_rule(&decision), None, "{:?}", decision.matched_rules);

    // In-process work the operator owns (its MCP server), with no request.
    let decision = manager
        .explain(ExecutionRequest {
            owner_tenant_id: OPERATOR.to_string(),
            ..subprocess("true")
        })
        .expect("explain");
    assert_eq!(floor_rule(&decision), None, "{:?}", decision.matched_rules);
}

/// The owner an in-process caller names cannot launder a request acting for
/// another tenant, and an API body cannot claim an owner at all.
#[test]
fn the_owner_tenant_cannot_override_the_request_context() {
    let manager = shared_manager("floor_mismatch");
    let decision = as_tenant("ten_a", || {
        manager.explain(ExecutionRequest {
            owner_tenant_id: OPERATOR.to_string(),
            ..subprocess("true")
        })
    })
    .expect("explain");
    assert_eq!(decision.resolution, DecisionResolution::Deny);
    assert_eq!(floor_rule(&decision), Some("tenant:mismatch"));

    let decoded: ExecutionRequest = serde_json::from_str(
        r#"{"command":"true","access":{},"ownerTenantId":"ten_operator","owner_tenant_id":"ten_operator"}"#,
    )
    .expect("decode");
    assert!(decoded.owner_tenant_id.is_empty());
}

/// MCP stdio servers start through the attached path: a tenant's server is
/// refused there too until it can run isolated (Stage 3.3).
#[test]
fn a_tenants_attached_process_is_refused_on_a_shared_daemon() {
    let manager = shared_manager("floor_attached");
    let (execution, attached) = manager
        .start_attached_execution(ExecutionRequest {
            owner_tenant_id: "ten_a".to_string(),
            ..subprocess("cat")
        })
        .expect("attached");
    assert!(attached.is_none(), "no process was started");
    assert_eq!(execution.status, ExecutionStatus::Denied);
    assert_eq!(
        floor_rule(&execution.decision),
        Some("hosting:isolation_required")
    );
}

/// A single-tenant daemon is unchanged: tenants' subprocess work runs.
#[test]
fn a_single_tenant_daemon_has_no_floor() {
    let manager = manager(&temp_dir("floor_single"));
    let decision = as_tenant("ten_a", || manager.explain(subprocess("true"))).expect("explain");
    assert_eq!(floor_rule(&decision), None, "{:?}", decision.matched_rules);
}

// --- Stage 3.5: network through the tenant egress proxy -------------------

fn networked(script: &str) -> ExecutionRequest {
    ExecutionRequest {
        access: AccessRequest {
            network_mode: Some(NetworkMode::Full),
            ..AccessRequest::default()
        },
        ..request(script)
    }
}

#[test]
fn tenant_egress_needs_an_internal_network() {
    let manager = manager(&temp_dir("egress_config"));
    let err = manager
        .configure_tenant_egress_with("bridge", "http://proxy:3128", &|_| Ok(false))
        .expect_err("a routed network is refused");
    assert!(err.contains("not --internal"), "{err}");
    assert!(
        manager
            .configure_tenant_egress_with("", "http://proxy:3128", &|_| Ok(true))
            .is_err()
    );
    assert!(
        manager
            .configure_tenant_egress_with("net", "http://proxy:3128", &|_| Err("gone".into()))
            .is_err()
    );
    // Still offline: nothing was configured.
    let decision = as_tenant("ten_a", || manager.explain(networked("true"))).expect("explain");
    assert_eq!(denial(&decision), Some("isolation:network"));
}

#[test]
fn a_networked_run_goes_through_the_configured_proxy_without_approval() {
    let manager = manager(&temp_dir("egress_on"));
    manager
        .configure_tenant_egress_with("kura-tenant", "http://kura-egress-proxy:3128", &|n| {
            Ok(n == "kura-tenant")
        })
        .expect("configured");
    let decision = as_tenant("ten_a", || manager.explain(networked("true"))).expect("explain");
    assert_eq!(denial(&decision), None, "{:?}", decision.matched_rules);
    assert!(
        !decision
            .matched_rules
            .iter()
            .any(|rule| rule.starts_with("network:approval")),
        "the proxy is the policy; no operator approval: {:?}",
        decision.matched_rules
    );

    // What the proxy cannot enforce per run is still refused.
    for access in [
        AccessRequest {
            network_mode: Some(NetworkMode::AllowList),
            allowed_hosts: vec!["example.com".to_string()],
            ..AccessRequest::default()
        },
        AccessRequest {
            network_mode: Some(NetworkMode::Full),
            allow_loopback: true,
            ..AccessRequest::default()
        },
    ] {
        let decision = as_tenant("ten_a", || {
            manager.explain(ExecutionRequest {
                access: access.clone(),
                ..request("true")
            })
        })
        .expect("explain");
        assert_eq!(denial(&decision), Some("isolation:network"), "{access:?}");
    }
}

#[test]
fn an_egress_key_needs_egress_and_can_be_turned_off_with_it() {
    let manager = manager(&temp_dir("egress_key"));
    let key = kura_egress::EgressKey::from_bytes(&[9u8; 32]).unwrap();
    let err = manager
        .configure_tenant_egress_key(key.clone())
        .expect_err("no egress yet");
    assert!(err.contains("not configured"), "{err}");
    manager
        .configure_tenant_egress_with("kura-tenant", "http://172.31.0.2:3128", &|_| Ok(true))
        .expect("configured");
    manager.configure_tenant_egress_key(key).expect("key");
    assert!(manager.tenant_egress_configured());
    manager.disable_tenant_egress();
    assert!(!manager.tenant_egress_configured());
    let decision = as_tenant("ten_a", || manager.explain(networked("true"))).expect("explain");
    assert_eq!(denial(&decision), Some("isolation:network"));
}

fn docker_tests_enabled(test: &str) -> bool {
    let enabled = std::env::var("KURA_DOCKER_TESTS").is_ok_and(|v| v == "1");
    if !enabled {
        eprintln!("skipping {test}: set KURA_DOCKER_TESTS=1 with a docker daemon to run it");
    }
    enabled
}

fn run_as(manager: &Manager, tenant: &str, script: &str) -> kura_sandbox::Execution {
    let started = as_tenant(tenant, || manager.start_execution(request(script))).expect("start");
    assert_eq!(
        started.decision.resolution,
        DecisionResolution::Allow,
        "{:?}",
        started.decision
    );
    wait(manager, &started.execution_id, Duration::from_secs(60))
}

fn wait(manager: &Manager, execution_id: &str, limit: Duration) -> kura_sandbox::Execution {
    let deadline = std::time::Instant::now() + limit;
    loop {
        let execution = manager.get_execution(execution_id).expect("execution");
        if is_terminal(execution.status) {
            return execution;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{execution_id} never finished"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The Stage 3 bar: a hostile workload cannot read another tenant's
/// workspace or the daemon's data dir, does not run as root, cannot write
/// the image, and runs under the configured cgroup limits.
#[test]
fn a_tenant_container_cannot_reach_another_tenant_or_the_daemon() {
    if !docker_tests_enabled("a_tenant_container_cannot_reach_another_tenant_or_the_daemon") {
        return;
    }
    let data_dir = temp_dir("docker_cross_tenant");
    std::fs::write(
        std::path::Path::new(&data_dir).join("daemon-secret"),
        "daemon",
    )
    .expect("seed daemon file");
    let manager = manager(&data_dir);

    let a = run_as(
        &manager,
        "ten_a",
        "echo a-secret > /workspace/note && id -u",
    );
    assert_eq!(a.status, ExecutionStatus::Completed, "{:?}", a.result);
    assert_eq!(a.result.stdout.trim(), tenant_uid("ten_a").to_string());

    let probe = format!(
        r#"
        test -e /workspace/note && echo SEES_OWN_PATH
        test -r {data_dir}/daemon-secret && echo SEES_DATA_DIR
        find / \( -path /proc -o -path /sys \) -prune -o -name note -print 2>/dev/null | grep -q . && echo FOUND_A
        touch /probe 2>/dev/null && echo ROOTFS_WRITABLE
        [ "$(id -u)" = 0 ] && echo IS_ROOT
        wget -q -T 2 -O- http://1.1.1.1 >/dev/null 2>&1 && echo HAS_NETWORK
        echo done
        "#
    );
    let b = run_as(&manager, "ten_b", &probe);
    assert_eq!(b.status, ExecutionStatus::Completed, "{:?}", b.result);
    assert_eq!(
        b.result.stdout.trim(),
        "done",
        "stderr: {}",
        b.result.stderr
    );

    // Tenant A still has its file: B's run did not share or wipe it.
    let again = run_as(&manager, "ten_a", "cat /workspace/note");
    assert_eq!(again.result.stdout.trim(), "a-secret");

    let limits = run_as(
        &manager,
        "ten_a",
        "cat /sys/fs/cgroup/memory.max /sys/fs/cgroup/pids.max",
    );
    let lines: Vec<&str> = limits.result.stdout.lines().collect();
    assert_eq!(
        lines,
        ["536870912", "256"],
        "cgroup v2 limits: {:?}",
        limits.result
    );
}

/// Cancelling a run removes its container; killing only the `docker run`
/// client used to leave the container running.
#[test]
fn cancelling_an_isolated_run_removes_its_container() {
    if !docker_tests_enabled("cancelling_an_isolated_run_removes_its_container") {
        return;
    }
    let data_dir = temp_dir("docker_cancel");
    let manager = manager(&data_dir);
    let started =
        as_tenant("ten_a", || manager.start_execution(request("sleep 60"))).expect("start");
    let name = format!("kura-exec-{}", started.execution_id);
    let listed = |all: bool| {
        let mut args = vec!["ps", "-q"];
        if all {
            args.push("-a");
        }
        let filter = format!("name={name}");
        args.extend(["--filter", filter.as_str()]);
        let output = std::process::Command::new("docker")
            .args(&args)
            .output()
            .expect("docker ps");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    // The container must really be running, or the cleanup below proves
    // nothing (a launch that failed outright would also leave none).
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while listed(false).is_empty() {
        let execution = manager
            .get_execution(&started.execution_id)
            .expect("execution");
        assert!(
            !is_terminal(execution.status),
            "run ended before its container came up: {:?}",
            execution.result
        );
        assert!(
            std::time::Instant::now() < deadline,
            "container {name} never started"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    manager
        .cancel_execution(&started.execution_id)
        .expect("cancel");
    wait(&manager, &started.execution_id, Duration::from_secs(30));
    assert!(
        listed(true).is_empty(),
        "container {name} survived its cancellation"
    );
}

/// Stage 3.3: an isolated container serves a live stdio session (how MCP
/// stdio servers run for tenants): bytes in through stdin come back on
/// stdout, the process sees only the tenant workspace, a secret passed as
/// env reaches the process without appearing in any host argv, and closing
/// stdin ends the run with the container gone.
#[test]
fn an_isolated_container_serves_a_live_stdio_session() {
    if !docker_tests_enabled("an_isolated_container_serves_a_live_stdio_session") {
        return;
    }
    use std::io::{BufRead, Write};
    let data_dir = temp_dir("docker_attached");
    let manager = manager(&data_dir);
    let secret = "sk-attached-secret-value";
    let mut request =
        request(r#"while read line; do echo "got:$line:$(pwd):$API_TOKEN:$(id -u)"; done"#);
    request
        .env
        .insert("API_TOKEN".to_string(), secret.to_string());
    let (execution, attached) =
        as_tenant("ten_a", || manager.start_attached_execution(request)).expect("attached");
    assert_eq!(
        execution.status,
        ExecutionStatus::Running,
        "{:?}",
        execution.result
    );
    let mut attached = attached.expect("live session");
    let name = format!("kura-exec-{}", execution.execution_id);

    // The secret is in the container, not on any host command line.
    let ps = std::process::Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .expect("ps");
    assert!(
        !String::from_utf8_lossy(&ps.stdout).contains(secret),
        "a host argv carries the secret"
    );

    let mut stdin = attached.stdin.take().expect("stdin");
    let mut stdout = std::io::BufReader::new(attached.stdout.take().expect("stdout"));
    writeln!(stdin, "ping").expect("write");
    stdin.flush().expect("flush");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read");
    assert_eq!(
        line.trim(),
        format!("got:ping:/workspace:{secret}:{}", tenant_uid("ten_a"))
    );

    drop(stdin);
    let finished = wait(&manager, &execution.execution_id, Duration::from_secs(30));
    assert_eq!(
        finished.status,
        ExecutionStatus::Completed,
        "{:?}",
        finished.result
    );
    let listed = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("name={name}")])
        .output()
        .expect("docker ps");
    assert!(String::from_utf8_lossy(&listed.stdout).trim().is_empty());
}

/// Removes the test's docker network and proxy container however the test
/// ends.
struct EgressFixture {
    network: String,
    proxy: String,
}

impl Drop for EgressFixture {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.proxy])
            .output();
        let _ = std::process::Command::new("docker")
            .args(["network", "rm", &self.network])
            .output();
    }
}

fn docker(args: &[&str]) -> String {
    let output = std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("docker");
    assert!(
        output.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Stage 3.5 end to end: a tenant container on the `--internal` tenant
/// network reaches the world only through `kura egress-proxy`. Plain HTTP
/// through the proxy works; the metadata address is refused by the proxy;
/// bypassing the proxy finds no route; DNS for outside names goes nowhere.
/// Needs `KURA_EGRESS_PROXY_BIN`: a `kura` binary runnable in ubuntu:24.04.
#[test]
fn a_tenant_container_reaches_the_network_only_through_the_egress_proxy() {
    if !docker_tests_enabled("a_tenant_container_reaches_the_network_only_through_the_egress_proxy")
    {
        return;
    }
    // With docker tests on, a missing binary is a failure, not a skip: a
    // skipped test reports `ok`, and this is the test that proves the only
    // network exit.
    let binary = std::env::var("KURA_EGRESS_PROXY_BIN")
        .expect("KURA_DOCKER_TESTS=1 needs KURA_EGRESS_PROXY_BIN (a kura binary for ubuntu:24.04)");
    let fixture = EgressFixture {
        network: format!("kura-tenant-test-{}", std::process::id()),
        proxy: format!("kura-egress-proxy-test-{}", std::process::id()),
    };
    docker(&["network", "create", "--internal", &fixture.network]);
    docker(&[
        "run",
        "-d",
        "--name",
        &fixture.proxy,
        "--mount",
        &format!("type=bind,src={binary},dst=/kura,readonly"),
        "ubuntu:24.04",
        "/kura",
        "egress-proxy",
        "--listen",
        "0.0.0.0:3128",
    ]);
    docker(&["network", "connect", &fixture.network, &fixture.proxy]);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let proxy_logs = || {
        let output = std::process::Command::new("docker")
            .args(["logs", &fixture.proxy])
            .output()
            .expect("docker logs");
        // The proxy logs to stderr.
        String::from_utf8_lossy(&output.stderr).to_string()
    };
    while !proxy_logs().contains("listening") {
        assert!(std::time::Instant::now() < deadline, "proxy never started");
        std::thread::sleep(Duration::from_millis(200));
    }

    let manager = manager(&temp_dir("docker_egress"));
    manager
        .configure_tenant_egress(&fixture.network, &format!("http://{}:3128", fixture.proxy))
        .expect("the test network is internal");
    let script = r#"
        wget -q -T 15 -O /dev/null http://example.com/ && echo PROXIED_OK
        wget -q -T 5 -O /dev/null http://169.254.169.254/ || echo METADATA_REFUSED
        env -u http_proxy -u HTTP_PROXY wget -q -T 5 -O /dev/null http://93.184.216.34/ || echo DIRECT_BLOCKED
        nslookup example.com >/dev/null 2>&1 || echo DNS_BLOCKED
    "#;
    let started = as_tenant("ten_a", || manager.start_execution(networked(script))).expect("start");
    assert_eq!(
        started.decision.resolution,
        DecisionResolution::Allow,
        "{:?}",
        started.decision
    );
    let run = wait(&manager, &started.execution_id, Duration::from_secs(90));
    let lines: Vec<&str> = run.result.stdout.lines().map(str::trim).collect();
    assert_eq!(
        lines,
        [
            "PROXIED_OK",
            "METADATA_REFUSED",
            "DIRECT_BLOCKED",
            "DNS_BLOCKED"
        ],
        "stderr: {}",
        run.result.stderr
    );
    let logs = proxy_logs();
    assert!(logs.contains("refused 169.254.169.254:80"), "{logs}");
}

// --- Stage 3.4: the container runtime (gVisor) ----------------------------

#[test]
fn a_tenant_runtime_must_be_registered_with_docker() {
    let manager = manager(&temp_dir("runtime_config"));
    let registered = || Ok(vec!["runc".to_string(), "runsc".to_string()]);
    for bad in ["", "run sc", "runsc;rm", "../runsc"] {
        assert!(
            manager
                .configure_tenant_runtime_with(bad, &registered)
                .is_err(),
            "accepted {bad:?}"
        );
    }
    let err = manager
        .configure_tenant_runtime_with("kata", &registered)
        .expect_err("an unregistered runtime is refused");
    assert!(
        err.contains("no runtime kata") && err.contains("runsc"),
        "{err}"
    );
    assert!(
        manager
            .configure_tenant_runtime_with("runsc", &|| Err("docker is down".into()))
            .is_err()
    );
    manager
        .configure_tenant_runtime_with("runsc", &registered)
        .expect("a registered runtime is accepted");
}

/// With `KURA_DOCKER_TESTS=1` and `KURA_DOCKER_TEST_RUNTIME=runsc` on a host
/// with gVisor registered: the tenant's code runs on gVisor's kernel, with
/// the same isolation as on the default runtime.
#[test]
fn an_isolated_run_uses_the_configured_runtime() {
    if !docker_tests_enabled("an_isolated_run_uses_the_configured_runtime") {
        return;
    }
    let Ok(runtime) = std::env::var("KURA_DOCKER_TEST_RUNTIME") else {
        eprintln!("skipping: set KURA_DOCKER_TEST_RUNTIME (e.g. runsc) to a registered runtime");
        return;
    };
    let manager = manager(&temp_dir("docker_runtime"));
    manager
        .configure_tenant_runtime(&runtime)
        .expect("runtime registered with docker");
    let run = run_as(
        &manager,
        "ten_a",
        "uname -r; id -u; touch /probe 2>/dev/null && echo ROOTFS_WRITABLE; echo hi > /workspace/f && cat /workspace/f",
    );
    assert_eq!(run.status, ExecutionStatus::Completed, "{:?}", run.result);
    let out: Vec<&str> = run.result.stdout.lines().collect();
    if runtime == "runsc" {
        assert!(out[0].contains("gvisor"), "kernel {:?}", out[0]);
    }
    assert_eq!(out[1..], [tenant_uid("ten_a").to_string().as_str(), "hi"]);
}

// --- Attached executions are sessions, not batch runs ---------------------

#[test]
fn an_attached_session_is_bounded_only_by_a_timeout_its_caller_sets() {
    assert_eq!(
        kura_sandbox::attached_watchdog(0, Duration::from_secs(30)),
        Duration::ZERO,
        "the profile's batch default does not end a session"
    );
    assert_eq!(
        kura_sandbox::attached_watchdog(2_000, Duration::from_secs(2)),
        Duration::from_secs(2)
    );
}

/// The process behind an attached session outlives a short request bound
/// only when there is none: with an explicit bound it is gone after it.
#[cfg(unix)]
#[test]
fn an_explicit_attached_timeout_still_ends_the_session() {
    use std::io::{BufRead, BufReader, Write};
    let manager = manager(&temp_dir("attached_timeout"));
    let start = |timeout_ms| {
        let (_, attached) = manager
            .start_attached_execution(ExecutionRequest {
                command: "/bin/cat".to_string(),
                requested_by: "test".to_string(),
                timeout_ms,
                ..ExecutionRequest::default()
            })
            .expect("start");
        let attached = attached.expect("attached");
        (
            attached.stdin.expect("stdin"),
            BufReader::new(attached.stdout.expect("stdout")),
        )
    };
    let echoes = |stdin: &mut std::process::ChildStdin,
                  stdout: &mut BufReader<std::process::ChildStdout>| {
        if writeln!(stdin, "ping")
            .and_then(|()| stdin.flush())
            .is_err()
        {
            return false;
        }
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap_or(0) > 0
    };
    let (mut bounded_in, mut bounded_out) = start(500);
    let (mut open_in, mut open_out) = start(0);
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(
        !echoes(&mut bounded_in, &mut bounded_out),
        "a 500 ms session outlived its bound"
    );
    assert!(
        echoes(&mut open_in, &mut open_out),
        "an unbounded session was ended"
    );
}

/// Under gVisor docker's embedded DNS does not answer, so the proxy has to
/// be an IP; a name is refused in whichever order the two are configured.
#[test]
fn under_runsc_the_egress_proxy_must_be_an_ip() {
    let registered = || Ok(vec!["runc".to_string(), "runsc".to_string()]);
    let internal = |_: &str| Ok(true);

    let by_name = manager(&temp_dir("runsc_proxy_name"));
    by_name
        .configure_tenant_egress_with("kura-tenant", "http://kura-egress-proxy:3128", &internal)
        .expect("a name is fine on runc");
    let err = by_name
        .configure_tenant_runtime_with("runsc", &registered)
        .expect_err("runsc with a proxy name");
    assert!(err.contains("by IP"), "{err}");
    by_name
        .configure_tenant_runtime_with("runc", &registered)
        .expect("runc resolves names");

    let runtime_first = manager(&temp_dir("runsc_proxy_order"));
    runtime_first
        .configure_tenant_runtime_with("runsc", &registered)
        .expect("runtime");
    assert!(
        runtime_first
            .configure_tenant_egress_with("kura-tenant", "http://kura-egress-proxy:3128", &internal)
            .is_err()
    );
    runtime_first
        .configure_tenant_egress_with("kura-tenant", "http://172.31.0.2:3128", &internal)
        .expect("an IP works under runsc");
}
