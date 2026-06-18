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
    // For projection MQOs (is_projection() == true), measures are intentionally
    // empty. For all other cases, empty measures is an error.
    if bound.measures.is_empty() && !bound.mqo.is_projection() {
        return Err(DaxCompileError::EmptyMeasures);
    }

    // Build the measure expression list, applying time-intel wrappers.
    // For projection MQOs this returns an empty vec.
    let measure_pairs = if bound.measures.is_empty() {
        Vec::new()
    } else {
        build_measure_pairs(bound, ctx)?
    };

    // Build groupBy columns list (from bound.dimensions). Each dimension level is
    // grounded to its per-level physical table (FR-1); a level that cannot be
    // grounded FR-4-declines (UngroundableLevel) rather than emitting an
    // /* ungrounded */ reference to the engine.
    let group_by_cols: Vec<String> = bound
        .dimensions
        .iter()
        .map(|d| level_col_ref_grounded(&d.unique_name, ctx))
        .collect::<Result<Vec<_>, _>>()?;

    // Build filter expressions. The query's dimension level unique_names are
    // passed so an ambiguous Member value (e.g. "M" in both Gender and Marital
    // Status) binds to the level the query groups by.
    let dim_levels: Vec<String> = bound.dimensions.iter().map(|d| d.unique_name.clone()).collect();
    // PRD-mqo-multi-dim-filter-dax-grounding: for projection MQOs with 2+ AND-combinable
    // top-level filters, emit a single combined KEEPFILTERS(FILTER(ALL(col1, col2, ...), pred1 && pred2 && ...))
    // instead of N separate FILTER(ALL(col)) args (which cartesian-explode at fine grain).
    let mut filter_exprs: Vec<String> = Vec::new();
    let combine_projection_filters = bound.mqo.is_projection()
        && bound.mqo.filters.len() >= 2
        && !bound.mqo.filters.iter().any(|f| {
            matches!(f, Filter::Group { op: mqo_spec::FilterGroupOp::Or, .. })
        });
    if combine_projection_filters {
        let mut preds: Vec<String> = Vec::new();
        let mut all_cols: Vec<String> = Vec::new();
        for f in &bound.mqo.filters {
            let (pred, cols) = filter_predicate(f, ctx, &dim_levels, 0)?;
            preds.push(format!("({pred})"));
            all_cols.extend(cols);
        }
        all_cols.sort();
        all_cols.dedup();
        let combined_pred = preds.join(" && ");
        let all_cols_str = all_cols.join(", ");
        filter_exprs.push(format!(
            "KEEPFILTERS(FILTER(ALL({all_cols_str}), {combined_pred})) /* combined-projection-filter */"
        ));
    } else {
        for f in &bound.mqo.filters {
            filter_exprs.push(filter_expr_ctx(f, ctx, &dim_levels)?);
        }
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
        // Apply limit TOPN. For projections, sort by declared order keys (FR1/FR2).
        // For regular queries, sort by first measure (existing behaviour).
        if bound.mqo.is_projection() {
            let sort_args =
                projection_topn_sort_args(bound, ctx)?;
            format!("TOPN({limit}, {inner}, {sort_args})")
        } else {
            let first_measure_ref = measure_dax_ref_ctx(&bound.measures[0].unique_name, ctx);
            format!("TOPN({limit}, {inner}, {first_measure_ref}, DESC)")
        }
    } else {
        inner
    };

    let mut dax = format!("EVALUATE\n{inner}");

    // Append ORDER BY (skipped for projection+limit: TOPN already sorts, FR3).
    append_order_by(&mut dax, bound, ctx)?;

    Ok(dax)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Build the TOPN sort-arg string for a projection MQO with a limit.
///
/// When the MQO declares `order` keys, resolves each key as a dimension-level
/// reference (`level_col_ref_grounded`) with its declared direction (FR1/FR2).
/// When no order keys are declared, falls back to sorting by the first
/// projected dimension ASC (preserves existing behaviour for unordered bounded
/// projections).
///
/// # Errors
///
/// Returns [`DaxCompileError::UngroundableLevel`] when an order key cannot be
/// resolved to a grounded dimension-level column (FR6).
fn projection_topn_sort_args(
    bound: &BoundMqoInput,
    ctx: Option<&DaxCatalogContext>,
) -> Result<String, DaxCompileError> {
    if let Some(order_keys) = &bound.mqo.order {
        if !order_keys.is_empty() {
            let mut args: Vec<String> = Vec::new();
            for ok in order_keys {
                let col_ref = level_col_ref_grounded(&ok.key, ctx)?;
                let dir = sort_dir_str(&ok.direction);
                args.push(format!("{col_ref}, {dir}"));
            }
            return Ok(args.join(", "));
        }
    }
    // No order keys declared — fall back to first dim ASC.
    let first_dim_ref = match bound.dimensions.first() {
        Some(d) => level_col_ref_grounded(&d.unique_name, ctx)?,
        None => "1".to_string(),
    };
    Ok(format!("{first_dim_ref}, ASC"))
}

/// Append an `ORDER BY` clause to `dax` when the MQO declares order keys.
///
/// Skipped entirely for projection+limit queries: the TOPN expression already
/// encodes the sort (FR3), and re-emitting it as `ORDER BY` would use
/// `measure_dax_ref_ctx` on dimension keys — the root-cause bug this PRD fixes.
///
/// For projection-without-limit and for all measure-bearing queries the ORDER
/// BY clause is appended. Projection keys are resolved as dimension-level
/// references (FR1); measure keys use `measure_dax_ref_ctx` (existing path).
///
/// # Errors
///
/// Returns [`DaxCompileError::UngroundableLevel`] when a projection order key
/// cannot be grounded (FR6).
fn append_order_by(
    dax: &mut String,
    bound: &BoundMqoInput,
    ctx: Option<&DaxCatalogContext>,
) -> Result<(), DaxCompileError> {
    let Some(order_keys) = &bound.mqo.order else {
        return Ok(());
    };
    if order_keys.is_empty() {
        return Ok(());
    }
    // Projection with a limit: TOPN already sorts — skip ORDER BY (FR3).
    if bound.mqo.is_projection() && bound.mqo.limit.is_some() {
        return Ok(());
    }
    // Measure-bearing with a limit: TOPN selects and sorts by primary measure — skip ORDER BY.
    // AtScale XMLA rejects ORDER BY on dimension column refs after TOPN, causing an engine
    // error (j=None) on queries like "top 20 stores by employee count, tie-break by name".
    if !bound.mqo.is_projection() && bound.mqo.limit.is_some() {
        return Ok(());
    }
    let order_parts: Result<Vec<String>, DaxCompileError> = order_keys
        .iter()
        .map(|ok| {
            let dir = sort_dir_str(&ok.direction);
            if bound.mqo.is_projection() {
                // FR1: resolve as dimension-level reference.
                let col_ref = level_col_ref_grounded(&ok.key, ctx)?;
                Ok(format!("{col_ref} {dir}"))
            } else {
                // FR1 (measure-bearing): dispatch by key kind, not query shape.
                // A key that matches a bound measure → measure ref (preserves byte-identical
                // output for all-measure ORDER BY, FR4). A key that resolves to a dimension
                // level → grounded level ref (FR2). Unknown keys propagate UngroundableLevel
                // via ? (FR7).
                let is_measure_key = bound.measures.iter().any(|m| m.unique_name == ok.key);
                if is_measure_key {
                    Ok(format!("{} {dir}", measure_dax_ref_ctx(&ok.key, ctx)))
                } else {
                    // PRD-mqo-orderby-fallback-to-measure-ref: when grounding fails for a
                    // non-bound-measure key (unique-name mismatch), fall back to measure ref
                    // instead of propagating UngroundableLevel (v0.16.0 regression).
                    match level_col_ref_grounded(&ok.key, ctx) {
                        Ok(col_ref) => Ok(format!("{col_ref} {dir}")),
                        Err(_) => Ok(format!("{} {dir}", measure_dax_ref_ctx(&ok.key, ctx))),
                    }
                }
            }
        })
        .collect();
    write!(dax, "\nORDER BY {}", order_parts?.join(", ")).expect("String write is infallible");
    Ok(())
}

