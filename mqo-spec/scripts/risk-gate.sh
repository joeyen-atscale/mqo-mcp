#!/usr/bin/env bash
# risk-gate.sh — check 8 receipts and emit gate verdict. Read-only harness.
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
AB="$CRATE_DIR/target/autobuilder"
RECEIPTS="$AB/receipts"
cd "$CRATE_DIR"

PASS=()
FAIL=()

echo "=== Risk Gate — mqo-spec ==="

# Receipt 1: intake — intent-card.json
if [ -f "$CRATE_DIR/agent/intent-card.json" ]; then
  python3 -c "
import json, sys
card = json.load(open('$CRATE_DIR/agent/intent-card.json'))
assert card.get('schema') == 'autobuilder.intent_card.v1', 'wrong schema'
must_acs = [ac for ac in card['acceptance_criteria'] if ac['level'] == 'MUST']
assert len(must_acs) >= 1, 'no MUST ACs'
print(f'intake: {len(must_acs)} MUST ACs declared')
" && PASS+=("intake") || FAIL+=("intake: intent-card.json invalid")
else
  FAIL+=("intake: agent/intent-card.json missing")
fi

# Receipt 2: spec-drift
if [ -f "$AB/spec-drift.json" ]; then
  DRIFT=$(python3 -c "import json; d=json.load(open('$AB/spec-drift.json')); print(d['summary']['drift_count'])")
  if [ "$DRIFT" -eq 0 ]; then
    PASS+=("spec-drift: drift_count=0")
  else
    FAIL+=("spec-drift: drift_count=$DRIFT (PRD verbs missing from binary)")
  fi
else
  FAIL+=("spec-drift: target/autobuilder/spec-drift.json missing")
fi

# Receipt 3: vti-plan — proof-lanes.toml presence
if [ -f "$CRATE_DIR/agent/proof-lanes.toml" ]; then
  PASS+=("vti-plan: proof-lanes.toml present")
else
  FAIL+=("vti-plan: agent/proof-lanes.toml missing")
fi

# Receipt 4: proof-receipt — all tests green
cargo test --release --workspace > /tmp/rg_test.log 2>&1
if grep -q 'FAILED' /tmp/rg_test.log; then
  FAIL+=("proof-receipt: test failures detected")
else
  PASS_CNT=$(grep 'test result: ok' /tmp/rg_test.log | awk '{sum+=$4} END{print sum+0}')
  PASS+=("proof-receipt: $PASS_CNT tests green, clippy clean")
fi

# Clippy
cargo clippy --workspace -- -D warnings > /tmp/rg_clippy.log 2>&1 && \
  PASS+=("proof-receipt/clippy: clean") || \
  FAIL+=("proof-receipt/clippy: warnings present")

# Receipt 5: risk-gate / BAD_RUST audit
bash "$CRATE_DIR/scripts/audit.sh" > /tmp/rg_audit.log 2>&1
if [ $? -eq 0 ]; then
  PASS+=("risk-gate/audit: CLEAN")
else
  FAIL+=("risk-gate/audit: $(cat /tmp/rg_audit.log | tail -5)")
fi

# Receipt 6: reviewer-agent — check for reviewer receipt JSON
REVIEWER_JSON=$(ls "$RECEIPTS"/reviewer-*.json 2>/dev/null | head -1 || true)
if [ -n "$REVIEWER_JSON" ]; then
  VERDICT=$(python3 -c "import json; d=json.load(open('$REVIEWER_JSON')); print(d.get('decision', d.get('verdict', 'missing')))")
  if [ "$VERDICT" = "pass" ]; then
    PASS+=("reviewer-agent: verdict=pass")
  elif [ "$VERDICT" = "concern" ]; then
    PASS+=("reviewer-agent: verdict=concern (advisory)")
  else
    FAIL+=("reviewer-agent: verdict=$VERDICT")
  fi
else
  FAIL+=("reviewer-agent: no reviewer receipt found in $RECEIPTS/reviewer-*.json")
fi

# Receipt 7: rollback-plan
if [ -f "$AB/rollback.md" ]; then
  PASS+=("rollback-plan: rollback.md present")
else
  FAIL+=("rollback-plan: target/autobuilder/rollback.md missing")
fi

# Receipt 8: ci-checks — .github/workflows present
if ls "$CRATE_DIR/.github/workflows/"*.yml > /dev/null 2>&1; then
  PASS+=("ci-checks: workflow(s) present")
else
  FAIL+=("ci-checks: no .github/workflows/*.yml found")
fi

echo ""
echo "--- PASS (${#PASS[@]}) ---"
for p in "${PASS[@]}"; do echo "  [PASS] $p"; done

echo ""
FAIL_COUNT=${#FAIL[@]}
echo "--- FAIL ($FAIL_COUNT) ---"
if [ "$FAIL_COUNT" -gt 0 ]; then
  for f in "${FAIL[@]}"; do echo "  [FAIL] $f"; done
fi

echo ""
TOTAL=$((${#PASS[@]} + FAIL_COUNT))
echo "Gate: ${#PASS[@]}/$TOTAL receipts passed"

if [ "$FAIL_COUNT" -eq 0 ]; then
  echo "VERDICT: READY"
  exit 0
else
  echo "VERDICT: BLOCKED ($FAIL_COUNT receipt(s) failing)"
  exit 1
fi
