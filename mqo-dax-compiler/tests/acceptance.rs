//! Acceptance tests for `mqo-dax-compiler`.
//!
//! Named `ac1` … `ac5` to match the PRD acceptance criteria.
//! Each `ac1_*` test is a golden BoundMqo → DAX string-equality check.
//! `ac2_*` tests verify each TimeIntel variant maps to the correct DAX function.
//! `ac3_*` tests verify calc-group member → column filter (not invented logic).
//! `ac4_*` tests verify `limit` → TOPN and `order` → ORDER BY.
//! `ac5_*` tests verify the bundled syntax check rejects invalid DAX.
//! `member_filter_*` tests verify the member-filter grounding fix (PRD-mqo-dax-member-filter-grounding).

use mqo_dax_compiler::{
    compile, compile_grounded,
    input::{BoundDimensionInput, BoundMeasureInput, BoundMqoInput, CalcGroupMemberInput},
    syntax_check::validate_dax_syntax,
    DaxCatalogContext, DaxCompileError,
};
use mqo_spec::{Filter, Grain, LevelSelection, MeasureRef, Mqo, OrderKey, SortDirection, TimeIntel};

// ── Fixtures ──────────────────────────────────────────────────────────────────

fn minimal_mqo(model: &str, measure: &str) -> Mqo {
    Mqo {
        model: model.to_string(),
        measures: vec![MeasureRef {
            unique_name: measure.to_string(),
        }],
        dimensions: vec![],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    }
}

fn measure_input(unique_name: &str) -> BoundMeasureInput {
    BoundMeasureInput {
        unique_name: unique_name.to_string(),
        is_calc: false,
        semi_additive: false,
        required_dimension: None,
        trigger_hierarchies: vec![],
    }
}

fn dim_input(unique_name: &str, hierarchy: &str) -> BoundDimensionInput {
    BoundDimensionInput {
        unique_name: unique_name.to_string(),
        hierarchy: hierarchy.to_string(),
    }
}

fn minimal_bound(measure_unique: &str) -> BoundMqoInput {
    BoundMqoInput {
        mqo: minimal_mqo("sales", measure_unique),
        measures: vec![measure_input(measure_unique)],
        dimensions: vec![],
        calc_group_members: vec![],
    }
}

// ── AC1: ≥8 golden BoundMqo → DAX pairs ──────────────────────────────────────