/// Return the DAX direction keyword for a [`SortDirection`].
fn sort_dir_str(dir: &SortDirection) -> &'static str {
    match dir {
        SortDirection::Asc => "ASC",
        SortDirection::Desc => "DESC",
    }
}

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
                    // PRD-mqo-rank-column-residual-suppression: do NOT append " Rank" to the
                    // label — the measure emits under its original name; the ordinal is
                    // expressed by TOPN ordering, never as an output column.
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
///
/// Resolve a level-less `Member` filter's hierarchy to a grounded column ref,
/// applying domain-aware grounding + the decline-not-fallback safety valve.
fn resolve_member_column(
    hierarchy: &str,
    members: &[String],
    ctx: Option<&DaxCatalogContext>,
    dim_levels: &[String],
) -> Result<String, DaxCompileError> {
    let level_unique_name = ctx
        .and_then(|c| {
            c.resolve_member_level(hierarchy, members, dim_levels).or_else(|| {
                if c.hierarchy_has_any_domain(hierarchy) {
                    None
                } else {
                    c.resolve_hierarchy_first_level(hierarchy)
                }
            })
        })
        .ok_or_else(|| DaxCompileError::UngroundedMemberFilter {
            hierarchy: hierarchy.to_string(),
            members: members.join(", "),
        })?;
    Ok(level_col_ref_ctx(level_unique_name, ctx))
}

/// Format a range bound for DAX: numeric verbatim, ISO-date string → `DATE(y,m,d)`.
fn range_bound_dax(b: &mqo_spec::RangeBound) -> String {
    b.as_f64()
        .map(|n| format!("{n}"))
        .or_else(|| b.as_str().map(|s| format!("DATE({})", s.replace('-', ","))))
        .unwrap_or_else(|| format!("{b:?}"))
}

