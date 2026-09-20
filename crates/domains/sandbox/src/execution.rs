//! Process execution for the sandbox manager.
//!
//! Ported from daemon/internal/sandbox/manager.go: the launch spec, docker mount
//! model, cancellation token (Go context.CancelFunc), capture buffer
//! (Go captureBuffer), subprocess result, executeSubprocess, executeDocker,
//! startedBackendMetadata, dockerImageForExecution, dockerNetworkMode,
//! dockerMountsForExecution, mergeBackendMetadata,
//! requiresManagedProviderFinalization, awaitsManagedProviderFinalization, and
//! the runExecution / runAttachedExecution manager methods. Attached execution
//! streams via std::thread + std::sync::mpsc, mirroring Go's goroutine +
//! buffered channel wait.

use std::collections::HashSet;
use std::io::Read;
use std::io::Write as _;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};

use crate::manager::{Manager, synchronize_execution_consumer_state};
use crate::redaction::collect_secret_redaction_values_from_process_env;
use crate::redaction::redact_secret_text;
use crate::{BackendKind, ErrorClass, Execution, ExecutionStatus, Result as SandboxResult};

/// Approval action used when requesting sandbox escalations (Go
/// sandboxApprovalAction).
pub const SANDBOX_APPROVAL_ACTION: &str = "sandbox.execute";
/// Resource kind used when requesting sandbox escalations (Go
/// sandboxResourceKind).
pub const SANDBOX_RESOURCE_KIND: &str = "sandbox_profile";
/// Event category for sandbox lifecycle events (Go eventCategory).
pub const EVENT_CATEGORY: &str = "sandbox";
/// Event resource kind for executions (Go resourceKindExecution).
pub const RESOURCE_KIND_EXECUTION: &str = "sandbox_execution";
/// backendMetadata processType for subprocess backends (Go
/// backendMetaProcessKind).
pub const BACKEND_META_PROCESS_KIND: &str = "process";
/// backendMetadata processType for docker backends (Go
/// backendMetaProcessKindContainer).
pub const BACKEND_META_PROCESS_KIND_CONTAINER: &str = "container";
/// Execution metadata key marking a completed subprocess that still awaits
/// managed-provider finalization (Go managedProviderPendingFinalizationKey).
pub const MANAGED_PROVIDER_PENDING_FINALIZATION_KEY: &str = "managedProviderFinalizationPending";
/// Default docker image used when KURA_SANDBOX_DOCKER_IMAGE is unset (Go
/// defaultDockerImage).
pub const DEFAULT_DOCKER_IMAGE: &str = "alpine:3.20";

/// Timeout for docker runtime/image probes (Go defaultDockerProbeTimeout).
pub const DEFAULT_DOCKER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Go `launchSpec`: everything executeSubprocess / executeDocker need.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub backend_kind: BackendKind,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<String>,
    pub secret_values: Vec<String>,
    pub stdin: String,
    pub timeout: Duration,
    pub kill_grace: Duration,
    pub capture_stdout: bool,
    pub capture_stderr: bool,
    pub max_output_bytes: i64,
    pub docker_image: String,
    pub docker_network: String,
    pub docker_mounts: Vec<DockerMount>,
}

/// Go `dockerMount`: a bind mount for docker executions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerMount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

/// Go `context.CancelFunc` -> cancellation token: an atomic flag plus the
/// registered child process handle so cancellation terminates the process.
#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Requests cancellation: sets the flag and force-kills the registered
    /// child (Go CommandContext kills the process when the context is done).
    /// The graceful interrupt path lives in the wait loops, which observe the
    /// flag and interrupt-then-kill when the child is still alive.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let pid = child.id();
            let _ = child.kill();
            // The child is spawned as its own process group (see the
            // manager's spawn), so its descendants go with it. Killing only
            // the direct child left `sh -c "sleep 5"` on dash — which does not
            // exec its last command — with a live `sleep` holding the output
            // pipes open, and the execution stayed non-terminal until that
            // grandchild finished on its own.
            kill_process_group(pid);
        }
    }

    pub(crate) fn register_child(&self, child: Child) {
        *self.child.lock().unwrap() = Some(child);
    }

    pub(crate) fn try_wait_child(&self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let mut guard = self.child.lock().unwrap();
        match guard.as_mut() {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }
    }

    pub(crate) fn kill_child(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let pid = child.id();
            let _ = child.kill();
            kill_process_group(pid);
        }
    }
}

