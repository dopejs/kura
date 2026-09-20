# Migration And Versioning Plan

## Purpose

This document defines how daemon persisted state evolves without silent corruption.

P0 uses SQLite as the local durable store. Schema evolution is versioned and explicit.

## Current Strategy

- current supported schema version: `4` (`kura_store::CURRENT_SCHEMA_VERSION`; the Go-era chain that reached `21` was collapsed into the v1 baseline at the Rust port)
- migration ledger table: `schema_migrations`
- version rule: the daemon applies forward migrations in ascending order
- compatibility rule: a database newer than the daemon-supported version is rejected on startup

The migration ledger records:

- `version`
- `name`
- `applied_at`

## Migration Semantics

### New Databases

For a brand-new database:

1. create `schema_migrations`
2. apply migration `1` baseline schema
3. apply later migrations in order

### Legacy Unversioned Databases

Older local databases may already contain the baseline tables but no migration ledger.

The daemon handles that case explicitly:

1. detect known legacy tables
2. bootstrap migration `1` in `schema_migrations`
3. continue applying newer migrations

This avoids forcing a manual destructive reset for early local installations.

### Future Databases

If the local database reports a schema version greater than the daemon understands, startup fails.

This is intentional. Running an older daemon binary against newer persisted state is a rollback risk.

## Rollback Expectations

Migrations are forward-only. There are no automatic down migrations.

Rollback expectation:

1. stop the daemon
2. restore the previous SQLite file from backup or snapshot
3. restart a daemon binary compatible with that restored schema version

**A backup must be taken with SQLite's backup API or `VACUUM INTO`, never
`cp`.** The daemon opens the database in WAL mode, so committed data sits in
`daemon.sqlite-wal` until a checkpoint; a copy of the main file alone is an
intact, empty database that passes `integrity_check` (D9, 2026-09-19).

Before activating a new binary, `kura daemon rehearse-upgrade` runs the
migrations against a snapshot and reports whether the real upgrade would
succeed — see `production-upgrade.md`.

For any migration that changes persisted semantics beyond additive indexes or metadata, release notes must say whether a pre-upgrade backup is mandatory.

## Authoring Rules

Every new persisted schema change must:

1. increment `CURRENT_SCHEMA_VERSION` in `crates/persistence/store/src/lib.rs`
2. add a named `SchemaMigration` entry in `crates/persistence/store/src/migrations.rs`
3. keep the migration idempotent
4. document rollback expectations
5. add at least one store-level migration test

## Current Coverage

The store test suite now covers:

- new database reaches current schema version
- legacy baseline schema upgrades to current version
- future schema version is rejected
- tenant identity tables, token lifecycle fields, token tenant grants, organization
  memberships, and invitations persist across restart

That is the minimum P0 migration confidence bar.

## Migration History (Rust era)

| Version | Name | Added |
|---|---|---|
| 1 | `baseline_v1_first_release` | the collapsed Go-era chain, including the tenant identity tables below |
| 2 | `memory_assets` | Roadmap 78 memory plane |
| 3 | `memory_asset_embeddings` | Stage 1.1 retrieval index; FK `ON DELETE CASCADE` onto `memory_assets` |
| 4 | `tool_profiles` | Stage 9.1b tool providers; partial unique index enforces one default per capability |

Each of v2–v4 has a store test that opens a database at the previous version
and asserts the upgrade lands the new table in place.

## Tenant Identity Tables (in the v1 baseline)

The Roadmap 34 tenant identity foundation, originally Go-era version `21`:

- `tenants`
- `principals`
- `memberships`
- `tenant_invitations`
- `token_tenant_grants`
- token lifecycle columns on `auth_tokens`
- `tenant_audit_events`

Rollback requires restoring a pre-upgrade SQLite backup. The daemon does not down-migrate
tenant identity records or widen token authority during rollback.
