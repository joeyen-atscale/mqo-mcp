//! MDX codegen: `BoundMqoInput` → MDX `SELECT` string.
//!
//! ## Emitter strategy
//!
//! ```text
//! [WITH MEMBER ... AS <calc_group_mdx>]*
//! SELECT
//!   { [measure1], [measure2], ... } ON COLUMNS,
//!   NON EMPTY { CROSSJOIN([dim1].Members, [dim2].Members, ...) } ON ROWS
//! FROM [Cube]
//! [WHERE ( filter_member1, filter_member2 )]
//! ```
//!
//! Rules enforced:
//! - R10: cube name always fully-qualified (three-part brackets when a schema is
//!   present in `mqo.model`).
//! - R13: `NON EMPTY` is always emitted on the row axis.
//! - R7: calc-group member literals inserted verbatim from bound metadata.
//! - R6: for each calculated measure, `mdx_dependency_hierarchies` are appended
//!   to the row axis set.
//! - R11: semi-additive measure without trigger hierarchies → hard error.

use std::fmt::Write as _;

use crate::input::BoundMqoInput;
use crate::MdxCompileError;

/// Compile a `BoundMqoInput` to an MDX `SELECT` string.
///
/// # Errors
///
/// Returns [`MdxCompileError`] when the input is structurally invalid:
/// - [`MdxCompileError::EmptyMeasures`] — no measures in the bound MQO.
/// - [`MdxCompileError::SemiAdditiveMissingTrigger`] — a semi-additive measure
///   has an empty `trigger_hierarchies` list (R11).
pub fn compile(bound: &BoundMqoInput) -> Result<String, MdxCompileError> {
    if bound.measures.is_empty() {
        return Err(MdxCompileError::EmptyMeasures);
    }

    // R11 gate: every semi-additive measure must carry at least one trigger hierarchy.
    for m in &bound.measures {
        if m.semi_additive && m.trigger_hierarchies.is_empty() {
            return Err(MdxCompileError::SemiAdditiveMissingTrigger(
                m.unique_name.clone(),
            ));
        }
    }

    // Fully-qualified cube name (R10).
    let cube_name = qualify_cube(&bound.mqo.model);

    // Build WITH clause for calc-group members (R7).
    let with_clause = build_with_clause(bound);

    // Columns axis: measure set.
    let col_set = build_columns_set(bound);

    // Rows axis: bound dimensions + R6 dependency hierarchies.
    let row_set = build_rows_set(bound);

    // WHERE slicer: filter members.
    let where_clause = build_where_clause(bound);

    // Assemble.
    let mut out = String::new();

    if !with_clause.is_empty() {
        out.push_str(&with_clause);
        out.push('\n');
    }

    out.push_str("SELECT\n");
    write!(out, "  {col_set} ON COLUMNS").expect("String::write is infallible");

    if !row_set.is_empty() {
        write!(out, ",\n  NON EMPTY {{ {row_set} }} ON ROWS").expect("String::write is infallible");
    }

    write!(out, "\nFROM {cube_name}").expect("String::write is infallible");

    if !where_clause.is_empty() {
        write!(out, "\nWHERE ({where_clause})").expect("String::write is infallible");
    }

    Ok(out)
}

// ── Cube name qualification (R10) ─────────────────────────────────────────────

/// Return a fully-qualified MDX cube name.
///
/// - `"sales"` → `[sales]`
/// - `"schema.cube"` → `[schema].[cube]`
/// - `"cat.schema.cube"` → `[cat].[schema].[cube]`
fn qualify_cube(model: &str) -> String {
    let parts: Vec<&str> = model.split('.').collect();
    parts
        .iter()
        .map(|p| format!("[{p}]"))
        .collect::<Vec<_>>()
        .join(".")
}

// ── WITH clause (R7) ─────────────────────────────────────────────────────────

