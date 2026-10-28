#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# macOS TMPDIR ends in "/"; join without doubling it, since code under test
# compares canonical paths against ones built from TMPDIR.
temp_base="${TMPDIR:-/tmp}"
temp_parent="${temp_base%/}/kura-test-runs"

mkdir -p "$temp_parent"

# SIGKILL and machine crashes bypass EXIT traps. Reclaim only old, isolated
# runs so an active test process is never removed underneath Cargo.
find "$temp_parent" \
  -mindepth 1 \
  -maxdepth 1 \
  -type d \
  -mmin +1440 \
  -exec rm -rf -- {} +

test_tmp="$(mktemp -d "$temp_parent/run.XXXXXX")"
cleanup() {
  rm -rf -- "$test_tmp"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

export TMPDIR="$test_tmp"
cd "$repo_root/crates"
exec_status=0
cargo test "$@" || exec_status=$?
exit "$exec_status"
