// brief.rs — Markdown brief renderer.
// Implements FR1–FR13, NFR1–NFR3.  No network calls; fully deterministic over inputs.

use crate::types::{HistoryRecord, PairCounts, Status, TigerRecord};
use std::collections::{BTreeMap, BTreeSet};

const ALL_SKIPPED: &str = "AllSkipped";

/// Banned alias strings (FR2 / specificity rule).
const BANNED_ALIASES: &[&str] = &["latest", "the cluster", "nonprod", "staging"];

/// Number of never-tested measures to list before collapsing (OQ3 — default 10).
const NEVER_TESTED_TOP_N: usize = 10;

/// Percentage of verified measures for a build.
/// Returns None when the build is AllSkipped or has zero denominator.
pub fn coverage_pct(record: &HistoryRecord) -> Option<f64> {
    if record.overall_verdict == ALL_SKIPPED {
        return None;
    }
    let total = record.measures.len();
    if total == 0 {
        return None;
    }
    let verified = record
        .measures
        .iter()
        .filter(|m| m.status == Status::Verified)
        .count();
    Some((verified as f64 / total as f64) * 100.0)
}

/// Group measures by backend pair, returning BTreeMap for stable ordering.
fn pair_counts(record: &HistoryRecord) -> BTreeMap<String, PairCounts> {
    let mut map: BTreeMap<String, PairCounts> = BTreeMap::new();
    for m in &record.measures {
        let e = map.entry(m.backend_pair.clone()).or_insert_with(|| PairCounts {
            backend_pair: m.backend_pair.clone(),
            verified: 0,
            mismatch: 0,
            never_tested: 0,
        });
        match m.status {
            Status::Verified => e.verified += 1,
            Status::Mismatch => e.mismatch += 1,
            Status::NeverTested => e.never_tested += 1,
        }
    }
    map
}

/// Validate that a rendered brief contains no banned aliases (NFR2 guard).
fn assert_no_aliases(brief: &str) {
    for alias in BANNED_ALIASES {
        debug_assert!(
            !brief.contains(alias),
            "Brief contains banned alias '{}' — FR2 violation",
            alias
        );
    }
}

