//! MDX structural syntax validation.
//!
//! This is a lexical/structural validator — it checks that the emitted MDX has
//! balanced delimiters, the required `SELECT` keyword, an axis clause, a `FROM`
//! clause with a cube name in bracket form, no empty member sets on any axis,
//! and that the row axis carries `NON EMPTY` when rows are emitted.
//!
//! No engine round-trip is required. This mirrors the DAX checker's depth and
//! fulfils the acceptance criteria in PRD-mqo-mdx-syntax-check.

/// Validate that an MDX string is structurally well-formed.
///
/// Checks performed (in order):
/// 1. Balanced `{` / `}`, `(` / `)`, `[` / `]` — never goes negative; sum
///    must be zero at end of input.
/// 2. Must contain the `SELECT` keyword (case-insensitive).
/// 3. Must contain at least one axis clause: `ON COLUMNS`, `ON ROWS`,
///    `ON 0`, or `ON 1` (case-insensitive).
/// 4. Must contain a `FROM [...]` clause — a `FROM` keyword (case-insensitive)
///    followed by at least one `[...]`-bracketed token.
/// 5. No empty member sets (`{}`) on any axis — an opening brace immediately
///    followed by optional whitespace and a closing brace is rejected.
/// 6. When an `ON ROWS` / `ON 1` axis is present, `NON EMPTY` must appear
///    before it (case-insensitive).
///
/// # Errors
///
/// Returns `Err(String)` with a human-readable description naming the specific
/// defect. Returns `Ok(())` on success.
///
/// # Totality
///
/// This function is total: any `&str` input (including the empty string and
/// arbitrary bytes) yields `Ok` or `Err` and never panics.
pub fn validate_mdx_syntax(mdx: &str) -> Result<(), String> {
    // ── Rule 1: balanced delimiters ──────────────────────────────────────────
    let mut brace_depth: i32 = 0;
    let mut paren_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;

    for ch in mdx.chars() {
        match ch {
            '{' => brace_depth += 1,
            '}' => {
                brace_depth -= 1;
                if brace_depth < 0 {
                    return Err("unmatched closing brace '}'".to_string());
                }
            }
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return Err("unmatched closing parenthesis ')'".to_string());
                }
            }
            '[' => bracket_depth += 1,
            ']' => {
                bracket_depth -= 1;
                if bracket_depth < 0 {
                    return Err("unmatched closing bracket ']'".to_string());
                }
            }
            _ => {}
        }
    }

    if brace_depth != 0 {
        return Err(format!(
            "unmatched braces: depth = {brace_depth} at end of input"
        ));
    }
    if paren_depth != 0 {
        return Err(format!(
            "unmatched parentheses: depth = {paren_depth} at end of input"
        ));
    }
    if bracket_depth != 0 {
        return Err(format!(
            "unmatched square brackets: depth = {bracket_depth} at end of input"
        ));
    }

    // ── Rule 2: SELECT keyword ───────────────────────────────────────────────
    let upper = mdx.to_uppercase();
    if !upper.contains("SELECT") {
        return Err("MDX is missing the SELECT keyword".to_string());
    }

    // ── Rule 3: at least one axis clause ────────────────────────────────────
    let has_axis = upper.contains("ON COLUMNS")
        || upper.contains("ON ROWS")
        || upper.contains("ON 0")
        || upper.contains("ON 1");
    if !has_axis {
        return Err(
            "MDX has no axis clause (expected ON COLUMNS, ON ROWS, ON 0, or ON 1)".to_string(),
        );
    }

    // ── Rule 4: FROM [cube] clause ───────────────────────────────────────────
    // Find "FROM" followed (allowing spaces/newlines) by a "["-prefixed token.
    if !has_from_with_bracket(&upper) {
        return Err(
            "MDX is missing a FROM clause with a cube name in bracket form (FROM [Cube])".to_string(),
        );
    }

    // ── Rule 5: no empty member sets ────────────────────────────────────────
    // An empty set is `{` followed by optional whitespace and `}`.
    if has_empty_brace_set(mdx) {
        return Err("MDX contains an empty member set '{}' on an axis".to_string());
    }

    // ── Rule 6: NON EMPTY on row axis ────────────────────────────────────────
    let has_row_axis = upper.contains("ON ROWS") || upper.contains("ON 1");
    if has_row_axis {
        // Find the position of the first "ON ROWS" or "ON 1" (whichever comes
        // first in the string).
        let row_pos = find_first_of(&upper, &["ON ROWS", "ON 1"]);
        // "NON EMPTY" must appear somewhere before the row axis keyword.
        let non_empty_pos = upper.find("NON EMPTY");
        match (non_empty_pos, row_pos) {
            (Some(ne_pos), Some(row_p)) if ne_pos < row_p => {}
            _ => {
                return Err(
                    "MDX row axis is present but NON EMPTY is missing before ON ROWS".to_string(),
                );
            }
        }
    }

    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Return `true` when `text` (already uppercased) contains `FROM` followed
