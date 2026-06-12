#!/usr/bin/env bash
# run-metrics.sh — run the test + quality harness; emit target/autobuilder/metrics.json.
# READ-ONLY: agent edits src/ only, not this script.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

OUT_DIR="target/autobuilder"
mkdir -p "$OUT_DIR"
METRICS="$OUT_DIR/metrics.json"

# ── cargo test ────────────────────────────────────────────────────────────────
TEST_OUT=$(cargo test --release 2>&1 || true)
echo "$TEST_OUT"
# Sum all "N passed" lines across multiple test binaries
AC_PASSING=$(echo "$TEST_OUT" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' | \
  python3 -c "import sys; print(sum(int(x) for x in sys.stdin))" 2>/dev/null || echo 0)
FAILED=$(echo "$TEST_OUT" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' | \
  python3 -c "import sys; print(sum(int(x) for x in sys.stdin))" 2>/dev/null || echo 0)
AC_PASSING="${AC_PASSING:-0}"
FAILED="${FAILED:-0}"
TEST_STATUS=$( [ "$FAILED" -eq 0 ] && echo "ok" || echo "fail" )

# ── clippy ────────────────────────────────────────────────────────────────────
CLIPPY_OUT=$(cargo clippy --workspace -- -D warnings 2>&1 || true)
CLIPPY_WARNINGS=$(echo "$CLIPPY_OUT" | grep -cE '^error\[|^warning\[' | tr -d '\n' | head -1 || echo 0)
CLIPPY_WARNINGS="${CLIPPY_WARNINGS:-0}"

# ── doc coverage (simple: count pub items with ///) ───────────────────────────
TOTAL_PUB=$(grep -rE '^\s*pub ' src/ | wc -l | tr -d ' \n' || echo 1)
DOC_PUB=$(grep -rE '^\s*///' src/ | wc -l | tr -d ' \n' || echo 0)
TOTAL_PUB="${TOTAL_PUB:-1}"
DOC_PUB="${DOC_PUB:-0}"
DOC_COVERAGE_PCT=$(python3 -c "print(round(min(${DOC_PUB}/max(${TOTAL_PUB},1),1.0)*100,1))" 2>/dev/null || echo 0)

# ── proptest density (count #[proptest] or proptest! macros) ──────────────────
PROPTEST_COUNT=$(grep -rE '#\[proptest\]|proptest!' src/ tests/ 2>/dev/null | wc -l | tr -d ' \n' || echo 0)
PROPTEST_COUNT="${PROPTEST_COUNT:-0}"

# ── audit findings ────────────────────────────────────────────────────────────
UNWRAP_COUNT=$(grep -rE '\.unwrap\(\)|\.expect\(' src/ 2>/dev/null | grep -v '//' | wc -l | tr -d ' \n' || echo 0)
UNWRAP_COUNT="${UNWRAP_COUNT:-0}"

# ── quality score ─────────────────────────────────────────────────────────────
# score = 10*ac_passing + 3*test_coverage_pct + 2*proptest_density + 1*doc_coverage_pct
#         - 2*audit_findings - 1*clippy_warning_count
QUALITY_SCORE=$(python3 -c "
ac = int('$AC_PASSING')
cov = 0   # no lcov in this project
prop = int('$PROPTEST_COUNT')
doc = float('$DOC_COVERAGE_PCT')
audit = int('$UNWRAP_COUNT')
clippy = int('$CLIPPY_WARNINGS')
score = 10*ac + 3*cov + 2*prop + 1*doc - 2*audit - 1*clippy
print(round(score,2))
" 2>/dev/null || echo 0)

python3 -c "
import json, sys
data = {
  'schema': 'autobuilder.metrics.v1',
  'ac_passing_count': int('$AC_PASSING'),
  'tests_failed': int('$FAILED'),
  'test_status': '$TEST_STATUS',
  'clippy_warning_count': int('$CLIPPY_WARNINGS'),
  'doc_coverage_pct': float('$DOC_COVERAGE_PCT'),
  'proptest_density': int('$PROPTEST_COUNT'),
  'audit_findings_count': int('$UNWRAP_COUNT'),
  'quality_score': float('$QUALITY_SCORE'),
  'mutation_kill_rate': None
}
print(json.dumps(data, indent=2))
" > "$METRICS"

cat "$METRICS"