/// Go `captureBuffer`: an io.Writer that keeps at most `limit` bytes and
/// records truncation, mirroring the Go bytes.Buffer-based implementation.
#[derive(Debug)]
pub struct CaptureBuffer {
    limit: usize,
    size: usize,
    truncated: bool,
    buf: Vec<u8>,
}

impl CaptureBuffer {
    #[must_use]
    pub fn new(limit: i64) -> Self {
        let limit = if limit <= 0 {
            64 * 1024
        } else {
            limit as usize
        };
        CaptureBuffer {
            limit,
            size: 0,
            truncated: false,
            buf: Vec::new(),
        }
    }

    /// Go `String()`: the captured content with surrounding whitespace
    /// trimmed.
    #[must_use]
    pub fn as_str(&self) -> String {
        String::from_utf8_lossy(&self.buf).trim().to_string()
    }

    /// Go `Truncated()`.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated || self.size > self.limit
    }
}

impl std::io::Write for CaptureBuffer {
    fn write(&mut self, p: &[u8]) -> std::io::Result<usize> {
        self.size += p.len();
        let remaining = self.limit.saturating_sub(self.buf.len());
        if remaining > 0 {
            let take = std::cmp::min(p.len(), remaining);
            self.buf.extend_from_slice(&p[..take]);
            if take < p.len() {
                self.truncated = true;
            }
        } else {
            self.truncated = true;
        }
        Ok(p.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Go `subprocessResult`: the raw outcome of executeSubprocess /
/// executeDocker before redaction and manager finalization.
#[derive(Debug, Clone, Default)]
pub struct SubprocessResult {
    pub status: ExecutionStatus,
    pub exit_code: Option<i64>,
    pub signal: String,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
    pub error_class: ErrorClass,
    pub error_code: String,
    pub error: String,
    pub backend_metadata: serde_json::Map<String, serde_json::Value>,
}

/// Go `AttachedExecution`: the execution handle plus the live stdin/stdout
/// pipes. Stderr is captured into the terminal result (Go leaves it nil on the
/// returned handle).
pub struct AttachedExecution {
    pub execution: Execution,
    pub stdin: Option<std::process::ChildStdin>,
    pub stdout: Option<std::process::ChildStdout>,
}

/// Go `executeSubprocess`: runs launch.Command with the captured/inherited
/// stdio wiring, a cancellation-aware wait loop (interrupt, grace, kill), and
/// captures stdout/stderr into bounded buffers.
#[must_use]
pub fn execute_subprocess(
    launch: &LaunchSpec,
    cancel: &CancellationToken,
    timed_out: &AtomicBool,
) -> SubprocessResult {
    let started_at = Utc::now();
    let mut command = Command::new(&launch.command);
    command.args(&launch.args);
    if !launch.cwd.is_empty() {
        command.current_dir(&launch.cwd);
    }
    command.env_clear();
    for item in &launch.env {
        if let Some((key, value)) = item.split_once('=') {
            command.env(key, value);
        }
    }
    if launch.stdin.is_empty() {
        command.stdin(Stdio::null());
    } else {
        command.stdin(Stdio::piped());
    }
    command.stdout(if launch.capture_stdout {
        Stdio::piped()
    } else {
        Stdio::inherit()
    });
    command.stderr(if launch.capture_stderr {
        Stdio::piped()
    } else {
        Stdio::inherit()
    });

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let mut metadata = serde_json::Map::new();
            metadata.insert("backend".to_string(), serde_json::json!("subprocess"));
            return SubprocessResult {
                status: ExecutionStatus::Failed,
                error_class: ErrorClass::LaunchFailed,
                error_code: "sandbox_launch_failed".to_string(),
                error: err.to_string(),
                backend_metadata: metadata,
                ..SubprocessResult::default()
            };
        }
    };
    let pid = child.id();

    // Write stdin from a helper thread (Go os/exec copies strings.Reader).
    if !launch.stdin.is_empty() {
        if let Some(mut stdin) = child.stdin.take() {
            let input = launch.stdin.clone();
            std::thread::spawn(move || {
                let _ = stdin.write_all(input.as_bytes());
                let _ = stdin.flush();
            });
        }
    } else {
        drop(child.stdin.take());
    }

    let stdout_capture = Arc::new(Mutex::new(CaptureBuffer::new(launch.max_output_bytes)));
    let stderr_capture = Arc::new(Mutex::new(CaptureBuffer::new(launch.max_output_bytes)));
    let mut readers = Vec::new();
    if launch.capture_stdout {
        if let Some(pipe) = child.stdout.take() {
            let capture = Arc::clone(&stdout_capture);
            readers.push(std::thread::spawn(move || read_pipe_into(pipe, capture)));
        }
    }
    if launch.capture_stderr {
        if let Some(pipe) = child.stderr.take() {
            let capture = Arc::clone(&stderr_capture);
            readers.push(std::thread::spawn(move || read_pipe_into(pipe, capture)));
        }
    }
    cancel.register_child(child);

    enum WaitOutcome {
        Finished(std::process::ExitStatus),
        WaitFailed(std::io::Error),
    }
    let outcome = loop {
        if cancel.is_cancelled() {
            // The cancelled path kills the child itself, so the readers are
            // joined only after the (inherently truncated) snapshot.
            let result = cancelled_subprocess_result(
                launch,
                pid,
                cancel,
                timed_out,
                &stdout_capture,
                &stderr_capture,
            );
            for handle in readers {
                let _ = handle.join();
            }
            return result;
        }
        match cancel.try_wait_child() {
            Ok(Some(status)) => break WaitOutcome::Finished(status),
            Ok(None) => {}
            Err(err) => break WaitOutcome::WaitFailed(err),
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    // Go command.Wait() waits for the stdout/stderr copy goroutines; the
    // child has exited (pipes at EOF), so join the pipe readers BEFORE the
    // result snapshots the capture buffers — reading them first drops output
    // still in flight on a loaded host.
    for handle in readers {
        let _ = handle.join();
    }
    match outcome {
        WaitOutcome::Finished(status) => finish_subprocess_result(
            launch,
            pid,
            started_at,
            status,
            &stdout_capture,
            &stderr_capture,
        ),
        WaitOutcome::WaitFailed(err) => {
            io_capture_failed_result(pid, err, &stdout_capture, &stderr_capture)
        }
    }
}

/// Reads a child pipe into a shared capture buffer until EOF.
pub(crate) fn read_pipe_into(mut pipe: impl Read, capture: Arc<Mutex<CaptureBuffer>>) {
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Ok(mut capture) = capture.lock() {
                    let _ = capture.write(&buf[..n]);
                }
            }
        }
    }
}

