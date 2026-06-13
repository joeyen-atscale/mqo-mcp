//! DAX codegen: `BoundMqoInput` → DAX `EVALUATE` string.
//!
//! ## Emitter strategy
//!
//! | Query shape | DAX pattern |
//! |---|---|
//! | Measures only (no dims) | `EVALUATE ROW("m", [Measure])` |
//! | Measures + dims | `EVALUATE SUMMARIZECOLUMNS(dims..., filters..., "m", [Measure])` |
//! | limit present | wrapped in `TOPN(n, ...)` |
//! | order present | appended `ORDER BY` clause |
//! | calc-group member | `KEEPFILTERS(FILTER(ALL(CalcGroup[Column]), CalcGroup[Column] = "member"))` |
//! | Member filter | `KEEPFILTERS(FILTER(ALL(Hierarchy[Level]), Hierarchy[Level] IN {...}))` |
//! | Range filter | `KEEPFILTERS(FILTER(ALL(Level[Level]), Level[Level] >= lo && Level[Level] <= hi))` |
//!
//! ## Time-intelligence mapping
//!
//! | [`TimeIntel`] variant | DAX pattern |
//! |---|---|
//! | `YoY` | `CALCULATE([M], SAMEPERIODLASTYEAR(<date_col>))` |
//! | `PriorPeriod` | `CALCULATE([M], DATEADD(<date_col>, -1, DAY))` |
//! | `ToDate { grain: Year }` | `CALCULATE([M], DATESYTD(<date_col>))` |
//! | `ToDate { grain: Quarter }` | `CALCULATE([M], DATESQTD(<date_col>))` |
//! | `ToDate { grain: Month }` | `CALCULATE([M], DATESMTD(<date_col>))` |
//! | `ToDate { grain: _ }` | `CALCULATE([M], DATESYTD(<date_col>))` (year) |
//! | `RunningTotal` | `CALCULATE([M], DATESINTORANGE(<date_col>, MIN(<date_col>), MAX(<date_col>)))` |
//! | `Share { of_level }` | `DIVIDE([M], CALCULATE([M], ALL(of_level)))` |
//! | `Rank { by, top_n }` | becomes `TOPN(n, ..., [by], DESC)` |
//!
//! `<date_col>` is resolved via [`DaxCatalogContext::date_level_unique_name`] when
//! a context is present; falls back to the literal `DateTable[Date]` placeholder
//! when no context is supplied (byte-identical to pre-grounding behaviour).
//!
//! ## Capability guard
//!
//! `YoY`, `PriorPeriod`, `ToDate`, and `RunningTotal` require a "Mark as Date
//! Table" designation in the tabular model. When a [`DaxCatalogContext`] is
//! supplied and `ctx.has_date_table` is `false` (the default for `AtScale` XMLA),
//! the compiler returns
//! [`DaxCompileError::UnsupportedTimeIntelligence`] **before** emitting any DAX
//! string — the engine never sees an unsupported op. `Share` and `Rank` are not
//! affected by this guard.

use std::fmt::Write as _;

use crate::catalog_context::DaxCatalogContext;
use crate::input::BoundMqoInput;
use crate::DaxCompileError;
use mqo_spec::{Filter, Grain, SortDirection, TimeIntel};

/// Compile a `BoundMqoInput` to a DAX `EVALUATE` string.
///
/// This is a convenience wrapper around [`compile_grounded`] with no catalog
/// context (emits binder `unique_name` strings as-is, byte-identical to the
/// pre-grounding behaviour).
///
/// # Errors
///
/// Returns [`DaxCompileError`] when the input is structurally invalid.
pub fn compile(bound: &BoundMqoInput) -> Result<String, DaxCompileError> {
    compile_grounded(bound, None)
}

