#!/usr/bin/env bash
set -euo pipefail
if [[ "$(uname -s)" != Linux ]]; then
  echo "This worker wrapper requires Linux and a systemd user manager." >&2
  exit 2
fi
limit="$(systemctl --user show wallpapermac-work.slice --property=MemoryMax --value)"
if [[ "$limit" != 8000000000 ]]; then
  echo "Run python3 scripts/setup-bounded-linux.py to apply the shared 8GB worker limit." >&2
  exit 2
fi
export CARGO_BUILD_JOBS=1
export RUST_TEST_THREADS=1
exec systemd-run --user --scope --quiet --collect \
  --slice=wallpapermac-work.slice --unit="wallpapermac-task-$$" \
  --property=MemoryAccounting=yes -- "$@"