/// by optional whitespace/newlines and then `[`.
fn has_from_with_bracket(upper: &str) -> bool {
    let mut rest = upper;
    while let Some(pos) = rest.find("FROM") {
        let after_from = &rest[pos + 4..]; // skip "FROM"
        let trimmed = after_from.trim_start_matches(|c: char| c.is_whitespace());
        if trimmed.starts_with('[') {
            return true;
        }
        // Advance past this occurrence and keep scanning.
        rest = &rest[pos + 4..];
    }
    false
}

/// Return `true` when `mdx` contains an empty brace set: `{` followed by
/// only whitespace and then `}`.
fn has_empty_brace_set(mdx: &str) -> bool {
    let mut chars = mdx.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '{' {
            // Walk forward from the opening brace.
            // If the first non-whitespace character is `}`, the set is empty.
            for (_, inner) in chars.by_ref() {
                if inner == '}' {
                    // No non-whitespace was seen before the closing brace.
                    return true;
                }
                if !inner.is_whitespace() {
                    // Non-whitespace content found — not an empty set.
                    // Consume until the matching `}` so we resume past this pair.
                    for (_, closing) in chars.by_ref() {
                        if closing == '}' {
                            break;
                        }
                    }
                    break;
                }
            }
        }
    }
    false
}

/// Return the byte position of the first occurrence of any of `needles` in
/// `haystack`, or `None` if none found.
fn find_first_of(haystack: &str, needles: &[&str]) -> Option<usize> {
    needles
        .iter()
        .filter_map(|n| haystack.find(n))
        .min()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        compile, BoundDimensionInput, BoundMeasureInput, BoundMqoInput, CalcGroupMemberInput,
    };
    use mqo_spec::{Filter, MeasureRef, Mqo};

    // ── Fixture helpers (mirrors tests.rs) ────────────────────────────────────

    fn minimal_mqo(model: &str) -> Mqo {
        Mqo {
            model: model.to_string(),
            measures: vec![MeasureRef {
                unique_name: "sales.revenue".to_string(),
            }],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: true,
        }
    }

    fn simple_measure(unique_name: &str) -> BoundMeasureInput {
        BoundMeasureInput {
            unique_name: unique_name.to_string(),
            is_calc: false,
            semi_additive: false,
            required_dimension: None,
            trigger_hierarchies: vec![],
            mdx_dependency_hierarchies: vec![],
        }
    }

    fn calc_measure(unique_name: &str, dep_hierarchies: Vec<&str>) -> BoundMeasureInput {
        BoundMeasureInput {
            unique_name: unique_name.to_string(),
            is_calc: true,
            semi_additive: false,
            required_dimension: None,
            trigger_hierarchies: vec![],
            mdx_dependency_hierarchies: dep_hierarchies.into_iter().map(String::from).collect(),
        }
    }

    fn semi_additive_measure(unique_name: &str, triggers: Vec<&str>) -> BoundMeasureInput {
        BoundMeasureInput {
            unique_name: unique_name.to_string(),
            is_calc: false,
            semi_additive: true,
            required_dimension: None,
            trigger_hierarchies: triggers.into_iter().map(String::from).collect(),
            mdx_dependency_hierarchies: vec![],
        }
    }

    fn bound_dim(unique_name: &str, hierarchy: &str) -> BoundDimensionInput {
        BoundDimensionInput {
            unique_name: unique_name.to_string(),
            hierarchy: hierarchy.to_string(),
        }
    }

    fn calc_group_member(
        calc_group: &str,
        member: &str,
        unique_name: &str,
        mdx: &str,
    ) -> CalcGroupMemberInput {
        CalcGroupMemberInput {
            calc_group: calc_group.to_string(),
            member: member.to_string(),
            unique_name: unique_name.to_string(),
            mdx: mdx.to_string(),
        }
    }

    // ── AC1: every existing compiled test case passes the check ───────────────

    #[test]
    fn sc_ac1_minimal_measure_only() {
        let bound = BoundMqoInput {
            mqo: minimal_mqo("sales"),
            measures: vec![simple_measure("sales.revenue")],
            dimensions: vec![],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "minimal measure-only MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_measure_with_one_dimension() {
        let bound = BoundMqoInput {
            mqo: minimal_mqo("tpcds"),
            measures: vec![simple_measure("sales.total_sales")],
            dimensions: vec![bound_dim("time.calendar.[Year]", "time.calendar")],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "measure+dim MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_measure_with_two_dimensions_crossjoin() {
        let bound = BoundMqoInput {
            mqo: minimal_mqo("tpcds"),
            measures: vec![simple_measure("sales.revenue")],
            dimensions: vec![
                bound_dim("time.calendar.[Year]", "time.calendar"),
                bound_dim("geo.country.[Country]", "geo.country"),
            ],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "crossjoin MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_three_part_cube_name() {
        let bound = BoundMqoInput {
            mqo: minimal_mqo("postgres.tpcds.tpcds_benchmark_model"),
            measures: vec![simple_measure("sales.revenue")],
            dimensions: vec![],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "three-part cube MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_member_filter_in_where() {
        let mut mqo = minimal_mqo("sales");
        mqo.filters.push(Filter::Member {
            hierarchy: "time.calendar".to_string(),
            members: vec!["2023".to_string()],
        });
        let bound = BoundMqoInput {
            mqo,
            measures: vec![simple_measure("sales.revenue")],
            dimensions: vec![],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "member-filter MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_two_measures_on_columns() {
        let bound = BoundMqoInput {
            mqo: minimal_mqo("sales"),
            measures: vec![
                simple_measure("sales.revenue"),
                simple_measure("sales.units_sold"),
            ],
            dimensions: vec![],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "two-measure MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_calc_measure_dep_hierarchy() {
        let bound = BoundMqoInput {
            mqo: minimal_mqo("sales"),
            measures: vec![calc_measure("sales.margin_pct", vec!["time.calendar"])],
            dimensions: vec![],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "calc-measure dep-hierarchy MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_calc_group_member() {
        let cgm_mdx = "Aggregate(PeriodsToDate([Time].[Calendar].[Year]))";
        let mut mqo = minimal_mqo("postgres.tpcds.tpcds_benchmark_model");
        mqo.filters.push(Filter::CalcGroupMember {
            calc_group: "Time Intelligence".to_string(),
            member: "YTD".to_string(),
        });
        let bound = BoundMqoInput {
            mqo,
            measures: vec![simple_measure("sales.total_sales")],
            dimensions: vec![bound_dim("time.calendar.[Year]", "time.calendar")],
            calc_group_members: vec![calc_group_member(
                "Time Intelligence",
                "YTD",
                "calc.ti.YTD",
                cgm_mdx,
            )],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "full query with calc-group MDX failed syntax check: {mdx}"
        );
    }

    #[test]
    fn sc_ac1_semi_additive_with_trigger() {
        let m = semi_additive_measure("sales.balance", vec!["time.calendar"]);
        let bound = BoundMqoInput {
            mqo: minimal_mqo("sales"),
            measures: vec![m],
            dimensions: vec![],
            calc_group_members: vec![],
        };
        let mdx = compile(&bound).expect("should compile");
        assert!(
            validate_mdx_syntax(&mdx).is_ok(),
            "semi-additive MDX failed syntax check: {mdx}"
        );
    }

    // ── AC2: Err cases ────────────────────────────────────────────────────────

    #[test]
    fn sc_ac2_unbalanced_open_brace() {
        let mdx = "SELECT\n  { [Measures].[Revenue] ON COLUMNS\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "unbalanced open brace must fail"
        );
    }

    #[test]
    fn sc_ac2_unbalanced_close_brace() {
        let mdx = "SELECT\n  { [Measures].[Revenue] }} ON COLUMNS\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "unbalanced close brace must fail"
        );
    }

    #[test]
    fn sc_ac2_unbalanced_paren() {
        let mdx =
            "SELECT\n  { CROSSJOIN([A].Members, [B].Members } ON COLUMNS\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "unbalanced paren must fail"
        );
    }

    #[test]
    fn sc_ac2_missing_from_clause() {
        let mdx = "SELECT\n  { [Measures].[Revenue] } ON COLUMNS\n[sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "missing FROM clause must fail"
        );
    }

    #[test]
    fn sc_ac2_from_without_bracket() {
        // "FROM sales" (no brackets) must be rejected (cube must be in [...] form).
        let mdx = "SELECT\n  { [Measures].[Revenue] } ON COLUMNS\nFROM sales";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "FROM without bracket cube must fail"
        );
    }

    #[test]
    fn sc_ac2_empty_set_on_rows() {
        let mdx =
            "SELECT\n  { [Measures].[Revenue] } ON COLUMNS,\n  NON EMPTY {} ON ROWS\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "empty set {{}} ON ROWS must fail"
        );
    }

    #[test]
    fn sc_ac2_empty_set_on_columns() {
        let mdx = "SELECT\n  {} ON COLUMNS\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "empty set {{}} ON COLUMNS must fail"
        );
    }

    #[test]
    fn sc_ac2_missing_select() {
        let mdx = "  { [Measures].[Revenue] } ON COLUMNS\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "missing SELECT must fail"
        );
    }

    #[test]
    fn sc_ac2_missing_axis_clause() {
        let mdx = "SELECT\n  { [Measures].[Revenue] }\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "missing axis clause must fail"
        );
    }

    #[test]
    fn sc_ac2_rows_without_non_empty() {
        // Row axis present but no NON EMPTY — must fail (R13 invariant).
        let mdx =
            "SELECT\n  { [Measures].[Revenue] } ON COLUMNS,\n  { [Calendar].[Year].Members } ON ROWS\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "rows without NON EMPTY must fail"
        );
    }

    // ── AC3 (fuzz): totality — no panics on arbitrary input ───────────────────

    #[test]
    fn sc_ac3_totality_empty_string() {
        // Must return Err (not panic) on empty input.
        let result = validate_mdx_syntax("");
        assert!(result.is_err(), "empty string must return Err");
    }

    #[test]
    fn sc_ac3_totality_random_ascii() {
        // Exhaustive ASCII sweep — each char alone must not panic.
        for b in 0_u8..=127_u8 {
            let s = String::from(b as char);
            let _ = validate_mdx_syntax(&s); // must not panic
        }
    }

    #[test]
    fn sc_ac3_totality_proptest_like_strings() {
        // A handful of adversarial strings that might trip naive parsers.
        let cases = [
            "{{{",
            "}}}",
            "(((",
            ")))",
            "[[[",
            "]]]",
            "SELECT FROM",
            "SELECT {} ON COLUMNS FROM []",
            "\0\x01\x02\x03",
            "SELECT\n  { [M].[X] } ON 0, NON EMPTY {} ON 1\nFROM [cube]",
        ];
        for s in cases {
            let _ = validate_mdx_syntax(s); // must not panic
        }
    }

    // ── Additional: ON 0 / ON 1 numbered axis forms are accepted ─────────────

    #[test]
    fn sc_numbered_axis_columns_only() {
        let mdx =
            "SELECT\n  { [Measures].[Revenue] } ON 0\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_ok(),
            "numbered ON 0 axis must be accepted: {mdx}"
        );
    }

    #[test]
    fn sc_numbered_axis_rows_with_non_empty() {
        let mdx =
            "SELECT\n  { [Measures].[Revenue] } ON 0,\n  NON EMPTY { [Calendar].[Year].Members } ON 1\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_ok(),
            "numbered ON 1 with NON EMPTY must be accepted: {mdx}"
        );
    }

    #[test]
    fn sc_numbered_axis_rows_without_non_empty() {
        let mdx =
            "SELECT\n  { [Measures].[Revenue] } ON 0,\n  { [Calendar].[Year].Members } ON 1\nFROM [sales]";
        assert!(
            validate_mdx_syntax(mdx).is_err(),
            "numbered ON 1 without NON EMPTY must fail"
        );
    }
}
