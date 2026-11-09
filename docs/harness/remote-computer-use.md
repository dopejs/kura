# Remote Computer Use: The Agent Drives the User's Own Computer

> Status: design, 2026-11-02. Decisions (user, 2026-11-02): macOS first; the
> local side is a Rust command-line daemon in this workspace; the model sees
> generic function tools, not a provider's native computer-use tool. Owner:
> daemon + `kura-device`.

## 1. What a tenant gets

The agent keeps running in the tenant's cell. A small program on the user's
Mac, `kura-device`, dials out to the cell and, while the user allows it,
takes screenshots and performs mouse and keyboard input on the agent's
behalf. The model sees the screen and acts through these tools:

| Tool | Does | Returns |
|---|---|---|
| `computer.screenshot` | captures the main display | screenshot |
| `computer.click` `{x, y, button?, count?}` | moves and clicks (`left`/`right`/`middle`, 1–3 clicks) | screenshot |
| `computer.move` `{x, y}` | moves the pointer | screenshot |
| `computer.drag` `{from:{x,y}, to:{x,y}}` | press, move, release | screenshot |
| `computer.type` `{text}` | types text (≤ 2000 chars) | screenshot |
| `computer.key` `{keys}` | a chord such as `cmd+shift+t`, `enter` | screenshot |
| `computer.scroll` `{x, y, dx, dy}` | scrolls at a point, in notches | screenshot |
| `computer.wait` `{ms}` | waits ≤ 5 s, for animations and loads | screenshot |

Coordinates are in the screenshot's pixel space (§6). Every action returns a
fresh screenshot, so the model never acts on a stale screen and a step costs
one round trip, not two.

Out of scope for the first version: Windows and Linux, multiple displays
(main display only), audio, clipboard read, file transfer, and the
provider-native computer-use tools (decision above; the action set is close
enough to Anthropic's that a native mapping can be added later without
changing the device protocol).

## 2. What exists today (verified)

- `domains/computeruse` drives only the server-side browser
  (`tenant_browsers.rs`); its session/action/artifact types (`lib.rs`) and
  `artifacts::Service::save_computer_use_artifact` are reusable for audit.
- Tool results are text only: `ResponseItem::FunctionCallOutput { call_id,
  output: String }` (`foundation/protocol/src/protocol.rs:42`),
  `kura_llm::Message.content: String` (`engine/llm/src/types.rs:33`),
  `ToolOutput.content: String` (`engine/core/src/tools.rs:16`). A screenshot
  cannot reach the model. The text crosses: session history
  (`engine/core/src/session.rs:507`) → `chat/src/round.rs:190` → the
  dispatcher's `Message` → `model-provider/src/llm_bridge.rs:74` → the three
  wires (`anthropic.rs:394`, `openai.rs:423`, `responses.rs:305`).
- The daemon has no WebSocket server (axum 0.8 without the `ws` feature).
- The gateway (`kura-gateway/internal/gateway/server.go`) forwards with
  `httputil.ReverseProxy`, which passes `Upgrade: websocket` through; its
  `statusWriter` implements `Unwrap`, so the hijack reaches the real
  connection (not yet exercised). The gateway authenticates the person
  (Cloudflare Access JWT, or a shared-mode session), drops the client's
  `Authorization` and sends the cell token instead (`server.go:269`). A
  headless device has neither an Access JWT nor a browser session.

## 3. Topology

```
 user's Mac                         gateway                      tenant cell
┌──────────────┐  wss (outbound)  ┌──────────┐  ws + cell token ┌────────────────────┐
│ kura-device  │ ───────────────▶ │ /v1/devices/connect ───────▶ │ devices::Hub       │
│ capture/input│ ◀─────────────── │ route by token prefix │ ◀─── │ computer.* tools   │
│ consent, stop│                  └──────────┘                   │ chat turn (agent)  │
└──────────────┘                                                 └────────────────────┘
```

- **The device always dials out.** Nothing listens on the Mac, so it works
  behind NAT and firewalls and the Mac exposes no port.
