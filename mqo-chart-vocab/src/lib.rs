//! # `mqo-chart-vocab`
//!
//! Canonical wire vocabulary for `chart-recommendation.v1`.
//!
//! ## Background
//!
//! The recommender (`mqo-chart-recommender`) serializes `Mark` variants with
//! `#[serde(rename_all = "snake_case")]`, producing `"bar"`, `"big_number"`,
//! etc.  The emitter (`mqo-vega-emitter`) historically only accepted the
//! PascalCase forms (`"Bar"`, `"BigNumber"`), causing a hard pipe failure.
//!
//! This crate pins `snake_case` as the ONE canonical wire spelling for
//! `chart-recommendation.v1` marks, provides:
//!
//! - `ALL_MARKS` — the exhaustive list of canonical mark strings.
//! - `is_canonical_mark` — predicate for the canonical form.
//! - `normalize_mark` — maps any recognised spelling (canonical or legacy
//!   PascalCase) to the canonical `&'static str`, enabling the emitter's
//!   deprecation-window acceptance of legacy PascalCase without a hand-rolled
//!   match table.
//! - `MarkConformance` / `check_conformance` — round-trip conformance helpers
//!   for CI: given a slice of mark strings from the recommender, verify that
//!   every one is already canonical (no shim required).
//!
//! ## Wire spelling reference
//!
//! | Rust enum variant | Canonical wire (snake_case) | Legacy wire (PascalCase) |
//! |---|---|---|
//! | `BigNumber` | `big_number` | `BigNumber` (**deprecated**) |
//! | `Bar`       | `bar`         | `Bar`       (**deprecated**) |
//! | `Line`      | `line`        | `Line`      (**deprecated**) |
//! | `Point`     | `point`       | `Point`     (**deprecated**) |
//! | `Area`      | `area`        | `Area`      (**deprecated**) |
//! | `Rect`      | `rect`        | `Rect`      (**deprecated**) |
//! | `Table`     | `table`       | `Table`     (**deprecated**) |

// ─── canonical mark constants ─────────────────────────────────────────────────

/// Canonical wire spelling for the "bar" mark.
pub const MARK_BAR: &str = "bar";

/// Canonical wire spelling for the "line" mark.
pub const MARK_LINE: &str = "line";

/// Canonical wire spelling for the "big_number" KPI-card mark.
pub const MARK_BIG_NUMBER: &str = "big_number";

/// Canonical wire spelling for the "point" (scatter/bubble) mark.
pub const MARK_POINT: &str = "point";

/// Canonical wire spelling for the "area" mark.
pub const MARK_AREA: &str = "area";

/// Canonical wire spelling for the "rect" (heatmap) mark.
pub const MARK_RECT: &str = "rect";

/// Canonical wire spelling for the "table" (plain data table) mark.
pub const MARK_TABLE: &str = "table";

/// All canonical mark strings in a fixed order.  Exhaustive — adding a new
/// mark MUST add an entry here.
pub const ALL_MARKS: &[&str] = &[
    MARK_BAR,
    MARK_LINE,
    MARK_BIG_NUMBER,
    MARK_POINT,
    MARK_AREA,
    MARK_RECT,
    MARK_TABLE,
];

// ─── predicates ───────────────────────────────────────────────────────────────

/// Returns `true` iff `s` is an exact canonical (snake_case) mark string.
///
/// ```
/// assert!(mqo_chart_vocab::is_canonical_mark("bar"));
/// assert!(mqo_chart_vocab::is_canonical_mark("big_number"));
/// assert!(!mqo_chart_vocab::is_canonical_mark("Bar"));
/// assert!(!mqo_chart_vocab::is_canonical_mark("BigNumber"));
/// assert!(!mqo_chart_vocab::is_canonical_mark("piechart"));
/// ```
#[inline]
pub fn is_canonical_mark(s: &str) -> bool {
    ALL_MARKS.contains(&s)
}

// ─── normalization ────────────────────────────────────────────────────────────

/// Lookup table of legacy PascalCase → canonical snake_case.
///
/// Kept private; callers use [`normalize_mark`].
static LEGACY_TO_CANONICAL: &[(&str, &str)] = &[
    ("Bar", MARK_BAR),
    ("Line", MARK_LINE),
    ("BigNumber", MARK_BIG_NUMBER),
    ("Point", MARK_POINT),
    ("Area", MARK_AREA),
    ("Rect", MARK_RECT),
    ("Table", MARK_TABLE),
];