/// Go `executeDocker`: builds the docker CLI invocation and delegates to
/// executeSubprocess, then decorates the backend metadata.
#[must_use]
pub fn execute_docker(
    launch: &LaunchSpec,
    cancel: &CancellationToken,
    timed_out: &AtomicBool,
) -> SubprocessResult {
    let mut args: Vec<String> = vec!["run".to_string(), "--rm".to_string(), "-i".to_string()];
    if !launch.docker_network.is_empty() {
        args.push("--network".to_string());
        args.push(launch.docker_network.clone());
    }
    if !launch.cwd.trim().is_empty() {
        args.push("--workdir".to_string());
        args.push(launch.cwd.clone());
    }
    for mount in &launch.docker_mounts {
        let mut spec = format!("type=bind,src={},dst={}", mount.source, mount.target);
        if mount.read_only {
            spec.push_str(",readonly");
        }
        args.push("--mount".to_string());
        args.push(spec);
    }
    for item in &launch.env {
        args.push("-e".to_string());
        args.push(item.clone());
    }
    args.push(launch.docker_image.clone());
    args.push(launch.command.clone());
    args.extend(launch.args.iter().cloned());

    let docker_launch = LaunchSpec {
        command: "docker".to_string(),
        args,
        ..launch.clone()
    };

    let mut result = execute_subprocess(&docker_launch, cancel, timed_out);
    result.backend_metadata.insert(
        "backend".to_string(),
        serde_json::json!(BackendKind::Docker.as_str()),
    );
    result.backend_metadata.insert(
        "dockerImage".to_string(),
        serde_json::json!(launch.docker_image),
    );
    result.backend_metadata.insert(
        "dockerNetwork".to_string(),
        serde_json::json!(launch.docker_network),
    );
    result.backend_metadata.insert(
        "mountCount".to_string(),
        serde_json::json!(launch.docker_mounts.len()),
    );
    result.backend_metadata.insert(
        "processType".to_string(),
        serde_json::json!(BACKEND_META_PROCESS_KIND_CONTAINER),
    );
    result
        .backend_metadata
        .insert("networkEnforcement".to_string(), serde_json::json!(true));
    result.backend_metadata.insert(
        "networkPolicyStrength".to_string(),
        serde_json::json!("container_runtime"),
    );
    result
}

