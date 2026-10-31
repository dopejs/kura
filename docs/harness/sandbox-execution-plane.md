# Sandbox Execution Plane

## Purpose

This document defines the detailed design target for `Roadmap 16: Sandbox Execution Plane`.

The sandbox system is not only a local command runner. It is the execution boundary for the Kura harness.

Its purpose is to make execution:

- policy-driven
- inspectable
- auditable
- backend-agnostic
- safe to extend across skills, provider bridges, MCP, and future tool orchestration

## Design Position

The design direction is:

- a **sandbox control plane**
- over a **multi-backend execution substrate**

This is intentionally closer to:

- OpenClaw's explicit sandbox control plane
- HermesAgent's multi-backend execution substrate
- Claude Code and Codex style permission and approval semantics

but without inheriting workspace-specific assumptions that Kura does not currently have.

## Goals

Roadmap 16 should close these goals:

1. make sandbox policy a first-class daemon object
2. support more than one backend model in the architecture, even if only one backend is implemented first
3. give skills, provider bridges, and future MCP servers a common execution boundary
4. distinguish policy rejection from runtime failure
5. leave an auditable trail for all sandboxed executions

## Non-Goals

Roadmap 16 should **not** try to close:

- VM-grade isolation
- browser or desktop isolation
- full container orchestration
- full orchestration planner
- context or memory behavior
- automatic self-improvement

Those should sit on top of the sandbox substrate, not inside it.

## Roadmap 38 Live Validation Quota Gate

Roadmap 38 reserves the `live_validation_attempts` quota category and defines the
preflight gate contract for future live validation. The gate must run before any live
side effect and must return stable quota denials when exhausted or fail closed when hosted
billing state is unavailable. Roadmap 38 does not create the Roadmap 40 live-validation
executor; concrete live-validation entry points added later must use the shared billing
reservation lifecycle before touching external systems.

## Benchmark Synthesis

### What to take from OpenClaw

- sandbox is a visible control-plane object
- effective policy should be explainable
- operators should be able to list and inspect active sandboxes
- backend, scope, and access shape should be explicit

### What to take from HermesAgent

- backend should not be hard-coded
- local subprocess, container, ssh, and remote execution should all fit the model
- execution environments should be pluggable without changing the control plane

### What to take from Claude Code and Codex

- permission semantics should be explicit
- approval gates should be part of the system model, not ad hoc wrappers
- allow, ask, and deny should be representable at policy level
- local execution and remote execution should eventually share one execution contract

## Core Principles

### 1. Control Plane First

Execution should not be configured ad hoc at the point where a tool runs.

The daemon should own:

- profiles
- effective policy
- backend selection
- approval requirements
- audit visibility

### 2. Backend Independence

The control plane should not assume:

- subprocess only
- container only
- local only

The first backend can be local subprocess. The model must still be written so docker or ssh backends fit later without rewriting policy objects.

### 3. Policy Is Separate From Capability

A skill, provider bridge, MCP server, or future tool can declare what it needs.

The sandbox system decides:

- whether that need is allowed
- which backend is acceptable
- whether approval is required

### 4. Audit Is Part Of The Product

The output of sandbox execution is not just `stdout` and `stderr`.

It must also record:

- what policy was applied
- which backend executed
- whether approval was required
- whether execution was allowed or denied
- whether the failure was policy, transport, timeout, cancellation, or subprocess failure

### 5. Test And Prod Isolation Must Stay Intact

Sandbox defaults must respect the current environment split:

- `test` uses `~/.kura-test`
- `prod` uses `~/.kura`

Nothing in the sandbox design should collapse that separation.

## Object Model

The sandbox system should introduce these first-class objects.

### SandboxProfile

Represents reusable policy and backend intent.

Suggested fields:

- `profileId`
- `title`
- `description`
- `backendKind`
- `defaultWorkDir`
- `filesystemPolicy`
- `networkPolicy`
- `envPolicy`
- `approvalPolicy`
- `processPolicy`
- `defaultTimeoutMs`
- `maxTimeoutMs`
- `restartable`
- `source`
- `active`