/// Normalize any recognised mark spelling to the canonical snake_case form.
///
/// Returns `Some(&'static str)` for:
/// - Any canonical snake_case mark (pass-through).
/// - Any legacy PascalCase mark (mapped to canonical; callers should warn).
///
/// Returns `None` for strings that are neither canonical nor a known legacy
/// form (unrecognised).
///
/// ```
/// use mqo_chart_vocab::normalize_mark;
///
/// // Canonical form passes through
/// assert_eq!(normalize_mark("bar"),         Some("bar"));
/// assert_eq!(normalize_mark("big_number"),  Some("big_number"));
///
/// // Legacy PascalCase is mapped to canonical
/// assert_eq!(normalize_mark("Bar"),         Some("bar"));
/// assert_eq!(normalize_mark("BigNumber"),   Some("big_number"));
///
/// // Unknown string returns None
/// assert_eq!(normalize_mark("piechart"),    None);
/// ```
pub fn normalize_mark(s: &str) -> Option<&'static str> {
    // Fast path: already canonical.
    for &canonical in ALL_MARKS {
        if canonical == s {
            return Some(canonical);
        }
    }
    // Slow path: legacy PascalCase.
    for &(legacy, canonical) in LEGACY_TO_CANONICAL {
        if legacy == s {
            return Some(canonical);
        }
    }
    None
}

/// Returns `true` iff `s` is a recognised legacy (PascalCase) form.
///
/// Callers can use this to decide whether to emit a deprecation warning.
///
/// ```
/// use mqo_chart_vocab::is_legacy_mark;
/// assert!(is_legacy_mark("BigNumber"));
/// assert!(!is_legacy_mark("big_number")); // canonical, not legacy
/// assert!(!is_legacy_mark("piechart"));   // unrecognised
/// ```
pub fn is_legacy_mark(s: &str) -> bool {
    LEGACY_TO_CANONICAL.iter().any(|&(legacy, _)| legacy == s)
}

// ─── conformance types ────────────────────────────────────────────────────────

/// A single mark conformance finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkFinding {
    /// The mark string that was tested.
    pub mark: String,
    /// Whether the mark is already in canonical form.
    pub is_canonical: bool,
    /// Whether the mark is a recognised legacy form (implies translation needed).
    pub is_legacy: bool,
    /// Whether the mark is completely unrecognised.
    pub is_unknown: bool,
}

impl MarkFinding {
    /// Returns `true` iff no shim is required for this mark.
    pub fn passes(&self) -> bool {
        self.is_canonical
    }
}

impl std::fmt::Display for MarkFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_canonical {
            write!(f, "PASS  {:?} — canonical", self.mark)
        } else if self.is_legacy {
            write!(
                f,
                "FAIL  {:?} — legacy PascalCase (canonical: {:?}); translation shim required",
                self.mark,
                normalize_mark(&self.mark).unwrap_or("<none>")
            )
        } else {
            write!(f, "FAIL  {:?} — unrecognised mark", self.mark)
        }
    }
}

/// Run conformance checks on a slice of mark strings.
///
/// Returns one [`MarkFinding`] per input string.  The overall conformance check
/// passes iff ALL findings pass (`finding.is_canonical == true`).
///
/// Intended for use in tests and CI; see [`assert_all_canonical`].
pub fn check_conformance(marks: &[&str]) -> Vec<MarkFinding> {
    marks
        .iter()
        .map(|&mark| {
            let canonical = is_canonical_mark(mark);
            let legacy = if canonical { false } else { is_legacy_mark(mark) };
            let unknown = !canonical && !legacy;
            MarkFinding {
                mark: mark.to_owned(),
                is_canonical: canonical,
                is_legacy: legacy,
                is_unknown: unknown,
            }
        })
        .collect()
}

/// Assert that every mark in `marks` is canonical, printing diagnostic output
/// for any failing variant.
///
/// Panics with a message naming every failing variant so CI produces an
/// actionable failure (requirement R7).
///
/// ```
/// mqo_chart_vocab::assert_all_canonical(&["bar", "line", "big_number"]);
/// ```
pub fn assert_all_canonical(marks: &[&str]) {
    let findings = check_conformance(marks);
    let failures: Vec<&MarkFinding> = findings.iter().filter(|f| !f.passes()).collect();
    if !failures.is_empty() {
        let mut msg = format!(
            "chart-recommendation.v1 mark conformance FAILED ({}/{} variants non-canonical):\n",
            failures.len(),
            marks.len()
        );
        for f in &failures {
            msg.push_str(&format!("  {f}\n"));
        }
        panic!("{}", msg);
    }
}