/// Compile a `BoundMqoInput` to a DAX `EVALUATE` string, optionally grounding
/// column references against a [`DaxCatalogContext`].
///
/// When `ctx` is `Some`:
/// - Dimension levels emit `'TableName'[Display Label]`
/// - Measures emit `[Display Label]`
/// - Unknown `unique_name`s fall back to their raw value and are annotated with
///   an inline `/* ungrounded: <unique_name> */` comment so queries can be
///   audited.
///
/// When `ctx` is `None` the output is byte-identical to [`compile`].
///
/// # Errors
///
/// Returns [`DaxCompileError`] when the input is structurally invalid.
pub fn compile_grounded(
    bound: &BoundMqoInput,
    ctx: Option<&DaxCatalogContext>,
) -> Result<String, DaxCompileError> {
    if bound.measures.is_empty() {
        return Err(DaxCompileError::EmptyMeasures);
    }

    // Build the measure expression list, applying time-intel wrappers.
    let measure_pairs = build_measure_pairs(bound, ctx)?;

    // Build groupBy columns list (from bound.dimensions).
    let group_by_cols: Vec<String> = bound
        .dimensions
        .iter()
        .map(|d| level_col_ref_ctx(&d.unique_name, ctx))
        .collect();

    // Build filter expressions. The query's dimension level unique_names are
    // passed so an ambiguous Member value (e.g. "M" in both Gender and Marital
    // Status) binds to the level the query groups by.
    let dim_levels: Vec<String> = bound.dimensions.iter().map(|d| d.unique_name.clone()).collect();
    let mut filter_exprs: Vec<String> = Vec::new();
    for f in &bound.mqo.filters {
        filter_exprs.push(filter_expr_ctx(f, ctx, &dim_levels)?);
    }
    // Calc-group member filters (from bound.calc_group_members).
    for cgm in &bound.calc_group_members {
        filter_exprs.push(calc_group_filter(&cgm.calc_group, &cgm.member));
    }

    // Check for Rank top_n — it imposes its own TOPN wrapping of the inner table.
    let rank_topn = rank_top_n_from_time_intel(&bound.mqo.time_intelligence);

    // Assemble the inner table expression.
    let inner = if group_by_cols.is_empty() && filter_exprs.is_empty() {
        // Simple measure-only: ROW form.
        // ROW("name1", expr1, "name2", expr2, ...)
        let pairs: Vec<String> = measure_pairs
            .iter()
            .flat_map(|(name, expr)| [format!("\"{name}\""), expr.clone()])
            .collect();
        format!("ROW({})", pairs.join(", "))
    } else {
        // SUMMARIZECOLUMNS(col1, col2, ..., filter1, filter2, ..., "name1", expr1, ...)
        let mut args: Vec<String> = Vec::new();
        args.extend(group_by_cols);
        for f in filter_exprs {
            args.push(f);
        }
        for (name, expr) in &measure_pairs {
            args.push(format!("\"{name}\""));
            args.push(expr.clone());
        }
        format!("SUMMARIZECOLUMNS({})", args.join(", "))
    };

    // Apply Rank TOPN wrapping if a Rank time-intel is present.
    let inner = if let Some((n, by_measure)) = rank_topn {
        let by_ref = measure_dax_ref_ctx(&by_measure, ctx);
        format!("TOPN({n}, {inner}, {by_ref}, DESC)")
    } else if let Some(limit) = bound.mqo.limit {
        // Apply limit TOPN.
        // TOPN wraps the inner table; we need a sort col for TOPN.
        // Use the first measure as the default sort col.
        let first_measure_ref = measure_dax_ref_ctx(&bound.measures[0].unique_name, ctx);
        format!("TOPN({limit}, {inner}, {first_measure_ref}, DESC)")
    } else {
        inner
    };

    let mut dax = format!("EVALUATE\n{inner}");

    // Append ORDER BY.
    if let Some(order_keys) = &bound.mqo.order {
        if !order_keys.is_empty() {
            let order_parts: Vec<String> = order_keys
                .iter()
                .map(|ok| {
                    let dir = match ok.direction {
                        SortDirection::Asc => "ASC",
                        SortDirection::Desc => "DESC",
                    };
                    format!("{} {dir}", measure_dax_ref_ctx(&ok.key, ctx))
                })
                .collect();
            write!(dax, "\nORDER BY {}", order_parts.join(", ")).expect("String write is infallible");
        }
    }

    Ok(dax)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Resolve the date column reference for time-intelligence function calls.
///
/// Resolution order (first match wins):
/// 1. `ctx.date_level_unique_name` looked up in `ctx.labels` →
///    `'TableName'[Display Label]` (fully grounded)
/// 2. `ctx.date_level_unique_name` present but not in labels →
///    `level_col_ref` of that name with an `/* ungrounded date: … */` annotation
/// 3. No context, or context has no `date_level_unique_name` →
///    fallback placeholder `DateTable[Date]` (byte-identical to pre-change)
fn resolve_date_col_ref(ctx: Option<&DaxCatalogContext>) -> String {
    let Some(c) = ctx else {
        return "DateTable[Date]".to_string();
    };
    let Some(ref date_unique) = c.date_level_unique_name else {
        return "DateTable[Date]".to_string();
    };
    if let Some(label) = c.labels.get(date_unique.as_str()) {
        return format!("'{}'[{label}]", c.table_name);
    }
    // unique_name present but not in labels — fall back to heuristic + annotation
    let fallback = level_col_ref(date_unique);
    format!("{fallback} /* ungrounded date: {date_unique} */")
}

/// Pre-dispatch capability guard for time-intel ops that require Mark-as-Date-Table.
///
/// Returns `Err(DaxCompileError::UnsupportedTimeIntelligence)` when `ctx` is
/// `Some` and `ctx.has_date_table` is `false` (the conservative default for
/// `AtScale` XMLA).
///
/// Returns `Ok(())` when:
/// - `ctx` is `None` (legacy/no-context path — guard is skipped for backward
///   compatibility so uncontexted callers are unaffected).
/// - `ctx.has_date_table` is `true` (engine explicitly declared capable).
fn check_date_table_support(op_name: &str, ctx: Option<&DaxCatalogContext>) -> Result<(), DaxCompileError> {
    let Some(c) = ctx else {
        // No context — skip the guard (backward-compat path).
        return Ok(());
    };
    if c.has_date_table {
        return Ok(());
    }
    Err(DaxCompileError::UnsupportedTimeIntelligence {
        op: op_name.to_string(),
        reason: "requires Mark-as-Date-Table designation not provided by `AtScale` XMLA"
            .to_string(),
    })
}

/// Return a human-readable grain name for error messages.
fn grain_name(grain: &Grain) -> &'static str {
    match grain {
        Grain::Year => "Year",
        Grain::Quarter => "Quarter",
        Grain::Month => "Month",
        Grain::Week => "Week",
        Grain::Day => "Day",
    }
}