### SandboxBackend

Represents the backend family, not an individual execution.

Suggested values:

- `subprocess`
- `docker`
- `ssh`
- `remote`

Only `subprocess` needs implementation in Roadmap 16.

### SandboxExecutionRequest

Represents one execution attempt.

Suggested fields:

- `executionId`
- `profileId`
- `backendKind`
- `command`
- `args`
- `cwd`
- `env`
- `stdinPolicy`
- `timeoutMs`
- `requestedBy`
- `resourceKind`
- `resourceId`
- `scope`
- `approvalId`
- `reason`
- `metadata`

### SandboxExecutionResult

Represents the terminal result of one execution attempt.

Suggested fields:

- `executionId`
- `status`
- `startedAt`
- `completedAt`
- `exitCode`
- `signal`
- `stdout`
- `stderr`
- `outputTruncated`
- `errorClass`
- `errorCode`
- `error`
- `partial`
- `backendMetadata`

### SandboxDecision

Represents the policy decision before execution starts.

Suggested fields:

- `decisionId`
- `executionId`
- `resolution`
- `matchedRules`
- `approvalRequired`
- `approvalStatus`
- `effectiveProfileId`
- `effectiveBackendKind`
- `explanation`

## Policy Model

The policy model should be explicit and composable.

### Filesystem Policy

Suggested shape:

- `mode`
  - `none`
  - `scoped`
  - `full`
- `readRoots`
- `writeRoots`
- `tempRoots`
- `allowDataDir`
- `allowUserAgentsDir`
- `allowHomeRead`
- `allowHomeWrite`

Roadmap 16 should not assume whole-home access is acceptable.

### Network Policy

Suggested shape:

- `mode`
  - `deny`
  - `allow_list`
  - `full`
- `allowedHosts`
- `allowedPorts`
- `allowLoopback`

The first backend may not be able to hard-enforce every network rule at OS level. The model should still be explicit, and enforcement strength should be visible.

### Environment Policy

Suggested shape:

- `mode`
  - `clean`
  - `inherit_safe`
  - `inherit_all`
- `allowedVars`
- `injectedVars`
- `secretRefs`
- `redactedVars`

This matters immediately for:

- provider bridges
- MCP credentials
- local CLI integrations

### Approval Policy

Suggested shape:

- `mode`
  - `allow`
  - `ask`
  - `deny`
- `requiredForCommands`
- `requiredForWritesOutsideRoots`
- `requiredForNetwork`
- `requiredForUnknownBackends`

### Process Policy

Suggested shape:

- `timeoutMs`
- `maxTimeoutMs`
- `killGraceMs`
- `captureStdout`
- `captureStderr`
- `maxOutputBytes`
- `allowStreaming`
- `restartOnFailure`

## Control Plane APIs

Roadmap 16 should expose at least these API groups.

### Profiles

- `GET /v1/sandboxes/profiles`
- `GET /v1/sandboxes/profiles/{profileId}`
- `POST /v1/sandboxes/profiles/reload`

### Executions

- `GET /v1/sandboxes/executions`
- `GET /v1/sandboxes/executions/{executionId}`
- `POST /v1/sandboxes/executions`
- `POST /v1/sandboxes/executions/{executionId}/cancel`

### Explain

- `POST /v1/sandboxes/explain`

This route is important. Operators need to know:

- which profile would be selected
- whether approval would be required
- which directories and network policy would apply
- why execution would be denied

### Events

Suggested event categories:

- `sandbox.execution_requested`
- `sandbox.decision_recorded`
- `sandbox.execution_started`
- `sandbox.execution_completed`
- `sandbox.execution_failed`
- `sandbox.execution_cancelled`
- `sandbox.execution_denied`

## Backend Model

### First Backend: Subprocess

Roadmap 16 should implement one backend:

- `subprocess`

This backend should support:

- explicit cwd
- explicit env policy
- timeout and cancellation
- stdout and stderr capture
- basic filesystem boundary checks before launch
- policy-level network declaration