- **One connection per device**, a WebSocket carrying JSON frames (§5).
  WebSocket rather than long-poll: actions are frequent, round-trip latency
  matters, and screenshots flow device → cell. Cloudflare tunnels carry
  WebSockets.
- **The cell is the source of truth for devices.** Without a gateway
  (self-hosted daemon) the device connects to the daemon directly; the
  protocol is identical.

## 4. Pairing, credentials, revocation

Implemented 2026-11-03 (slice 2): `domains/devices` (`registry`, `hub`,
`connection`), `foundation/device-protocol`, store migration v9
(`devices`, `device_pairings`), `surface/api/src/routes/devices.rs`.

1. The signed-in user asks for a pairing code (`POST /v1/devices/pairings`,
   a tenant route). The daemon stores the hash of a single-use code, valid
   10 minutes, bound to the tenant and principal, and returns
   `kpair.<tenant>.<secret>` (128-bit secret).
2. On the Mac: `kura-device pair <code> --server https://…`. The device
   exchanges the code (`POST /v1/devices/pair`, an open route; the code is
   the credential) for a device token `kdev.<tenant>.<secret>` (256-bit)
   and stores it in the macOS Keychain. The code is spent and the device
   created in one transaction; an unknown, spent or expired code all answer
   the same 401.
3. Each connection (`GET /v1/devices/connect`, WebSocket) sends the token in
   `X-Kura-Device-Token` and is refused with 401 before the upgrade if it is
   unknown, revoked, or names another tenant. The daemon stores only
   SHA-256 hashes, with the device's name, platform, created / last seen
   (once per connect and disconnect) and the principal who paired it.
   `GET /v1/devices` lists with online state; `POST
   /v1/devices/{id}/revoke` revokes and drops a live connection at once
   (the device is told `session_end{reason: revoked}`). Another tenant's
   device is a 404.

`<tenant>` is the routing part: the gateway already maps tenants to cells.
It is not secret, and the secret is free of `.`, so the gateway splits from
the right. The gateway's only new job: for exactly `/v1/devices/pair` and
`/v1/devices/connect`, skip person auth, look up the placement by
`<tenant>`, rate-limit per tenant and per source IP, and forward with the
cell token **plus** the device's own credential (the code in the body, the
token in `X-Kura-Device-Token`). The cell validates the credential; a
forged tenant reaches a cell that rejects it. The gateway never validates or
stores device secrets, so there is one source of truth. The daemon itself
does not rate-limit these two routes: guessing a 128-bit code within its ten
minutes is not a practical attack, and each refusal costs one indexed
lookup.