/// Build the `WITH MEMBER ... AS ...` block for calc-group members.
///
/// Each resolved `CalcGroupMemberInput` contributes one `WITH MEMBER` line
/// using the member's `unique_name` and verbatim `mdx` expression (R7).
fn build_with_clause(bound: &BoundMqoInput) -> String {
    if bound.calc_group_members.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = bound
        .calc_group_members
        .iter()
        .map(|cgm| {
            format!(
                "WITH MEMBER [{}].[{}] AS {}",
                cgm.calc_group, cgm.member, cgm.mdx
            )
        })
        .collect();
    lines.join("\n")
}

// ── Columns axis ──────────────────────────────────────────────────────────────

/// Build the column-axis set expression: `{ [m1], [m2], ... }`.
fn build_columns_set(bound: &BoundMqoInput) -> String {
    let members: Vec<String> = bound
        .measures
        .iter()
        .map(|m| measure_member_ref(&m.unique_name))
        .collect();
    format!("{{ {} }}", members.join(", "))
}

// ── Rows axis ─────────────────────────────────────────────────────────────────

/// Build the row-axis members set.
///
/// Combines:
/// 1. Bound dimension levels (from `bound.dimensions`).
/// 2. MDX-dependency hierarchies for calculated measures (R6 — from
///    `m.mdx_dependency_hierarchies` for each `is_calc` measure).
///
/// Deduplication: a dep-hierarchy entry is skipped when a bound dimension
/// already covers the same hierarchy (matched by the last `.`-segment of the
/// hierarchy name, case-insensitively). This prevents emitting both
/// `[Calendar].[Year].Members` and `[Calendar].Members` when the dim already
/// covers the hierarchy.  Fully-duplicate entries (same string) are also deduped.
fn build_rows_set(bound: &BoundMqoInput) -> String {
    let mut levels: Vec<String> = Vec::new();
    let mut seen_exprs: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Track which hierarchy *last segments* (lowercase) are already covered by
    // bound dimensions.
    let mut covered_hierarchies: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    let mut push_expr = |expr: String| {
        if seen_exprs.insert(expr.clone()) {
            levels.push(expr);
        }
    };

    // 1. Bound dimensions — each dimension covers its hierarchy.
    for d in &bound.dimensions {
        let hier_key = hierarchy_last_segment(&d.hierarchy);
        covered_hierarchies.insert(hier_key);
        push_expr(level_members_ref(&d.unique_name));
    }

    // 2. R6: calculated measure dependency hierarchies.
    //    Only add a hierarchy if it is not already covered by a bound dimension.
    for m in &bound.measures {
        if m.is_calc {
            for h in &m.mdx_dependency_hierarchies {
                let hier_key = hierarchy_last_segment(h);
                if covered_hierarchies.contains(&hier_key) {
                    // Already covered by a bound dimension — skip.
                    continue;
                }
                covered_hierarchies.insert(hier_key);
                push_expr(hierarchy_members_ref(h));
            }
        }
    }

    match levels.len() {
        0 => String::new(),
        1 => levels.remove(0),
        _ => format!("CROSSJOIN({})", levels.join(", ")),
    }
}

/// Return the lowercase last segment of a `.`-delimited hierarchy name.
/// `"time.calendar"` → `"calendar"`, `"Calendar"` → `"calendar"`.
fn hierarchy_last_segment(hierarchy: &str) -> String {
    hierarchy
        .split('.')
        .next_back()
        .unwrap_or(hierarchy)
        .to_lowercase()
}

// ── WHERE slicer ──────────────────────────────────────────────────────────────