/// Produce a boolean DAX **predicate** (not a wrapped FILTER) plus the set of
/// columns it references, for a leaf filter. Used both by the standalone filter
/// arms (which wrap it in `KEEPFILTERS(FILTER(ALL(cols), pred))`) and by
/// `Filter::Group` (which combines predicates with `||` / `&&` — real OR).
/// `depth` bounds nesting: a Group may contain leaves or one level of sub-Groups.
fn filter_predicate(
    filter: &Filter,
    ctx: Option<&DaxCatalogContext>,
    dim_levels: &[String],
    depth: usize,
) -> Result<(String, Vec<String>), DaxCompileError> {
    match filter {
        Filter::Member { hierarchy, members } => {
            if members.is_empty() {
                return Err(DaxCompileError::EmptyMemberFilter { hierarchy: hierarchy.clone() });
            }
            let col = resolve_member_column(hierarchy, members, ctx, dim_levels)?;
            let list: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
            Ok((format!("{col} IN {{{}}}", list.join(", ")), vec![col]))
        }
        Filter::MemberLevel { level, members, exclude, .. } => {
            // FR-2: `level` may be a bare display label ("Ship Mode Type") or a
            // full unique_name ("ship_mode.[Ship Mode Type]"); both ground to the
            // same column. FR-4: decline (UngroundableLevel) instead of emitting
            // an /* ungrounded */ ref.
            let col = level_col_ref_grounded(level, ctx)?;
            let list: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
            let set = format!("{col} IN {{{}}}", list.join(", "));
            let pred = if *exclude { format!("NOT({set})") } else { set };
            Ok((pred, vec![col]))
        }
        Filter::Range { level, lo, hi } => {
            let col = if let Some(c) = ctx {
                if c.labels.contains_key(level.as_str()) {
                    level_col_ref_ctx(level, ctx)
                } else if let Some(un) = c.resolve_level_label(level) {
                    level_col_ref_ctx(un, ctx)
                } else {
                    return Err(DaxCompileError::UngroundedRangeFilter { level: level.clone() });
                }
            } else {
                level_col_ref_ctx(level, ctx)
            };
            let pred = format!("{col} >= {} && {col} <= {}", range_bound_dax(lo), range_bound_dax(hi));
            Ok((pred, vec![col]))
        }
        Filter::Group { op, filters } => {
            if depth >= 2 {
                return Err(DaxCompileError::UngroundedMemberFilter {
                    hierarchy: "Group".to_string(),
                    members: "filter nesting exceeds two levels".to_string(),
                });
            }
            if filters.is_empty() {
                return Err(DaxCompileError::UngroundedMemberFilter {
                    hierarchy: "Group".to_string(),
                    members: "empty filter group".to_string(),
                });
            }
            let mut preds = Vec::new();
            let mut cols = Vec::new();
            for f in filters {
                let (p, c) = filter_predicate(f, ctx, dim_levels, depth + 1)?;
                preds.push(format!("({p})"));
                cols.extend(c);
            }
            let joiner = match op {
                mqo_spec::FilterGroupOp::Or => " || ",
                mqo_spec::FilterGroupOp::And => " && ",
            };
            Ok((preds.join(joiner), cols))
        }
        Filter::CalcGroupMember { .. } => Err(DaxCompileError::UngroundedMemberFilter {
            hierarchy: "Group".to_string(),
            members: "CalcGroupMember cannot appear inside a filter Group".to_string(),
        }),
    }
}

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
            // contains the member value(s) (PRD-mqo-member-filter-domain-grounding).
            // When no domain match is found, DECLINE with a typed error rather than
            // silently grounding to the first level (PRD-mqo-member-grounding-decline-
            // not-fallback) — the first-level fallback was the source of the silent
            // 0-row misgrounds (customer_demographics="M" → [Credit Rating]). Safety
            // valve (OQ-1): fall back to first-level ONLY when the hierarchy carries
            // no captured domains at all, so un-ingested deployments don't regress to
            // a mass decline.
            let level_unique_name = ctx
                .and_then(|c| {
                    c.resolve_member_level(hierarchy, members, dim_levels)
                        .or_else(|| {
                            if c.hierarchy_has_any_domain(hierarchy) {
                                None // domains exist but none matched → decline (below)
                            } else {
                                c.resolve_hierarchy_first_level(hierarchy)
                            }
                        })
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
        Filter::Group { .. } => {
            // Real boolean semantics (PRD-mqo-filter-predicate-grammar): build ONE
            // combined predicate (`||` for OR-of-AND-groups, `&&` for AND-of-OR-groups)
            // over ALL referenced columns, wrapped in a single FILTER. Because every
            // dimension level is a column on the flattened 'atscale_catalogs' table,
            // `ALL(col1, col2, …)` is a valid multi-column table the predicate filters.
            let (pred, mut cols) = filter_predicate(filter, ctx, dim_levels, 0)?;
            cols.sort();
            cols.dedup();
            let all_cols = cols.join(", ");
            Ok(format!("KEEPFILTERS(FILTER(ALL({all_cols}), {pred})) /* filter-group */"))
        }
        Filter::MemberLevel { level, members, exclude, .. } => {
            // Caller pinned the level explicitly (PRD-mqo-member-filter-explicit-level):
            // bind directly to it, no domain-scan grounding. `exclude` → NOT-IN.
            // FR-2: accept a bare display label OR a full unique_name as `level`;
            // both ground to the same column. FR-4: decline rather than emit an
            // /* ungrounded */ reference to the engine.
            let col = level_col_ref_grounded(level, ctx)?;
            let member_list: Vec<String> =
                members.iter().map(|m| format!("\"{m}\"")).collect();
            let set = format!("{col} IN {{{}}}", member_list.join(", "));
            let pred = if *exclude { format!("NOT({set})") } else { set };
            Ok(format!(
                "KEEPFILTERS(FILTER(ALL({col}), {pred})) /* member-at-level */"
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
            let lo_s = lo.as_f64().map(|n| format!("{n}"))
                .or_else(|| lo.as_str().map(|s| format!("DATE({})", s.replace('-', ","))))
                .unwrap_or_else(|| format!("{lo:?}"));
            let hi_s = hi.as_f64().map(|n| format!("{n}"))
                .or_else(|| hi.as_str().map(|s| format!("DATE({})", s.replace('-', ","))))
                .unwrap_or_else(|| format!("{hi:?}"));
            Ok(format!(
                "KEEPFILTERS(FILTER(ALL({col}), {col} >= {lo_s} && {col} <= {hi_s}))"
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
/// - With context and a known `unique_name`: `'<physical table>'[Display Label]`
///   where the physical table is the level's hierarchy prefix (FR-1), single-quoted
///   (FR-3 handles spaces/reserved chars).
/// - With context but unknown `unique_name`: `<fallback_ref> /* ungrounded: <unique_name> */`
/// - Without context: delegates to `level_col_ref` (existing behaviour)
///
/// This is the *infallible* variant used where the level is already known to be
/// catalog-resolved (e.g. measure-bearing query paths). Paths that must FR-4-decline
/// on an unmappable level use [`level_col_ref_grounded`] instead.
fn level_col_ref_ctx(unique_name: &str, ctx: Option<&DaxCatalogContext>) -> String {
    let Some(c) = ctx else {
        return level_col_ref(unique_name);
    };

    if let Some(label) = c.labels.get(unique_name) {
        // Per-level physical table (FR-1): the hierarchy prefix of the
        // unique_name, NOT the single global table_name (= the PGWire database
        // name, invalid as a DAX table). Fall back to the global table_name for
        // backward compat with contexts built before the per-level map existed.
        let table = c.tables.get(unique_name).unwrap_or(&c.table_name);
        return format!("{}[{label}]", quote_table_ident(table));
    }

    // Unknown unique_name — fall back to heuristic and annotate.
    let fallback = level_col_ref(unique_name);
    format!("{fallback} /* ungrounded: {unique_name} */")
}

/// Single-quote a DAX table identifier (FR-3).
///
/// `AtScale` XMLA accepts (and we always emit) single-quoted table names so that
/// hierarchy names containing spaces or DAX-reserved characters are valid
/// (`'Ship Mode'[Carrier]`). An embedded apostrophe is doubled per DAX escaping.
fn quote_table_ident(table: &str) -> String {
    format!("'{}'", table.replace('\'', "''"))
}

/// Emit a grounded `'<physical table>'[Display Label]` column reference for a
/// level, or FR-4-decline with [`DaxCompileError::UngroundableLevel`] when the
/// level cannot be grounded.
///
/// Accepts `key` as either a fully-qualified `unique_name`
/// (`ship_mode.[Ship Mode Type]`) or a bare display label (`Ship Mode Type`) —
/// both resolve to the same grounded column (FR-2). When a `ctx` is present and
/// `key` matches neither, returns `UngroundableLevel` instead of emitting a
/// `/* ungrounded */` reference to the engine (FR-4).
///
/// With no `ctx`, delegates to the heuristic [`level_col_ref`] (backward compat).
fn level_col_ref_grounded(
    key: &str,
    ctx: Option<&DaxCatalogContext>,
) -> Result<String, DaxCompileError> {
    let Some(c) = ctx else {
        return Ok(level_col_ref(key));
    };
    let unique_name = c.canonical_level_key(key).ok_or_else(|| {
        DaxCompileError::UngroundableLevel { unique_name: key.to_string() }
    })?;
    Ok(level_col_ref_ctx(&unique_name, ctx))
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
                projection: false,
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

    // ── Decline-not-fallback (PRD-mqo-member-grounding-decline-not-fallback) ──

    fn demographics_ctx(with_domain: bool) -> DaxCatalogContext {
        let (d1, d2) = if with_domain {
            (r#","domain":["F","M"]"#, r#","domain":["D","M","S","U","W"]"#)
        } else {
            ("", "")
        };
        let json = format!(
            r#"{{"catalog":"atscale_catalogs","columns":[
              {{"kind":"level","unique_name":"customer_demographics.[Gender]","label":"Gender","hierarchy":"customer_demographics","level":"Gender"{d1}}},
              {{"kind":"level","unique_name":"customer_demographics.[Marital Status]","label":"Marital Status","hierarchy":"customer_demographics","level":"Marital Status"{d2}}},
              {{"kind":"measure","unique_name":"tpcds.m","label":"M"}}
            ]}}"#
        );
        DaxCatalogContext::from_json(&json).unwrap()
    }

    fn bound_with_member_filter(hierarchy: &str, member: &str) -> BoundMqoInput {
        BoundMqoInput {
            mqo: Mqo {
                model: "test".to_string(),
                measures: vec![],
                dimensions: vec![],
                filters: vec![mqo_spec::Filter::Member {
                    hierarchy: hierarchy.to_string(),
                    members: vec![member.to_string()],
                }],
                limit: None,
                order: None,
                time_intelligence: vec![],
                non_empty: false,
                projection: false,
            },
            measures: vec![BoundMeasureInput {
                unique_name: "tpcds.m".to_string(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
                trigger_hierarchies: vec![],
            }],
            dimensions: vec![],
            calc_group_members: vec![],
        }
    }

    /// Domains exist but the ambiguous member matches none unambiguously → DECLINE
    /// with a typed error, never a silent first-level grounding.
    #[test]
    fn ambiguous_member_with_domains_declines_not_fallback() {
        let ctx = demographics_ctx(true);
        let bound = bound_with_member_filter("customer_demographics", "M");
        let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
        assert!(
            matches!(err, crate::DaxCompileError::UngroundedMemberFilter { .. }),
            "expected a typed decline, got {err:?}"
        );
    }

    /// No captured domains anywhere on the hierarchy → safety valve keeps the
    /// legacy first-level fallback (un-ingested deployments don't mass-decline).
    #[test]
    fn member_with_no_domains_falls_back_to_first_level() {
        let ctx = demographics_ctx(false);
        let bound = bound_with_member_filter("customer_demographics", "M");
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            dax.contains("grounded-from-member"),
            "expected first-level grounding, got {dax}"
        );
    }

    /// Explicit-level member filter (PRD-mqo-member-filter-explicit-level): pins
    /// the level directly, disambiguating "M" to Gender (not Marital/Credit Rating).
    #[test]
    fn member_level_pins_the_named_level() {
        let ctx = demographics_ctx(true);
        let mut bound = bound_with_member_filter("customer_demographics", "M");
        bound.mqo.filters = vec![mqo_spec::Filter::MemberLevel {
            hierarchy: "customer_demographics".to_string(),
            level: "customer_demographics.[Gender]".to_string(),
            members: vec!["M".to_string()],
            exclude: false,
        }];
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(dax.contains("[Gender]"), "expected Gender column, got {dax}");
        assert!(dax.contains("member-at-level"), "got {dax}");
    }

    /// `exclude: true` emits a NOT-IN predicate.
    #[test]
    fn member_level_exclude_emits_not_in() {
        let ctx = demographics_ctx(true);
        let mut bound = bound_with_member_filter("customer_demographics", "M");
        bound.mqo.filters = vec![mqo_spec::Filter::MemberLevel {
            hierarchy: "customer_demographics".to_string(),
            level: "customer_demographics.[Marital Status]".to_string(),
            members: vec!["U".to_string()],
            exclude: true,
        }];
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(dax.contains("NOT("), "expected NOT-IN, got {dax}");
    }

    /// `Filter::Group` OR compiles to a single FILTER with `||` over both columns
    /// (real OR semantics, `PRD-mqo-filter-predicate-grammar` — not the AND stub).
    #[test]
    fn group_or_emits_disjunctive_predicate() {
        let ctx = demographics_ctx(true);
        let mut bound = bound_with_member_filter("customer_demographics", "M");
        bound.mqo.filters = vec![mqo_spec::Filter::Group {
            op: mqo_spec::FilterGroupOp::Or,
            filters: vec![
                mqo_spec::Filter::MemberLevel {
                    hierarchy: "customer_demographics".to_string(),
                    level: "customer_demographics.[Gender]".to_string(),
                    members: vec!["F".to_string()],
                    exclude: false,
                },
                mqo_spec::Filter::MemberLevel {
                    hierarchy: "customer_demographics".to_string(),
                    level: "customer_demographics.[Marital Status]".to_string(),
                    members: vec!["M".to_string()],
                    exclude: false,
                },
            ],
        }];
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(dax.contains("||"), "expected disjunctive predicate, got {dax}");
        assert!(dax.contains("[Gender]") && dax.contains("[Marital Status]"), "got {dax}");
        // single FILTER wrapping both, not two separate KEEPFILTERS
        assert_eq!(dax.matches("FILTER(ALL(").count(), 1, "expected ONE combined FILTER: {dax}");
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
        // FR-1: grounds to the per-level physical table (hierarchy prefix of the
        // unique_name = "inventory_date_dimension"), NOT the catalog/database name.
        assert!(
            dax.contains("'inventory_date_dimension'[Inventory Calendar Month]"),
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

    /// FR-4: an unknown dimension `unique_name` with a context present declines
    /// with a typed `UngroundableLevel` error naming the level — it does NOT emit a
    /// `/* ungrounded */` reference to the engine (which the engine rejects with an
    /// opaque 500).
    #[test]
    fn grounded_unknown_level_declines_ungroundable() {
        let ctx = fixture_ctx();
        let bound = minimal_bound("tpcds.total_store_sales", Some("no.such.level"));
        let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
        assert!(
            matches!(err, crate::DaxCompileError::UngroundableLevel { ref unique_name } if unique_name == "no.such.level"),
            "expected UngroundableLevel(no.such.level), got: {err:?}"
        );
    }

    /// FR-4 boundary: with NO context, an unknown level still falls back to the
    /// heuristic ref (backward compat — un-grounded paths are unchanged).
    #[test]
    fn unknown_level_no_ctx_uses_heuristic() {
        let bound = minimal_bound("tpcds.total_store_sales", Some("foo.bar.[Baz]"));
        let dax = compile_grounded(&bound, None).unwrap();
        assert!(
            dax.contains("Bar[Baz]"),
            "no-ctx path should use heuristic level_col_ref, got: {dax}"
        );
    }

    // ── Range-filter grounding tests ──────────────────────────────────────────

    /// Range filter with a bare display label → resolves via reverse lookup.
    ///
    /// `fixture_ctx()` has: `unique_name` "inventory\_date\_dimension.calendar.\[Inventory Calendar Month\]"
    /// → label "Inventory Calendar Month", per-level table "inventory\_date\_dimension".
    #[test]
    fn range_filter_bare_label_resolves() {
        let ctx = fixture_ctx();
        let filter = Filter::Range {
            level: "Inventory Calendar Month".to_string(), // bare label from fixture
            lo: mqo_spec::RangeBound::Number(1.0_f64),
            hi: mqo_spec::RangeBound::Number(12.0_f64),
        };
        let result = filter_expr_ctx(&filter, Some(&ctx), &[]).unwrap();
        assert!(
            result.contains("'inventory_date_dimension'"),
            "expected grounded column with per-level table name, got: {result}"
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
            lo: mqo_spec::RangeBound::Number(1.0_f64),
            hi: mqo_spec::RangeBound::Number(12.0_f64),
        };
        let result = filter_expr_ctx(&filter, Some(&ctx), &[]).unwrap();
        assert!(
            result.contains("Inventory Calendar Month"),
            "should keep label: {result}"
        );
        assert!(
            result.contains("'inventory_date_dimension'"),
            "should be grounded to per-level table: {result}"
        );
    }

    /// Range filter with an unknown level and a context → `UngroundedRangeFilter`.
    #[test]
    fn range_filter_unknown_level_fails_loud() {
        let ctx = fixture_ctx();
        let filter = Filter::Range {
            level: "Nonexistent Level XYZ".to_string(),
            lo: mqo_spec::RangeBound::Number(1.0_f64),
            hi: mqo_spec::RangeBound::Number(5.0_f64),
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
            lo: mqo_spec::RangeBound::Number(1.0_f64),
            hi: mqo_spec::RangeBound::Number(10.0_f64),
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
                projection: false,
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

    // ── Projection MQO tests (PRD-mqo-attribute-projection) ──────────────────

    fn projection_bound(dim_unique: &str) -> BoundMqoInput {
        BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".to_string(),
                measures: vec![],
                dimensions: vec![mqo_spec::LevelSelection {
                    hierarchy: "ship_mode".to_string(),
                    level: dim_unique.to_string(),
                }],
                filters: vec![],
                limit: None,
                order: None,
                time_intelligence: vec![],
                non_empty: false,
                projection: true,
            },
            measures: vec![],
            dimensions: vec![BoundDimensionInput {
                unique_name: dim_unique.to_string(),
                hierarchy: "ship_mode".to_string(),
            }],
            calc_group_members: vec![],
        }
    }

    /// AC-1: projection MQO with a level and no measures → SUMMARIZECOLUMNS with no measure args.
    #[test]
    fn projection_mqo_emits_summarizecolumns_without_measure() {
        let bound = projection_bound("ship_mode.[Carrier]");
        let dax = compile(&bound).unwrap();
        assert!(
            dax.contains("SUMMARIZECOLUMNS"),
            "projection must emit SUMMARIZECOLUMNS, got: {dax}"
        );
        // A measure-arg pair looks like `"Label", [MeasureRef]` — a quoted string followed
        // by a [measure_ref] argument. For a projection, no such quoted-name+measure pair
        // should appear. We verify by checking no quoted column name arg exists.
        // The dimension reference `Ship_mode[Carrier]` is valid — that IS the column.
        assert!(
            !dax.contains(r#""Carrier","#),
            "no quoted measure-name arg should appear in SUMMARIZECOLUMNS, got: {dax}"
        );
        assert!(
            dax.starts_with("EVALUATE"),
            "must start with EVALUATE, got: {dax}"
        );
        // The dimension column reference MUST appear.
        assert!(
            dax.contains("Carrier"),
            "dimension column reference must appear in output, got: {dax}"
        );
    }

    /// Projection + filter → SUMMARIZECOLUMNS with filter but no measure arg.
    #[test]
    fn projection_with_filter_emits_summarizecolumns_with_filter_no_measure() {
        let mut bound = projection_bound("ship_mode.[Carrier]");
        bound.mqo.filters = vec![mqo_spec::Filter::MemberLevel {
            hierarchy: "ship_mode".to_string(),
            level: "ship_mode.[Ship Mode Type]".to_string(),
            members: vec!["EXPRESS".to_string()],
            exclude: false,
        }];
        let dax = compile(&bound).unwrap();
        assert!(
            dax.contains("SUMMARIZECOLUMNS"),
            "got: {dax}"
        );
        assert!(
            dax.contains("EXPRESS"),
            "filter must appear, got: {dax}"
        );
    }

    /// Non-projection measureless MQO still returns `EmptyMeasures`.
    #[test]
    fn non_projection_measureless_returns_empty_measures_error() {
        let bound = BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".to_string(),
                measures: vec![],
                dimensions: vec![mqo_spec::LevelSelection {
                    hierarchy: "ship_mode".to_string(),
                    level: "Carrier".to_string(),
                }],
                filters: vec![],
                limit: None,
                order: None,
                time_intelligence: vec![],
                non_empty: false,
                projection: false,
            },
            measures: vec![],
            dimensions: vec![BoundDimensionInput {
                unique_name: "ship_mode.[Carrier]".to_string(),
                hierarchy: "ship_mode".to_string(),
            }],
            calc_group_members: vec![],
        };
        let err = compile(&bound).unwrap_err();
        assert!(
            matches!(err, crate::DaxCompileError::EmptyMeasures),
            "expected EmptyMeasures, got: {err:?}"
        );
    }
}

// ── PRD-mqo-projection-orderby-dimension-key: projection ORDER BY regression ────
#[cfg(test)]
mod projection_orderby_tests {
    use super::compile_grounded;
    use crate::catalog_context::DaxCatalogContext;
    use crate::input::{BoundDimensionInput, BoundMeasureInput, BoundMqoInput};
    use mqo_spec::{Mqo, OrderKey, SortDirection};

    /// Catalog with customer dimension levels used by the three timeout cases.
    fn customer_ctx() -> DaxCatalogContext {
        let json = r#"{
            "catalog": "atscale_catalogs",
            "columns": [
                {"unique_name":"customer_name_dimension.[Customer First Name]","label":"Customer First Name","kind":"level","hierarchy":"customer_name_dimension","level":"Customer First Name"},
                {"unique_name":"customer_demographics.[Gender]","label":"Gender","kind":"level","hierarchy":"customer_demographics","level":"Gender"},
                {"unique_name":"customer_demographics.[Income Band]","label":"Income Band","kind":"level","hierarchy":"customer_demographics","level":"Income Band"},
                {"unique_name":"customer_demographics.[Vehicle Count]","label":"Vehicle Count","kind":"level","hierarchy":"customer_demographics","level":"Vehicle Count"},
                {"unique_name":"customer_demographics.[Buy Potential]","label":"Buy Potential","kind":"level","hierarchy":"customer_demographics","level":"Buy Potential"},
                {"unique_name":"customer_demographics.[Customer ID]","label":"Customer ID","kind":"level","hierarchy":"customer_demographics","level":"Customer ID"}
            ]
        }"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    /// Build a projection `BoundMqoInput` from a list of (unique\_name, hierarchy)
    /// dim pairs, with optional order keys and limit.
    fn proj_with_order(
        dims: &[(&str, &str)],
        order: Vec<OrderKey>,
        limit: Option<u64>,
    ) -> BoundMqoInput {
        BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".into(),
                measures: vec![],
                dimensions: dims
                    .iter()
                    .map(|(u, h)| mqo_spec::LevelSelection {
                        hierarchy: (*h).to_string(),
                        level: (*u).to_string(),
                    })
                    .collect(),
                filters: vec![],
                limit,
                order: if order.is_empty() { None } else { Some(order) },
                time_intelligence: vec![],
                non_empty: false,
                projection: true,
            },
            measures: vec![],
            dimensions: dims
                .iter()
                .map(|(u, h)| BoundDimensionInput {
                    unique_name: (*u).to_string(),
                    hierarchy: (*h).to_string(),
                })
                .collect(),
            calc_group_members: vec![],
        }
    }

    /// AC1 / FR1: a projection with ORDER BY + LIMIT must emit TOPN with a
    /// dimension-level reference — NOT a measure reference like `[Customer First Name]`.
    /// Previously, `measure_dax_ref_ctx` was called on the order key, producing a bare
    /// measure ref that XMLA could not resolve (300s hang).
    #[test]
    fn projection_orderby_limit_emits_topn_with_dim_ref_not_measure_ref() {
        let ctx = customer_ctx();
        let bound = proj_with_order(
            &[("customer_name_dimension.[Customer First Name]", "customer_name_dimension")],
            vec![OrderKey {
                key: "customer_name_dimension.[Customer First Name]".to_string(),
                direction: SortDirection::Asc,
            }],
            Some(20),
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();

        // Must contain TOPN (limit applied via TOPN).
        assert!(dax.contains("TOPN(20,"), "expected TOPN(20, ...), got: {dax}");

        // The sort expression must be the grounded dim-level column ref, not a bare [Label].
        // A bare measure ref looks like `[Customer First Name]` (no table prefix).
        // The correct dim ref looks like `'customer_name_dimension'[Customer First Name]`.
        assert!(
            dax.contains("'customer_name_dimension'[Customer First Name]"),
            "TOPN must sort by grounded dim-level ref, got: {dax}"
        );

        // Must NOT contain a bare measure reference `[Customer First Name]` (the bug).
        // The presence of the grounded ref above means the bare ref check must allow
        // the table-qualified form but not a standalone [label].
        // We strip the table-qualified occurrences and check the remainder.
        let stripped = dax.replace("'customer_name_dimension'[Customer First Name]", "REPLACED");
        assert!(
            !stripped.contains("[Customer First Name]"),
            "bare measure ref must not appear in projection ORDER BY, got: {dax}"
        );

        // Must NOT have a trailing ORDER BY clause (TOPN already sorts, FR3).
        assert!(
            !dax.contains("ORDER BY"),
            "projection+limit must not emit trailing ORDER BY, got: {dax}"
        );
    }

    /// AC2 / FR2: projection with 4 order keys (New-Jersey shape) — all 4 must
    /// appear in the TOPN sort args, in declared order with declared directions.
    #[test]
    fn projection_four_order_keys_all_honored_in_topn() {
        let ctx = customer_ctx();
        let bound = proj_with_order(
            &[
                ("customer_name_dimension.[Customer First Name]", "customer_name_dimension"),
                ("customer_demographics.[Income Band]", "customer_demographics"),
                ("customer_demographics.[Vehicle Count]", "customer_demographics"),
                ("customer_demographics.[Buy Potential]", "customer_demographics"),
            ],
            vec![
                OrderKey { key: "customer_name_dimension.[Customer First Name]".to_string(), direction: SortDirection::Asc },
                OrderKey { key: "customer_demographics.[Income Band]".to_string(), direction: SortDirection::Asc },
                OrderKey { key: "customer_demographics.[Vehicle Count]".to_string(), direction: SortDirection::Asc },
                OrderKey { key: "customer_demographics.[Buy Potential]".to_string(), direction: SortDirection::Asc },
            ],
            Some(20),
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();

        assert!(dax.contains("TOPN(20,"), "got: {dax}");
        assert!(dax.contains("'customer_name_dimension'[Customer First Name]"), "got: {dax}");
        assert!(dax.contains("'customer_demographics'[Income Band]"), "got: {dax}");
        assert!(dax.contains("'customer_demographics'[Vehicle Count]"), "got: {dax}");
        assert!(dax.contains("'customer_demographics'[Buy Potential]"), "got: {dax}");
        // All four must appear as sort args, not just the first.
        let topn_start = dax.find("TOPN(20,").expect("TOPN must be present");
        let topn_segment = &dax[topn_start..];
        // Each grounded ref must appear in the TOPN expression.
        for label in &["Customer First Name", "Income Band", "Vehicle Count", "Buy Potential"] {
            assert!(
                topn_segment.contains(label),
                "TOPN must include sort key {label}, got: {topn_segment}"
            );
        }
        assert!(!dax.contains("ORDER BY"), "no trailing ORDER BY for projection+limit, got: {dax}");
    }

    /// AC3 / FR2: direction respected — DESC and ASC keys emit correct direction tokens.
    #[test]
    fn projection_order_direction_respected() {
        let ctx = customer_ctx();
        // income-band-9 shape: Vehicle Count DESC, Customer ID ASC.
        let bound = proj_with_order(
            &[
                ("customer_demographics.[Vehicle Count]", "customer_demographics"),
                ("customer_demographics.[Customer ID]", "customer_demographics"),
            ],
            vec![
                OrderKey { key: "customer_demographics.[Vehicle Count]".to_string(), direction: SortDirection::Desc },
                OrderKey { key: "customer_demographics.[Customer ID]".to_string(), direction: SortDirection::Asc },
            ],
            Some(20),
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();

        assert!(dax.contains("TOPN(20,"), "got: {dax}");
        // Verify that Vehicle Count appears before DESC and Customer ID before ASC.
        let topn_start = dax.find("TOPN(20,").expect("TOPN must be present");
        let topn_segment = &dax[topn_start..];
        // DESC must appear (for Vehicle Count).
        assert!(topn_segment.contains("DESC"), "DESC direction must appear, got: {topn_segment}");
        // ASC must appear (for Customer ID).
        assert!(topn_segment.contains("ASC"), "ASC direction must appear, got: {topn_segment}");
        assert!(!dax.contains("ORDER BY"), "no trailing ORDER BY for projection+limit, got: {dax}");
    }

    /// AC5 / FR4: measure-bearing ORDER BY is byte-identical when a projection
    /// change is NOT involved — guard that the non-projection path is unchanged.
    #[test]
    fn measure_bearing_orderby_uses_measure_ref_path() {
        let json = r#"{
            "catalog": "atscale_catalogs",
            "columns": [
                {"unique_name":"customer_name_dimension.[Customer First Name]","label":"Customer First Name","kind":"level","hierarchy":"customer_name_dimension","level":"Customer First Name"},
                {"unique_name":"tpcds.total_sales","label":"Total Sales","kind":"measure"}
            ]
        }"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        let bound = BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".into(),
                measures: vec![],
                dimensions: vec![mqo_spec::LevelSelection {
                    hierarchy: "customer_name_dimension".to_string(),
                    level: "customer_name_dimension.[Customer First Name]".to_string(),
                }],
                filters: vec![],
                limit: Some(10),
                order: Some(vec![OrderKey {
                    key: "tpcds.total_sales".to_string(),
                    direction: SortDirection::Desc,
                }]),
                time_intelligence: vec![],
                non_empty: false,
                projection: false, // measure-bearing, not projection
            },
            measures: vec![BoundMeasureInput {
                unique_name: "tpcds.total_sales".to_string(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
                trigger_hierarchies: vec![],
            }],
            dimensions: vec![BoundDimensionInput {
                unique_name: "customer_name_dimension.[Customer First Name]".to_string(),
                hierarchy: "customer_name_dimension".to_string(),
            }],
            calc_group_members: vec![],
        };
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        // Measure-bearing query with a limit should emit TOPN with measure ref.
        assert!(dax.contains("TOPN(10,"), "got: {dax}");
        assert!(dax.contains("[Total Sales]"), "measure ref must appear in TOPN, got: {dax}");
        // ORDER BY is suppressed for measure-bearing+limit: TOPN already handles sort,
        // and AtScale XMLA rejects ORDER BY on dimension refs after TOPN.
        assert!(!dax.contains("ORDER BY"), "measure-bearing+limit must NOT emit ORDER BY, got: {dax}");
    }

    /// AC6 (FR6): unresolvable order key in a projection should decline with a typed error.
    #[test]
    fn projection_unresolvable_order_key_declines() {
        let ctx = customer_ctx();
        let bound = proj_with_order(
            &[("customer_name_dimension.[Customer First Name]", "customer_name_dimension")],
            vec![OrderKey {
                key: "nonexistent_dimension.[No Such Level]".to_string(),
                direction: SortDirection::Asc,
            }],
            Some(20),
        );
        let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
        assert!(
            matches!(err, crate::DaxCompileError::UngroundableLevel { ref unique_name } if unique_name.contains("No Such Level")),
            "expected UngroundableLevel for bad order key, got: {err:?}"
        );
    }
}

// ── PRD-mqo-aggregation-orderby-dax-fix: measure-bearing ORDER BY with dim keys ─
#[cfg(test)]
mod aggregation_orderby_tests {
    use super::compile_grounded;
    use crate::catalog_context::DaxCatalogContext;
    use crate::input::{BoundDimensionInput, BoundMeasureInput, BoundMqoInput};
    use mqo_spec::{Mqo, OrderKey, SortDirection};

    fn agg_ctx() -> DaxCatalogContext {
        let json = r#"{
            "catalog": "atscale_catalogs",
            "columns": [
                {"unique_name":"items.[Item Product Name]","label":"Item Product Name","kind":"level","hierarchy":"items","level":"Item Product Name"},
                {"unique_name":"date_dim.[Sold Calendar Year]","label":"Sold Calendar Year","kind":"level","hierarchy":"date_dim","level":"Sold Calendar Year"},
                {"unique_name":"products.[Product category]","label":"Product category","kind":"level","hierarchy":"products","level":"Product category"},
                {"unique_name":"tpcds.total_net_profit","label":"Total Net Profit","kind":"measure"},
                {"unique_name":"tpcds.total_quantity_sold","label":"Total Quantity Sold","kind":"measure"},
                {"unique_name":"tpcds.total_product_count","label":"Total Product Count","kind":"measure"}
            ]
        }"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    fn agg_bound(
        measure_unique: &str,
        dim_unique: &str,
        dim_hierarchy: &str,
        order: Vec<OrderKey>,
        limit: Option<u64>,
    ) -> BoundMqoInput {
        BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".into(),
                measures: vec![],
                dimensions: vec![mqo_spec::LevelSelection {
                    hierarchy: dim_hierarchy.to_string(),
                    level: dim_unique.to_string(),
                }],
                filters: vec![],
                limit,
                order: if order.is_empty() { None } else { Some(order) },
                time_intelligence: vec![],
                non_empty: false,
                projection: false,
            },
            measures: vec![BoundMeasureInput {
                unique_name: measure_unique.to_string(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
                trigger_hierarchies: vec![],
            }],
            dimensions: vec![BoundDimensionInput {
                unique_name: dim_unique.to_string(),
                hierarchy: dim_hierarchy.to_string(),
            }],
            calc_group_members: vec![],
        }
    }

    /// AC1: dimension tie-breaker in a measure-bearing ORDER BY emits a grounded level ref.
    /// Shape: total-net-profit-per-product — measure DESC, dimension ASC.
    /// Uses limit=None so ORDER BY is emitted (not suppressed by the TOPN guard).
    #[test]
    fn measure_bearing_dimension_tiebreaker_resolves_to_level_ref() {
        let ctx = agg_ctx();
        let bound = agg_bound(
            "tpcds.total_net_profit",
            "items.[Item Product Name]",
            "items",
            vec![
                OrderKey { key: "tpcds.total_net_profit".to_string(), direction: SortDirection::Desc },
                OrderKey { key: "items.[Item Product Name]".to_string(), direction: SortDirection::Asc },
            ],
            None,
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        // Measure key must use measure ref (square-bracket only, no table prefix)
        assert!(dax.contains("[Total Net Profit] DESC"), "measure sort must use measure ref, got: {dax}");
        // Dimension key must use grounded level ref — table-qualified with the hierarchy name
        assert!(
            dax.contains("'items'[Item Product Name] ASC"),
            "dimension tiebreaker must be table-qualified (grounded), got: {dax}"
        );
    }

    /// AC2: dimension as the primary sort key for a measure-bearing aggregation.
    /// Shape: total-quantity-sold-per-year — ordered by Sold Calendar Year ASC.
    #[test]
    fn measure_bearing_dimension_primary_sort_resolves_to_level_ref() {
        let ctx = agg_ctx();
        let bound = agg_bound(
            "tpcds.total_quantity_sold",
            "date_dim.[Sold Calendar Year]",
            "date_dim",
            vec![OrderKey {
                key: "date_dim.[Sold Calendar Year]".to_string(),
                direction: SortDirection::Asc,
            }],
            None,
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        // Must emit a grounded (table-qualified) level ref, not a bare measure ref
        assert!(
            dax.contains("'date_dim'[Sold Calendar Year] ASC"),
            "dimension primary sort must be table-qualified (grounded), got: {dax}"
        );
    }

    /// AC3: all order keys honored, directions preserved across a mixed list (no limit).
    #[test]
    fn measure_bearing_mixed_order_honors_all_keys_and_directions() {
        let ctx = agg_ctx();
        let bound = agg_bound(
            "tpcds.total_net_profit",
            "items.[Item Product Name]",
            "items",
            vec![
                OrderKey { key: "tpcds.total_net_profit".to_string(), direction: SortDirection::Desc },
                OrderKey { key: "items.[Item Product Name]".to_string(), direction: SortDirection::Asc },
            ],
            None,
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        let order_pos = dax.find("ORDER BY").expect("must emit ORDER BY when no limit");
        let order_clause = &dax[order_pos..];
        // Both keys must appear in ORDER BY, measure first (declared order), direction correct
        let measure_pos = order_clause.find("[Total Net Profit] DESC").expect("measure key missing");
        let dim_pos = order_clause.find("'items'[Item Product Name] ASC").expect("dimension key missing");
        assert!(measure_pos < dim_pos, "measure key must precede dimension key in declared order");
    }

    /// AC4: measure-bearing with a TOPN limit must NOT emit ORDER BY.
    /// AtScale XMLA rejects ORDER BY on dimension column refs after TOPN, causing j=None
    /// engine errors on queries like "top 20 stores by employee count, tie-break by name".
    #[test]
    fn measure_bearing_with_limit_suppresses_order_by() {
        let ctx = agg_ctx();
        let bound = agg_bound(
            "tpcds.total_net_profit",
            "items.[Item Product Name]",
            "items",
            vec![
                OrderKey { key: "tpcds.total_net_profit".to_string(), direction: SortDirection::Desc },
                OrderKey { key: "items.[Item Product Name]".to_string(), direction: SortDirection::Asc },
            ],
            Some(20),
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            !dax.contains("ORDER BY"),
            "measure-bearing+limit must NOT emit ORDER BY (TOPN handles sort), got:\n{dax}"
        );
        // TOPN wrapper must still be present with the correct measure sort key
        assert!(dax.contains("TOPN(20"), "TOPN limit must still be present, got:\n{dax}");
    }

    /// AC1 (PRD-mqo-orderby-fallback-to-measure-ref): an order key that is NOT in
    /// bound.measures and cannot ground as a dimension level falls back to a measure
    /// reference (measure_dax_ref_ctx), returning Ok instead of propagating
    /// UngroundableLevel (v0.16.0 regression removed in v0.17.0).
    /// Uses limit=None so ORDER BY is emitted (not suppressed by the TOPN guard).
    #[test]
    fn measure_bearing_unresolvable_order_key_falls_back_to_measure_ref() {
        let ctx = agg_ctx();
        let bound = agg_bound(
            "tpcds.total_net_profit",
            "items.[Item Product Name]",
            "items",
            vec![
                OrderKey { key: "tpcds.total_net_profit".to_string(), direction: SortDirection::Desc },
                OrderKey { key: "nonexistent.[No Such Dim]".to_string(), direction: SortDirection::Asc },
            ],
            None,
        );
        let dax = compile_grounded(&bound, Some(&ctx)).expect("should return Ok with measure-ref fallback");
        // The unresolvable key falls back to measure_dax_ref_ctx → [[No Such Dim]] form.
        assert!(
            dax.contains("[[No Such Dim]]"),
            "expected measure-ref fallback [[No Such Dim]] in ORDER BY, got:\n{dax}"
        );
        // The bound measure key still uses the proper measure ref.
        assert!(dax.contains("[Total Net Profit] DESC"), "expected bound measure ref in ORDER BY");
    }
}

// ── PRD-mqo-projection-dax-grounding: per-level table + filter key alignment ────
#[cfg(test)]
mod projection_grounding_tests {
    use super::compile_grounded;
    use crate::catalog_context::DaxCatalogContext;
    use crate::input::{BoundDimensionInput, BoundMqoInput};
    use mqo_spec::Mqo;

    /// Catalog whose `catalog` (database) name is `atscale_catalogs` — the live
    /// failure case. `ship_mode` hierarchy has Carrier + Ship Mode Type levels.
    /// A space-bearing hierarchy ("Ship Mode") is included for the FR-3 quote test.
    fn ship_mode_ctx() -> DaxCatalogContext {
        let json = r#"{
            "catalog": "atscale_catalogs",
            "schema": "tpcds_Snowflake",
            "columns": [
                {"unique_name":"ship_mode.[Carrier]","label":"Carrier","kind":"level","hierarchy":"ship_mode","level":"Carrier"},
                {"unique_name":"ship_mode.[Ship Mode Type]","label":"Ship Mode Type","kind":"level","hierarchy":"ship_mode","level":"Ship Mode Type"},
                {"unique_name":"store_dimension.[Store Name]","label":"Store Name","kind":"level","hierarchy":"store_dimension","level":"Store Name"},
                {"unique_name":"store_dimension.[Store Manager]","label":"Store Manager","kind":"level","hierarchy":"store_dimension","level":"Store Manager"}
            ]
        }"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    fn proj(dims: &[&str]) -> BoundMqoInput {
        BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".into(),
                measures: vec![],
                // is_projection() requires mqo.dimensions to be non-empty.
                dimensions: dims
                    .iter()
                    .map(|u| mqo_spec::LevelSelection {
                        hierarchy: u.split('.').next().unwrap_or("").to_string(),
                        level: (*u).to_string(),
                    })
                    .collect(),
                filters: vec![],
                limit: None,
                order: None,
                time_intelligence: vec![],
                non_empty: false,
                projection: true,
            },
            measures: vec![],
            dimensions: dims
                .iter()
                .map(|u| BoundDimensionInput {
                    unique_name: (*u).to_string(),
                    hierarchy: u.split('.').next().unwrap_or("").to_string(),
                })
                .collect(),
            calc_group_members: vec![],
        }
    }

    /// AC-2 (dimension half): the projection dimension grounds to the per-level
    /// physical table `ship_mode`, NOT the database name `atscale_catalogs`.
    #[test]
    fn ac2_projection_dim_grounds_to_hierarchy_table() {
        let ctx = ship_mode_ctx();
        let dax = compile_grounded(&proj(&["ship_mode.[Carrier]"]), Some(&ctx)).unwrap();
        assert!(dax.contains("'ship_mode'[Carrier]"), "got: {dax}");
        assert!(!dax.contains("atscale_catalogs"), "must not use db name, got: {dax}");
        assert!(!dax.contains("/* ungrounded"), "no ungrounded annotation, got: {dax}");
    }

    /// AC-2 (filter half) + FR-2: a `MemberLevel` filter whose `level` is the BARE
    /// label ("Ship Mode Type") still grounds to `'ship_mode'[Ship Mode Type]` —
    /// no `/* ungrounded */`, no unquoted space-bearing identifier.
    #[test]
    fn ac2_member_level_filter_bare_label_grounds() {
        let ctx = ship_mode_ctx();
        let mut bound = proj(&["ship_mode.[Carrier]"]);
        bound.mqo.filters = vec![mqo_spec::Filter::MemberLevel {
            hierarchy: "ship_mode".into(),
            level: "Ship Mode Type".into(), // bare label, as the live failure carried
            members: vec!["EXPRESS".into()],
            exclude: false,
        }];
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(dax.contains("'ship_mode'[Ship Mode Type]"), "got: {dax}");
        assert!(!dax.contains("/* ungrounded"), "got: {dax}");
        // No unquoted multi-word identifier (the old bug emitted `Ship Mode Type[...]`).
        assert!(!dax.contains(" Ship Mode Type["), "no unquoted space-bearing ident, got: {dax}");
    }

    /// FR-2: the same filter with a FULL `unique_name` as `level` grounds identically.
    #[test]
    fn member_level_filter_unique_name_grounds_identically() {
        let ctx = ship_mode_ctx();
        let mut bound = proj(&["ship_mode.[Carrier]"]);
        bound.mqo.filters = vec![mqo_spec::Filter::MemberLevel {
            hierarchy: "ship_mode".into(),
            level: "ship_mode.[Ship Mode Type]".into(),
            members: vec!["EXPRESS".into()],
            exclude: false,
        }];
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(dax.contains("'ship_mode'[Ship Mode Type]"), "got: {dax}");
    }

    /// AC-3: multi-level projection — each level grounds to its physical table.
    #[test]
    fn ac3_multi_level_projection_grounds_each_table() {
        let ctx = ship_mode_ctx();
        let dax = compile_grounded(
            &proj(&["store_dimension.[Store Name]", "store_dimension.[Store Manager]"]),
            Some(&ctx),
        )
        .unwrap();
        assert!(dax.contains("'store_dimension'[Store Name]"), "got: {dax}");
        assert!(dax.contains("'store_dimension'[Store Manager]"), "got: {dax}");
    }

    /// AC-4: a projection level absent from the catalog declines with a typed
    /// `UngroundableLevel` naming the level — no DAX emitted.
    #[test]
    fn ac4_ungroundable_projection_level_declines() {
        let ctx = ship_mode_ctx();
        let err = compile_grounded(&proj(&["nope.[Mystery]"]), Some(&ctx)).unwrap_err();
        assert!(
            matches!(err, crate::DaxCompileError::UngroundableLevel { ref unique_name } if unique_name == "nope.[Mystery]"),
            "expected UngroundableLevel(nope.[Mystery]), got: {err:?}"
        );
    }

    /// AC-5: a hierarchy/table name containing a space is single-quoted.
    #[test]
    fn ac5_space_bearing_table_is_quoted() {
        let json = r#"{
            "catalog": "atscale_catalogs",
            "columns": [
                {"unique_name":"Ship Mode.[Carrier]","label":"Carrier","kind":"level","hierarchy":"Ship Mode","level":"Carrier"}
            ]
        }"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        let dax = compile_grounded(&proj(&["Ship Mode.[Carrier]"]), Some(&ctx)).unwrap();
        assert!(dax.contains("'Ship Mode'[Carrier]"), "space-bearing table must be quoted, got: {dax}");
    }
}

// ── PRD-mqo-date-member-cross-dimension-filter: combined filter integration ──
#[cfg(test)]
mod date_cross_dimension_codegen_tests {
    use super::compile_grounded;
    use crate::catalog_context::DaxCatalogContext;
    use crate::input::{BoundDimensionInput, BoundMeasureInput, BoundMqoInput};
    use mqo_spec::{Filter, Mqo};

    /// Catalog with:
    /// - `sold_date_dimensions`: Sold Calendar Year (domain: years) + Sold Date Key (no year-exact domain)
    /// - `store_dimension`: Store Name (domain: store names) + Gender (domain: F, M)
    /// - measure: Net Profit
    fn tpcds_combined_ctx() -> DaxCatalogContext {
        let json = r#"{
            "catalog": "atscale_catalogs",
            "columns": [
                {"unique_name":"sold_date_dimensions.[Sold Calendar Year]","label":"Sold Calendar Year","kind":"level","hierarchy":"sold_date_dimensions","level":"Year","domain":["1998","1999","2000","2001","2002","2003"]},
                {"unique_name":"sold_date_dimensions.[Sold Date Key]","label":"Sold Date Key","kind":"level","hierarchy":"sold_date_dimensions","level":"Date Key","domain":["20020101","20020102","20011231"]},
                {"unique_name":"store_dimension.[Store Name]","label":"Store Name","kind":"level","hierarchy":"store_dimension","level":"Store Name","domain":["ese","bar","baz"]},
                {"unique_name":"store_dimension.[Gender]","label":"Gender","kind":"level","hierarchy":"store_dimension","level":"Gender","domain":["F","M"]},
                {"unique_name":"tpcds.net_profit","label":"Net Profit","kind":"measure"}
            ]
        }"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    /// Build a `BoundMqoInput` with two Member filters and optional group-by dimensions.
    fn bound_two_filters(
        f1_hierarchy: &str,
        f1_member: &str,
        f2_hierarchy: &str,
        f2_member: &str,
        dim_unique_names: &[&str],
    ) -> BoundMqoInput {
        BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".to_string(),
                measures: vec![],
                dimensions: dim_unique_names
                    .iter()
                    .map(|u| mqo_spec::LevelSelection {
                        hierarchy: u.split('.').next().unwrap_or("").to_string(),
                        level: (*u).to_string(),
                    })
                    .collect(),
                filters: vec![
                    Filter::Member {
                        hierarchy: f1_hierarchy.to_string(),
                        members: vec![f1_member.to_string()],
                    },
                    Filter::Member {
                        hierarchy: f2_hierarchy.to_string(),
                        members: vec![f2_member.to_string()],
                    },
                ],
                limit: None,
                order: None,
                time_intelligence: vec![],
                non_empty: false,
                projection: false,
            },
            measures: vec![BoundMeasureInput {
                unique_name: "tpcds.net_profit".to_string(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
                trigger_hierarchies: vec![],
            }],
            dimensions: dim_unique_names
                .iter()
                .map(|u| BoundDimensionInput {
                    unique_name: (*u).to_string(),
                    hierarchy: u.split('.').next().unwrap_or("").to_string(),
                })
                .collect(),
            calc_group_members: vec![],
        }
    }

    /// AC1 (FR1): Combined date-year filter + store-name filter — both legs bind.
    /// Verifies `customers-ese-store-2001` style query.
    #[test]
    fn ac1_combined_date_and_store_name_filter_both_legs_bind() {
        let ctx = tpcds_combined_ctx();
        let bound = bound_two_filters(
            "sold_date_dimensions", "2001",
            "store_dimension", "ese",
            &[],
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        // Date leg must bind to the year level (not Sold Date Key).
        assert!(
            dax.contains("[Sold Calendar Year]"),
            "date leg must bind to Sold Calendar Year, got: {dax}"
        );
        // Non-date leg must bind to Store Name.
        assert!(
            dax.contains("[Store Name]"),
            "non-date leg must bind to Store Name, got: {dax}"
        );
        // Both filter predicates must appear in ONE SUMMARIZECOLUMNS.
        assert!(
            dax.contains("SUMMARIZECOLUMNS"),
            "must emit SUMMARIZECOLUMNS, got: {dax}"
        );
        assert_eq!(
            dax.matches("KEEPFILTERS").count(),
            2,
            "must emit exactly two KEEPFILTERS (one per filter leg), got: {dax}"
        );
    }

    /// AC2 (FR2): Year member resolves to year level, not date-key level.
    /// A date-key level in the catalog does NOT steal the year binding.
    #[test]
    fn ac2_year_2002_binds_to_year_level_not_date_key() {
        let ctx = tpcds_combined_ctx();
        let bound = bound_two_filters(
            "sold_date_dimensions", "2002",
            "store_dimension", "ese",
            &[],
        );
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        assert!(
            dax.contains("[Sold Calendar Year]"),
            "2002 must bind to Sold Calendar Year, got: {dax}"
        );
        assert!(
            !dax.contains("[Sold Date Key]"),
            "2002 must NOT bind to Sold Date Key, got: {dax}"
        );
    }

    /// AC3 (FR6): Year as filter + Store/Gender as group-by dimensions.
    /// Verifies `net-profit-tier-by-store-gender-2002` style query.
    #[test]
    fn ac3_year_filter_plus_store_gender_groupby() {
        let ctx = tpcds_combined_ctx();
        // Year is a filter; Store Name and Gender are group-by dimensions.
        let mut bound = BoundMqoInput {
            mqo: Mqo {
                model: "tpcds".to_string(),
                measures: vec![],
                dimensions: vec![
                    mqo_spec::LevelSelection {
                        hierarchy: "store_dimension".to_string(),
                        level: "store_dimension.[Store Name]".to_string(),
                    },
                    mqo_spec::LevelSelection {
                        hierarchy: "store_dimension".to_string(),
                        level: "store_dimension.[Gender]".to_string(),
                    },
                ],
                filters: vec![Filter::Member {
                    hierarchy: "sold_date_dimensions".to_string(),
                    members: vec!["2002".to_string()],
                }],
                limit: None,
                order: None,
                time_intelligence: vec![],
                non_empty: false,
                projection: false,
            },
            measures: vec![BoundMeasureInput {
                unique_name: "tpcds.net_profit".to_string(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
                trigger_hierarchies: vec![],
            }],
            dimensions: vec![
                BoundDimensionInput {
                    unique_name: "store_dimension.[Store Name]".to_string(),
                    hierarchy: "store_dimension".to_string(),
                },
                BoundDimensionInput {
                    unique_name: "store_dimension.[Gender]".to_string(),
                    hierarchy: "store_dimension".to_string(),
                },
            ],
            calc_group_members: vec![],
        };
        let _ = &mut bound; // suppress warning
        let dax = compile_grounded(&bound, Some(&ctx)).unwrap();
        // Year filter must appear.
        assert!(
            dax.contains("[Sold Calendar Year]"),
            "date filter must reference year level, got: {dax}"
        );
        // Group-by dimensions must appear.
        assert!(
            dax.contains("[Store Name]"),
            "Store Name group-by must appear, got: {dax}"
        );
        assert!(
            dax.contains("[Gender]"),
            "Gender group-by must appear, got: {dax}"
        );
        // The year must appear as a FILTER, not only as a group-by column.
        assert!(
            dax.contains("KEEPFILTERS"),
            "year must be expressed as a KEEPFILTERS filter, got: {dax}"
        );
        // SUMMARIZECOLUMNS must be present.
        assert!(
            dax.contains("SUMMARIZECOLUMNS"),
            "must emit SUMMARIZECOLUMNS, got: {dax}"
        );
    }

    /// AC4 (FR3): Date leg unresolvable → decline naming the date filter.
    /// When the year member can't be matched, the whole query declines.
    #[test]
    fn ac4_date_leg_unresolvable_declines_loud() {
        let ctx = tpcds_combined_ctx();
        // "9999" is not in any domain.
        let bound = bound_two_filters(
            "sold_date_dimensions", "9999",
            "store_dimension", "ese",
            &[],
        );
        let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
        assert!(
            matches!(err, crate::DaxCompileError::UngroundedMemberFilter { ref hierarchy, .. }
                if hierarchy == "sold_date_dimensions"),
            "should decline naming the date filter hierarchy, got: {err:?}"
        );
    }

    /// AC5 (FR3): Non-date leg unresolvable → decline naming the non-date filter.
    #[test]
    fn ac5_nondate_leg_unresolvable_declines_loud() {
        let ctx = tpcds_combined_ctx();
        // "unknown_store" is not in any domain.
        let bound = bound_two_filters(
            "sold_date_dimensions", "2002",
            "store_dimension", "unknown_store",
            &[],
        );
        let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
        assert!(
            matches!(err, crate::DaxCompileError::UngroundedMemberFilter { ref hierarchy, .. }
                if hierarchy == "store_dimension"),
            "should decline naming the non-date filter hierarchy, got: {err:?}"
        );
    }
}