It does **not** need to provide hard OS containerization in the first roadmap.

### Future Backends

The model should be compatible with later backends:

- `docker`
- `ssh`
- `remote`

That means backend-specific fields must stay in backend metadata, not leak into the common execution contract.

## Integration Points

### Skills

Skills should not directly execute arbitrary scripts through ad hoc logic.

Instead, when execution becomes allowed in later roadmaps, skills should declare:

- required profile
- required filesystem access
- required network access

and run through sandbox execution requests.

### Provider Bridges

Managed CLI providers are an immediate consumer:

- `claude_managed`
- `codex_managed`

They should eventually move from ad hoc subprocess handling to explicit sandboxed provider-bridge execution.

### MCP

MCP should be designed assuming:

- each MCP server may run in a sandbox profile
- credentials are provided through env policy
- network access is explicitly declared

### Tool Orchestration

Tool orchestration should not invent its own process execution path.

It should submit work into sandbox execution with:

- profile
- approval context
- scope
- execution metadata

## Audit And Observability

Roadmap 16 should record enough information to answer:

- what tried to run
- which policy applied
- whether execution was denied
- whether approval was required
- which backend ran it
- what it wrote to stdout or stderr
- whether it timed out
- whether it was cancelled

The system should explicitly distinguish:

- policy denial
- approval rejection
- backend launch failure
- runtime process failure
- timeout
- cancellation

## Failure Model

The smallest useful error classes are:

- `policy_denied`
- `approval_required`
- `approval_rejected`
- `invalid_profile`
- `backend_unavailable`
- `launch_failed`
- `process_failed`
- `timeout`
- `cancelled`
- `io_capture_failed`

These should be durable and queryable through APIs, not only logs.

## Security Position

Roadmap 16 should be honest about its security posture.

The first sandbox backend will be:

- policy-rich
- auditable
- process-isolated

but not yet:

- kernel-level fully isolated
- container-grade hardened
- VM-grade hardened

That is acceptable for the roadmap as long as:

- the control plane is explicit
- the limits are documented
- later backends can strengthen isolation without redesigning the system

## Suggested Implementation Order

1. Define schema-backed contracts for profiles, decisions, and executions
2. Add control-plane APIs for profiles, executions, and explain
3. Implement subprocess backend with cwd, env, timeout, and cancellation
4. Add filesystem and network policy evaluation
5. Add durable events and execution history
6. Add operator docs and contract tests

## Roadmap 16 Implementation Notes

The current daemon implementation now closes the first sandbox roadmap with:

- a built-in `subprocess_default` sandbox profile
- `GET /v1/sandboxes/profiles`
- `GET /v1/sandboxes/profiles/{profileId}`
- `POST /v1/sandboxes/profiles/reload`
- `GET /v1/sandboxes/executions`
- `GET /v1/sandboxes/executions/{executionId}`
- `POST /v1/sandboxes/executions`
- `POST /v1/sandboxes/executions/{executionId}/cancel`
- `POST /v1/sandboxes/explain`
- sandbox-backed managed provider bridge execution for:
  - `claude_managed`
  - `codex_managed`

The current subprocess backend enforces:

- explicit cwd selection
- safe env inheritance plus injected daemon vars
- stdout and stderr capture with truncation limits
- timeout and cancellation
- preflight filesystem scope checks
- policy-declared network access evaluation
- durable execution history and sandbox events

The first real sandbox consumers are now the managed CLI provider bridges.

That means:

- Claude managed auth status, logout, and prompt execution now run through sandbox profiles
- Codex managed logout and prompt execution now run through sandbox profiles
- Codex managed auth-state inspection now evaluates provider-owned local state through sandbox-owned requirement and policy checks before any file read proceeds
- provider auth responses and auth events now carry managed-provider provenance, failure-class, and enforcement-strength metadata
- sandbox execution resources and lifecycle events now carry managed-provider provenance and truthful enforcement-strength metadata
- managed provider execution is no longer an ad hoc `exec.CommandContext` path

