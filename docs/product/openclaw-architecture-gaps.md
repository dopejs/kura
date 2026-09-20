# OpenClaw Architecture Gaps

## Purpose

This document captures where OpenClaw appears to have architectural improvement space and what design direction Kura may take in response.

This is not a dismissal of OpenClaw. It is a gap analysis against our target of a long-lived personal agent OS with stronger runtime, context, and memory boundaries.

**Scope note (2026-09-18):** this document compares *architecture* and is not
maintained against OpenClaw releases. Feature-level tracking of what OpenClaw
ships and what we adopt lives in
[`../harness/agent-deepening-program.md`](../harness/agent-deepening-program.md),
which covers releases 2026.8.1 through 2026.9.4.

## Framing

OpenClaw already has a strong product surface:

- long-running gateway daemon
- Web UI and multi-channel access
- session routing
- tools and nodes
- plugins and skills
- context and memory extension slots

The main improvement opportunity is not feature count. It is architectural separation and runtime clarity.

## 1. Gateway Responsibility Is Too Broad

### Observed Pattern

The Gateway acts as the single control plane for sessions, routing, channels, WebSocket control, config, cron, web UI, and Canvas hosting.

### Risk

- one large failure domain
- harder reasoning about ownership and lifecycle
- more difficult upgrades and internal evolution
- UI, channel, and runtime concerns can become tightly coupled

### Our Design Direction

Separate logical responsibilities even if V1 still ships as one process:

- daemon shell
- runtime core
- channel adapters
- tool host
- operator API
- UI surfaces

We can keep a single deployable daemon while enforcing cleaner internal boundaries.

## 2. Runtime State Is Too Coupled To Workspace Prompt Files

### Observed Pattern

OpenClaw uses workspace files such as `AGENTS.md`, `SOUL.md`, `TOOLS.md`, `MEMORY.md`, and bootstrap files as major runtime inputs.

### Risk

- state becomes hard to reason about and audit
- persona, policy, memory, and bootstrap concerns blur together
- migration and replay are harder
- runtime truth is partly implicit and file-shaped

### Our Design Direction

Treat editable files as overlays, not primary runtime truth.

Keep:

- human-editable instructions
- persona shaping
- bootstrap hints
- operator notes

Move into structured state:

- runtime state
- policy state
- memory state
- handoff state
- checkpoint state

## 3. Context Engine Extensibility Happens Too Late In The Pipeline

### Observed Pattern

OpenClaw's context engine controls ingest, assemble, compact, and after-turn hooks.

### Risk

- context extension remains prompt-pipeline-centric
- runtime state is still upstream and loosely structured
- context assembly risks becoming a substitute for a real execution model

### Our Design Direction

Build context assembly on top of explicit runtime objects:

- run
- task
- step
- event
- artifact
- policy decision
- checkpoint

In this model, context is a compiled view of runtime state rather than the place where runtime state is effectively invented.

## 4. Memory Is Still Too Workspace-File-Centric

### Observed Pattern

OpenClaw's memory system has improved beyond plain files, but canonical memory still leans on markdown files in the workspace plus derived indexes and promotion flows.

### Risk

- weak separation between durable memory and editable notes
- harder provenance and rollback
- limited support for layered scopes such as user, project, task, and domain
- stronger retrieval features still inherit the constraints of the file model

### Our Design Direction

Design memory as an independent plane with:

- explicit layers
- scope boundaries
- provenance
- promotion policy
- audit trail

Markdown can remain a useful view or export format, but not the sole canonical store.

## 5. Session Model Is Stronger Than The Run Model

### Observed Pattern

OpenClaw is highly session-oriented. This works well for chat-native products across DMs, groups, webhooks, cron, and nodes.

### Risk

- execution can remain conversation-shaped instead of task-shaped
- long-running work is harder to manage
- checkpoint, cancellation, and replay become less central than they should be
- the system risks staying a chat gateway rather than becoming an agent OS runtime

### Our Design Direction

Keep sessions as ingress and continuity objects, but make runtime execution center on:

- run
- task
- step
- artifact
- checkpoint
- resume and cancel flows

Session should not be the only execution primitive.

## 6. Plugin Power Comes With Soft Trust Boundaries

### Observed Pattern

OpenClaw plugins can run in-process and register RPC methods, HTTP routes, tools, background services, skills, and context engines.

### Risk

- broad trust surface
- weak containment for faulty or malicious plugins
- security and stability depend heavily on plugin quality
- difficult permission modeling for high-risk capabilities

### Our Design Direction

Introduce stronger plugin tiers:

- metadata-only or declarative extensions
- trusted in-process plugins
- isolated out-of-process capability providers

High-risk capabilities should prefer explicit capability-host boundaries over unrestricted in-process execution.

## 7. Control Plane Protocol Has Too Many Concerns In One Surface

### Observed Pattern

The Gateway WebSocket protocol covers operator control, node transport, chat control, config, and admin workflows.

### Risk

- protocol surface grows large and harder to evolve
- clients need to understand too much of the system
- versioning pressure increases across unrelated responsibilities

### Our Design Direction

Keep a unified transport if useful, but separate logical APIs:

- operator API
- runtime API
- node capability API
- event stream API

The transport can stay unified without collapsing everything into a single undifferentiated method space.

## 8. Product Surfaces Are Rich, But Capability Domains Are Not Cleanly Layered

### Observed Pattern

OpenClaw covers chat, nodes, Canvas, voice, browser, cron, plugins, and coding-agent style workflows.

### Risk

- domain-specific features can distort core abstractions
- core runtime becomes shaped by whichever feature was added last
- product growth can outpace architectural clarity

### Our Design Direction

Separate:

- core runtime
- control plane and surfaces
- capability domains

Coding, architecture, voice, browser, calendar, and messaging should all be domain packs or capability layers built on top of the same core primitives.

## Summary Table

| Area | OpenClaw Pattern | Main Risk | Kura Direction |
| --- | --- | --- | --- |
| Gateway | many responsibilities in one daemon | large failure and ownership surface | split shell, runtime, adapters, tools, UI logically |
| Workspace files | prompt files act like runtime state | implicit truth and weak auditability | keep overlays, move truth into structured state |
| Context | plugin hook around prompt assembly | runtime remains under-structured | compile context from explicit runtime objects |
| Memory | markdown-first with derived indexes | weak provenance and scope layering | independent memory plane |
| Execution | session-first | chat-shaped runtime | run/task/step-first runtime |
| Plugins | broad in-process powers | soft trust boundary | tiered extension model |
| Protocol | one broad WS method surface | evolution and versioning pressure | separate logical APIs |
| Product layering | rich features, loose domain boundaries | abstraction drift | core primitives plus domain packs |

## Implication For Kura

The target should not be "build OpenClaw again."

The target should be:

- match the capability surface where it matters
- avoid inheriting unnecessary architectural coupling
- rebuild runtime, context, memory, and capability boundaries with cleaner primitives

## Immediate Next Use

Use this document as input to:

- product scope
- module map
- runtime object model
- daemon and control-plane design
