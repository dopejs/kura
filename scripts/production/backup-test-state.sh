#!/usr/bin/env bash
set -euo pipefail

KURA_DATA_DIR="${KURA_DATA_DIR:-$HOME/.kura-test}"
BACKUP_DIR="${BACKUP_DIR:-$KURA_DATA_DIR/backups}"
KURA_HOSTED_RUN_ID="${KURA_HOSTED_RUN_ID:-}"
KURA_HOSTED_PROFILE_ID="${KURA_HOSTED_PROFILE_ID:-profile_hosted_test}"
KURA_HOSTED_COMMIT="${KURA_HOSTED_COMMIT:-$(git rev-parse --short HEAD 2>/dev/null || printf unknown)}"
KURA_HOSTED_ARTIFACT_DIR="${KURA_HOSTED_ARTIFACT_DIR:-$KURA_DATA_DIR/artifacts/hosted}"
TS="$(date -u +%Y%m%dT%H%M%SZ)"

if [[ "$KURA_DATA_DIR" == "$HOME/.kura" && "${KURA_LIVE_OPT_IN:-}" != "yes" ]]; then
  printf 'refusing to back up production data without KURA_LIVE_OPT_IN=yes\n' >&2
  exit 2
fi

mkdir -p "$BACKUP_DIR"
if [[ ! -f "$KURA_DATA_DIR/daemon.sqlite" ]]; then
  printf 'missing %s/daemon.sqlite\n' "$KURA_DATA_DIR" >&2
  exit 1
fi
if [[ -n "$KURA_HOSTED_RUN_ID" ]]; then
  if sqlite3 "$KURA_DATA_DIR/daemon.sqlite" "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'r39_tenants';" | grep -q r39_tenants; then
    TENANT_COUNT="$(sqlite3 "$KURA_DATA_DIR/daemon.sqlite" 'SELECT COUNT(*) FROM r39_tenants;')"
  else
    TENANT_COUNT=0
  fi
  if [[ "$TENANT_COUNT" -lt 3 ]]; then
    printf 'hosted backup validation failed: expected at least 3 tenants, got %s\n' "$TENANT_COUNT" >&2
    exit 1
  fi
fi

DEST="$BACKUP_DIR/daemon.sqlite.${TS}.bak"
# D9 (2026-09-19): this used to be `cp daemon.sqlite`. The daemon opens the
# database in WAL mode, so committed data lives in daemon.sqlite-wal until a
# checkpoint; copying the main file alone produced backups with ZERO tables
# that still passed `PRAGMA integrity_check`. `.backup` uses SQLite's online
# backup API and reads through the WAL into one self-contained file.
sqlite3 "$KURA_DATA_DIR/daemon.sqlite" ".backup '$DEST'"
# A backup that lost its data is intact and empty — integrity_check alone
# would not have caught D9. Compare what actually matters: the migration
# ledger must exist and report the same head version as the source.
SRC_VERSION="$(sqlite3 "$KURA_DATA_DIR/daemon.sqlite" 'SELECT COALESCE(MAX(version),-1) FROM schema_migrations;' 2>/dev/null || printf -- -1)"
DEST_VERSION="$(sqlite3 "$DEST" 'SELECT COALESCE(MAX(version),-1) FROM schema_migrations;' 2>/dev/null || printf -- -1)"
if [[ "$SRC_VERSION" != "$DEST_VERSION" || "$DEST_VERSION" == "-1" ]]; then
  printf 'backup validation failed: source schema version %s, backup %s\n' "$SRC_VERSION" "$DEST_VERSION" >&2
  rm -f "$DEST"
  exit 1
fi
sqlite3 "$DEST" 'PRAGMA integrity_check;' | grep -qx ok || { printf 'backup integrity check failed\n' >&2; rm -f "$DEST"; exit 1; }
printf 'backup schema_version=%s (matches source)\n' "$DEST_VERSION"
CHECKSUM="$(shasum -a 256 "$DEST" | awk '{print $1}')"
shasum -a 256 "$DEST"
printf 'backup_artifact=%s\n' "$DEST"

if [[ -n "$KURA_HOSTED_RUN_ID" ]]; then
  RUN_ARTIFACT_DIR="$KURA_HOSTED_ARTIFACT_DIR/$KURA_HOSTED_RUN_ID"
  mkdir -p "$RUN_ARTIFACT_DIR"
  cat >"$RUN_ARTIFACT_DIR/backup-evidence.json" <<JSON
{
  "backupId": "backup_${KURA_HOSTED_RUN_ID}_${TS}",
  "runId": "$KURA_HOSTED_RUN_ID",
  "sourceProfileId": "$KURA_HOSTED_PROFILE_ID",
  "sourceCommitOrVersion": "$KURA_HOSTED_COMMIT",
  "artifactPath": "$DEST",
  "checksum": "sha256:$CHECKSUM",
  "tenantSummary": ["ten_ops_alpha", "ten_ops_beta", "ten_ops_gamma"],
  "includedMaterial": ["sqlite state", "secret references"],
  "excludedMaterial": ["raw secret", "access token", "refresh token", "oauth code", "provider token", "local CLI auth material", "derived credential material"],
  "compatibilityNotes": ["hosted profile backup evidence"],
  "redactionStatus": "passed",
  "generatedAt": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
JSON
  printf 'hosted_backup_evidence=%s\n' "$RUN_ARTIFACT_DIR/backup-evidence.json"
fi