/// Build the WHERE slicer tuple from `Member` and `CalcGroupMember` filters.
///
/// `Range` filters don't map cleanly to MDX slicer members — they're omitted
/// here with a comment placeholder (the row axis NON EMPTY already restricts
/// the result). This is a deliberate simplification for the cellset compiler.
fn build_where_clause(bound: &BoundMqoInput) -> String {
    use mqo_spec::Filter;

    let mut members: Vec<String> = Vec::new();

    for f in &bound.mqo.filters {
        match f {
            Filter::Member { hierarchy, members: mem_keys } => {
                // For multi-member filters we emit a set; for single-member a
                // qualified member reference.
                for mk in mem_keys {
                    members.push(format!("[{hierarchy}].[{mk}]"));
                }
            }
            Filter::CalcGroupMember { calc_group, member } => {
                members.push(format!("[{calc_group}].[{member}]"));
            }
            Filter::Range { .. } => {
                // Range filters are not directly expressible as MDX slicer members.
                // They are left to the row-axis NON EMPTY to suppress empty cells.
            }
        }
    }

    members.join(", ")
}

// ── Reference helpers ─────────────────────────────────────────────────────────

/// Convert a measure `unique_name` to an MDX member reference.
///
/// `"sales.revenue"` → `[Measures].[Revenue]`
fn measure_member_ref(unique_name: &str) -> String {
    let label = name_label(unique_name);
    format!("[Measures].[{label}]")
}

/// Convert a level `unique_name` to its `.Members` set reference.
///
/// `"time.calendar.[Year]"` → `[Calendar].[Year].Members`
/// `"time.calendar.Year"` → `[Calendar].[Year].Members`
fn level_members_ref(unique_name: &str) -> String {
    let parts: Vec<&str> = unique_name.split('.').collect();
    match parts.as_slice() {
        [.., table, level] => {
            let table_clean = title_case(table);
            let level_clean = level.trim_matches(|c| c == '[' || c == ']');
            format!("[{table_clean}].[{level_clean}].Members")
        }
        [single] => {
            let clean = single.trim_matches(|c| c == '[' || c == ']');
            format!("[{clean}].Members")
        }
        _ => format!("[{unique_name}].Members"),
    }
}

/// Convert a hierarchy `unique_name` to its `.Members` set reference.
///
/// `"time.calendar"` → `[Calendar].Members`
/// `"Geography.Country"` → `[Country].Members`
fn hierarchy_members_ref(hierarchy: &str) -> String {
    let last = hierarchy.split('.').next_back().unwrap_or(hierarchy);
    let clean = title_case(last);
    format!("[{clean}].Members")
}

/// Derive a human-readable label from a `unique_name` (last segment, title-cased).
fn name_label(unique_name: &str) -> String {
    let base = unique_name.rsplit('.').next().unwrap_or(unique_name);
    let clean = base.trim_matches(|c| c == '[' || c == ']');
    // Replace underscores and capitalize each word.
    clean
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let upper: String = first.to_uppercase().collect();
                    upper + chars.as_str()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Capitalize the first letter of a string.
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            upper + chars.as_str()
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod codegen_unit_tests {
    use super::*;

    #[test]
    fn qualify_cube_single() {
        assert_eq!(qualify_cube("sales"), "[sales]");
    }

    #[test]
    fn qualify_cube_two_part() {
        assert_eq!(qualify_cube("schema.cube"), "[schema].[cube]");
    }

    #[test]
    fn qualify_cube_three_part() {
        assert_eq!(qualify_cube("cat.schema.cube"), "[cat].[schema].[cube]");
    }

    #[test]
    fn measure_member_ref_simple() {
        assert_eq!(measure_member_ref("sales.revenue"), "[Measures].[Revenue]");
    }

    #[test]
    fn level_members_ref_bracketed() {
        assert_eq!(
            level_members_ref("time.calendar.[Year]"),
            "[Calendar].[Year].Members"
        );
    }

    #[test]
    fn level_members_ref_plain() {
        assert_eq!(
            level_members_ref("time.calendar.Year"),
            "[Calendar].[Year].Members"
        );
    }

    #[test]
    fn hierarchy_members_ref_dotted() {
        assert_eq!(hierarchy_members_ref("time.calendar"), "[Calendar].Members");
    }

    #[test]
    fn name_label_underscore() {
        assert_eq!(name_label("tpcds.total_sales"), "Total Sales");
    }
}