/// Builds the subprocess result from a normal (non-cancelled) exit.
fn finish_subprocess_result(
    _launch: &LaunchSpec,
    pid: u32,
    started_at: DateTime<Utc>,
    status: std::process::ExitStatus,
    stdout_capture: &Arc<Mutex<CaptureBuffer>>,
    stderr_capture: &Arc<Mutex<CaptureBuffer>>,
) -> SubprocessResult {
    let completed_at = Utc::now();
    let mut metadata = serde_json::Map::new();
    metadata.insert("backend".to_string(), serde_json::json!("subprocess"));
    metadata.insert("pid".to_string(), serde_json::json!(pid));
    metadata.insert(
        "startedAt".to_string(),
        serde_json::json!(started_at.to_rfc3339_opts(SecondsFormat::Nanos, true)),
    );
    metadata.insert(
        "completedAt".to_string(),
        serde_json::json!(completed_at.to_rfc3339_opts(SecondsFormat::Nanos, true)),
    );
    metadata.insert(
        "platform".to_string(),
        serde_json::json!(std::env::consts::OS),
    );
    metadata.insert(
        "architecture".to_string(),
        serde_json::json!(std::env::consts::ARCH),
    );
    let stdout = capture_string(stdout_capture);
    let stderr = capture_string(stderr_capture);
    let output_truncated = capture_truncated(stdout_capture) || capture_truncated(stderr_capture);
    if status.success() {
        return SubprocessResult {
            status: ExecutionStatus::Completed,
            exit_code: Some(0),
            stdout,
            stderr,
            output_truncated,
            backend_metadata: metadata,
            ..SubprocessResult::default()
        };
    }
    let exit_code = status.code().unwrap_or(-1) as i64;
    SubprocessResult {
        status: ExecutionStatus::Failed,
        exit_code: Some(exit_code),
        stdout,
        stderr,
        output_truncated,
        error_class: ErrorClass::ProcessFailed,
        error_code: "sandbox_process_failed".to_string(),
        error: format!("exit status {exit_code}"),
        backend_metadata: metadata,
        ..SubprocessResult::default()
    }
}

/// Go's non-ExitError wait failure (pipe copy / wait error) branch.
fn io_capture_failed_result(
    pid: u32,
    err: std::io::Error,
    stdout_capture: &Arc<Mutex<CaptureBuffer>>,
    stderr_capture: &Arc<Mutex<CaptureBuffer>>,
) -> SubprocessResult {
    let mut metadata = serde_json::Map::new();
    metadata.insert("backend".to_string(), serde_json::json!("subprocess"));
    metadata.insert("pid".to_string(), serde_json::json!(pid));
    SubprocessResult {
        status: ExecutionStatus::Failed,
        stdout: capture_string(stdout_capture),
        stderr: capture_string(stderr_capture),
        output_truncated: capture_truncated(stdout_capture) || capture_truncated(stderr_capture),
        error_class: ErrorClass::IoCaptureFailed,
        error_code: "sandbox_wait_failed".to_string(),
        error: err.to_string(),
        backend_metadata: metadata,
        ..SubprocessResult::default()
    }
}

