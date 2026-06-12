#!/usr/bin/env bash
# audit.sh — BAD_RUST scan. Read-only harness.
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$CRATE_DIR"

FINDINGS=0
ERRORS=()

# BAD_RUST-1: unsafe blocks
if grep -rn 'unsafe ' src/; then
  ERRORS+=("BAD_RUST-1: unsafe code found in src/")
  FINDINGS=$((FINDINGS+1))
fi

# BAD_RUST-2: unwrap() in library code (non-test)
if grep -n '\.unwrap()' src/lib.rs | grep -v 'cfg(test)'; then
  ERRORS+=("BAD_RUST-2: unwrap() in library code (use expect or error propagation)")
  FINDINGS=$((FINDINGS+1))
fi

# BAD_RUST-3: panic! in library code
if grep -n 'panic!' src/lib.rs | grep -v 'test\|expect'; then
  ERRORS+=("BAD_RUST-3: panic! in library code")
  FINDINGS=$((FINDINGS+1))
fi

# BAD_RUST-4: TODO/FIXME/HACK markers
if grep -rn 'TODO\|FIXME\|HACK' src/; then
  ERRORS+=("BAD_RUST-4: TODO/FIXME/HACK markers in src/")
  FINDINGS=$((FINDINGS+1))
fi

if [ $FINDINGS -eq 0 ]; then
  echo "audit: CLEAN (0 findings)"
  exit 0
else
  echo "audit: $FINDINGS finding(s)"
  for e in "${ERRORS[@]}"; do echo "  $e"; done
  exit 1
fi
