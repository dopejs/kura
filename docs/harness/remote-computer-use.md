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

1. The signed-in user asks for a pairing code (`POST /v1/devices/pairings`,
   web or TUI). The daemon stores a single-use code, valid 10 minutes, bound
   to the tenant and principal, and returns `kpair_<route>_<secret>`.
2. On the Mac: `kura-device pair <code> --server https://…`. The device
   exchanges the code (`POST /v1/devices/pair`, unauthenticated,
   rate-limited) for a device token `kdev_<route>_<secret>` and stores it in
   the macOS Keychain.
3. Each connection authenticates with that token. The daemon stores only a
   hash, with the device's name, platform, first/last seen and the principal
   who paired it. `GET /v1/devices` lists, `DELETE /v1/devices/{id}` revokes
   and closes any live connection at once.

`<route>` is an opaque routing id (the gateway's placement id for the user,
or empty when there is no gateway). The gateway's only new job: for exactly
`/v1/devices/pair` and `/v1/devices/connect`, skip person auth, look up the
placement by `<route>`, rate-limit per route and per source IP, and forward
with the cell token **plus** the original device credential in
`X-Kura-Device-Token`. The cell validates the credential; a forged route
reaches a cell that rejects it. The gateway never validates or stores device
secrets, so there is one source of truth.

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
- Every screenshot is also saved as a computer-use artifact (existing
  `save_computer_use_artifact`), tenant-scoped, so the audit trail outlives
  the prompt window. Retention follows the tenant's artifact retention.

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
  tools only to vision models (slice 4 gates `computer.*` on the model
  catalog; a non-vision model would get a 400 today); adapter-RPC and
  managed CLI providers still see text only.

## 8. Safety

The Mac is the trust boundary, so the device enforces the rules that protect
the user; the cell cannot override them.

On the device:
- **Local consent per session.** `session.request` shows a macOS
  notification with the agent's reason; without an explicit allow within
  60 s it is denied. `--trust-for 30m` can pre-approve sessions for a while;
  the default is ask every time.
- **Visible state.** A menu-bar item shows idle / in control; while in
  control the pointer has a coloured halo.
- **Kill switch.** A global hotkey (default `ctrl+opt+cmd+.`) and the menu
  item end the session immediately and refuse new ones for 10 minutes.
- **User takes over.** Physical mouse or keyboard input during a session
  pauses it; the agent's next action gets `user_active` and must ask.
- **Never-touch apps.** A local deny list of bundle ids (defaults: password
  managers, Keychain Access, System Settings › Privacy & Security, Terminal);
  a click or keystroke while one of them is frontmost is refused with
  `denied_app`. Screenshots of them are blacked out before upload.
- **Secure input.** When macOS secure input is on (a password field has
  focus), `type` is refused.

In the cell:
- Off by default: the tenant capability `computer.remote` must be enabled,
  and the tools are offered to the model only while a device is online.
- Risky chords (`cmd+q`, `cmd+w`, `cmd+delete`, `cmd+shift+delete`,
  `cmd+opt+esc`) and `type` containing a newline after a URL-like string go
  through the existing approval flow (`domains/policy`).
- Screen content is data, never instructions: the tool descriptions and the
  system prompt say so, and the approval rule above blunts an injected
  "press cmd+q".
- Every action and its screenshot are recorded as computer-use actions and
  artifacts with the session, turn and tool call ids.

## 9. macOS implementation

- Capture: ScreenCaptureKit (`SCScreenshotManager`) via a small Objective-C
  bridge or a maintained crate; `CGWindowListCreateImage` is deprecated.
  Requires the Screen Recording permission.
- Input: Quartz `CGEvent` (mouse, keyboard, scroll). Requires the
  Accessibility permission.
- Frontmost app: `NSWorkspace.frontmostApplication`; secure input:
  `IsSecureEventInputEnabled`.
- Both permissions are checked at start (`kura-device doctor`) with the
  exact System Settings pane to open. The binary must be signed for the
  permissions to stick across upgrades; unsigned dev builds are re-prompted.
- Crate choice (capture/input bindings) is made in slice 3 after checking
  maintenance and macOS 15+ support; the device keeps capture and input
  behind a `Platform` trait so Windows/Linux are additive.

## 10. Observability

- Metrics: `kura_devices_connected` (gauge), `kura_device_sessions_total
  {outcome=granted|denied|timeout}`, `kura_device_actions_total{op,outcome}`,
  `kura_device_action_seconds{op}` (histogram, round trip),
  `kura_device_screenshot_bytes` (histogram).
- Logs: connect/disconnect with device id, protocol, latency; each action
  with session, op, outcome, ms (never the typed text; its length only).
- Events: `device.connected`, `device.disconnected`, `computer.session.*`,
  so clients can show "agent is using your Mac".
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
2. **Device registry and hub in the daemon**: pairing endpoints, hashed
   tokens, revocation, the WebSocket endpoint and protocol (§5), schemas in
   `schemas/api`, SDK, `make daemon-contract-test`. Tested with a fake
   device in-process.
3. **`kura-device` for macOS**: pair, Keychain, connect/reconnect, capture,
   input, consent, menu-bar state, kill switch, deny list, `doctor`.
4. **`computer.*` tools and gateway pass-through**: tool host wiring,
   capability gate, approvals for risky chords, audit artifacts, metrics;
   gateway `/v1/devices/*` route-by-token with rate limits. End to end on
   sg-2 through the gateway with a real Mac.
5. **Evaluation**: a fixed set of desktop tasks (open an app, fill a form,
   find a file) with success rate and steps, to decide whether generic
   tools are good enough or a native mapping is needed.

Rollback: every slice is behind `computer.remote` (off by default) except
slice 1, which is additive on the wire and reverts as one commit.