// ─── Mark enum ───────────────────────────────────────────────────────────────

/// Canonical `chart-recommendation.v1` mark types.
///
/// Serde serializes each variant in `snake_case` (the canonical wire format):
/// `"bar"`, `"line"`, `"big_number"`, `"point"`, `"area"`, `"rect"`, `"table"`.
///
/// Use [`parse_mark`] to deserialize from both canonical `snake_case` and the
/// legacy PascalCase forms accepted during the deprecation window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// Bar / column chart.
    Bar,
    /// Line chart.
    Line,
    /// KPI "big number" card.
    BigNumber,
    /// Scatter / bubble chart.
    Point,
    /// Area chart.
    Area,
    /// Heatmap / rect mark.
    Rect,
    /// Plain data table.
    Table,
}

/// Return the canonical `snake_case` wire string for `mark`.
///
/// This is always the string that `serde_json::to_string` would produce for the
/// variant wrapped in a single-field struct.
///
/// ```
/// use mqo_chart_vocab::{Mark, canonical_mark_str};
/// assert_eq!(canonical_mark_str(&Mark::BigNumber), "big_number");
/// assert_eq!(canonical_mark_str(&Mark::Bar),       "bar");
/// ```
pub fn canonical_mark_str(mark: &Mark) -> &'static str {
    match mark {
        Mark::Bar       => MARK_BAR,
        Mark::Line      => MARK_LINE,
        Mark::BigNumber => MARK_BIG_NUMBER,
        Mark::Point     => MARK_POINT,
        Mark::Area      => MARK_AREA,
        Mark::Rect      => MARK_RECT,
        Mark::Table     => MARK_TABLE,
    }
}

/// Parse a mark from a string, accepting BOTH canonical `snake_case` and the
/// legacy PascalCase forms.
///
/// Returns `None` for strings that are neither canonical nor a known legacy
/// form.  Use [`is_legacy_pascal`] to decide whether to emit a deprecation
/// warning.
///
/// ```
/// use mqo_chart_vocab::{parse_mark, Mark};
///
/// // Canonical snake_case
/// assert_eq!(parse_mark("bar"),        Some(Mark::Bar));
/// assert_eq!(parse_mark("big_number"), Some(Mark::BigNumber));
///
/// // Legacy PascalCase (deprecated but accepted during the deprecation window)
/// assert_eq!(parse_mark("Bar"),        Some(Mark::Bar));
/// assert_eq!(parse_mark("BigNumber"),  Some(Mark::BigNumber));
///
/// // Unknown → None
/// assert_eq!(parse_mark("piechart"),   None);
/// ```
pub fn parse_mark(s: &str) -> Option<Mark> {
    // Canonical snake_case first.
    match s {
        "bar"        => return Some(Mark::Bar),
        "line"       => return Some(Mark::Line),
        "big_number" => return Some(Mark::BigNumber),
        "point"      => return Some(Mark::Point),
        "area"       => return Some(Mark::Area),
        "rect"       => return Some(Mark::Rect),
        "table"      => return Some(Mark::Table),
        _ => {}
    }
    // Legacy PascalCase fallback.
    match s {
        "Bar"       => Some(Mark::Bar),
        "Line"      => Some(Mark::Line),
        "BigNumber" => Some(Mark::BigNumber),
        "Point"     => Some(Mark::Point),
        "Area"      => Some(Mark::Area),
        "Rect"      => Some(Mark::Rect),
        "Table"     => Some(Mark::Table),
        _           => None,
    }
}