/// Render the full Markdown brief for `target` record, given the full ordered history
/// (oldest-first) and optional build that precedes the target (for delta).
pub fn render_brief(
    history: &[HistoryRecord],
    target: &HistoryRecord,
    prior: Option<&HistoryRecord>,
) -> String {
    let mut out = String::new();

    // --- Title ---
    let title = format!(
        "# DAX Parity Status — build `{}` (`{}`) on `{}`\n",
        target.build_id, target.version, target.cluster
    );
    out.push_str(&title);
    out.push('\n');

    // --- FR1: Headline coverage % as FIRST content line ---
    let headline = match coverage_pct(target) {
        Some(pct) => format!(
            "**Parity coverage: {:.0}%** — build `{}` (`{}`) on `{}` (recorded {})\n",
            pct, target.build_id, target.version, target.cluster, target.recorded_at
        ),
        None => format!(
            "**Not measured this build (no live backends — `AllSkipped`)** — build `{}` (`{}`) on `{}` (recorded {})\n",
            target.build_id, target.version, target.cluster, target.recorded_at
        ),
    };
    out.push_str(&headline);
    out.push('\n');

    // --- FR4: Newly-disagree section (MUST appear above per-pair and coverage-gap) ---
    out.push_str("## Measures that newly disagree this build\n\n");
    if target.overall_verdict == ALL_SKIPPED {
        out.push_str(
            "_Build was AllSkipped (no live backends). No regression verdict possible._\n\n",
        );
    } else if prior.is_none() {
        out.push_str("_No prior build in history to compare against — first recorded build._\n\n");
    } else if target.deltas.newly_broken.is_empty() {
        // FR10: explicit clean-bill line
        out.push_str(&format!(
            "No measures newly disagree in build `{}` (`{}`).\n\n",
            target.build_id, target.version
        ));
    } else {
        // Sort by measure name then backend pair for stable output (NFR1)
        let mut entries = target.deltas.newly_broken.clone();
        entries.sort_by(|a, b| a.measure.cmp(&b.measure).then(a.backend_pair.cmp(&b.backend_pair)));
        out.push_str("| Measure | Backend pair |\n");
        out.push_str("|---|---|\n");
        for e in &entries {
            out.push_str(&format!("| {} | {} |\n", e.measure, e.backend_pair));
        }
        out.push('\n');
    }

    // FR13 (MAY): newly-verified / recovered section
    if !target.deltas.newly_verified.is_empty() && target.overall_verdict != ALL_SKIPPED {
        out.push_str("## Newly verified (recovered) this build\n\n");
        let mut entries = target.deltas.newly_verified.clone();
        entries.sort_by(|a, b| a.measure.cmp(&b.measure).then(a.backend_pair.cmp(&b.backend_pair)));
        out.push_str("| Measure | Backend pair |\n");
        out.push_str("|---|---|\n");
        for e in &entries {
            out.push_str(&format!("| {} | {} |\n", e.measure, e.backend_pair));
        }
        out.push('\n');
    }

    // --- FR5: Per-backend-pair breakdown ---
    out.push_str("## Coverage by backend pair\n\n");
    if target.overall_verdict == ALL_SKIPPED {
        out.push_str("_Not measured this build (AllSkipped — no live backends)._\n\n");
    } else if target.measures.is_empty() {
        out.push_str("_No measure records for this build._\n\n");
    } else {
        let counts = pair_counts(target);
        out.push_str("| Backend pair | Verified | Mismatch | Never-tested | Coverage % |\n");
        out.push_str("|---|---|---|---|---|\n");
        for (_, pc) in &counts {
            let total = pc.verified + pc.mismatch + pc.never_tested;
            let pct = if total > 0 {
                format!("{:.0}%", (pc.verified as f64 / total as f64) * 100.0)
            } else {
                "—".to_string()
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                pc.backend_pair, pc.verified, pc.mismatch, pc.never_tested, pct
            ));
        }
        out.push('\n');
    }

    // --- FR8: Never-tested (coverage gap) section ---
    out.push_str("## Coverage gaps (never-tested measures)\n\n");
    if target.overall_verdict == ALL_SKIPPED {
        out.push_str("_Not measured this build (AllSkipped)._\n\n");
    } else {
        // Collect unique (measure, backend_pair) pairs for never-tested, sorted for stability
        let never_tested: BTreeSet<(String, String)> = target
            .measures
            .iter()
            .filter(|m| m.status == Status::NeverTested)
            .map(|m| (m.measure.clone(), m.backend_pair.clone()))
            .collect();
        if never_tested.is_empty() {
            out.push_str("_No never-tested measures — full coverage._\n\n");
        } else {
            let shown: Vec<_> = never_tested.iter().take(NEVER_TESTED_TOP_N).collect();
            let remainder = never_tested.len().saturating_sub(NEVER_TESTED_TOP_N);
            out.push_str("| Measure | Backend pair |\n");
            out.push_str("|---|---|\n");
            for (measure, pair) in &shown {
                out.push_str(&format!("| {} | {} |\n", measure, pair));
            }
            if remainder > 0 {
                out.push_str(&format!("\n_…and {} more never-tested measures._\n", remainder));
            }
            out.push('\n');
        }
    }

    // --- FR6: Coverage-% text trend (oldest→newest, all builds in history) ---
    out.push_str("## Parity coverage trend\n\n");
    out.push_str("| Build | Version | Cluster | Coverage % |\n");
    out.push_str("|---|---|---|---|\n");
    for h in history {
        let pct = match coverage_pct(h) {
            Some(p) => format!("{:.0}%", p),
            None => "not measured (AllSkipped)".to_string(),
        };
        out.push_str(&format!(
            "| `{}` | `{}` | `{}` | {} |\n",
            h.build_id, h.version, h.cluster, pct
        ));
    }
    out.push('\n');

    // Assert no aliases crept in (NFR2 guard — debug mode only to preserve perf in release)
    assert_no_aliases(&out);

    out
}