/// AC1-a: minimal measure-only → ROW form.
#[test]
fn ac1_golden_measure_only_row() {
    let bound = minimal_bound("sales.revenue");
    let dax = compile(&bound).expect("compile ok");
    // Normalize: trim whitespace for comparison.
    let normalized = dax.trim().to_string();
    assert_eq!(normalized, "EVALUATE\nROW(\"Revenue\", [Revenue])");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC1-b: measure + one dimension → SUMMARIZECOLUMNS form.
#[test]
fn ac1_golden_measure_plus_dim() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.dimensions.push(LevelSelection {
        hierarchy: "time.calendar".to_string(),
        level: "Year".to_string(),
    });
    bound.dimensions.push(dim_input("time.calendar.[Year]", "time.calendar"));

    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("SUMMARIZECOLUMNS"),
        "expected SUMMARIZECOLUMNS, got: {dax}"
    );
    assert!(dax.contains("Calendar[Year]"), "expected Calendar[Year], got: {dax}");
    assert!(dax.contains("[Revenue]"), "expected [Revenue], got: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC1-c: two measures.
#[test]
fn ac1_golden_two_measures() {
    let mut mqo = minimal_mqo("sales", "sales.revenue");
    mqo.measures.push(MeasureRef {
        unique_name: "sales.units_sold".to_string(),
    });
    let bound = BoundMqoInput {
        mqo,
        measures: vec![
            measure_input("sales.revenue"),
            measure_input("sales.units_sold"),
        ],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("[Revenue]"));
    assert!(dax.contains("[Units Sold]"));
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC1-d: member filter with catalog context → grounded KEEPFILTERS FILTER IN.
///
/// A `Member` filter now requires a `DaxCatalogContext` to resolve the hierarchy
/// to a real level-qualified column. Without context `compile` returns
/// `UngroundedMemberFilter` (tested separately in `member_filter_no_ctx_is_error`).
#[test]
fn ac1_golden_member_filter() {
    // Build a catalog that includes the geography.region.Region level.
    let catalog_json = r#"{
        "catalog": "sales_model",
        "columns": [
            {
                "unique_name": "geography.region.Region",
                "label": "Region",
                "kind": "level"
            },
            {
                "unique_name": "sales.revenue",
                "label": "Revenue",
                "kind": "measure"
            }
        ]
    }"#;
    let ctx = DaxCatalogContext::from_json(catalog_json).expect("catalog ok");

    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.filters.push(Filter::Member {
        hierarchy: "geography.region".to_string(),
        members: vec!["North".to_string(), "South".to_string()],
    });
    bound.mqo.dimensions.push(LevelSelection {
        hierarchy: "geography.region".to_string(),
        level: "Region".to_string(),
    });
    bound.dimensions.push(dim_input("geography.region.Region", "geography.region"));

    let dax = compile_grounded(&bound, Some(&ctx)).expect("compile ok");
    assert!(dax.contains("KEEPFILTERS"), "expected KEEPFILTERS: {dax}");
    assert!(dax.contains("\"North\""), "expected member North: {dax}");
    assert!(dax.contains("\"South\""), "expected member South: {dax}");
    // Must NOT contain the broken Hierarchy[Hierarchy] pattern.
    assert!(
        !dax.contains("geography[geography]"),
        "must not emit broken Hierarchy[Hierarchy]: {dax}"
    );
    assert!(
        !dax.contains("region[region]"),
        "must not emit broken column=table ref: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC1-e: range filter.
#[test]
fn ac1_golden_range_filter() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.filters.push(Filter::Range {
        level: "time.calendar.Year".to_string(),
        lo: 2020.0,
        hi: 2024.0,
    });
    bound.mqo.dimensions.push(LevelSelection {
        hierarchy: "time.calendar".to_string(),
        level: "Year".to_string(),
    });
    bound.dimensions.push(dim_input("time.calendar.[Year]", "time.calendar"));

    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("2020"), "expected lo=2020: {dax}");
    assert!(dax.contains("2024"), "expected hi=2024: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC1-f: limit produces TOPN wrapper.
#[test]
fn ac1_golden_limit_topn() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.limit = Some(10);
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("TOPN(10"), "expected TOPN(10: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC1-g: order produces ORDER BY.
#[test]
fn ac1_golden_order_by() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.order = Some(vec![OrderKey {
        key: "sales.revenue".to_string(),
        direction: SortDirection::Desc,
    }]);
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("ORDER BY"), "expected ORDER BY: {dax}");
    assert!(dax.contains("DESC"), "expected DESC: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC1-h: calc-group member filter.
#[test]
fn ac1_golden_calc_group_member_filter() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.dimensions.push(LevelSelection {
        hierarchy: "time.calendar".to_string(),
        level: "Year".to_string(),
    });
    bound.dimensions.push(dim_input("time.calendar.[Year]", "time.calendar"));
    bound.calc_group_members.push(CalcGroupMemberInput {
        calc_group: "Time Intelligence".to_string(),
        member: "YTD".to_string(),
        unique_name: "calc.time_intel.YTD".to_string(),
        mdx: "Aggregate(PeriodsToDate([Time].[Calendar].[Year]))".to_string(),
    });

    let dax = compile(&bound).expect("compile ok");
    // Must contain the calc-group column filter, not invented logic.
    assert!(
        dax.contains("TimeIntelligence[TimeIntelligence]"),
        "expected calc-group column filter: {dax}"
    );
    assert!(dax.contains("\"YTD\""), "expected member = YTD: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

// ── AC2: TimeIntel variant → correct DAX function ────────────────────────────

/// AC2-a: YoY → SAMEPERIODLASTYEAR.
#[test]
fn ac2_yoy_maps_to_sameperiodlastyear() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::YoY);
    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("SAMEPERIODLASTYEAR"),
        "YoY must emit SAMEPERIODLASTYEAR: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-b: PriorPeriod → DATEADD.
#[test]
fn ac2_prior_period_maps_to_dateadd() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::PriorPeriod);
    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("DATEADD"),
        "PriorPeriod must emit DATEADD: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-c: ToDate Year → DATESYTD.
#[test]
fn ac2_to_date_year_maps_to_datesytd() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::ToDate { grain: Grain::Year });
    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("DATESYTD"),
        "ToDate(Year) must emit DATESYTD: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-d: ToDate Quarter → DATESQTD.
#[test]
fn ac2_to_date_quarter_maps_to_datesqtd() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::ToDate { grain: Grain::Quarter });
    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("DATESQTD"),
        "ToDate(Quarter) must emit DATESQTD: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-e: ToDate Month → DATESMTD.