/// Build `(label, dax_expr)` pairs for each measure, with time-intel wrappers.
fn build_measure_pairs(
    bound: &BoundMqoInput,
    ctx: Option<&DaxCatalogContext>,
) -> Result<Vec<(String, String)>, DaxCompileError> {
    // First: check that every time-intel Rank/Share measure ref is in the query.
    for ti in &bound.mqo.time_intelligence {
        match ti {
            TimeIntel::Rank { by, .. } => {
                if !bound.measures.iter().any(|m| &m.unique_name == by) {
                    return Err(DaxCompileError::UnknownTimeIntelMeasure(by.clone()));
                }
            }
            TimeIntel::Share { of_level } => {
                if of_level.is_empty() {
                    return Err(DaxCompileError::EmptyShareLevel);
                }
            }
            _ => {}
        }
    }

    // Pre-dispatch capability guard: check all time-intel ops that require
    // Mark-as-Date-Table BEFORE emitting any DAX strings.
    // `Share` and `Rank` do not require Mark-as-Date-Table; everything else does.
    for ti in &bound.mqo.time_intelligence {
        match ti {
            TimeIntel::YoY => check_date_table_support("YoY", ctx)?,
            TimeIntel::PriorPeriod => check_date_table_support("PriorPeriod", ctx)?,
            TimeIntel::ToDate { grain } => {
                let op_name = format!("ToDate({})", grain_name(grain));
                check_date_table_support(&op_name, ctx)?;
            }
            TimeIntel::RunningTotal => check_date_table_support("RunningTotal", ctx)?,
            // Share and Rank do not use SAMEPERIODLASTYEAR / DATES* — no guard needed.
            TimeIntel::Share { .. } | TimeIntel::Rank { .. } => {}
        }
    }

    // Resolve the date column reference once (grounded or fallback placeholder).
    let date_col = resolve_date_col_ref(ctx);

    let mut pairs: Vec<(String, String)> = Vec::new();

    for m in &bound.measures {
        let base_ref = measure_dax_ref_ctx(&m.unique_name, ctx);
        let label = measure_label_ctx(&m.unique_name, ctx);

        // Apply each time-intel wrapper in order.
        let mut current_expr = base_ref.clone();
        let mut current_label = label.clone();

        for ti in &bound.mqo.time_intelligence {
            match ti {
                TimeIntel::YoY => {
                    current_label = format!("{current_label} YoY");
                    current_expr = format!(
                        "CALCULATE({current_expr}, SAMEPERIODLASTYEAR({date_col}))"
                    );
                }
                TimeIntel::PriorPeriod => {
                    current_label = format!("{current_label} PriorPeriod");
                    current_expr = format!(
                        "CALCULATE({current_expr}, DATEADD({date_col}, -1, DAY))"
                    );
                }
                TimeIntel::ToDate { grain } => {
                    let (suffix, dax_fn) = to_date_fn(grain);
                    current_label = format!("{current_label} {suffix}");
                    current_expr =
                        format!("CALCULATE({current_expr}, {dax_fn}({date_col}))");
                }
                TimeIntel::RunningTotal => {
                    current_label = format!("{current_label} RunningTotal");
                    current_expr = format!(
                        "CALCULATE({current_expr}, DATESINTORANGE({date_col}, MIN({date_col}), MAX({date_col})))"
                    );
                }
                TimeIntel::Share { of_level } => {
                    current_label = format!("{current_label} Share");
                    let level_ref = level_col_ref_ctx(of_level, ctx);
                    current_expr = format!(
                        "DIVIDE({current_expr}, CALCULATE({base_ref}, ALL({level_ref})))"
                    );
                }
                TimeIntel::Rank { .. } => {
                    // Rank is handled at the table level (TOPN wrapper), not per-measure.
                    // We still need to emit the measure itself.
                    current_label = format!("{current_label} Rank");
                }
            }
        }

        pairs.push((current_label, current_expr));
    }

    Ok(pairs)
}

/// Extract `(n, by_measure_unique_name)` from `Rank { by, top_n }` time-intel if present.
fn rank_top_n_from_time_intel(ti: &[TimeIntel]) -> Option<(u32, String)> {
    for op in ti {
        if let TimeIntel::Rank { by, top_n } = op {
            let n = top_n.unwrap_or(10);
            return Some((n, by.clone()));
        }
    }
    None
}


