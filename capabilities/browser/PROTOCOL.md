# Browser worker protocol

The daemon's computer-use plane drives a browser through a supervised child
process instead of linking one. `crates/domains/computeruse/src/subprocess_driver.rs`
(`SubprocessDriver`) is the daemon side; `worker.mjs` here is the reference
worker (Playwright + Chromium). Any process that speaks this protocol can
replace it.

## Transport

Line-delimited JSON over the worker's stdin/stdout. One request per line,
one response per line, strictly in order; `stderr` is passed through to the
daemon's log. The daemon holds at most one exchange in flight.

## Requests

| `op` | fields | response |
|------|--------|----------|
| `start_session` | `session`, `input` (`CreateSessionInput`) | `session` |
| `execute_action` | `session`, `action` | `session`, `action`, `captures` |
| `close_session` | `session` | `session` |
| `ping` | — | `{ok:true}` |

`session` and `action` are the daemon's own wire shapes (`Session`,
`Action` in `kura-computeruse`, camelCase). The worker returns the updated
objects; the daemon persists them and records captures through the artifact
recorder.

```json
{"id":1,"op":"start_session","session":{"computerUseSessionId":"cus_1", "...":"..."},"input":{"initialUrl":"https://example.org"}}
{"id":1,"ok":true,"session":{"computerUseSessionId":"cus_1","status":"active","currentPage":{"url":"https://example.org/","title":"Example Domain"}, "...":"..."}}
```

## Captures

```json
{"kind":"screenshot","mimeType":"image/png","fileName":"screenshot.png","contentBase64":"iVBORw0…"}
{"kind":"page_snapshot","mimeType":"application/json","fileName":"page-snapshot.json","contentBase64":"eyJ…"}
```

`kind` is one of `screenshot`, `page_snapshot`, `download`.

## Errors

`{"id":n,"ok":false,"error":"…"}`. An action that fails *as an action*
(navigation failure, target mismatch, unsupported action) is **not** a
protocol error: it is returned `ok:true` with `action.status = "failed"` and
`action.failureClass` set, exactly as the in-memory driver reports it.

## Supervision (daemon side)

- Per-request timeout (`timeoutMs`, default 30000). A hung worker is killed.
- A worker that exits, hangs, or answers malformed/out-of-order JSON is
  reported to the capability supervisor (`report_failure`) and respawned on
  the next call; a healthy answer reports health. The supervisor's
  backoff/circuit-break is visible at `GET /v1/capabilities`.
- The worker holds no credentials. Its only configuration is its
  environment (`KURA_PLAYWRIGHT_PATH`, `KURA_BROWSER_HEADLESS`).

## Enabling

`<data_dir>/plugins.json`:

```json
{ "entries": { "computer-use": { "config": {
  "driver": "subprocess",
  "command": "node",
  "args": ["/path/to/kura/capabilities/browser/worker.mjs"],
  "env": { "KURA_PLAYWRIGHT_PATH": "/path/to/node_modules/playwright" },
  "timeoutMs": 30000
} } } }
```

Without this entry the computer-use plane keeps the in-memory driver.
