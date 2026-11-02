# Tenant Browser: A Throwaway Browser per Tenant

> Status: implemented 2026-11-02 (agent-runtime-program.md 3.3, browser
> half). Decision: a throwaway browser per tenant session (user, 2026-11-02);
> persistent logins are out of scope. Owner: daemon.

## 1. What a tenant gets

A tenant's agent can browse the web through chat tools. The browser runs in
the tenant's own isolated container (§5 of the sandbox plane). It reaches
the internet only through the egress proxy (3.5), keeps nothing between
sessions, and is removed when the session ends or sits idle.

| Tool | Does | Parallel-safe |
|---|---|---|
| `browser.open` `{url}` | starts the tenant's browser if needed, loads `url` | no |
| `browser.read` | the current page: url, title, visible text | no (shares the page) |
| `browser.click` `{selector}` | clicks, then returns the page | no |
| `browser.type` `{selector, text}` | fills a field, then returns the page | no |
| `browser.back` | history back, then returns the page | no |
| `browser.close` | ends the session and removes the container | no |

What the model gets back:
- The page as text (url, title, `innerText`), bounded like every tool
  result and framed as untrusted data (2.8).
- No screenshots: the chat loop is text-only.

Only `http`/`https` URLs can be opened. `file:`, `data:`, `javascript:` and
`chrome:` are refused before anything starts.

## 2. Why the agent drives it, not the API

The computer-use API is built around runs, and a shared daemon keeps run
creation operator-only. A tenant's agent is the part that needs a browser,
so the browser is offered as chat tools. The operator's computer-use plane
(runs, approvals, recorded artifacts, the host browser worker) is
unchanged.

## 3. How a session runs

```
browser.open ──> TenantBrowsers (one per daemon)
                   │ tenant has a live session? use it
                   │ else: caps check → sandbox.start_attached_execution(
                   │         profile tenant_isolated, owner = tenant,
                   │         network full (egress proxy), image + limits below)
                   ▼
            docker run -i [--runtime runsc] … kura-browser-worker
                   │  line-JSON worker protocol (capabilities/browser/PROTOCOL.md)
                   ▼
            Playwright + Chromium, HOME=/workspace, no saved profile
```

- **One session per tenant.** A tenant's turns, in any thread, share it,
  and calls are serialized. A second tenant gets its own container.
- **Daemon-wide cap** `KURA_BROWSER_MAX_SESSIONS` (default 4). At the cap,
  `browser.open` fails with "no browser is free right now". The model sees
  the error and the turn goes on.
- **Idle timeout** `KURA_BROWSER_IDLE_SECONDS` (default 600). A reaper
  closes idle sessions. Closing cancels the execution, and the sandbox
  removes the container.
- **Requests time out** after 30 s, the worker's own bound. A dead or
  silent worker ends the session; the next `browser.open` starts a fresh
  one.
- **Shutdown** closes every session.

Per-session container limits (in-process only; never from a request):

| | Value | Measured need (sg-2) |
|---|---|---|
| image | `KURA_BROWSER_WORKER_IMAGE` (pin a digest) | — |
| memory | 1024 MiB | 160 MiB on runc, 207 MiB on runsc, for a simple page |
| pids | 512 | 50–76 |
| cpus | 1 | — |
| tmpfs `/tmp` | 256 MiB | — |

All other isolation is the tenant container's: uid, no capabilities,
read-only root, the tenant's own workspace, and the egress proxy as the
only route out. With `KURA_SANDBOX_DOCKER_RUNTIME=runsc` the browser runs
under gVisor too: a session starts in ~2.3 s instead of ~1.2 s, and actions
take 10–150 ms instead of 5–90 ms.

## 4. When it is offered

The tools are registered only when all of these hold:
- `KURA_BROWSER_WORKER_IMAGE` is set;
- tenant egress is configured (3.5), since without a network a browser is
  useless and the sandbox would refuse it anyway;
- the sandbox plane is present.

Otherwise the boot log says which condition is missing, and turns get no
browser tools. Single and shared mode behave the same way. On a single
daemon the operator's own turns use the same path.

## 5. Risks

- **Prompt injection drives the browser.** A page can tell the agent to
  click or type. The session has no logins and no saved cookies, and
  reaches only what the egress proxy allows (public addresses). So the
  worst case is an anonymous action on a public site under the platform's
  egress IP. Click and type run serially and are recorded as tool calls.
- **Memory.** Each session takes 160–210 MiB idle and more on heavy pages.
  The cap and the 1 GiB per-container limit bound it, and an
  out-of-memory kill ends only that session.
- **Image size.** The official Playwright base is 3.7 GB. The worker image
  installs Chromium only.

## 6. Observability

- `kura_browser_sessions_total{outcome}`. `outcome` is one of `started`,
  `closed`, `idle_closed`, `failed` or `refused_capacity`.
- `[kura] browser:` log lines on start, close and failure, with the tenant
  and execution id.
- Each session is a sandbox execution (`requested_by = "browser"`), visible
  wherever executions are.

## 7. Verification (2026-11-02)

- **Unit tests:**
  - the pool against the protocol fake (reuse, cap, idle reaping, a dead
    worker replaced, a failed launch, URL refusal, shutdown);
  - the chat tools over a fake worker;
  - the boot settings;
  - `AttachedWorker` over handed-in pipes.
- **CI:** builds the worker image and drives a start, a click and a
  snapshot under the isolated flags.
- **End to end on dopejs-sg-2** (`scripts/tenant-browser-e2e/`), on runc
  and on runsc, every check passing:
  - a provisioned tenant's chat makes the scripted model call
    `browser.open https://example.com/`, and the reply carries the page
    text;
  - one container, labelled with the tenant, runs under the configured
    runtime, on the internal tenant network only, read-only with no
    capabilities;
  - memory: 140 MiB on runc, 190 MiB on runsc;
  - turn time: 1.6 s on runc, 2.6 s on runsc;
  - a private address goes to the proxy and is refused there;
  - the metadata address is unreachable;
  - with the proxy stopped, nothing loads (no other way out);
  - the idle container is removed;
  - no call used the operator's key.

**Found by the end to end:** under runsc a tenant container cannot resolve
the proxy's container name, because docker's embedded DNS does not answer
inside gVisor. Every networked run failed, the browser included. The proxy
must then be addressed by IP: give it a fixed address on the tenant network
(`docker network create --internal --subnet …` and
`docker network connect --ip …`). The daemon refuses to start with runsc
and a proxy named by host, and says so.

With a tenant egress key (sandbox plane §8, 3.6), the worker's proxy URL
carries the tenant's credential. Chromium ignores credentials in a proxy
URL, so the worker hands them to Playwright, which answers the proxy's
`407` challenge. Verified on sg-2 under runc and runsc.

Chromium skips the proxy for link-local addresses (169.254.0.0/16). On the
internal network those have no route, so the metadata service stays
unreachable. The proxy refuses everything else that is not public.

## 8. Rollback

Unset `KURA_BROWSER_WORKER_IMAGE`: the tools disappear and nothing else
changes. No store migration is involved.
