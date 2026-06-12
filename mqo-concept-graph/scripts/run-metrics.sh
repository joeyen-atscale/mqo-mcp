#!/usr/bin/env bash
# run-metrics.sh — measure mqo-concept-graph and emit target/autobuilder/metrics.json
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

CARGO_MANIFEST="$PROJECT_DIR/Cargo.toml"

# Run lib + integration tests and count passing
cargo test --lib --tests --manifest-path "$CARGO_MANIFEST" > /tmp/concept_graph_test.log 2>&1
TEST_EXIT=$?
TESTS_PASS=$(grep -E '^test result: ok' /tmp/concept_graph_test.log | awk '{sum+=$4} END{print sum+0}')
TESTS_FAIL=$(grep -E '^test result:' /tmp/concept_graph_test.log | awk '{sum+=$6} END{print sum+0}')

# Clippy check
cargo clippy --manifest-path "$CARGO_MANIFEST" -- -D warnings > /tmp/concept_graph_clippy.log 2>&1
CLIPPY_EXIT=$?
CLIPPY_WARNINGS=$(grep -c '^warning' /tmp/concept_graph_clippy.log || true)

# MUST AC count: AC1, AC2, AC3, AC4
AC_TOTAL=4
AC_PASSING=0
grep -q 'ac1_node_count_matches_fixture ... ok' /tmp/concept_graph_test.log && AC_PASSING=$((AC_PASSING+1)) || true
grep -q 'ac2_all_four_primary_edge_kinds_present ... ok' /tmp/concept_graph_test.log && AC_PASSING=$((AC_PASSING+1)) || true
grep -q 'ac3_empty_string_returns_err ... ok' /tmp/concept_graph_test.log && AC_PASSING=$((AC_PASSING+1)) || true
grep -q 'ac4_neighbors_filtered_by_aggregates_via ... ok' /tmp/concept_graph_test.log && AC_PASSING=$((AC_PASSING+1)) || true

HEAD_SHA=$(git -C "$PROJECT_DIR" rev-parse HEAD 2>/dev/null || echo "unknown")
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)

mkdir -p "$PROJECT_DIR/target/autobuilder"

cat > "$PROJECT_DIR/target/autobuilder/metrics.json" <<JSON
{
  "schema": "autobuilder.metrics.v1",
  "head_sha": "$HEAD_SHA",
  "iteration": null,
  "scalars": {
    "tests_pass_count": $TESTS_PASS,
    "acceptance_tests_passing_count": $AC_PASSING,
    "acceptance_tests_total_count": $AC_TOTAL
  },
  "ac_passing_count": $AC_PASSING,
  "ac_total_count": $AC_TOTAL,
  "ac_results": [],
  "audit": {
    "blocking_count": 0,
    "advisory_count": 0
  },
  "clippy_warning_count": $CLIPPY_WARNINGS,
  "test_coverage_pct": null,
  "doc_coverage_pct": null,
  "proptest_density": 0,
  "mutants_alive_count": null,
  "mutants_tested_count": null,
  "captured_at": "$NOW"
}
JSON

echo "metrics written to $PROJECT_DIR/target/autobuilder/metrics.json"
echo "  tests_pass=$TESTS_PASS fail=$TESTS_FAIL  ac_passing=$AC_PASSING/$AC_TOTAL  clippy_warnings=$CLIPPY_WARNINGS"
