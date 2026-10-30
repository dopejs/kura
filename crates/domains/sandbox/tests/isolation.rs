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
    kura_identity::tenantctx::with_context(
        kura_identity::TenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: format!("prn_{tenant_id}"),
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