The current degradation and security boundary is intentionally explicit:

- filesystem enforcement is preflight scope validation, not kernel-level mediation
- network enforcement for subprocess is `declared_only`, not OS-hardened packet isolation
- approval escalation is integrated with the daemon approval plane and remains single-hop for
  the current tool-call execution surface
- in-flight executions are recovered as cancelled on daemon restart instead of being left in an indeterminate running state

## Roadmap 16 Completion Standard

Roadmap 16 is only complete when:

- sandbox is a first-class daemon plane
- the first backend executes through explicit control-plane contracts
- policy can explain allow, ask, or deny outcomes
- executions are auditable and inspectable
- the design is demonstrably ready to host skills, provider bridges, MCP, and future tool orchestration

## Post-Roadmap 16 Remaining Work

Roadmap 16 closes the sandbox control plane and first backend.

It does **not** mean sandbox is already the finished execution substrate for the harness.

The remaining work falls into four follow-on slices.

### 1. Execution Requirement Declarations And Consumer Convergence

This was the immediate prerequisite slice before MCP, and its core behavior is now closed for the current in-scope consumers.

It closed:

- a common requirement declaration model shared by managed-provider bridges, the current skill registry and explicit skill-selection surfaces, and the current high-risk local tool-call path
- explicit secret scope and redaction semantics for sandbox env injection and operator-visible projections
- executable-skill env injection resolves from the active daemon data dir
  (`~/.kura-test/skill-secrets.json` or `~/.kura/skill-secrets.json`) instead of shared
  ambient process env
- execution provenance that identifies which current consumer requested sandbox work or approval-gated preflight evaluation
- the remaining non-converged current consumers beyond the initial managed-provider slice

This slice exists so MCP and future tool execution do not grow on top of ad hoc consumer-specific behavior.

### 2. MCP On Top Of Sandbox

With requirement declarations, secret scope, and provenance now explicit for the current consumers, MCP can become a real sandbox consumer without becoming the first place where those contracts matter.

This slice should close:

- MCP server registry and profile binding
- MCP process and transport lifecycle through sandbox execution
- MCP credential injection through env policy and secret refs
- tool exposure policy, approval visibility, and operator inspection

MCP should not introduce a parallel unmanaged execution path.

Current daemon behavior now matches that closure:

- MCP server lifecycle is daemon-managed and sandbox-backed
- MCP transport startup, cancellation, restart, and failure state are operator-visible
- tool exposure is deny-by-default and explicit per tool plus runtime surface
- enabled MCP servers recover from persisted daemon state on restart when current policy and config still allow them

### 3. Skill And Tool Execution Through Sandbox

This slice is now closed for executable skills and the current high-risk local-tool path
(`exec`, `shell`, `browser`).

It closes:

- skill requirement manifests
- tool subprocess execution through sandbox requests instead of ad hoc launch paths
- runtime-visible cancellation, timeout, and failure classification for sandbox-backed tool execution
- provenance linking output back to the skill, tool, and sandbox profile that produced it
- redacted secret-scope projection for executable skills and sandbox-linked tool outputs

This is the last step before a fuller orchestration layer can rely on sandbox as the default execution boundary.

### 4. Stronger Isolation And Additional Backends

This step is now closed for the first stronger backend.

The sandbox execution plane now includes:

- baseline `subprocess_default`
- stronger `docker_default`
- backend capability inspection through `/v1/config` and sandbox profile routes
- explain-time selection truth for selected versus `unsupported` outcomes
- fail-closed handling when a request explicitly requires `docker` but the host or declared
  access rules cannot satisfy it

The current stronger-backend slice proves that the existing control plane can host more than
one real backend without introducing a second execution API.

What is still deferred:

- broader migration of local capability families beyond executable skills
- additional stronger backends such as SSH or remote execution
- VM-grade hardening beyond the current subprocess/container split

### 5. Tenant-Isolated Container Backend (Agent Runtime Program 3.1, 2026-10-30)

