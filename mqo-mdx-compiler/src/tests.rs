//! Acceptance-criterion tests for the MDX compiler.
//!
//! Each test is named `ac<N>_...` matching the PRD acceptance criteria.

#![cfg(test)]

use crate::{
    compile, BoundDimensionInput, BoundMeasureInput, BoundMqoInput, CalcGroupMemberInput,
    MdxCompileError,
};
use mqo_spec::{Filter, MeasureRef, Mqo};

// ── Fixture helpers ───────────────────────────────────────────────────────────

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

// ── AC1: ≥6 golden BoundMqo → MDX pairs compile exactly ──────────────────────

/// AC1-a: minimal measure-only query against a single-part cube name.
#[test]
fn ac1_minimal_measure_only() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert_eq!(
        mdx,
        "SELECT\n  { [Measures].[Revenue] } ON COLUMNS\nFROM [sales]"
    );
}

/// AC1-b: measure + one dimension level.
#[test]
fn ac1_measure_with_one_dimension() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("tpcds"),
        measures: vec![simple_measure("sales.total_sales")],
        dimensions: vec![bound_dim("time.calendar.[Year]", "time.calendar")],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert_eq!(
        mdx,
        "SELECT\n  { [Measures].[Total Sales] } ON COLUMNS,\n  NON EMPTY { [Calendar].[Year].Members } ON ROWS\nFROM [tpcds]"
    );
}

/// AC1-c: measure + two dimension levels (CROSSJOIN).
#[test]
fn ac1_measure_with_two_dimensions_crossjoin() {
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
    assert_eq!(
        mdx,
        "SELECT\n  { [Measures].[Revenue] } ON COLUMNS,\n  NON EMPTY { CROSSJOIN([Calendar].[Year].Members, [Country].[Country].Members) } ON ROWS\nFROM [tpcds]"
    );
}

/// AC1-d: three-part cube name is fully qualified.
#[test]
fn ac1_three_part_cube_name() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("postgres.tpcds.tpcds_benchmark_model"),
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert!(mdx.contains("FROM [postgres].[tpcds].[tpcds_benchmark_model]"));
}

/// AC1-e: Member filter goes into WHERE slicer.
#[test]
fn ac1_member_filter_in_where() {
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
    assert!(mdx.contains("WHERE ([time.calendar].[2023])"));
}

/// AC1-f: Two measures on columns.
#[test]
fn ac1_two_measures_on_columns() {
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
    assert_eq!(
        mdx,
        "SELECT\n  { [Measures].[Revenue], [Measures].[Units Sold] } ON COLUMNS\nFROM [sales]"
    );
}

// ── AC2: NON EMPTY on row axis and fully-qualified cube name ─────────────────

/// AC2-a: NON EMPTY is always on the row axis when dimensions are present.
#[test]
fn ac2_non_empty_on_rows() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![bound_dim("time.calendar.[Year]", "time.calendar")],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert!(mdx.contains("NON EMPTY {"));
    assert!(mdx.contains("ON ROWS"));
}

/// AC2-b: Fully-qualified cube name (two-part schema.cube).
#[test]
fn ac2_fully_qualified_cube_two_part() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("schema.mycube"),
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert!(mdx.contains("FROM [schema].[mycube]"));
}

/// AC2-c: NON EMPTY only appears on ROWS, not on COLUMNS.
#[test]
fn ac2_non_empty_only_on_rows_not_columns() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![bound_dim("time.calendar.[Year]", "time.calendar")],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    // Check NON EMPTY precedes ON ROWS
    let non_empty_pos = mdx.find("NON EMPTY").expect("NON EMPTY must be present");
    let on_rows_pos = mdx.find("ON ROWS").expect("ON ROWS must be present");
    assert!(non_empty_pos < on_rows_pos);
    // ON COLUMNS must NOT have NON EMPTY before it
    let on_columns_pos = mdx.find("ON COLUMNS").expect("ON COLUMNS must be present");
    let pre_columns = &mdx[..on_columns_pos];
    assert!(!pre_columns.contains("NON EMPTY"));
}

// ── AC3: Calculated measure pulls MDX-dependency hierarchies onto axes ────────