/// Go's ctx.Done() branch: interrupt, grace, kill; the result distinguishes
/// timeout (deadline exceeded) from plain cancellation.
fn cancelled_subprocess_result(
    launch: &LaunchSpec,
    pid: u32,
    cancel: &CancellationToken,
    timed_out: &AtomicBool,
    stdout_capture: &Arc<Mutex<CaptureBuffer>>,
    stderr_capture: &Arc<Mutex<CaptureBuffer>>,
) -> SubprocessResult {
    let mut signal = String::new();
    if pid != 0 {
        send_interrupt(pid);
        signal = "interrupt".to_string();
    }
    let grace = max_duration(launch.kill_grace, Duration::from_secs(1));
    let deadline = Instant::now() + grace;
    loop {
        if matches!(cancel.try_wait_child(), Ok(Some(_))) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !matches!(cancel.try_wait_child(), Ok(Some(_))) {
        cancel.kill_child();
        signal = "kill".to_string();
        let reap_deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if matches!(cancel.try_wait_child(), Ok(Some(_))) {
                break;
            }
            if Instant::now() >= reap_deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let (error_class, error_code, error_text) = if timed_out.load(Ordering::SeqCst) {
        (
            ErrorClass::Timeout,
            "sandbox_timeout".to_string(),
            "execution timed out".to_string(),
        )
    } else {
        (
            ErrorClass::Cancelled,
            "sandbox_cancelled".to_string(),
            "execution was cancelled".to_string(),
        )
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert("backend".to_string(), serde_json::json!("subprocess"));
    SubprocessResult {
        status: status_for_cancel(timed_out.load(Ordering::SeqCst)),
        stdout: capture_string(stdout_capture),
        stderr: capture_string(stderr_capture),
        signal,
        output_truncated: capture_truncated(stdout_capture) || capture_truncated(stderr_capture),
        error_class,
        error_code,
        error: error_text,
        backend_metadata: metadata,
        ..SubprocessResult::default()
    }
}

fn capture_string(capture: &Arc<Mutex<CaptureBuffer>>) -> String {
    capture.lock().map(|c| c.as_str()).unwrap_or_default()
}

fn capture_truncated(capture: &Arc<Mutex<CaptureBuffer>>) -> bool {
    capture.lock().map(|c| c.truncated()).unwrap_or(false)
}

/// Go `statusForContext`: deadline exceeded maps to Failed, otherwise
/// Cancelled.
#[must_use]
pub fn status_for_cancel(timed_out: bool) -> ExecutionStatus {
    if timed_out {
        ExecutionStatus::Failed
    } else {
        ExecutionStatus::Cancelled
    }
}

/// Go `maxDuration`.
#[must_use]
pub fn max_duration(value: Duration, fallback: Duration) -> Duration {
    if value.is_zero() { fallback } else { value }
}

/// Force-kills every process in the child's process group. Best-effort; a
/// group that already exited is not an error.
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_process_group(_pid: u32) {}

/// Sends SIGINT to a child pid (Go os.Interrupt). Best-effort.
#[cfg(unix)]
fn send_interrupt(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGINT);
    }
}

#[cfg(not(unix))]
fn send_interrupt(_pid: u32) {}

/// Go `startedBackendMetadata`.
#[must_use]
pub fn started_backend_metadata(
    execution: &Execution,
    extra: Option<&serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "backend".to_string(),
        serde_json::json!(execution.backend_kind.as_str()),
    );
    match execution.backend_kind {
        BackendKind::Docker => {
            metadata.insert("networkEnforcement".to_string(), serde_json::json!(true));
            metadata.insert(
                "networkPolicyStrength".to_string(),
                serde_json::json!("container_runtime"),
            );
            metadata.insert(
                "processType".to_string(),
                serde_json::json!(BACKEND_META_PROCESS_KIND_CONTAINER),
            );
        }
        _ => {
            metadata.insert(
                "networkEnforcement".to_string(),
                serde_json::json!(
                    execution.decision.effective_backend_kind == BackendKind::Subprocess
                ),
            );
            metadata.insert(
                "networkPolicyStrength".to_string(),
                serde_json::json!("declared_only"),
            );
            metadata.insert(
                "processType".to_string(),
                serde_json::json!(BACKEND_META_PROCESS_KIND),
            );
        }
    }
    if let Some(extra) = extra {
        for (key, value) in extra {
            metadata.insert(key.clone(), value.clone());
        }
    }
    metadata
}

/// Go `dockerImageForExecution`.
#[must_use]
pub fn docker_image_for_execution() -> String {
    if let Ok(value) = std::env::var("KURA_SANDBOX_DOCKER_IMAGE") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    DEFAULT_DOCKER_IMAGE.to_string()
}

/// Go `dockerNetworkMode`.
#[must_use]
pub fn docker_network_mode(access: &crate::AccessRequest) -> String {
    if access.network_mode == Some(crate::NetworkMode::Deny) || access.network_mode.is_none() {
        return "none".to_string();
    }
    "bridge".to_string()
}