`docker_default` takes its mounts and network from the request and runs as
the image's user. That is a stronger local sandbox, not tenant isolation. The
`tenant_isolated` profile (`isolation: "tenant"`) builds each run's container
from the acting tenant alone (`crates/domains/sandbox/src/isolation.rs`):

- **Filesystem:** only `<data_dir>/tenants/<tenant>/workspace`, mounted at
  `/workspace`. The root filesystem is read-only, `/tmp` is a 64 MB
  `noexec` tmpfs, and nothing of the daemon's data dir is mounted.
  `KURA_SANDBOX_TENANT_HOST_ROOT` maps the tenants directory to the docker
  host's path when the daemon itself runs in a container.
- **Identity:** a per-tenant uid (FNV of the tenant id into
  `[100000, 100000+2^30)`), `--cap-drop ALL` and `no-new-privileges`. The
  tenant directory is 0700 and its workspace 0777: the daemon need not be
  root to hand the workspace to the container uid.
- **Resources:** `--cpus 1`, `--memory 512m` with no extra swap, and
  `--pids-limit 256`, overridable through `KURA_SANDBOX_TENANT_{CPUS,MEMORY_MB,PIDS,TMPFS_MB}`.
- **Network:** `none`. A request for network, extra roots, loopback, or a
  working dir outside `/workspace` is denied with an `isolation:*` rule,
  never silently narrowed. Egress through the policy-checking proxy, and
  the DNS that goes with it, belong to 3.5.
- **Tenant:** taken from the request's tenant context. With no tenant, or an
  id that cannot be a path component, the run is denied.
- **Lifecycle:** every docker run is named `kura-exec-<execution id>`, so
  cancel and timeout `docker rm -f` the container. Killing the `docker run`
  client never stopped it. This fixes `docker_default` too.

Runs go through the `ExecutionBackend` seam (`backend_for`). Each backend
reports `tenant_isolated()`, which is the property Stage 3.2 will require in
shared mode. Explain stays a dry run; the workspace is created when the run
starts.

Proof: `crates/domains/sandbox/tests/isolation.rs`. The denial tests run
everywhere. The container tests run in CI's Linux job
(`KURA_DOCKER_TESTS=1`): tenant B cannot find tenant A's file or read the
daemon's data dir, is not root, cannot write the root filesystem, has no
network, and runs under the cgroup v2 limits. A cancelled run leaves no
container.

Not yet: MCP stdio servers still run as host subprocesses, because attached
executions refuse docker (3.3). No consumer selects `tenant_isolated` on its
own yet, and shared mode does not yet require it (3.2).

### 6. Shared-Hosting Floor (Agent Runtime Program 3.2, 2026-10-31)

On a shared daemon (`KURA_HOSTING=shared`), the sandbox manager is told
the operator tenant at boot (`require_tenant_isolation`). From then on,
`prepare` sends work to a non-isolating backend (`subprocess_default`,
`docker_default`, the managed-provider profiles) only when the work is
provably the operator's:

- **The operator acting through a request:** the request context names the
  operator tenant with the owner role, as `Hosting::is_platform_operator`
  requires.
- **In-process work the operator owns:** the caller sets
  `ExecutionRequest.owner_tenant_id` to the operator tenant, as MCP does
  for a server the operator owns.

Everything else gets `hosting:isolation_required` with no process started:
tenants, non-owner members of the operator tenant, and work nobody owns.
Unknown ownership is treated as tenant work; that is the fail-closed
direction. `owner_tenant_id` is `#[serde(skip)]`, so an API body cannot
claim it. When a request context is also present the two must agree, or
the request is denied with `tenant:mismatch`.

The floor asks the backend seam (`backend_for_profile(..).tenant_isolated()`)
rather than matching profile ids, so a future isolated backend (gVisor,
3.4) passes without changes here.

### 7. MCP Stdio In The Isolated Plane (Agent Runtime Program 3.3, 2026-10-31)

Attached executions used to refuse docker entirely. An attached run on the
**isolated** backend is now allowed: the `docker run -i` client holds the
session's stdin and stdout, the container is the same tenant-built
container as any isolated run, and the container is force-removed when the
session ends or is cancelled. A request-shaped (`docker_default`) attached
run is still refused.

