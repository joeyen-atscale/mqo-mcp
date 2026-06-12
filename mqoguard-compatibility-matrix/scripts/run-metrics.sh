#!/usr/bin/env bash
# run-metrics.sh — single-script orchestrator that produces target/autobuilder/metrics.json.
#
# READ-ONLY: the edit-agent must not modify this file.
set -uo pipefail
cd "$(dirname "$0")/.."

OUT=target/autobuilder/metrics.json
LOG=target/autobuilder/run.log
mkdir -p target/autobuilder
: > "$LOG"

HEAD_SHA=$(git rev-parse HEAD 2>/dev/null || echo "unknown")
CAPTURED=$(date -u +%Y-%m-%dT%H:%M:%SZ)

echo "::gate cargo check" | tee -a "$LOG"
cargo check --workspace 2>&1 | tee -a "$LOG" || true

echo "::gate cargo clippy" | tee -a "$LOG"
CLIPPY_WARNINGS=0
if ! cargo clippy --workspace --message-format=json -- -D warnings > target/autobuilder/clippy.json 2>&1; then
  CLIPPY_WARNINGS=$(jq -s '[.[] | select(.reason == "compiler-message" and .message.level == "warning")] | length' target/autobuilder/clippy.json 2>/dev/null || echo 0)
fi

echo "::gate cargo test" | tee -a "$LOG"
cargo test --workspace --no-fail-fast 2>&1 | tee target/autobuilder/test-output.txt | tee -a "$LOG" || true

AC_TOTAL=$(find tests -maxdepth 1 -name 'acceptance_*.rs' -type f 2>/dev/null | wc -l | tr -d ' ')
AC_PASSING=0
if [ "${AC_TOTAL:-0}" -gt 0 ]; then
  AC_PASSING=$(grep -cE '^test acceptance_[a-z0-9_]+ \.\.\. ok' target/autobuilder/test-output.txt || true)
fi

AUDIT_OUT=target/autobuilder/audit.json
BLOCKING=0
ADVISORY=0
if [ -x "$HOME/.claude/skills/autobuilder/rules/audit-checks.sh" ]; then
  if ! "$HOME/.claude/skills/autobuilder/rules/audit-checks.sh" . > "$AUDIT_OUT" 2>&1; then : ; fi
  BLOCKING=$(jq -r '.blocking_count // 0' "$AUDIT_OUT" 2>/dev/null || echo 0)
  ADVISORY=$(jq -r '.advisory_count // 0' "$AUDIT_OUT" 2>/dev/null || echo 0)
fi

INTENT_METRIC_NAME=""
if [ -f agent/intent-card.json ]; then
  INTENT_METRIC_NAME=$(jq -r '.unfakeable_metric.name // empty' agent/intent-card.json 2>/dev/null || echo "")
fi
SCALARS_JSON=$(jq -n \
  --argjson ac "$AC_PASSING" \
  --argjson total "$AC_TOTAL" \
  --arg metric_name "$INTENT_METRIC_NAME" \
  '({ acceptance_tests_passing_count: $ac, acceptance_tests_total_count: $total })
   + (if ($metric_name != "" and $metric_name != "acceptance_tests_passing_count")
      then { ($metric_name): $ac } else {} end)')

jq -n \
  --arg head "$HEAD_SHA" \
  --arg captured "$CAPTURED" \
  --argjson scalars "$SCALARS_JSON" \
  --argjson ac_pass "$AC_PASSING" \
  --argjson ac_total "$AC_TOTAL" \
  --argjson clippy "$CLIPPY_WARNINGS" \
  --argjson blocking "$BLOCKING" \
  --argjson advisory "$ADVISORY" \
  '{
    schema: "autobuilder.metrics.v1",
    head_sha: $head,
    iteration: null,
    scalars: $scalars,
    ac_passing_count: $ac_pass,
    ac_total_count: $ac_total,
    ac_results: [],
    audit: { blocking_count: $blocking, advisory_count: $advisory },
    clippy_warning_count: $clippy,
    test_coverage_pct: null,
    doc_coverage_pct: null,
    proptest_density: null,
    mutants_alive_count: null,
    mutants_tested_count: null,
    captured_at: $captured
  }' > "$OUT"

echo "metrics written to $OUT"