/// Go `dockerMountsForExecution`.
#[must_use]
pub fn docker_mounts_for_execution(cwd: &str, access: &crate::AccessRequest) -> Vec<DockerMount> {
    let mut roots: Vec<DockerMount> =
        Vec::with_capacity(access.read_roots.len() + access.write_roots.len() + 2);
    let mut writable: HashSet<String> = HashSet::new();
    for root in &access.write_roots {
        let trimmed = crate::manager::clean_path(root.trim());
        if trimmed != "." && !trimmed.is_empty() {
            writable.insert(trimmed);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |root: &str, read_only: bool| {
        let trimmed = crate::manager::clean_path(root.trim());
        if trimmed == "." || trimmed.is_empty() {
            return;
        }
        if seen.contains(&trimmed) {
            if !read_only {
                for mount in roots.iter_mut() {
                    if mount.source == trimmed {
                        mount.read_only = false;
                    }
                }
            }
            return;
        }
        seen.insert(trimmed.clone());
        roots.push(DockerMount {
            source: trimmed.clone(),
            target: trimmed,
            read_only,
        });
    };
    if !cwd.is_empty() {
        let is_writable = writable.contains(&crate::manager::clean_path(cwd));
        add(cwd, !is_writable);
    }
    for root in &access.read_roots {
        let is_writable = writable.contains(&crate::manager::clean_path(root.trim()));
        add(root, !is_writable);
    }
    for root in &access.write_roots {
        add(root, false);
    }
    roots
}

/// Go `mergeBackendMetadata`: folds managed-provider execution metadata into
/// the backend metadata map.
#[must_use]
pub fn merge_backend_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
    execution: &Execution,
) -> serde_json::Map<String, serde_json::Value> {
    let mut out = metadata.clone();
    if let Some(provider_id) = trimmed_metadata(&execution.metadata, "managedProviderId") {
        out.insert(
            "managedProviderId".to_string(),
            serde_json::json!(provider_id),
        );
    }
    if let Some(action) = trimmed_metadata(&execution.metadata, "managedProviderAction") {
        out.insert(
            "managedProviderAction".to_string(),
            serde_json::json!(action),
        );
    }
    if let Some(operation_id) = trimmed_metadata(&execution.metadata, "managedProviderOperationId")
    {
        out.insert(
            "managedProviderOperationId".to_string(),
            serde_json::json!(operation_id),
        );
    }
    if let Some(strength) = trimmed_metadata(&execution.metadata, "enforcementStrength") {
        out.insert(
            "enforcementStrength".to_string(),
            serde_json::json!(strength),
        );
    }
    if let Some(classes) = trimmed_metadata(&execution.metadata, "sensitiveStateClasses") {
        let split: Vec<String> = classes.split(',').map(|s| s.to_string()).collect();
        out.insert(
            "sensitiveStateClasses".to_string(),
            serde_json::json!(split),
        );
    }
    out
}

fn trimmed_metadata(
    metadata: &std::collections::HashMap<String, String>,
    key: &str,
) -> Option<String> {
    metadata
        .get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Go `requiresManagedProviderFinalization`.
#[must_use]
pub fn requires_managed_provider_finalization(execution: &Execution) -> bool {
    trimmed_metadata(&execution.metadata, "managedProviderId").is_some()
        && trimmed_metadata(&execution.metadata, "managedProviderOperationId").is_some()
}

/// Go `awaitsManagedProviderFinalization`.
#[must_use]
pub fn awaits_managed_provider_finalization(execution: &Execution) -> bool {
    execution
        .metadata
        .get(MANAGED_PROVIDER_PENDING_FINALIZATION_KEY)
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

impl Manager {
    /// Go `runExecution`: marks the execution running, runs the subprocess
    /// (or docker) with a timeout watchdog, applies secret redaction, records
    /// the terminal result, and persists/publishes — deferring the terminal
    /// event when managed-provider finalization is pending.
    pub(crate) fn run_execution(
        &self,
        cancel: CancellationToken,
        mut execution: Execution,
        launch: Option<LaunchSpec>,
    ) {
        let Some(launch) = launch else { return };
        {
            let now = Utc::now();
            execution.status = ExecutionStatus::Running;
            execution.started_at = Some(now);
            execution.updated_at = now;
            execution.result.status = ExecutionStatus::Running;
            execution.result.started_at = Some(now);
            execution.result.backend_metadata =
                merge_backend_metadata(&started_backend_metadata(&execution, None), &execution);
            synchronize_execution_consumer_state(&mut execution);
        }
        self.store_execution(&execution);
        if self
            .persist_consumer_contract(execution.consumer.as_ref())
            .is_ok()
            && self.persist_execution(&execution).is_ok()
        {
            let _ = self.publish_execution_started(&execution);
        }

        let timed_out = Arc::new(AtomicBool::new(false));
        if launch.timeout > Duration::ZERO {
            let watchdog_cancel = cancel.clone();
            let watchdog_flag = Arc::clone(&timed_out);
            let timeout = launch.timeout;
            std::thread::spawn(move || {
                std::thread::sleep(timeout);
                watchdog_flag.store(true, Ordering::SeqCst);
                watchdog_cancel.cancel();
            });
        }

        let result = match launch.backend_kind {
            BackendKind::Docker => crate::redaction::redact_subprocess_result(
                execute_docker(&launch, &cancel, &timed_out),
                &launch.secret_values,
            ),
            _ => crate::redaction::redact_subprocess_result(
                execute_subprocess(&launch, &cancel, &timed_out),
                &launch.secret_values,
            ),
        };

        let completed_at = Utc::now();
        execution.status = result.status;
        execution.updated_at = completed_at;
        execution.completed_at = Some(completed_at);
        execution.result = SandboxResult {
            execution_id: execution.execution_id.clone(),
            status: result.status,
            started_at: execution.started_at,
            completed_at: Some(completed_at),
            exit_code: result.exit_code,
            signal: result.signal,
            stdout: result.stdout,
            stderr: result.stderr,
            output_truncated: result.output_truncated,
            error_class: result.error_class.as_str().to_string(),
            error_code: result.error_code,
            error: result.error,
            partial: false,
            backend_metadata: merge_backend_metadata(&result.backend_metadata, &execution),
            consumer: execution.consumer.clone(),
            ..SandboxResult::default()
        };
        execution.started_at = execution.result.started_at;
        synchronize_execution_consumer_state(&mut execution);

        let delay_terminal = {
            let mut inner = self.inner.write();
            inner.cancels.remove(&execution.execution_id);
            let delay = requires_managed_provider_finalization(&execution)
                && execution.status == ExecutionStatus::Completed;
            if delay {
                execution.metadata.insert(
                    MANAGED_PROVIDER_PENDING_FINALIZATION_KEY.to_string(),
                    "true".to_string(),
                );
                inner.pending_final.insert(execution.execution_id.clone());
            }
            inner
                .executions
                .insert(execution.execution_id.clone(), execution.clone());
            delay
        };

        if self
            .persist_consumer_contract(execution.consumer.as_ref())
            .is_ok()
            && self.persist_execution(&execution).is_ok()
        {
            if delay_terminal {
                return;
            }
            let _ = self.publish_execution_terminal(&execution);
        }
    }

    /// Go `runAttachedExecution`: waits on the attached child (std::thread +
    /// std::sync::mpsc), captures stderr into the shared buffer, and finalizes
    /// the execution. Stdin/stdout stay live on the AttachedExecution handle.
    pub(crate) fn run_attached_execution(
        &self,
        cancel: CancellationToken,
        mut execution: Execution,
        stderr_capture: Arc<Mutex<CaptureBuffer>>,
        pid: u32,
        timeout: Duration,
    ) {
        let secret_values = execution
            .consumer
            .as_ref()
            .map(collect_secret_redaction_values_from_process_env)
            .unwrap_or_default();

        if timeout > Duration::ZERO {
            let watchdog_cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(timeout);
                watchdog_cancel.cancel();
            });
        }

        let (tx, rx) = mpsc::channel::<Result<std::process::ExitStatus, String>>();
        let wait_token = cancel.clone();
        std::thread::spawn(move || {
            loop {
                match wait_token.try_wait_child() {
                    Ok(Some(status)) => {
                        let _ = tx.send(Ok(status));
                        return;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let _ = tx.send(Err(err.to_string()));
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        let mut result: Option<SubprocessResult> = None;
        loop {
            match rx.try_recv() {
                Ok(Ok(status)) => {
                    let completed_at = Utc::now();
                    let mut metadata = serde_json::Map::new();
                    metadata.insert("backend".to_string(), serde_json::json!("subprocess"));
                    metadata.insert("pid".to_string(), serde_json::json!(pid));
                    metadata.insert(
                        "completedAt".to_string(),
                        serde_json::json!(completed_at.to_rfc3339_opts(SecondsFormat::Nanos, true)),
                    );
                    metadata.insert(
                        "platform".to_string(),
                        serde_json::json!(std::env::consts::OS),
                    );
                    metadata.insert(
                        "architecture".to_string(),
                        serde_json::json!(std::env::consts::ARCH),
                    );
                    let stderr =
                        redact_secret_text(&capture_string(&stderr_capture), &secret_values);
                    let output_truncated = capture_truncated(&stderr_capture);
                    if status.success() {
                        result = Some(SubprocessResult {
                            status: ExecutionStatus::Completed,
                            exit_code: Some(0),
                            stderr,
                            output_truncated,
                            backend_metadata: metadata,
                            ..SubprocessResult::default()
                        });
                    } else {
                        let exit_code = status.code().unwrap_or(-1) as i64;
                        result = Some(SubprocessResult {
                            status: ExecutionStatus::Failed,
                            exit_code: Some(exit_code),
                            stderr,
                            output_truncated,
                            error_class: ErrorClass::ProcessFailed,
                            error_code: "sandbox_process_failed".to_string(),
                            error: redact_secret_text(
                                &format!("exit status {exit_code}"),
                                &secret_values,
                            ),
                            backend_metadata: metadata,
                            ..SubprocessResult::default()
                        });
                    }
                    break;
                }
                Ok(Err(err)) => {
                    let mut metadata = serde_json::Map::new();
                    metadata.insert("backend".to_string(), serde_json::json!("subprocess"));
                    result = Some(SubprocessResult {
                        status: ExecutionStatus::Failed,
                        stderr: redact_secret_text(
                            &capture_string(&stderr_capture),
                            &secret_values,
                        ),
                        output_truncated: capture_truncated(&stderr_capture),
                        error_class: ErrorClass::IoCaptureFailed,
                        error_code: "sandbox_wait_failed".to_string(),
                        error: redact_secret_text(&err, &secret_values),
                        backend_metadata: metadata,
                        ..SubprocessResult::default()
                    });
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
            if cancel.is_cancelled() {
                let mut metadata = serde_json::Map::new();
                metadata.insert("backend".to_string(), serde_json::json!("subprocess"));
                result = Some(SubprocessResult {
                    status: ExecutionStatus::Cancelled,
                    stderr: redact_secret_text(&capture_string(&stderr_capture), &secret_values),
                    output_truncated: capture_truncated(&stderr_capture),
                    error_class: ErrorClass::Cancelled,
                    error_code: "sandbox_cancelled".to_string(),
                    error: "execution was cancelled".to_string(),
                    backend_metadata: metadata,
                    ..SubprocessResult::default()
                });
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let result = result.unwrap_or_default();

        let completed_at = Utc::now();
        execution.status = result.status;
        execution.updated_at = completed_at;
        execution.completed_at = Some(completed_at);
        execution.result = SandboxResult {
            execution_id: execution.execution_id.clone(),
            status: result.status,
            started_at: execution.started_at,
            completed_at: Some(completed_at),
            exit_code: result.exit_code,
            signal: result.signal,
            stdout: result.stdout,
            stderr: result.stderr,
            output_truncated: result.output_truncated,
            error_class: result.error_class.as_str().to_string(),
            error_code: result.error_code,
            error: result.error,
            partial: false,
            backend_metadata: merge_backend_metadata(&result.backend_metadata, &execution),
            consumer: execution.consumer.clone(),
            ..SandboxResult::default()
        };
        execution.started_at = execution.result.started_at;
        synchronize_execution_consumer_state(&mut execution);

        {
            let mut inner = self.inner.write();
            inner.cancels.remove(&execution.execution_id);
            inner
                .executions
                .insert(execution.execution_id.clone(), execution.clone());
        }
        if self
            .persist_consumer_contract(execution.consumer.as_ref())
            .is_ok()
            && self.persist_execution(&execution).is_ok()
        {
            let _ = self.publish_execution_terminal(&execution);
        }
    }
}