For a server whose profile is `tenant_isolated`, MCP sends no host working
dir and derives no mount roots from it. Roots or network the server
declares are still sent, so the sandbox refuses them with its reason
instead of dropping them.

Container environment values travel as `-e KEY`, with the value in the
`docker` client's own environment, never on its argv, which any host user
can read through `ps`. Names that steer the client (`PATH`, `HOME`,
`XDG_RUNTIME_DIR`, `DOCKER_*`) go inline, to the container only. This
applies to every isolated run.

### 8. Tenant Egress Proxy (Agent Runtime Program 3.5, 2026-10-31)

`kura egress-proxy` (`crates/foundation/egress-proxy`) is the network exit
for tenant containers. It speaks HTTP/1.1 forward proxying: `CONNECT
host:port` tunnels TLS byte-for-byte, and absolute-form `GET
http://host/...` is rewritten to origin form, with `Proxy-Authorization`
and `Proxy-Connection` stripped. Each request:

1. needs a port on the allow list (`--allow-port`, default 80 and 443);
2. is resolved once by `EgressPolicy::resolve_pinned`, and every address
   must pass (no loopback, RFC 1918, link-local/metadata, unique-local,
   or `--block-host` names);
3. is dialled at that resolved address. The name is never looked up again,
   which closes DNS rebinding for this path.

A refusal is a `403` with `X-Kura-Egress-Denial: <code>` and a logged
reason. The policy is the strict default, deliberately not the daemon's
`egress` config: the operator may admit private networks for their own
integrations, and tenants must not inherit that. The proxy cannot see
inside TLS, so policy is by destination only.

What makes it the only exit is topology, which is not wired yet: tenant
containers on a docker `--internal` network (kernel-enforced, no route
out), with the proxy as a sidecar attached to that network and to the
outside one.

## Explicit Prerequisites Before MCP

The next roadmap should not be "MCP immediately" without these prerequisites being explicit:

- requirement declarations for filesystem, network, secrets, and execution mode
- secret scope and redaction policy that the daemon can enforce and explain
- execution provenance identifying consumer kind and consumer instance
- managed local consumers converged on sandbox enough that MCP does not become the first place where hidden access paths matter

Without those pieces, MCP would likely reintroduce unmanaged credential and process behavior under a new name.

Those prerequisites are now in place for the current managed-provider, skill-inspection,
MCP, executable-skill, and high-risk local-tool surfaces. What remains for later roadmaps
is broader local-capability migration plus stronger backends.

## Hosted Credential Isolation

Sandbox and MCP secret references resolve through hosted tenant secrets when a tenant
context is active. Resolution is scoped to the active tenant, and missing or disabled
tenant secrets fail closed before the sandboxed process receives environment material.

Operator projections expose only redacted secret scope outcomes:

- consumer kind and id
- `secretRef`
- environment scope
- delivery kind
- resolution (`resolved`, `unavailable`, `denied`, or `not_applicable`)
- redaction rule

`POST /v1/sandboxes/explain` includes secret scope outcomes only for callers with
`credentials.inspect` or the relevant credential-management permission in the active
tenant. Viewers and tenantless callers do not receive secret refs from sandbox explain
payloads.

MCP server resources similarly expose tenant ownership, lifecycle state, unavailable
reasons, websocket auth summaries, and redacted secret summaries. A server or tool that
cannot resolve its active tenant secret remains configured but unavailable/disabled
until the tenant credential is rotated or reconnected. Another tenant's matching
secret ref or MCP server id cannot satisfy the request.

## Recommended Post-16 Order

1. close execution requirement declarations and consumer convergence
2. add MCP on top of the sandbox plane
3. move executable skill and local tool subprocess execution onto sandbox
4. add a stronger backend and backend capability negotiation

Steps 1 through 3 are now complete for the current in-scope consumers. The remaining order
is stronger backends next, then any broader migration of lower-risk local capability paths.