/// Build the Tiger-compatible record for a given history entry (FR7).
pub fn build_tiger_record(record: &HistoryRecord) -> TigerRecord {
    let counts = pair_counts(record);
    TigerRecord {
        build_id: record.build_id.clone(),
        version: record.version.clone(),
        cluster: record.cluster.clone(),
        parity_coverage_pct: coverage_pct(record),
        backend_pair_counts: counts.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DeltaEntry, Deltas, MeasureStatus};

    fn make_record(build_id: &str, version: &str, measures: Vec<MeasureStatus>, deltas: Deltas, overall_verdict: &str) -> HistoryRecord {
        HistoryRecord {
            build_id: build_id.to_string(),
            version: version.to_string(),
            cluster: "mcp-aws.atscaleinternal.com".to_string(),
            recorded_at: "2026-06-10T10:00:00Z".to_string(),
            overall_verdict: overall_verdict.to_string(),
            measures,
            deltas,
        }
    }

    #[test]
    fn test_coverage_pct_normal() {
        let r = make_record("b-1", "v0.3.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
            MeasureStatus { measure: "M2".into(), backend_pair: "DAX↔SQL".into(), status: Status::Mismatch },
            MeasureStatus { measure: "M3".into(), backend_pair: "DAX↔SQL".into(), status: Status::NeverTested },
            MeasureStatus { measure: "M4".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
        ], Deltas::default(), "Agree");
        let pct = coverage_pct(&r).unwrap();
        assert!((pct - 50.0).abs() < 0.01, "expected 50%, got {}", pct);
    }

    #[test]
    fn test_coverage_pct_all_skipped() {
        let r = make_record("b-skip", "v0.3.0", vec![], Deltas::default(), "AllSkipped");
        assert!(coverage_pct(&r).is_none());
    }

    #[test]
    fn test_coverage_pct_100() {
        let r = make_record("b-2", "v0.3.1", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
            MeasureStatus { measure: "M2".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
        ], Deltas::default(), "Agree");
        assert!((coverage_pct(&r).unwrap() - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_headline_contains_build_id_and_version() {
        let r = make_record("b-2026-06-10.1", "v0.3.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
        ], Deltas::default(), "Agree");
        let brief = render_brief(&[r.clone()], &r, None);
        assert!(brief.contains("b-2026-06-10.1"), "build id missing from brief");
        assert!(brief.contains("v0.3.0"), "version missing from brief");
        assert!(brief.contains("mcp-aws.atscaleinternal.com"), "cluster missing from brief");
    }

    #[test]
    fn test_no_banned_aliases() {
        let r = make_record("b-3", "v0.3.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
        ], Deltas::default(), "Agree");
        let brief = render_brief(&[r.clone()], &r, None);
        for alias in BANNED_ALIASES {
            assert!(!brief.contains(alias), "brief contains banned alias '{}'", alias);
        }
    }

    #[test]
    fn test_clean_bill_line_when_no_regressions() {
        let prior = make_record("b-A", "v0.2.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
        ], Deltas::default(), "Agree");
        let target = make_record("b-B", "v0.3.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
        ], Deltas { newly_broken: vec![], newly_verified: vec![] }, "Agree");
        let history = vec![prior.clone(), target.clone()];
        let brief = render_brief(&history, &target, Some(&prior));
        assert!(brief.contains("No measures newly disagree"), "clean-bill line missing");
    }

    #[test]
    fn test_regression_list_exact_match() {
        let prior = make_record("b-A", "v0.2.0", vec![], Deltas::default(), "Agree");
        let target = make_record("b-B", "v0.3.0", vec![
            MeasureStatus { measure: "Total Returns".into(), backend_pair: "DAX↔SQL".into(), status: Status::Mismatch },
            MeasureStatus { measure: "Avg Net Profit".into(), backend_pair: "DAX↔SQL".into(), status: Status::Mismatch },
        ], Deltas {
            newly_broken: vec![
                DeltaEntry { measure: "Total Returns".into(), backend_pair: "DAX↔SQL".into() },
                DeltaEntry { measure: "Avg Net Profit".into(), backend_pair: "DAX↔SQL".into() },
            ],
            newly_verified: vec![],
        }, "Mismatch");
        let history = vec![prior.clone(), target.clone()];
        let brief = render_brief(&history, &target, Some(&prior));
        assert!(brief.contains("Total Returns"), "Total Returns missing from regression list");
        assert!(brief.contains("Avg Net Profit"), "Avg Net Profit missing from regression list");
        // Regression section must appear before Coverage by backend pair (FR4)
        let reg_pos = brief.find("Measures that newly disagree").unwrap();
        let pair_pos = brief.find("Coverage by backend pair").unwrap();
        assert!(reg_pos < pair_pos, "Regression section must appear before per-pair breakdown");
    }

    #[test]
    fn test_all_skipped_not_zero_percent() {
        let r = make_record("b-skip", "v0.3.0", vec![], Deltas::default(), "AllSkipped");
        let brief = render_brief(&[r.clone()], &r, None);
        assert!(!brief.contains("0%"), "AllSkipped must not show 0% coverage");
        assert!(brief.contains("AllSkipped") || brief.contains("no live backends"),
            "AllSkipped brief must mention AllSkipped or no live backends");
    }

    #[test]
    fn test_single_build_no_prior_message() {
        let r = make_record("b-only", "v0.1.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
        ], Deltas::default(), "Agree");
        let brief = render_brief(&[r.clone()], &r, None);
        assert!(brief.contains("No prior build"), "single-build should mention no prior build");
    }

    #[test]
    fn test_never_tested_section_present() {
        let r = make_record("b-x", "v0.3.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
            MeasureStatus { measure: "M2".into(), backend_pair: "DAX↔SQL".into(), status: Status::NeverTested },
            MeasureStatus { measure: "M3".into(), backend_pair: "DAX↔SQL".into(), status: Status::NeverTested },
        ], Deltas::default(), "Agree");
        let brief = render_brief(&[r.clone()], &r, None);
        assert!(brief.contains("M2"), "never-tested measure M2 should appear");
        assert!(brief.contains("M3"), "never-tested measure M3 should appear");
    }

    #[test]
    fn test_tiger_record_build_id_and_pct_match_brief() {
        let r = make_record("b-2026-06-10.1", "v0.3.0", vec![
            MeasureStatus { measure: "M1".into(), backend_pair: "DAX↔SQL".into(), status: Status::Verified },
            MeasureStatus { measure: "M2".into(), backend_pair: "DAX↔SQL".into(), status: Status::Mismatch },
        ], Deltas::default(), "Mismatch");
        let rec = build_tiger_record(&r);
        assert_eq!(rec.build_id, "b-2026-06-10.1");
        assert_eq!(rec.version, "v0.3.0");
        let pct = rec.parity_coverage_pct.unwrap();
        assert!((pct - 50.0).abs() < 0.01);
    }
}