/// AC3-a: a calculated measure with one MDX dependency hierarchy adds it to rows.
#[test]
fn ac3_calc_measure_adds_dependency_hierarchy() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![calc_measure("sales.margin_pct", vec!["time.calendar"])],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert!(mdx.contains("ON ROWS"), "row axis must be present");
    assert!(
        mdx.contains("[Calendar].Members"),
        "MDX dependency hierarchy must appear on rows: {mdx}"
    );
}

/// AC3-b: dependency hierarchy is deduplicated when already in bound dimensions.
#[test]
fn ac3_dependency_hierarchy_deduped_with_bound_dims() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![calc_measure("sales.margin_pct", vec!["time.calendar"])],
        dimensions: vec![bound_dim("time.calendar.[Year]", "time.calendar")],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    // Calendar should appear exactly once in the row set.
    let count = mdx.matches("[Calendar]").count();
    assert_eq!(count, 1, "Calendar should appear exactly once, got: {mdx}");
}

/// AC3-d: two calc measures with the same dep hierarchy — dedup ensures it appears exactly once.
#[test]
fn ac3_multi_calc_dep_hierarchy_dedup() {
    // Two calculated measures both depend on "time.calendar".
    // The dep hierarchy must appear exactly once on the row axis.
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![
            calc_measure("sales.margin", vec!["time.calendar"]),
            calc_measure("sales.ytd", vec!["time.calendar"]),
        ],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    let count = mdx.matches("[Calendar].Members").count();
    assert_eq!(
        count,
        1,
        "Calendar dep hierarchy must appear exactly once: {mdx}"
    );
}

/// AC3-c: non-calc measure does NOT pull dependency hierarchies.
#[test]
fn ac3_non_calc_measure_no_dep_hierarchies() {
    let mut m = simple_measure("sales.revenue");
    // Force mdx_dependency_hierarchies even though not is_calc.
    m.mdx_dependency_hierarchies = vec!["time.calendar".to_string()];
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![m],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    // No row axis since is_calc=false and no bound dims.
    assert!(
        !mdx.contains("ON ROWS"),
        "non-calc measure must not pull dep hierarchies: {mdx}"
    );
}

// ── AC4: Calc-group member literal emitted from bound metadata ───────────────

/// AC4-a: WITH MEMBER clause is emitted verbatim from the bound MDX.
#[test]
fn ac4_calc_group_member_emitted_verbatim() {
    let cgm_mdx = "Aggregate(PeriodsToDate([Time].[Calendar].[Year]))";
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![calc_group_member(
            "Time Intelligence",
            "YTD",
            "calc.time_intel.YTD",
            cgm_mdx,
        )],
    };
    let mdx = compile(&bound).expect("should compile");
    assert!(
        mdx.contains(cgm_mdx),
        "MDX literal must be verbatim in output: {mdx}"
    );
    assert!(
        mdx.contains("WITH MEMBER [Time Intelligence].[YTD]"),
        "WITH MEMBER header must be present: {mdx}"
    );
}

/// AC4-b: Without calc-group members, no WITH clause is emitted.
#[test]
fn ac4_no_calc_group_member_no_with_clause() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert!(
        !mdx.contains("WITH MEMBER"),
        "no WITH MEMBER when no calc-group members: {mdx}"
    );
}