Tradeoff: a stolen device token lets someone impersonate the device (feed
the agent fake screenshots, receive the agent's actions); it does not give
control of the real Mac or access to the tenant's API. Revocation is
immediate. Binding tokens to a device-held key (proof of possession) is a
later hardening, listed in §11.

## 5. Wire protocol

JSON text frames, one object each, `{"type": …, "id": …}`; versioned by a
`protocol` field in `hello`. Screenshots are binary-safe base64 inside JSON
for v1 (simplicity over ~33 % overhead; §11).

| Direction | Frame | Meaning |
|---|---|---|
| device → cell | `hello {protocol, device, platform, screen:{w,h,scale}}` | after connect |
| cell → device | `session.request {session, reason, ttl_s}` | the agent wants control |
| device → cell | `session.granted {session, until}` / `session.denied` | the user's local answer |
| cell → device | `action {id, session, op, args}` | one tool call |
| device → cell | `result {id, ok, error?, screenshot?:{media_type, w, h, data}}` | after the action settles |
| either | `session.end {session, reason}` | user stop, TTL, turn end, revocation |
| either | `ping`/`pong` every 20 s | dead-peer detection (60 s) |

Rules:
- One live session per device, one action in flight per session. The cell
  queues nothing: a second concurrent call fails fast with `busy`.
- An action carries a deadline (10 s, `wait` up to 15 s). A timeout or a
  dropped connection mid-action returns a tool error that says **the outcome
  is unknown**, so the model takes a screenshot before retrying instead of
  clicking twice.
- The device refuses any `action` outside a granted session.

## 6. Screenshots, coordinates, tokens

- The device captures the main display at native resolution and scales it
  so the long edge is ≤ 1280 px (keeps aspect ratio; Retina 2880×1800 →
  1280×800), JPEG q≈80. The model works in that space; the device maps back
  (`x_native = x * native_w / shot_w`) and clamps to the display.
- **Only the last 3 screenshots stay in the prompt.** Older ones are replaced
  by the text `[screenshot omitted; step N]` when the prompt is assembled.
  Without this a 50-step task re-sends ~50 images every round. The cut moves
  each step, which costs some prompt-cache reuse inside a CUA turn; that is
  the accepted tradeoff.
- Every screenshot is also written to
  `<data_dir>/artifacts/remote-computer/<tenant>/<session>/<seq>-<call>.jpg`
  and named on the image (`ImagePart.artifact`), so the audit trail outlives
  the prompt window: the checkpoint keeps the name, the file keeps the
  pixels. (As built in slice 4, this is a file and not a
  `computer_use_artifacts` row: that table's foreign keys tie it to the
  browser's runs, sessions and actions, which a chat turn does not have.)
  Nothing prunes these files yet; retention is a follow-up.

## 7. Prerequisite: images in tool results

Additive and backward compatible:

- `ResponseItem::FunctionCallOutput` gains `images: Vec<ImagePart>`
  (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`), and
  `kura_llm::Message` the same, so stored checkpoints and dispatch records
  read unchanged and text-only tools serialize byte-for-byte as before (the
  prompt-cache prefix test, `surface/app/tests/prompt_cache_prefix.rs`,
  guards this).
- `ImagePart { media_type, data: base64, artifact: Option<String> }`.
  `data` is `#[serde(skip)]`: every persisted copy (checkpoint, dispatch
  record) keeps only `artifact`. A resumed turn shows the model
  `[image omitted]` instead of rehydrating, on purpose: after a restart the
  screen has changed, and the model should take a new screenshot.
- `ToolOutput` gains `images` the same way; existing tools are untouched.
- Wires: Anthropic puts `[{type:text}, {type:image, source:{type:base64}}]`
  in `tool_result.content`; Responses uses an `output` array with
  `input_image`; chat completions cannot put images in a `tool` message, so
  the text stays there and a `user` message with `image_url` parts follows
  the run of `tool` messages (nothing else may sit between calls and their
  results).
- Implemented 2026-11-02 (slice 1). Not yet done: offering image-returning
  tools only to vision models. The model catalog has no vision flag to gate
  on, so slice 4 relies on the operator: `computer.remote` is enabled per
  tenant, and a tenant whose model cannot read images gets the provider's
  400 on the first screenshot. Adapter-RPC and managed CLI providers still
  see text only.

## 8. Safety

The Mac is the trust boundary, so the device enforces the rules that protect
the user; the cell cannot override them.

On the device:
- **Local consent per session.** `session.request` shows a macOS
  alert with the agent's reason (§9); without an explicit allow within
  60 s it is denied. `--trust-for 30m` can pre-approve sessions for a while;
  the default is ask every time.
- **Visible state.** A menu-bar item shows idle / in control; while in
  control the pointer has a coloured halo. (Deferred until a signed app
  bundle exists, §9.)
- **Kill switch.** A global hotkey (default `ctrl+opt+cmd+.`) and
  `kura-device stop` end the session immediately and refuse new ones for 10 minutes.
  An action in flight stops at its next step (one typed chunk, one click,
  one step of a drag, 50 ms of a wait) and a held mouse button is released.
- **User takes over.** Physical mouse or keyboard input during a session
  ends it, including in the middle of an action, which stops at its next
  step; the action and the next one get `user_active`, and the agent must
  ask again.
- **Never-touch apps.** A local deny list of bundle ids (defaults: password
  managers, Keychain Access, System Settings › Privacy & Security, Terminal);
  a click or keystroke while one of them is frontmost is refused with
  `denied_app`. Screenshots of them are blacked out before upload.
- **Secure input.** When macOS secure input is on (a password field has
  focus), `type` is refused.

In the cell:
- Off by default: `entries.computer-use.config.remote` must be enabled
  (`{"enabled": true, "tenants": [...]}`; a shared daemon must name its
  tenants or the boot fails), and the tools are offered to a turn only
  while one of the tenant's devices is online.
- Risky chords (`cmd+q`, `cmd+w`, `cmd+delete`, `cmd+shift+delete`,
  `cmd+opt+esc`) and `type` containing a newline after a URL-like string go
  through the existing approval flow (`domains/policy`).
- Screen content is data, never instructions: the tool descriptions and the
  system prompt say so, and the approval rule above blunts an injected
  "press cmd+q".
- Every action is recorded by `chat.tool.called` (thread, call id,
  arguments, output) and its screenshot by the file above. Typed text is in
  that event's arguments, as for `browser.type`; it is never in the logs.
- A session belongs to the turn that started it: when the turn ends its
  session is ended (`session.end {reason: "turn ended"}`).

## 9. macOS implementation

Implemented 2026-11-03 (slice 3): `crates/surface/device` (package and
binary `kura-device`). The session rules (`agent.rs`) and the connection
(`link.rs`) are platform-free and run against a fake computer in tests;
macOS lives behind the `Platform` and `Consent` traits
(`platform/macos.rs`), so Windows/Linux are additive.

- Capture: `/usr/sbin/screencapture -x -C -D1` to a private scratch file,
  read and deleted at once. Chosen over binding ScreenCaptureKit: Apple
  keeps the tool on whatever capture API the OS uses (`CGDisplayCreateImage`
  is obsolete), it honours Screen Recording the same way, and it keeps async
  Objective-C out of the device. Cost: one process per screenshot, measured
  204 ms capture + 30 ms scale/encode on a 3840×2160 display (release). The
  device checks Screen Recording before each capture, because without it
  the tool silently returns only the wallpaper.
- Scaling and JPEG are pure Rust (`image`), so the pipeline is tested on
  every platform. The screenshot is always scaled to `fit(display pixels)`,
  the same size the coordinates are mapped from, so the two cannot drift.
- Input: Quartz `CGEvent` from a private event source (the person's held
  modifiers do not leak in), posted at the HID tap. `type` sends Unicode in
  chunks of 16 UTF-16 units, so any layout and any script type correctly;
  `\n` and `\t` are the Return and Tab keys. `key` chords use ANSI key
  codes (as other CUA tools do; a non-US layout presses the same physical
  keys). Requires Accessibility.
- Never-touch apps: the window server's window list (owner pid → bundle id
  through `NSRunningApplication`). The frontmost app is the owner of the
  topmost layer-0 window; `NSWorkspace.frontmostApplication` goes stale
  without a main run loop. A click, scroll or either end of a drag that
  lands on such a window is refused, as is any input while one is in front;
  their windows are black in every screenshot. The default list covers
  password managers, Keychain Access, System Settings and common terminals;
  `--deny-app` adds more.
- Secure input: `IsSecureEventInputEnabled`; `type` is refused, `key`
  is not (Return in a password field is how people submit it).
- Take-over and kill switch: one listen-only event tap on its own run-loop
  thread. Every event the device posts carries a tag in
  `kCGEventSourceUserData`; any untagged mouse or key event is the person.
  Input more than 3 s after the grant ends the session (`user_active`); the
  grace covers the hand leaving the Allow button. `ctrl+opt+cmd+.` (or
  `kura-device stop`, which sends SIGUSR1) ends the session
  (`user_stopped`) and refuses requests for 10 minutes. If the tap cannot be
  created, `run` refuses to start: no kill switch, no control.
- Consent: a CoreFoundation alert (`CFUserNotificationCreate`), which
  needs no app bundle, with "Deny" as the default button and a 60 s timeout.
  It states the agent's reason (clipped to 300 characters), the duration,
  and how to take control back. It is waited on 250 ms at a time from the
  thread that created it (Core Foundation does not promise the object is
  safe to share), so a prompt nobody can answer any more is taken off the
  screen: the connection dropped, the kill switch, or a newer request
  (`session_denied` with `cancelled`, or `kill_switch`). Checked on a real
  Mac by an ignored test (`the_consent_alert_waits_in_short_polls_…`).
- `doctor` checks Screen Recording (`CGPreflightScreenCaptureAccess`) and
  Accessibility (`AXIsProcessTrusted`), names the pane, and with `--open`
  opens it; `run` refuses to start without both. For a CLI the grant
  belongs to the app that launched it (the terminal) unless it runs under
  launchd.
- Files: the token is in the Keychain (service `dev.kura.device`, account
  the server URL), never on disk; `config.json` (server, device id, name,
  0600) and the pid file are in `~/Library/Application Support/kura-device`
  (`KURA_DEVICE_HOME` overrides it); the log is
  `~/Library/Logs/kura-device.log`, without typed text or secrets.
- Deferred from §8: the menu-bar item and the pointer halo need an app
  bundle and a main-thread AppKit loop; until a signed app exists, the
  consent alert and the log are the visible state, and `kura-device status`
  shows whether it is running.

## 10. Observability

- Metrics (the registry has counters and histograms, no gauges):
  `kura_device_connections_total{event=opened|closed|replaced|revoked}`
  (opened minus the rest is the number connected),
  `kura_device_sessions_total{outcome=granted|denied|timeout}`,
  `kura_device_actions_total{op,outcome}`,
  `kura_device_action_duration_seconds{op}` (round trip),
  `kura_device_screenshot_bytes_total` (divide by ok actions for the mean
  size), `kura_computer_approvals_total{outcome=approved|rejected|timeout|error}`.
- Logs: connect/disconnect with device id, protocol, latency; each action
  with session, op, outcome, ms (never the typed text; its length only).
- Events: `chat.tool.called` per action and `policy.approval_requested`
  per risky one. Not built: `device.connected`, `device.disconnected`,
  `computer.session.*`, for clients to show "agent is using your Mac".
- Device side: `~/Library/Logs/kura-device.log`, and `kura-device status`.

## 11. Failure modes and limits

| Case | Behaviour |
|---|---|
| Device offline | tools absent from the prompt; a call already planned fails `device_offline` |
| Drop mid-action | outcome unknown (§5); the device ends the session on reconnect |
| User denies / kill switch | tool error `session_denied` / `session_ended_by_user`; the agent must not retry in this turn |
| Cell restart | connections drop; devices reconnect with backoff (1 s → 60 s, jitter); no session survives |
| Display change mid-session | `hello` is re-sent; the next screenshot carries the new size |
| Slow uplink | screenshot quality steps down to keep a result under 300 KB |

Known limits: main display only; base64 overhead; device tokens are bearer
tokens (no proof of possession yet); click accuracy of generic tools is
lower than a provider-native computer-use tool and is not measured yet
(slice 5).

## 12. Slices

Each slice is shippable and tested on its own.

1. **Images in tool results** (§7): protocol, `kura_llm`, the three wires,
   persistence by artifact reference, last-3 window. Tests per wire with
   fixtures; the prompt-cache prefix test stays green.
2. **Device registry and hub in the daemon** (done 2026-11-03): pairing
   endpoints, hashed tokens, revocation, the WebSocket endpoint and
   protocol (§5), schemas in `schemas/api`, SDK. Tested with fake devices
   in-process and one real WebSocket client end to end.
3. **`kura-device` for macOS** (done 2026-11-03, §9): pair, Keychain,
   connect/reconnect, capture, input, consent, kill switch, take-over,
   deny list, `doctor`/`status`/`stop`/`unpair`. Tested with a fake
   computer, against the cell's own `Hub` and connection loop, and by hand
   on macOS against a test daemon: pair, spent code refused, connect,
   kill switch, reconnect after a cell restart (1 s → 2 s → 4 s), exit on a
   revoked token. The menu-bar item is deferred (§9).
4. **`computer.*` tools and gateway pass-through**: tool host wiring,
   capability gate, approvals for risky chords, audit artifacts, metrics;
   gateway `/v1/devices/*` route-by-token with rate limits. End to end on
   sg-2 through the gateway with a real Mac.

   As built (daemon side, 2026-11-09):
   - Tools (`surface/app/src/computer_tools.rs`): `computer.start {reason,
     device?}` asks for a session and returns the first screenshot;
     `computer.act {op, …}` takes one protocol action as is, so the cell and
     the device read the same shape; `computer.stop` hands control back.
     Errors start with a stable code (`no_session`, `device_offline`,
     `session_denied`, `session_ended`, `outcome_unknown_*`, `user_active`,
     `denied_app`, `secure_input`, `approval_rejected`, `approval_timeout`)
     and say what to do next.
   - Gate: `entries.computer-use.config.remote` → `AppState::computer_remote`
     (`kura_devices::RemoteGate`). Pairing and connecting stay available
     with the gate off; nothing can ask a connected device for a session.
   - Approvals (`kura_devices::rules`): `cmd+q`/`cmd+w` (any extra
     modifiers), `cmd+delete`/`cmd+forwarddelete`, `cmd+opt+esc`, a `type`
     with a newline after an address, and Return right after an address was
     typed. The chord parser moved into `kura-device-protocol::keys`, so the
     cell recognises a chord in every dialect the device accepts. The
     approval is raised like `POST /v1/policy/approvals`, bound to the
     tenant, and waited on for 180 s.
   - Tests: the gate in every hosting mode; tools offered only to the
     allowed tenant with its device online; a turn against a fake device
     through the real `Hub` (screenshot saved, invalid actions refused before
     the wire, take-over ends control, the session ends with the turn);
     approve, reject and time out a risky chord (the device never sees a
     rejected one).
   - Not built: vision gating (§7), session events (§10), screenshot
     retention, a per-turn action budget.
   - End to end on macOS, directly against an embedded daemon (2026-11-09,
     no gateway): a scripted model drove `computer.start`, `screenshot`,
     `move`, `wait`, `key cmd+q` and `computer.stop` through the real chat
     path, WebSocket and `kura-device`. The person allowed the session in
     the alert; `cmd+q` waited for approval, was rejected and never reached
     the device; an unanswered alert denied the session after 60 s. The run
     found that every pre-dispatch hook (the builtin `context` and
     `session-strategy` hooks always run) dropped the screenshot bytes in
     its JSON round trip, so the model read `[image omitted]`; fixed in
     `kura-chat` by restoring the bytes per tool call after the hooks. With
     the fix the model received each screenshot, at most three at a time,
     byte for byte the saved copy. Not exercised yet: `type`, `click`,
     take-over and the kill switch on real hardware.
   - Gateway path (2026-11-09, staging on sg-2; kura-gateway
     `docs/design.md` §17.1): a shared daemon behind `kura-gateway -mode
     shared` with a fake IdP, loopback only. A real Mac's `kura-device`
     paired and connected through the gateway; another tenant could not
     see or revoke it; forged and unknown-tenant tokens were refused; it
     reconnected about 1 s after a gateway restart and stayed up idle for
     145 s; disabling the account or revoking the device dropped it and it
     exited. No `computer.*` session went through the gateway (that needs
     a model and a person to allow it). The sg-1 pilot is still in cells
     mode, where device routes answer 404.
5. **Evaluation**: a fixed set of desktop tasks (open an app, fill a form,
   find a file) with success rate and steps, to decide whether generic
   tools are good enough or a native mapping is needed.

Rollback: every slice is behind `computer.remote` (off by default) except
slice 1, which is additive on the wire and reverts as one commit.