/// Derive a human-readable label from a `unique_name`.
/// `"sales.revenue"` → `"Revenue"`, `"tpcds.total_sales"` → `"Total Sales"`.
fn measure_label(unique_name: &str) -> String {
    let base = unique_name.rsplit('.').next().unwrap_or(unique_name);
    // Replace underscores and capitalize each word.
    base.split('_')
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

/// Convert a dimension-level `unique_name` like `"time.calendar.[Year]"` to
/// a DAX column reference `Calendar[Year]`.
///
/// Supports two forms:
/// - `"time.calendar.[Year]"` → `Calendar[Year]`
/// - `"time.calendar.Year"` → `Calendar[Year]`
/// - `"Year"` (bare) → `Calendar[Year]` (fallback: uses last segment as both)
fn level_col_ref(unique_name: &str) -> String {
    let parts: Vec<&str> = unique_name.split('.').collect();
    match parts.as_slice() {
        [.., table, level] => {
            let table_clean = title_case(table);
            let level_clean = level.trim_matches(|c| c == '[' || c == ']');
            format!("{table_clean}[{level_clean}]")
        }
        [single] => {
            let level_clean = single.trim_matches(|c| c == '[' || c == ']');
            format!("{level_clean}[{level_clean}]")
        }
        _ => unique_name.to_string(),
    }
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

/// Build the DAX label-suffix and function name for `ToDate { grain }`.
fn to_date_fn(grain: &Grain) -> (&'static str, &'static str) {
    match grain {
        Grain::Quarter => ("QTD", "DATESQTD"),
        Grain::Month => ("MTD", "DATESMTD"),
        // Year, Week, Day all fall back to DATESYTD (year-to-date is the natural default)
        Grain::Year | Grain::Week | Grain::Day => ("YTD", "DATESYTD"),
    }
}

/// Build a KEEPFILTERS(FILTER(ALL(...), ...)) expression for a calc-group member.
///
/// The calc-group column is modeled as `CalcGroupName[CalcGroupName]`.
fn calc_group_filter(calc_group: &str, member: &str) -> String {
    // Normalize: remove spaces for the column reference.
    let col_name = calc_group.replace(' ', "");
    format!(
        "KEEPFILTERS(FILTER(ALL({col_name}[{col_name}]), {col_name}[{col_name}] = \"{member}\"))"
    )
}


/// Build a filter expression for a [`Filter`] variant, with optional catalog grounding.
///
/// # Errors
///
/// - [`DaxCompileError::EmptyMemberFilter`] when a `Member` filter's `members` list
///   is empty (an empty DAX `IN {}` set is invalid).
/// - [`DaxCompileError::UngroundedMemberFilter`] when a `Member` filter's hierarchy
///   cannot be resolved to a real level column — either because no
///   `DaxCatalogContext` was supplied or because the context carries no level
///   entries for that hierarchy.
/// - [`DaxCompileError::UngroundedRangeFilter`] when a `Range` filter's `level`
///   cannot be resolved to a real column reference — a `DaxCatalogContext` is
///   present but `level` is neither a known unique-name nor a recognized display
///   label.
fn filter_expr_ctx(
    filter: &Filter,
    ctx: Option<&DaxCatalogContext>,
    dim_levels: &[String],
) -> Result<String, DaxCompileError> {
    match filter {
        Filter::Member { hierarchy, members } => {
            // Guard: empty members list is never valid DAX (IN {} is a syntax error).
            if members.is_empty() {
                return Err(DaxCompileError::EmptyMemberFilter {
                    hierarchy: hierarchy.clone(),
                });
            }

            // Resolve the hierarchy to a real level unique_name via the catalog.
            // Without a grounded column reference the engine rejects the query
            // with "Unknown column [<hierarchy>]", so we must fail loud here
            // rather than emitting Hierarchy[Hierarchy].
            // Domain-aware grounding: bind to the level whose enumerated domain
            // contains the member value(s); fall back to the hierarchy's first
            // level only when no domain match is found (PRD-mqo-member-filter-
            // domain-grounding). This fixes the silent mis-binding where e.g.
            // customer_demographics="M" bound to [Credit Rating] and returned 0 rows.
            let level_unique_name = ctx
                .and_then(|c| {
                    c.resolve_member_level(hierarchy, members, dim_levels)
                        .or_else(|| c.resolve_hierarchy_first_level(hierarchy))
                })
                .ok_or_else(|| DaxCompileError::UngroundedMemberFilter {
                    hierarchy: hierarchy.clone(),
                    members: members.join(", "),
                })?;

            let col = level_col_ref_ctx(level_unique_name, ctx);
            let member_list: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
            Ok(format!(
                "KEEPFILTERS(FILTER(ALL({col}), {col} IN {{{}}})) /* grounded-from-member */",
                member_list.join(", ")
            ))
        }
        Filter::Range { level, lo, hi } => {
            // Resolve the level to a grounded column reference.
            //
            // When a DaxCatalogContext is present we must never emit
            // Level[Level] — an unqualified name whose table doesn't exist
            // causes an XMLA 500.  Accept two forms:
            //   1. A known unique-name (key in ctx.labels) — direct.
            //   2. A bare display label — reverse-lookup to unique-name.
            // Anything else with a context → fail loud so the caller gets
            // an actionable error instead of an opaque engine 500.
            // No context → fall through to heuristic (backward compat).
            let col = if let Some(c) = ctx {
                if c.labels.contains_key(level.as_str()) {
                    level_col_ref_ctx(level, ctx)
                } else if let Some(unique_name) = c.resolve_level_label(level) {
                    level_col_ref_ctx(unique_name, ctx)
                } else {
                    return Err(DaxCompileError::UngroundedRangeFilter {
                        level: level.clone(),
                    });
                }
            } else {
                level_col_ref_ctx(level, ctx)
            };
            Ok(format!(
                "KEEPFILTERS(FILTER(ALL({col}), {col} >= {lo} && {col} <= {hi}))"
            ))
        }
        Filter::CalcGroupMember { calc_group, member } => {
            Ok(calc_group_filter(calc_group, member))
        }
    }
}

// ── Catalog-context-aware name resolvers ──────────────────────────────────────

/// Return the display label for a measure `unique_name`.
///
/// When `ctx` is `Some` and the `unique_name` is found, returns the catalog label.
/// Falls back to the heuristic `measure_label` derivation when ctx is absent or
/// the name is unknown.
fn measure_label_ctx(unique_name: &str, ctx: Option<&DaxCatalogContext>) -> String {
    if let Some(c) = ctx {
        if let Some(label) = c.labels.get(unique_name) {
            return label.clone();
        }
    }
    measure_label(unique_name)
}

/// Emit a DAX measure reference `[Display Label]`, grounded when a catalog context
/// is present and the `unique_name` is known.
///
/// Unknown names fall back to the heuristic derivation (no `/* ungrounded */`
/// comment on measure refs since they lack table qualification anyway).
fn measure_dax_ref_ctx(unique_name: &str, ctx: Option<&DaxCatalogContext>) -> String {
    let label = measure_label_ctx(unique_name, ctx);
    format!("[{label}]")
}

/// Emit a DAX column reference for a dimension level.
///
/// - With context and a known `unique_name`: `'TableName'[Display Label]`
/// - With context but unknown `unique_name`: `<fallback_ref> /* ungrounded: <unique_name> */`
/// - Without context: delegates to `level_col_ref` (existing behaviour)
fn level_col_ref_ctx(unique_name: &str, ctx: Option<&DaxCatalogContext>) -> String {
    let Some(c) = ctx else {
        return level_col_ref(unique_name);
    };

    if let Some(label) = c.labels.get(unique_name) {
        // Apostrophe-quoted table name for safety (handles spaces/hyphens).
        return format!("'{}'[{label}]", c.table_name);
    }

    // Unknown unique_name — fall back to heuristic and annotate.
    let fallback = level_col_ref(unique_name);
    format!("{fallback} /* ungrounded: {unique_name} */")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_label_simple() {
        assert_eq!(measure_label("sales.revenue"), "Revenue");
    }

    #[test]
    fn measure_label_underscore() {
        assert_eq!(measure_label("tpcds.total_sales"), "Total Sales");
    }

    #[test]
    fn level_col_ref_bracketed() {
        assert_eq!(level_col_ref("time.calendar.[Year]"), "Calendar[Year]");
    }

    #[test]
    fn level_col_ref_plain() {
        assert_eq!(level_col_ref("time.calendar.Year"), "Calendar[Year]");
    }

    use crate::catalog_context::DaxCatalogContext;
    use crate::input::{BoundDimensionInput, BoundMeasureInput, BoundMqoInput};
    use mqo_spec::Mqo;

    /// Kill mutant: `delete match arm [single]` in `level_col_ref`.
    /// A bare name (no dots) must produce `Name[Name]`, not fall through.
    #[test]
    fn level_col_ref_bare() {
        assert_eq!(level_col_ref("Year"), "Year[Year]");
    }

    /// Kill mutant: `replace == with !=` or `replace || with &&` in `level_col_ref`
    /// bracket trimming. A bracketed single segment must strip the brackets.
    #[test]
    fn level_col_ref_bare_bracketed() {
        assert_eq!(level_col_ref("[Year]"), "Year[Year]");
    }

    // ── Grounding tests ───────────────────────────────────────────────────────

    fn fixture_ctx() -> DaxCatalogContext {
        let json = r#"{
            "catalog": "tpcds_benchmark_model",
            "columns": [
                {
                    "unique_name": "inventory_date_dimension.calendar.[Inventory Calendar Month]",
                    "label": "Inventory Calendar Month",
                    "kind": "level"
                },
                {
                    "unique_name": "tpcds.total_store_sales",
                    "label": "Total Store Sales",
                    "kind": "measure"
                }
            ]
        }"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    fn minimal_bound(measure_unique: &str, dim_unique: Option<&str>) -> BoundMqoInput {
        BoundMqoInput {
            mqo: serde_json::from_str(
                r#"{"model":"test","measures":[],"dimensions":[],"filters":[],"time_intelligence":[],"non_empty":false}"#,
            )
            .unwrap_or_else(|_| Mqo {
                model: "test".to_string(),
                measures: vec![],
                dimensions: vec![],
                filters: vec![],
                limit: None,
                order: None,
                time_intelligence: vec![],
                non_empty: false,
            }),
            measures: vec![BoundMeasureInput {
                unique_name: measure_unique.to_string(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
                trigger_hierarchies: vec![],
            }],
            dimensions: dim_unique
                .map(|u| {
                    vec![BoundDimensionInput {
                        unique_name: u.to_string(),
                        hierarchy: u.to_string(),
                    }]
                })
                .unwrap_or_default(),
            calc_group_members: vec![],
        }
    }

    /// `compile(bound)` with no catalog must be byte-identical to `compile_grounded(bound, None)`.
    #[test]
    fn compile_no_ctx_identical_to_compile() {
        let bound = minimal_bound("tpcds.total_store_sales", None);
        let via_compile = compile(&bound).unwrap();
        let via_grounded = compile_grounded(&bound, None).unwrap();
        assert_eq!(via_compile, via_grounded);
    }

    /// With a catalog context, a known dimension level emits `'TableName'[Display Label]`.
    #[test]
    fn grounded_dimension_level_emits_table_label() {
        let ctx = fixture_ctx();
        let bound = minimal_bound(
            "tpcds.total_store_sales",
            Some("inventory_date_dimension.calendar.[Inventory Calendar Month]"),
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            dax.contains("'tpcds_benchmark_model'[Inventory Calendar Month]"),
            "expected grounded level ref, got: {dax}"
        );
    }

    /// With a catalog context, a known measure emits [Display Label].
    #[test]
    fn grounded_measure_emits_display_label() {
        let ctx = fixture_ctx();
        let bound = minimal_bound("tpcds.total_store_sales", None);
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            dax.contains("[Total Store Sales]"),
            "expected grounded measure ref, got: {dax}"
        );
    }

    /// Unknown `unique_name` falls back gracefully — no panic, annotated with comment.
    #[test]
    fn grounded_unknown_level_falls_back_with_comment() {
        let ctx = fixture_ctx();
        let bound = minimal_bound("tpcds.total_store_sales", Some("no.such.level"));
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        // Should contain ungrounded annotation for the dim ref.
        assert!(
            dax.contains("/* ungrounded: no.such.level */"),
            "expected ungrounded annotation, got: {dax}"
        );
        // Must not panic — query should still be valid (SUMMARIZECOLUMNS present).
        // Syntax check: SUMMARIZECOLUMNS is present despite the comment.
        assert!(
            crate::syntax_check::validate_dax_syntax(&dax).is_ok(),
            "syntax check failed on ungrounded output: {dax}"
        );
    }

    // ── Range-filter grounding tests ──────────────────────────────────────────

    /// Range filter with a bare display label → resolves via reverse lookup.
    ///
    /// fixture_ctx() has: unique_name "inventory_date_dimension.calendar.[Inventory Calendar Month]"
    /// → label "Inventory Calendar Month", table "tpcds_benchmark_model".
    #[test]
    fn range_filter_bare_label_resolves() {
        let ctx = fixture_ctx();
        let filter = Filter::Range {
            level: "Inventory Calendar Month".to_string(), // bare label from fixture
            lo: 1.0_f64,
            hi: 12.0_f64,
        };
        let result = filter_expr_ctx(&filter, Some(&ctx), &[]).unwrap();
        assert!(
            result.contains("'tpcds_benchmark_model'"),
            "expected grounded column with table name, got: {result}"
        );
        assert!(
            !result.contains("Inventory Calendar Month[Inventory Calendar Month]"),
            "bare-label heuristic must not appear, got: {result}"
        );
    }

    /// Range filter with a fully-qualified unique-name → unchanged output.
    #[test]
    fn range_filter_unique_name_passes_through() {
        let ctx = fixture_ctx();
        let filter = Filter::Range {
            level: "inventory_date_dimension.calendar.[Inventory Calendar Month]".to_string(),
            lo: 1.0_f64,
            hi: 12.0_f64,
        };
        let result = filter_expr_ctx(&filter, Some(&ctx), &[]).unwrap();
        assert!(
            result.contains("Inventory Calendar Month"),
            "should keep label: {result}"
        );
        assert!(
            result.contains("'tpcds_benchmark_model'"),
            "should be grounded to table: {result}"
        );
    }

    /// Range filter with an unknown level and a context → UngroundedRangeFilter.
    #[test]
    fn range_filter_unknown_level_fails_loud() {
        let ctx = fixture_ctx();
        let filter = Filter::Range {
            level: "Nonexistent Level XYZ".to_string(),
            lo: 1.0_f64,
            hi: 5.0_f64,
        };
        let err = filter_expr_ctx(&filter, Some(&ctx), &[]).unwrap_err();
        assert!(
            matches!(err, DaxCompileError::UngroundedRangeFilter { .. }),
            "expected UngroundedRangeFilter, got: {err}"
        );
    }

    /// Range filter without a context → heuristic path (backward compat).
    #[test]
    fn range_filter_no_ctx_heuristic() {
        let filter = Filter::Range {
            level: "some.hierarchy.Level".to_string(),
            lo: 1.0_f64,
            hi: 10.0_f64,
        };
        let result = filter_expr_ctx(&filter, None, &[]).unwrap();
        assert!(
            result.contains("KEEPFILTERS"),
            "should emit KEEPFILTERS: {result}"
        );
    }

    /// `compile_grounded` with ctx still passes `validate_dax_syntax`.
    #[test]
    fn grounded_output_passes_syntax_check() {
        let ctx = fixture_ctx();
        let bound = minimal_bound("tpcds.total_store_sales", None);
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            crate::syntax_check::validate_dax_syntax(&dax).is_ok(),
            "syntax check failed: {dax}"
        );
    }

    // ── Time-intelligence grounding + guard tests ─────────────────────────────

    /// Build a `BoundMqoInput` with a single time-intel op.
    fn bound_with_time_intel(measure_unique: &str, ti: mqo_spec::TimeIntel) -> BoundMqoInput {
        BoundMqoInput {
            mqo: Mqo {
                model: "test".to_string(),
                measures: vec![],
                dimensions: vec![],
                filters: vec![],
                limit: None,
                order: None,
                time_intelligence: vec![ti],
                non_empty: false,
            },
            measures: vec![BoundMeasureInput {
                unique_name: measure_unique.to_string(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
                trigger_hierarchies: vec![],
            }],
            dimensions: vec![],
            calc_group_members: vec![],
        }
    }

    /// Build a `DaxCatalogContext` that signals `has_date_table` = true and carries
    /// a grounded date dimension reference.
    fn date_table_capable_ctx() -> DaxCatalogContext {
        let json = r#"{
            "catalog": "tpcds_benchmark_model",
            "has_date_table": true,
            "date_level_unique_name": "sold_date_dimension.calendar.[Sold Calendar Year]",
            "columns": [
                {
                    "unique_name": "tpcds.total_store_sales",
                    "label": "Total Store Sales",
                    "kind": "measure"
                },
                {
                    "unique_name": "sold_date_dimension.calendar.[Sold Calendar Year]",
                    "label": "Sold Calendar Year",
                    "kind": "level"
                }
            ]
        }"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    /// Build a `DaxCatalogContext` that signals `has_date_table` = false (`AtScale` default).
    fn atscale_xmla_ctx() -> DaxCatalogContext {
        let json = r#"{
            "catalog": "tpcds_benchmark_model",
            "has_date_table": false,
            "columns": [
                {
                    "unique_name": "tpcds.total_store_sales",
                    "label": "Total Store Sales",
                    "kind": "measure"
                }
            ]
        }"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    /// AC2 + FR2: `YoY` with `AtScale` XMLA context (`has_date_table=false`) must return
    /// `UnsupportedTimeIntelligence`, never emit a DAX string.
    #[test]
    fn yoy_with_no_date_table_returns_unsupported_error() {
        let ctx = atscale_xmla_ctx();
        let bound = bound_with_time_intel("tpcds.total_store_sales", mqo_spec::TimeIntel::YoY);
        let result = compile_grounded(&bound, Some(&ctx));
        assert!(
            matches!(
                result,
                Err(crate::DaxCompileError::UnsupportedTimeIntelligence { ref op, .. })
                if op == "YoY"
            ),
            "expected UnsupportedTimeIntelligence(YoY), got: {result:?}"
        );
    }

    /// FR3: The `UnsupportedTimeIntelligence` error names the op and reason.
    #[test]
    fn unsupported_error_names_op_and_reason() {
        let ctx = atscale_xmla_ctx();
        let bound = bound_with_time_intel("tpcds.total_store_sales", mqo_spec::TimeIntel::PriorPeriod);
        let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
        match err {
            crate::DaxCompileError::UnsupportedTimeIntelligence { op, reason } => {
                assert_eq!(op, "PriorPeriod");
                assert!(
                    reason.contains("Mark-as-Date-Table"),
                    "reason should mention Mark-as-Date-Table, got: {reason}"
                );
            }
            other => panic!("expected UnsupportedTimeIntelligence, got: {other:?}"),
        }
    }

    /// FR4: `UnsupportedTimeIntelligence` is type-distinct from other error variants.
    #[test]
    fn unsupported_time_intel_is_type_distinct_from_infra_errors() {
        let ctx = atscale_xmla_ctx();
        let bound = bound_with_time_intel("tpcds.total_store_sales", mqo_spec::TimeIntel::YoY);
        let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();

        // Must match the specific variant — not EmptyMeasures, UnknownTimeIntelMeasure,
        // EmptyShareLevel, DeserializeError, or SyntaxCheckFailed.
        assert!(
            matches!(err, crate::DaxCompileError::UnsupportedTimeIntelligence { .. }),
            "wrong variant: {err:?}"
        );
        assert!(
            !matches!(err, crate::DaxCompileError::EmptyMeasures),
            "must not be EmptyMeasures"
        );
    }

    /// AC1 + FR1: `YoY` with a `has_date_table=true` context grounded to the catalog
    /// date dimension — must NOT contain the literal token `DateTable[Date]`.
    #[test]
    fn yoy_with_date_table_ctx_grounds_date_ref() {
        let ctx = date_table_capable_ctx();
        let bound = bound_with_time_intel("tpcds.total_store_sales", mqo_spec::TimeIntel::YoY);
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            !dax.contains("DateTable[Date]"),
            "emitted DAX must not contain placeholder DateTable[Date], got: {dax}"
        );
        assert!(
            dax.contains("'tpcds_benchmark_model'[Sold Calendar Year]"),
            "emitted DAX must contain grounded date ref, got: {dax}"
        );
        assert!(
            dax.contains("SAMEPERIODLASTYEAR"),
            "YoY must emit SAMEPERIODLASTYEAR, got: {dax}"
        );
    }

    /// AC4 + FR6 + NFR1: No context → byte-identical to pre-change (placeholder preserved).
    #[test]
    fn yoy_with_no_ctx_emits_placeholder_unchanged() {
        let bound = bound_with_time_intel("tpcds.total_store_sales", mqo_spec::TimeIntel::YoY);
        let dax = compile_grounded(&bound, None).unwrap();
        // Legacy path: placeholder must appear verbatim.
        assert!(
            dax.contains("SAMEPERIODLASTYEAR(DateTable[Date])"),
            "no-context path must preserve DateTable[Date] placeholder, got: {dax}"
        );
    }

    /// FR5 + guardrail: `ToDate` with a date-table-capable context is NOT rejected.
    #[test]
    fn to_date_with_date_table_ctx_is_not_rejected() {
        let ctx = date_table_capable_ctx();
        let bound = bound_with_time_intel(
            "tpcds.total_store_sales",
            mqo_spec::TimeIntel::ToDate { grain: mqo_spec::Grain::Year },
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            dax.contains("DATESYTD"),
            "ToDate(Year) should emit DATESYTD, got: {dax}"
        );
        assert!(
            !dax.contains("DateTable[Date]"),
            "ToDate must not contain placeholder when grounded, got: {dax}"
        );
    }

    /// `FR2`: `ToDate` with `AtScale` XMLA context (no date table) is rejected.
    #[test]
    fn to_date_with_no_date_table_returns_unsupported_error() {
        let ctx = atscale_xmla_ctx();
        let bound = bound_with_time_intel(
            "tpcds.total_store_sales",
            mqo_spec::TimeIntel::ToDate { grain: mqo_spec::Grain::Quarter },
        );
        let result = compile_grounded(&bound, Some(&ctx));
        assert!(
            matches!(
                result,
                Err(crate::DaxCompileError::UnsupportedTimeIntelligence { ref op, .. })
                if op.starts_with("ToDate")
            ),
            "expected UnsupportedTimeIntelligence(ToDate), got: {result:?}"
        );
    }

    /// Share is NOT gated by Mark-as-Date-Table — must compile even without it.
    #[test]
    fn share_with_no_date_table_ctx_is_not_rejected() {
        let ctx = atscale_xmla_ctx();
        let bound = bound_with_time_intel(
            "tpcds.total_store_sales",
            mqo_spec::TimeIntel::Share {
                of_level: "inventory_date_dimension.calendar.[Inventory Calendar Month]"
                    .to_string(),
            },
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            dax.contains("DIVIDE"),
            "Share should emit DIVIDE, got: {dax}"
        );
    }

    /// Inferred `date_level` from `kind="date_level"` column entry.
    #[test]
    fn date_level_inferred_from_kind_date_level() {
        let json = r#"{
            "catalog": "my_model",
            "has_date_table": true,
            "columns": [
                {
                    "unique_name": "date_dim.calendar.[Calendar Year]",
                    "label": "Calendar Year",
                    "kind": "date_level"
                },
                {
                    "unique_name": "sales.revenue",
                    "label": "Revenue",
                    "kind": "measure"
                }
            ]
        }"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        assert_eq!(
            ctx.date_level_unique_name.as_deref(),
            Some("date_dim.calendar.[Calendar Year]"),
            "should infer date_level from kind=date_level"
        );
        // Compile a YoY and verify it's grounded.
        let bound = bound_with_time_intel("sales.revenue", mqo_spec::TimeIntel::YoY);
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            dax.contains("'my_model'[Calendar Year]"),
            "YoY must reference inferred date level, got: {dax}"
        );
        assert!(
            !dax.contains("DateTable[Date]"),
            "must not contain placeholder, got: {dax}"
        );
    }
}
