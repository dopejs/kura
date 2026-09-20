# Production Upgrade Runbook

**Scope**: tenant-scoped single-node upgrade. Multi-node managed service
rollout, clustering, and distributed failover are outside Roadmap 39.

**Elapsed-time target**: representative upgrade verification must complete in
90 minutes or less, including preflight and postflight evidence.

## Preflight

1. Stop write traffic or the test daemon before copying state.
2. Take a verified backup using `scripts/production/backup-test-state.sh`.
   As of 2026-09-19 the script uses SQLite's online backup (`.backup`) and
   refuses to produce a backup whose migration head differs from the source.
   **Backups taken before that date with the old script may be empty**: the
   daemon runs in WAL mode and the script copied only the main file (D9).
   Re-take any backup you intend to rely on.
3. Run:

   ```bash
   scripts/production/upgrade-preflight.sh
   ```

4. Record tenant integrity, required config, quota/accounting state, schema
   version, backup integrity, hosted deployment identity, data location,
   artifact location, daemon health, configuration readiness, and blocking
   findings.

## Rehearse (before activation)

With the target binary installed but the daemon **not yet started on it**:

```bash
kura daemon rehearse-upgrade          # add --keep to retain the scratch dir
```

This runs the target binary against an isolated snapshot of the data
directory — a `VACUUM INTO` copy of the database plus the plugin profile,
external plugins and local secret backend — and performs everything the real
start would: migrations, plugin assembly, every manager. The real data
directory is opened read-only for its version and its snapshot, and the
report fails if that version moved. Exit 0 means the upgrade *would* succeed;
exit 1 prints findings and keeps the scratch directory for inspection.

The report's `activation` and `rollback` fields say what to do next, because
the answer changes at exactly one moment: before the real daemon starts on
the new binary, rollback is nothing; after it, rollback is restore from
backup.

## Upgrade (activation)

Only after a passing rehearsal: start the daemon on the target build and keep
migration lifecycle logs. This is the moment the real database is migrated
and the point of no return for in-place rollback.

## Postflight

Run:

```bash
scripts/production/upgrade-postflight.sh
```

Record tenant data counts, quota/accounting consistency, credential remediation
state, health checks, operational diagnostics, rollback guidance, and elapsed
time. When `KURA_HOSTED_RUN_ID` is set, the preflight and postflight helpers
write `upgrade-preflight.json` and `upgrade-postflight.json` under the hosted
run artifact directory.

## Rollback Decision

In-place rollback is safe only when persisted state remains compatible with the
previous binary. When migrations changed persisted state in a way that cannot be
reversed safely, restore from backup is the only acceptable recovery path. Old
binaries must not be pointed at incompatible newer schema versions.