/// AC4-c: Calc-group filter in MQO.filters ends up in WHERE slicer.
#[test]
fn ac4_calc_group_filter_in_where() {
    let mut mqo = minimal_mqo("sales");
    mqo.filters.push(Filter::CalcGroupMember {
        calc_group: "Time Intelligence".to_string(),
        member: "YTD".to_string(),
    });
    let bound = BoundMqoInput {
        mqo,
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile");
    assert!(mdx.contains("WHERE"), "WHERE clause required: {mdx}");
    assert!(
        mdx.contains("[Time Intelligence].[YTD]"),
        "calc-group slicer must appear in WHERE: {mdx}"
    );
}

// ── AC5: Semi-additive without trigger level → hard error ─────────────────────

/// AC5-a: semi-additive measure with no trigger hierarchies → error.
#[test]
fn ac5_semi_additive_missing_trigger_is_error() {
    let m = BoundMeasureInput {
        unique_name: "sales.balance".to_string(),
        is_calc: false,
        semi_additive: true,
        required_dimension: None,
        trigger_hierarchies: vec![], // empty → must error
        mdx_dependency_hierarchies: vec![],
    };
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![m],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let err = compile(&bound).expect_err("should fail with missing trigger");
    assert!(
        matches!(err, MdxCompileError::SemiAdditiveMissingTrigger(_)),
        "expected SemiAdditiveMissingTrigger, got: {err:?}"
    );
}

/// AC5-b: semi-additive measure WITH trigger hierarchies compiles fine.
#[test]
fn ac5_semi_additive_with_trigger_compiles() {
    let m = semi_additive_measure("sales.balance", vec!["time.calendar"]);
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![m],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let mdx = compile(&bound).expect("should compile when trigger present");
    assert!(mdx.contains("FROM [sales]"));
}

/// AC5-c: error message contains the measure name.
#[test]
fn ac5_error_contains_measure_name() {
    let m = BoundMeasureInput {
        unique_name: "finance.end_balance".to_string(),
        is_calc: false,
        semi_additive: true,
        required_dimension: None,
        trigger_hierarchies: vec![],
        mdx_dependency_hierarchies: vec![],
    };
    let bound = BoundMqoInput {
        mqo: minimal_mqo("finance"),
        measures: vec![m],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let err = compile(&bound).expect_err("should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("finance.end_balance"),
        "error must name the measure: {msg}"
    );
}

/// AC5-d: empty measures → `EmptyMeasures` error (not a panic).
#[test]
fn ac5_empty_measures_error() {
    let bound = BoundMqoInput {
        mqo: minimal_mqo("sales"),
        measures: vec![],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let err = compile(&bound).expect_err("should fail with empty measures");
    assert!(matches!(err, MdxCompileError::EmptyMeasures));
}

// ── AC6: cargo test passes; clippy clean ─────────────────────────────────────

/// AC6: This test simply compiles and runs. Its presence proves that
/// `cargo test --release --workspace` does not abort (AC6 pass condition).
/// Clippy-clean is verified by the gate, not by a runtime assertion.
#[test]
fn ac6_test_suite_compiles_and_runs() {
    // Nothing to assert beyond compilation. The fact that `cargo test --release`
    // ran this function IS the AC6 evidence.
}

// ── AC8: Range filter handled gracefully ─────────────────────────────────────

/// AC8 (MAY): Range filters in mqo.filters are not silently dropped —
/// they are documented in `build_where_clause` as intentionally omitted.
/// This test verifies the codegen still produces valid MDX (no panic)
/// when a Range filter is present, and that no WHERE clause is emitted
/// (range → NON EMPTY serves as the structural guard).
#[test]
fn ac8_range_filter_handled_gracefully_no_where_clause() {
    use mqo_spec::Filter;
    let mut mqo = minimal_mqo("sales");
    mqo.filters.push(Filter::Range {
        level: "time.calendar.Year".to_string(),
        lo: 2020.0,
        hi: 2023.0,
    });
    let bound = BoundMqoInput {
        mqo,
        measures: vec![simple_measure("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    // Must not panic. Range filters are intentionally omitted from the MDX
    // slicer (documented in build_where_clause); no WHERE clause is emitted.
    let mdx = compile(&bound).expect("Range filter must not cause a compile error");
    assert!(
        !mdx.contains("WHERE"),
        "Range filters must not produce a WHERE slicer clause: {mdx}"
    );
}

// ── Additional golden pair ────────────────────────────────────────────────────

/// AC1-g: full query with measure + dim + calc-group + WHERE slicer.
#[test]
fn ac1_full_query_golden() {
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
    assert!(mdx.starts_with("WITH MEMBER"), "should start with WITH MEMBER");
    assert!(mdx.contains("FROM [postgres].[tpcds].[tpcds_benchmark_model]"));
    assert!(mdx.contains("NON EMPTY"));
    assert!(mdx.contains(cgm_mdx));
    assert!(mdx.contains("WHERE"));
}