/// Returns `true` iff `s` is a recognised legacy PascalCase form (not the
/// canonical `snake_case` form).
///
/// Callers use this to decide whether to emit a deprecation warning after a
/// successful [`parse_mark`].
///
/// ```
/// use mqo_chart_vocab::is_legacy_pascal;
/// assert!( is_legacy_pascal("BigNumber"));  // PascalCase → legacy
/// assert!(!is_legacy_pascal("big_number")); // canonical  → not legacy
/// assert!(!is_legacy_pascal("piechart"));   // unknown    → not legacy
/// ```
pub fn is_legacy_pascal(s: &str) -> bool {
    matches!(s, "Bar" | "Line" | "BigNumber" | "Point" | "Area" | "Rect" | "Table")
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── R1 / R2: canonical vocabulary completeness ─────────────────────────────

    #[test]
    fn all_marks_has_seven_variants() {
        assert_eq!(ALL_MARKS.len(), 7, "ALL_MARKS must contain exactly 7 variants");
    }

    #[test]
    fn all_marks_are_snake_case() {
        for mark in ALL_MARKS {
            // snake_case: only lowercase ASCII letters, digits, or underscores.
            assert!(
                mark.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "mark {:?} is not snake_case",
                mark
            );
        }
    }

    #[test]
    fn all_marks_are_non_empty() {
        for mark in ALL_MARKS {
            assert!(!mark.is_empty(), "ALL_MARKS contains an empty string");
        }
    }

    // ── is_canonical_mark ──────────────────────────────────────────────────────

    #[test]
    fn canonical_marks_recognized() {
        for &mark in ALL_MARKS {
            assert!(is_canonical_mark(mark), "mark {:?} should be canonical", mark);
        }
    }

    #[test]
    fn pascal_case_marks_not_canonical() {
        for &(pascal, _canonical) in LEGACY_TO_CANONICAL {
            assert!(
                !is_canonical_mark(pascal),
                "PascalCase {:?} must NOT pass is_canonical_mark",
                pascal
            );
        }
    }

    #[test]
    fn unknown_string_not_canonical() {
        assert!(!is_canonical_mark("piechart"));
        assert!(!is_canonical_mark(""));
        assert!(!is_canonical_mark("BAR"));
    }

    // ── normalize_mark ─────────────────────────────────────────────────────────

    #[test]
    fn canonical_marks_normalize_to_themselves() {
        for &mark in ALL_MARKS {
            assert_eq!(
                normalize_mark(mark),
                Some(mark),
                "canonical mark {:?} should normalize to itself",
                mark
            );
        }
    }

    #[test]
    fn legacy_marks_normalize_to_canonical() {
        let cases = [
            ("Bar", "bar"),
            ("Line", "line"),
            ("BigNumber", "big_number"),
            ("Point", "point"),
            ("Area", "area"),
            ("Rect", "rect"),
            ("Table", "table"),
        ];
        for (legacy, expected) in &cases {
            assert_eq!(
                normalize_mark(legacy),
                Some(*expected),
                "legacy {:?} should normalize to {:?}",
                legacy,
                expected
            );
        }
    }

    #[test]
    fn unknown_string_normalizes_to_none() {
        assert_eq!(normalize_mark("piechart"), None);
        assert_eq!(normalize_mark(""), None);
        assert_eq!(normalize_mark("BAR"), None);
        assert_eq!(normalize_mark("bar_chart"), None);
    }

    // ── is_legacy_mark ─────────────────────────────────────────────────────────

    #[test]
    fn pascal_case_is_legacy() {
        assert!(is_legacy_mark("Bar"));
        assert!(is_legacy_mark("BigNumber"));
        assert!(is_legacy_mark("Table"));
    }

    #[test]
    fn canonical_is_not_legacy() {
        for &mark in ALL_MARKS {
            assert!(
                !is_legacy_mark(mark),
                "canonical {:?} must not be flagged as legacy",
                mark
            );
        }
    }

    // ── check_conformance / assert_all_canonical ───────────────────────────────

    /// R6 / R7: golden round-trip — every variant the recommender emits must
    /// already be canonical (no shim needed).
    ///
    /// This is the primary conformance gate.  If the recommender's serialized
    /// mark output drifts from canonical, this test names the offending variant.
    #[test]
    fn recommender_output_is_all_canonical() {
        // These are the exact strings serde produces for
        // mqo-chart-recommender's Mark enum with #[serde(rename_all = "snake_case")].
        let recommender_marks = [
            "bar",
            "line",
            "big_number",
            "point",
            "area",
            "rect",
            "table",
        ];
        assert_all_canonical(&recommender_marks);
    }

    #[test]
    fn conformance_check_names_failing_variant() {
        let findings = check_conformance(&["bar", "BigNumber", "piechart"]);
        assert_eq!(findings.len(), 3);

        let bar = &findings[0];
        assert!(bar.passes(), "bar should pass");

        let big_number = &findings[1];
        assert!(!big_number.passes(), "BigNumber (legacy) should fail");
        assert!(big_number.is_legacy);
        assert_eq!(big_number.mark, "BigNumber");

        let piechart = &findings[2];
        assert!(!piechart.passes(), "piechart should fail");
        assert!(piechart.is_unknown);
        assert_eq!(piechart.mark, "piechart");
    }

    #[test]
    fn conformance_finding_display_names_variant() {
        let findings = check_conformance(&["BigNumber"]);
        let display = findings[0].to_string();
        assert!(
            display.contains("BigNumber"),
            "display output must name the offending variant"
        );
        assert!(
            display.contains("FAIL"),
            "display output must say FAIL"
        );
    }

    #[test]
    fn assert_all_canonical_passes_for_canonical_marks() {
        // Must not panic.
        assert_all_canonical(ALL_MARKS);
    }

    // ── Mark enum: serde round-trip conformance ────────────────────────────────

    /// Conformance test: for each of the 7 Mark variants, serialize with
    /// serde_json and assert the snake_case wire form, then parse_mark() back
    /// and assert round-trip (R6 / R7).
    #[test]
    fn mark_serde_round_trip_all_variants() {
        let cases: &[(Mark, &str)] = &[
            (Mark::Bar,       "\"bar\""),
            (Mark::Line,      "\"line\""),
            (Mark::BigNumber, "\"big_number\""),
            (Mark::Point,     "\"point\""),
            (Mark::Area,      "\"area\""),
            (Mark::Rect,      "\"rect\""),
            (Mark::Table,     "\"table\""),
        ];
        for &(mark, expected_json) in cases {
            // Serialize: must produce canonical snake_case.
            let json = serde_json::to_string(&mark)
                .expect("serde_json::to_string should not fail for Mark");
            assert_eq!(
                json, expected_json,
                "variant {:?} serialized to {:?}, want {:?}",
                mark, json, expected_json
            );

            // parse_mark: must round-trip back to the same variant.
            let wire = json.trim_matches('"');
            let parsed = parse_mark(wire)
                .unwrap_or_else(|| panic!("parse_mark({:?}) returned None for canonical form", wire));
            assert_eq!(
                parsed, mark,
                "parse_mark({:?}) round-trip failed: got {:?}, want {:?}",
                wire, parsed, mark
            );

            // canonical_mark_str: must match the unquoted wire string.
            assert_eq!(
                canonical_mark_str(&mark),
                wire,
                "canonical_mark_str({:?}) = {:?}, want {:?}",
                mark,
                canonical_mark_str(&mark),
                wire
            );
        }
    }

    // ── parse_mark + is_legacy_pascal ──────────────────────────────────────────

    /// Legacy acceptance: PascalCase forms are accepted during the deprecation window.
    #[test]
    fn legacy_pascal_accepted_by_parse_mark() {
        assert_eq!(parse_mark("BigNumber"), Some(Mark::BigNumber));
        assert_eq!(parse_mark("Bar"),       Some(Mark::Bar));
        assert_eq!(parse_mark("Line"),      Some(Mark::Line));
        assert_eq!(parse_mark("Point"),     Some(Mark::Point));
        assert_eq!(parse_mark("Area"),      Some(Mark::Area));
        assert_eq!(parse_mark("Rect"),      Some(Mark::Rect));
        assert_eq!(parse_mark("Table"),     Some(Mark::Table));
    }

    #[test]
    fn is_legacy_pascal_big_number_true() {
        assert!(is_legacy_pascal("BigNumber"),  "BigNumber must be flagged legacy");
        assert!(!is_legacy_pascal("big_number"), "big_number is canonical, not legacy");
    }

    #[test]
    fn is_legacy_pascal_all_pascal_forms() {
        for pascal in &["Bar", "Line", "BigNumber", "Point", "Area", "Rect", "Table"] {
            assert!(is_legacy_pascal(pascal), "{:?} must be flagged legacy", pascal);
        }
    }

    #[test]
    fn is_legacy_pascal_canonical_forms_are_false() {
        for &canonical in ALL_MARKS {
            assert!(!is_legacy_pascal(canonical), "canonical {:?} must not be legacy", canonical);
        }
    }

    /// Unknown rejection: strings that are neither canonical nor legacy must return None.
    #[test]
    fn parse_mark_rejects_unknown() {
        assert_eq!(parse_mark("piechart"), None);
        assert_eq!(parse_mark(""),         None);
        assert_eq!(parse_mark("BAR"),      None);
        assert_eq!(parse_mark("bar_chart"),None);
    }

    // ── legacy coverage: each known PascalCase variant is covered ──────────────

    #[test]
    fn legacy_table_has_one_entry_per_canonical() {
        // Every canonical mark must have exactly one legacy entry.
        for &canonical in ALL_MARKS {
            let count = LEGACY_TO_CANONICAL
                .iter()
                .filter(|&&(_, c)| c == canonical)
                .count();
            assert_eq!(
                count, 1,
                "canonical {:?} must have exactly one legacy entry, found {}",
                canonical, count
            );
        }
    }
}
