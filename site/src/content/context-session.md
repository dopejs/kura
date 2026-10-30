# Context & Session

For a personal agent, **context management is session management** — and
in Kura both are plugins on the `chat/pre-dispatch` waterfall, so any
plugin (builtin or external) can modify or replace the policy.

## The default context plugin

Runs first in the waterfall. Deterministic, budgeted, always cited:

1. **Memory bootstrap** — the tenant's Ready **L3 personas**, then **L2
   scenarios** (newest first), injected as system-frame messages under
   `memoryBudgetChars` (default 4000). Every injected line carries its
   citation inline: `Memory[l3 mem_xxx] persona: …` — recalled memory is
   evidence, never bare text. L1 atoms are **never bulk-injected**.
2. **Query-time recall** — the turn's last user message recalls Ready L1
   atoms through a three-ranker fusion: BM25 + recency + character-trigram
   vectors, combined with reciprocal-rank fusion (RRF). Atoms with no
   lexical or vector affinity are never recalled. Budget:
   `retrievalBudgetChars` (default 2000). Injected as
   `Memory[l1 …] (recalled): …`.
   Both budgets can count estimated tokens instead: set `budgetUnit:
   "tokens"` with `memoryBudgetTokens` / `retrievalBudgetTokens` (unset,
   they are the char budgets divided by 4). Every assembly record shows
   chars and tokens used either way.
3. **Symbolic compression** — oversized non-frame messages (default >8000
   chars) externalize to a full-content L0 memory ref; the window keeps a
   200-char preview plus the citation. Token cost drops; the evidence path
   stays (`GET /v1/memory/assets/{id}`).
4. **Binding-aware loadout** — `agent`-visibility assets inject only for
   the active agent profile bound to them.

Every pass emits a `context.assembled` event carrying the
**AssemblyRecord**: each inclusion (asset, layer, chars, source) and each
exclusion with its reason (`over_budget`/`empty_content`/`visibility`).
Nothing enters or misses the context silently.

## The session-strategy plugin

Runs after context. Frame-preserving window shaping:

- System messages (persona, skills, safety, injected memory) are the
  **frame** — never elided.
- The most recent `keepRecent` messages always survive.
- Over budget, the oldest history collapses into one marker — and the
  elided span is **captured to memory first** (thread-linked L0), so the
  marker cites it: `…elided span captured as Memory[l0_ref mem_x]`. The
  model can drill back. Eviction never destroys context.

Two budgets, keyed by source: **personal** long sessions (48k chars) and
**IM threads** (`sourceKind: channel`, 16k) — one context per thread.

## On-demand retrieval API

The same fused ranking, exposed for clients and tools:

```bash
curl -s http://127.0.0.1:19192/v1/retrieval/queries \
  -H 'content-type: application/json' \
  -d '{"query": "which package manager do we use", "limit": 5}'
```

Hits carry rank, source links, and drill-down member ids.
