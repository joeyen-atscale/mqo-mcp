#!/usr/bin/env bash
# run-metrics.sh — emit target/autobuilder/metrics.json
# Read-only harness: agent must not modify this file.
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$CRATE_DIR/target/autobuilder/metrics.json"
mkdir -p "$CRATE_DIR/target/autobuilder"

cd "$CRATE_DIR"

# -- tests -------------------------------------------------------------------
cargo test --release --workspace > /tmp/mqo_test.log 2>&1
TEST_EXIT=$?
TESTS_PASS=$(grep -E '^test result: ok' /tmp/mqo_test.log | awk '{sum+=$4} END{print sum+0}')
TESTS_FAIL=$(grep -E '^test result:' /tmp/mqo_test.log | awk '{sum+=$6} END{print sum+0}')

# -- clippy ------------------------------------------------------------------
cargo clippy --workspace -- -D warnings > /tmp/mqo_clippy.log 2>&1
CLIPPY_EXIT=$?
CLIPPY_WARNINGS=$(grep -c '^warning' /tmp/mqo_clippy.log || true)

# -- AC pass counts ----------------------------------------------------------
# AC1: round_trip_fixtures, AC2: schema_is_valid, AC3: validate_rejects*, AC4: all_golden_fixtures, AC5: clippy+test, AC6: bound_mqo*
AC_MUST_PASS=0
AC_SHOULD_PASS=0
grep -q 'round_trip_fixtures ... ok' /tmp/mqo_test.log && AC_MUST_PASS=$((AC_MUST_PASS+1)) || true
grep -q 'schema_is_valid_json_schema ... ok' /tmp/mqo_test.log && AC_MUST_PASS=$((AC_MUST_PASS+1)) || true
grep -q 'validate_rejects_empty_measures ... ok' /tmp/mqo_test.log && grep -q 'validate_rejects_limit_zero ... ok' /tmp/mqo_test.log && grep -q 'validate_rejects_range_lo_gt_hi ... ok' /tmp/mqo_test.log && AC_MUST_PASS=$((AC_MUST_PASS+1)) || true
grep -q 'all_golden_fixtures_parse_and_validate ... ok' /tmp/mqo_test.log && AC_MUST_PASS=$((AC_MUST_PASS+1)) || true
[ "$TEST_EXIT" -eq 0 ] && [ "$CLIPPY_EXIT" -eq 0 ] && AC_MUST_PASS=$((AC_MUST_PASS+1)) || true
grep -q 'bound_mqo_fields_present ... ok' /tmp/mqo_test.log && AC_SHOULD_PASS=$((AC_SHOULD_PASS+1)) || true

AC_PASSING=$((AC_MUST_PASS + AC_SHOULD_PASS))
QUALITY_SCORE=$((10*AC_PASSING - 2*0 - 1*CLIPPY_WARNINGS))

cat > "$OUT" <<EOF
{
  "schema": "autobuilder.metrics.v1",
  "generated_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "tests_pass_count": $TESTS_PASS,
  "tests_fail_count": $TESTS_FAIL,
  "test_exit": $TEST_EXIT,
  "clippy_exit": $CLIPPY_EXIT,
  "clippy_warning_count": $CLIPPY_WARNINGS,
  "ac_passing_count": $AC_PASSING,
  "ac_must_pass": $AC_MUST_PASS,
  "ac_should_pass": $AC_SHOULD_PASS,
  "quality_score": $QUALITY_SCORE,
  "mutation_kill_rate": null,
  "doc_coverage_pct": null,
  "proptest_density": 0,
  "audit_findings_count": 0
}
EOF

echo "metrics written to $OUT"
echo "  tests_pass=$TESTS_PASS fail=$TESTS_FAIL  ac_passing=$AC_PASSING  quality_score=$QUALITY_SCORE"