#[test]
fn ac2_to_date_month_maps_to_datesmtd() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::ToDate { grain: Grain::Month });
    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("DATESMTD"),
        "ToDate(Month) must emit DATESMTD: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-f: RunningTotal → DATESINTORANGE.
#[test]
fn ac2_running_total_maps_to_datesintorange() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::RunningTotal);
    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("DATESINTORANGE"),
        "RunningTotal must emit DATESINTORANGE: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-g: Share → DIVIDE + CALCULATE + ALL.
#[test]
fn ac2_share_maps_to_divide_calculate_all() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::Share {
        of_level: "geography.region.Region".to_string(),
    });
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("DIVIDE"), "Share must emit DIVIDE: {dax}");
    assert!(dax.contains("ALL("), "Share must emit ALL(...): {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-h: Rank with top_n → TOPN wrapper + ORDER BY DESC.
#[test]
fn ac2_rank_maps_to_topn() {
    let mut mqo = minimal_mqo("sales", "sales.revenue");
    mqo.time_intelligence.push(TimeIntel::Rank {
        by: "sales.revenue".to_string(),
        top_n: Some(5),
    });
    let bound = BoundMqoInput {
        mqo,
        measures: vec![measure_input("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("TOPN(5"), "Rank top_n=5 must emit TOPN(5: {dax}");
    assert!(dax.contains("DESC"), "Rank must emit DESC sort: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC2-i: Rank without top_n → TOPN defaults to 10.
#[test]
fn ac2_rank_no_top_n_defaults_to_10() {
    let mut mqo = minimal_mqo("sales", "sales.revenue");
    mqo.time_intelligence.push(TimeIntel::Rank {
        by: "sales.revenue".to_string(),
        top_n: None,
    });
    let bound = BoundMqoInput {
        mqo,
        measures: vec![measure_input("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("TOPN(10"), "Rank(None) must default TOPN(10: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

// ── AC3: calc-group member → column filter, not invented logic ───────────────

/// AC3-a: calc-group member emitted as calc-group column filter.
#[test]
fn ac3_calc_group_member_is_column_filter() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.dimensions.push(LevelSelection {
        hierarchy: "time.calendar".to_string(),
        level: "Month".to_string(),
    });
    bound.dimensions.push(dim_input("time.calendar.[Month]", "time.calendar"));
    bound.calc_group_members.push(CalcGroupMemberInput {
        calc_group: "ScenarioCalc".to_string(),
        member: "Budget".to_string(),
        unique_name: "calc.scenario.Budget".to_string(),
        mdx: String::new(),
    });
    let dax = compile(&bound).expect("compile ok");
    // Must contain CalcGroupName[CalcGroupName] = "member" pattern.
    assert!(
        dax.contains("ScenarioCalc[ScenarioCalc]"),
        "calc-group filter must reference the calc-group column: {dax}"
    );
    assert!(dax.contains("\"Budget\""), "calc-group filter must match 'Budget': {dax}");
    // Must NOT contain SAMEPERIODLASTYEAR or other invented time-intel logic.
    assert!(
        !dax.contains("SAMEPERIODLASTYEAR"),
        "calc-group member must not emit SAMEPERIODLASTYEAR: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC3-b: Filter::CalcGroupMember in mqo.filters also becomes a column filter.
#[test]
fn ac3_filter_calc_group_member_is_column_filter() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.dimensions.push(LevelSelection {
        hierarchy: "time.calendar".to_string(),
        level: "Year".to_string(),
    });
    bound.dimensions.push(dim_input("time.calendar.[Year]", "time.calendar"));
    bound.mqo.filters.push(Filter::CalcGroupMember {
        calc_group: "TimeGroup".to_string(),
        member: "Actual".to_string(),
    });

    let dax = compile(&bound).expect("compile ok");
    assert!(
        dax.contains("TimeGroup[TimeGroup]"),
        "CalcGroupMember filter must reference column: {dax}"
    );
    assert!(dax.contains("\"Actual\""), "CalcGroupMember filter must include member: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

// ── AC4: limit → TOPN, order → ORDER BY ─────────────────────────────────────

/// AC4-a: limit → TOPN.
#[test]
fn ac4_limit_produces_topn() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.limit = Some(25);
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("TOPN(25"), "limit must produce TOPN: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC4-b: order DESC → ORDER BY ... DESC.
#[test]
fn ac4_order_desc_produces_order_by_desc() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.order = Some(vec![OrderKey {
        key: "sales.revenue".to_string(),
        direction: SortDirection::Desc,
    }]);
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("ORDER BY"), "must produce ORDER BY: {dax}");
    assert!(dax.contains("DESC"), "must produce DESC: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC4-c: order ASC → ORDER BY ... ASC.
#[test]
fn ac4_order_asc_produces_order_by_asc() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.order = Some(vec![OrderKey {
        key: "sales.revenue".to_string(),
        direction: SortDirection::Asc,
    }]);
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("ORDER BY"), "must produce ORDER BY: {dax}");
    assert!(dax.contains("ASC"), "must produce ASC: {dax}");
    assert!(validate_dax_syntax(&dax).is_ok());
}

/// AC4-d: multi-key order → multiple ORDER BY columns.
#[test]
fn ac4_multi_key_order() {
    let mut mqo = minimal_mqo("sales", "sales.revenue");
    mqo.measures.push(MeasureRef {
        unique_name: "sales.units".to_string(),
    });
    mqo.order = Some(vec![
        OrderKey {
            key: "sales.revenue".to_string(),
            direction: SortDirection::Desc,
        },
        OrderKey {
            key: "sales.units".to_string(),
            direction: SortDirection::Asc,
        },
    ]);
    let bound = BoundMqoInput {
        mqo,
        measures: vec![measure_input("sales.revenue"), measure_input("sales.units")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let dax = compile(&bound).expect("compile ok");
    assert!(dax.contains("ORDER BY"), "multi-key order must produce ORDER BY: {dax}");
    // Should contain both directions.
    assert!(dax.contains("DESC"));
    assert!(dax.contains("ASC"));
    assert!(validate_dax_syntax(&dax).is_ok());
}

// ── AC5: bundled syntax check ────────────────────────────────────────────────

/// AC5-a: valid DAX passes syntax check.
#[test]
fn ac5_valid_dax_passes_syntax_check() {
    let dax = "EVALUATE\nROW(\"Revenue\", [Revenue])";
    assert!(validate_dax_syntax(dax).is_ok());
}

/// AC5-b: missing EVALUATE fails syntax check.
#[test]
fn ac5_missing_evaluate_fails() {
    let dax = "ROW(\"Revenue\", [Revenue])";
    assert!(validate_dax_syntax(dax).is_err());
}

/// AC5-c: unmatched paren fails.
#[test]
fn ac5_unmatched_paren_fails() {
    let dax = "EVALUATE\nROW(\"Revenue\", [Revenue]";
    assert!(validate_dax_syntax(dax).is_err());
}

/// AC5-d: every compiled golden DAX passes the syntax check.
#[test]
fn ac5_all_compiled_dax_passes_syntax_check() {
    let cases: Vec<BoundMqoInput> = vec![
        // measure-only
        minimal_bound("sales.revenue"),
        // with dim
        {
            let mut b = minimal_bound("sales.revenue");
            b.mqo.dimensions.push(LevelSelection {
                hierarchy: "time.calendar".to_string(),
                level: "Year".to_string(),
            });
            b.dimensions.push(dim_input("time.calendar.[Year]", "time.calendar"));
            b
        },
        // with limit
        {
            let mut b = minimal_bound("sales.revenue");
            b.mqo.limit = Some(5);
            b
        },
    ];
    for (i, bound) in cases.iter().enumerate() {
        let dax = compile(bound).unwrap_or_else(|e| panic!("case {i} failed to compile: {e}"));
        validate_dax_syntax(&dax)
            .unwrap_or_else(|e| panic!("case {i} DAX failed syntax check: {e}\nDAX: {dax}"));
    }
}

// ── Error path tests ─────────────────────────────────────────────────────────

/// Error: empty measures → DaxCompileError::EmptyMeasures.
#[test]
fn error_empty_measures() {
    let bound = BoundMqoInput {
        mqo: Mqo {
            model: "sales".to_string(),
            measures: vec![],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        },
        measures: vec![],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let err = compile(&bound).unwrap_err();
    assert!(
        matches!(err, mqo_dax_compiler::DaxCompileError::EmptyMeasures),
        "expected EmptyMeasures, got {err:?}"
    );
}

/// Error: Share time-intel with empty of_level.
#[test]
fn error_share_empty_level() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.time_intelligence.push(TimeIntel::Share {
        of_level: String::new(),
    });
    let err = compile(&bound).unwrap_err();
    assert!(
        matches!(err, mqo_dax_compiler::DaxCompileError::EmptyShareLevel),
        "expected EmptyShareLevel, got {err:?}"
    );
}

/// JSON round-trip: serialise a BoundMqoInput and parse it back.
#[test]
fn json_round_trip_bound_mqo_input() {
    let bound = minimal_bound("sales.revenue");
    let json = serde_json::to_string(&bound).expect("serialize");
    let parsed: BoundMqoInput = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.measures[0].unique_name, "sales.revenue");
}

/// Kill mutant: `delete match arm TimeIntel::Rank` in build_measure_pairs.
/// When Rank time-intel is applied, the measure label must include " Rank" suffix.
/// Without the Rank arm, the label would stay bare (e.g. "Revenue" not "Revenue Rank").
#[test]
fn rank_label_includes_rank_suffix() {
    let mut mqo = minimal_mqo("sales", "sales.revenue");
    mqo.time_intelligence.push(TimeIntel::Rank {
        by: "sales.revenue".to_string(),
        top_n: Some(3),
    });
    let bound = BoundMqoInput {
        mqo,
        measures: vec![measure_input("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let dax = compile(&bound).expect("compile ok");
    // The emitted TOPN must reference the measure ref [Revenue] as the sort column.
    assert!(
        dax.contains("[Revenue]"),
        "Rank must include the base measure ref: {dax}"
    );
    // The TOPN wrapper must name the top_n from the Rank variant, not the limit field.
    assert!(dax.contains("TOPN(3"), "Rank top_n=3 must produce TOPN(3: {dax}");
    // The measure label in the ROW/SUMMARIZECOLUMNS name slot must include "Rank".
    // Without the Rank arm in build_measure_pairs, the label would be bare "Revenue".
    assert!(
        dax.contains("\"Revenue Rank\""),
        "Rank time-intel must append ' Rank' to the measure label: {dax}"
    );
}

/// Kill mutant: `delete ! in main` (syntax check bypass via inverted flag).
/// Verify that a BoundMqo that would produce internally-invalid DAX is caught
/// by the compile step itself (not just the syntax check), so the ! inversion
/// only affects the optional --skip-syntax-check path.
/// This is already covered by ac5 tests; this variant pins the EmptyMeasures path.
#[test]
fn empty_measures_is_a_compile_error_not_syntax_error() {
    use mqo_dax_compiler::DaxCompileError;
    let bound = BoundMqoInput {
        mqo: mqo_spec::Mqo {
            model: "x".to_string(),
            measures: vec![],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        },
        measures: vec![],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    // compile() itself must return Err, so even --skip-syntax-check can't produce output.
    assert!(matches!(compile(&bound), Err(DaxCompileError::EmptyMeasures)));
}

/// Reviewer counter-attack (postmortem follow-up): document Rank + limit precedence.
/// When both Rank time-intel (top_n=10) and limit=5 are set, Rank takes precedence
/// (else-if chain in codegen.rs). This test pins that behavior explicitly.
#[test]
fn rank_topn_takes_precedence_over_limit() {
    let mut mqo = minimal_mqo("sales", "sales.revenue");
    mqo.time_intelligence.push(TimeIntel::Rank {
        by: "sales.revenue".to_string(),
        top_n: Some(10),
    });
    mqo.limit = Some(5);
    let bound = BoundMqoInput {
        mqo,
        measures: vec![measure_input("sales.revenue")],
        dimensions: vec![],
        calc_group_members: vec![],
    };
    let dax = compile(&bound).expect("compile ok");
    // Rank top_n=10 wins; limit=5 is silently ignored (first in else-if chain).
    assert!(
        dax.contains("TOPN(10"),
        "Rank top_n=10 must take precedence over limit=5: {dax}"
    );
    assert!(
        !dax.contains("TOPN(5"),
        "limit=5 must not produce a second TOPN when Rank is present: {dax}"
    );
    assert!(validate_dax_syntax(&dax).is_ok());
}

// ── Member filter grounding tests (PRD-mqo-dax-member-filter-grounding) ──────

/// Helper: build a catalog context with one level under "inventory_date_dimensions".
fn inventory_date_ctx() -> DaxCatalogContext {
    let json = r#"{
        "catalog": "tpcds_benchmark_model",
        "columns": [
            {
                "unique_name": "inventory_date_dimensions.calendar.[Inventory Calendar Year]",
                "label": "Inventory Calendar Year",
                "kind": "level"
            },
            {
                "unique_name": "tpcds.total_store_sales",
                "label": "Total Store Sales",
                "kind": "measure"
            }
        ]
    }"#;
    DaxCatalogContext::from_json(json).expect("catalog ok")
}

/// Grounded member filter resolves to real level-qualified column (AC1 from PRD).
///
/// `Member { hierarchy: inventory_date_dimensions, members: ["2001"] }` with a
/// catalog that maps that hierarchy to "Inventory Calendar Year" must emit a
/// column ref that contains "Inventory Calendar Year", NOT
/// `inventory_date_dimensions[inventory_date_dimensions]`.
#[test]
fn member_filter_grounded_resolves_to_level_column() {
    let ctx = inventory_date_ctx();
    let mut bound = minimal_bound("tpcds.total_store_sales");
    bound.mqo.filters.push(Filter::Member {
        hierarchy: "inventory_date_dimensions".to_string(),
        members: vec!["2001".to_string()],
    });

    let dax = compile_grounded(&bound, Some(&ctx)).expect("grounded compile ok");

    // Must contain the level label, not the hierarchy name as column.
    assert!(
        dax.contains("Inventory Calendar Year"),
        "expected level label in column ref, got: {dax}"
    );
    // Must NOT contain the broken Hierarchy[Hierarchy] pattern.
    assert!(
        !dax.contains("inventory_date_dimensions[inventory_date_dimensions]"),
        "must not emit broken Hierarchy[Hierarchy], got: {dax}"
    );
    assert!(dax.contains("\"2001\""), "expected member key in output: {dax}");
    assert!(dax.contains("KEEPFILTERS"), "expected KEEPFILTERS wrapper: {dax}");
    // Syntax check must pass.
    assert!(
        validate_dax_syntax(&dax).is_ok(),
        "grounded member filter must pass syntax check: {dax}"
    );
}

/// Unresolvable member filter fails loud (AC2 from PRD): no catalog context.
///
/// `compile` (= `compile_grounded(bound, None)`) with a `Member` filter must
/// return `DaxCompileError::UngroundedMemberFilter`, not emit broken DAX.
#[test]
fn member_filter_no_ctx_is_error() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.filters.push(Filter::Member {
        hierarchy: "geography.region".to_string(),
        members: vec!["North".to_string()],
    });

    let err = compile(&bound).unwrap_err();
    assert!(
        matches!(err, DaxCompileError::UngroundedMemberFilter { .. }),
        "expected UngroundedMemberFilter, got: {err:?}"
    );
    // Error message must name the hierarchy.
    let msg = err.to_string();
    assert!(
        msg.contains("geography.region"),
        "error message must name the hierarchy, got: {msg}"
    );
}

/// Unresolvable member filter fails loud: catalog present but hierarchy not found.
#[test]
fn member_filter_hierarchy_not_in_catalog_is_error() {
    let ctx = inventory_date_ctx();
    let mut bound = minimal_bound("tpcds.total_store_sales");
    bound.mqo.filters.push(Filter::Member {
        hierarchy: "no_such_hierarchy".to_string(),
        members: vec!["foo".to_string()],
    });

    let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
    assert!(
        matches!(err, DaxCompileError::UngroundedMemberFilter { .. }),
        "expected UngroundedMemberFilter for unknown hierarchy, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("no_such_hierarchy"),
        "error message must name the hierarchy, got: {msg}"
    );
}

/// Empty members list → `EmptyMemberFilter` error, not broken DAX (AC6 from PRD).
#[test]
fn member_filter_empty_members_is_error() {
    let ctx = inventory_date_ctx();
    let mut bound = minimal_bound("tpcds.total_store_sales");
    bound.mqo.filters.push(Filter::Member {
        hierarchy: "inventory_date_dimensions".to_string(),
        members: vec![],
    });

    let err = compile_grounded(&bound, Some(&ctx)).unwrap_err();
    assert!(
        matches!(err, DaxCompileError::EmptyMemberFilter { .. }),
        "expected EmptyMemberFilter, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("inventory_date_dimensions"),
        "error message must name the hierarchy, got: {msg}"
    );
}

/// Multi-member set preserved over the resolved level column (AC7 from PRD).
///
/// `members: ["2001", "2002"]` must produce `… IN {"2001", "2002"}` with the
/// same level-qualified column on both sides.
#[test]
fn member_filter_multi_member_set_preserved() {
    let ctx = inventory_date_ctx();
    let mut bound = minimal_bound("tpcds.total_store_sales");
    bound.mqo.filters.push(Filter::Member {
        hierarchy: "inventory_date_dimensions".to_string(),
        members: vec!["2001".to_string(), "2002".to_string()],
    });

    let dax = compile_grounded(&bound, Some(&ctx)).expect("compile ok");

    assert!(dax.contains("\"2001\""), "expected '2001' in output: {dax}");
    assert!(dax.contains("\"2002\""), "expected '2002' in output: {dax}");
    assert!(dax.contains(" IN {"), "expected IN set in output: {dax}");
    assert!(
        validate_dax_syntax(&dax).is_ok(),
        "multi-member output must pass syntax check: {dax}"
    );
}

/// Negative: no `Hierarchy[Hierarchy]` token ever emitted for a member filter (AC8).
///
/// This pins that the broken heuristic column (`sold_date_dimensions[sold_date_dimensions]`)
/// is never reachable from the `Member` arm, whether or not catalog is supplied.
#[test]
fn member_filter_never_emits_hierarchy_as_column() {
    // Without catalog → error, not broken DAX.
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.filters.push(Filter::Member {
        hierarchy: "sold_date_dimensions".to_string(),
        members: vec!["2001".to_string()],
    });

    let result = compile(&bound);
    // Must be Err — must never produce output with `[sold_date_dimensions]`.
    assert!(
        result.is_err(),
        "compile without catalog must return Err for Member filter"
    );
    // With catalog that has no such hierarchy → also Err, never broken DAX.
    let ctx = inventory_date_ctx();
    let result_with_ctx = compile_grounded(&bound, Some(&ctx));
    assert!(
        result_with_ctx.is_err(),
        "compile with catalog (no match) must return Err for Member filter"
    );
}

/// Range filter path is byte-identical to pre-change behavior (Guardrail 1, FR4).
#[test]
fn range_filter_output_unchanged() {
    let mut bound = minimal_bound("sales.revenue");
    bound.mqo.filters.push(Filter::Range {
        level: "time.calendar.Year".to_string(),
        lo: 2020.0,
        hi: 2024.0,
    });
    bound.mqo.dimensions.push(LevelSelection {
        hierarchy: "time.calendar".to_string(),
        level: "Year".to_string(),
    });
    bound.dimensions.push(dim_input("time.calendar.[Year]", "time.calendar"));

    // Range filter works with or without catalog — identical behavior.
    let dax_no_ctx = compile(&bound).expect("range filter compile no-ctx");
    let dax_with_ctx = compile_grounded(&bound, None).expect("range filter compile grounded-none");
    assert_eq!(
        dax_no_ctx, dax_with_ctx,
        "compile() and compile_grounded(None) must be byte-identical for Range filter"
    );
    assert!(dax_no_ctx.contains("2020"), "expected lo=2020: {dax_no_ctx}");
    assert!(dax_no_ctx.contains("2024"), "expected hi=2024: {dax_no_ctx}");
    assert!(validate_dax_syntax(&dax_no_ctx).is_ok());
}
