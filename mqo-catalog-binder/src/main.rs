//! `mqo-bind` — resolve an MQO against a catalog snapshot.
//!
//! Usage:
//!   mqo-bind --mqo <mqo.json> --catalog <snapshot.json>
//!   mqo-bind --mqo <mqo.json> --catalog <snapshot.json> --enriched-catalog <enriched.json>
//!
//! Exit codes:
//!   0  — bound successfully; stdout is a `BoundMqo` JSON object
//!   3  — one or more references are ambiguous, or a Member filter member matches
//!         multiple levels; stdout is `{"ambiguous":[...]}` or `{"member_ambiguous":[...]}`
//!   4  — one or more references were not found, or a Member filter member is not
//!         in the domain of any enumerated level, or a MemberLevel filter is on a
//!         near-twin hierarchy that cannot co-resolve with the projected dimension;
//!         stdout is `{"not_found":[...]}`, `{"member_unbound":[...]}`, or
//!         `{"member_unbound_cross_hierarchy":[...]}`
//!   5  — one or more measure×dimension pairs are cross-fact incompatible; stdout is `{"incompatible":[...]}`
//!   6  — a multi-fact MQO requests a date level not conformed across the referenced facts; stdout is `{"date_role_incompatible":[...]}`
//!   2  — I/O error, bad arguments, or malformed --enriched-catalog file

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use mqo_catalog_binder::binder;
use mqo_catalog_binder::catalog;
use mqo_catalog_binder::compat::EnrichedColumnGroups;

use clap::Parser;
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-bind",
    about = "Resolve an MQO against a catalog snapshot, emitting a BoundMqo or error report"
)]
struct Args {
    /// Path to the MQO JSON file
    #[arg(long)]
    mqo: PathBuf,

    /// Path to the catalog snapshot JSON file
    #[arg(long)]
    catalog: PathBuf,

    /// Optional path to an enriched-catalog.v1 JSON file for cross-fact compatibility checking.
    /// When absent, compatibility checking is disabled and behavior is identical to the prior release.
    #[arg(long)]
    enriched_catalog: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();

    // Load MQO
    let mqo_text = std::fs::read_to_string(&args.mqo).unwrap_or_else(|e| {
        eprintln!("mqo-bind: cannot read --mqo file: {e}");
        process::exit(2);
    });
    let mqo: mqo_spec::Mqo = serde_json::from_str(&mqo_text).unwrap_or_else(|e| {
        eprintln!("mqo-bind: --mqo file is not valid MQO JSON: {e}");
        process::exit(2);
    });

    // Load catalog snapshot
    let catalog_text = std::fs::read_to_string(&args.catalog).unwrap_or_else(|e| {
        eprintln!("mqo-bind: cannot read --catalog file: {e}");
        process::exit(2);
    });
    let snapshot: catalog::CatalogSnapshot =
        serde_json::from_str(&catalog_text).unwrap_or_else(|e| {
            eprintln!("mqo-bind: --catalog file is not valid snapshot JSON: {e}");
            process::exit(2);
        });

    // Optionally load enriched catalog (fail loudly on present-but-broken — NFR4).
    let enriched: Option<EnrichedColumnGroups> = match args.enriched_catalog {
        None => None,
        Some(ref path) => match EnrichedColumnGroups::from_path(path) {
            Ok(e) => Some(e),
            Err(msg) => {
                eprintln!("mqo-bind: {msg}");
                process::exit(2);
            }
        },
    };

    let result = match &enriched {
        Some(e) => binder::bind_with_date_roles(&mqo, &snapshot, e),
        None => binder::bind(&mqo, &snapshot),
    };

    match result {
        binder::BindResult::Bound(bound) => {
            println!("{}", serde_json::to_string_pretty(&*bound).expect("serialize"));
            process::exit(0);
        }
        binder::BindResult::Ambiguous(items) => {
            let out = serde_json::json!({ "ambiguous": items });
            println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
            process::exit(3);
        }
        binder::BindResult::NotFound(items) => {
            let out = serde_json::json!({ "not_found": items });
            println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
            process::exit(4);
        }
        binder::BindResult::Incompatible(reports) => {
            let out = serde_json::json!({ "incompatible": reports });
            println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
            process::exit(5);
        }
        binder::BindResult::DateRoleIncompatible(rejections) => {
            let out = serde_json::json!({ "date_role_incompatible": rejections });
            println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
            process::exit(6);
        }
        binder::BindResult::MemberUnbound(errors) => {
            let out = serde_json::json!({ "member_unbound": errors });
            println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
            process::exit(4);
        }
        binder::BindResult::MemberAmbiguous(errors) => {
            let out = serde_json::json!({ "member_ambiguous": errors });
            println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
            process::exit(3);
        }
        binder::BindResult::MemberUnboundCrossHierarchy(errors) => {
            let out = serde_json::json!({ "member_unbound_cross_hierarchy": errors });
            println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
            process::exit(4);
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use crate::binder::{bind, BindResult};
    use crate::catalog::{
        CalcGroupEntry, CatalogSnapshot, ColumnEntry, DescribeModelOutput, SemiAdditiveInfo,
    };
    use mqo_spec::{Filter, LevelSelection, MeasureRef, Mqo};

    fn minimal_mqo(measure_name: &str) -> Mqo {
        Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef {
                unique_name: measure_name.to_string(),
            }],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
            projection: false,
        }
    }

    fn fixture_snapshot() -> CatalogSnapshot {
        CatalogSnapshot {
            columns: vec![
                ColumnEntry {
                    unique_name: "sales.revenue".to_string(),
                    label: "Revenue".to_string(),
                    kind: "measure".to_string(),
                    hierarchy: None,
                    level: None,
                    semi_additive: None,
                    required_dimension: None,
                    is_calc: false,
                    ..Default::default()
                },
                ColumnEntry {
                    unique_name: "sales.units_sold".to_string(),
                    label: "Units Sold".to_string(),
                    kind: "measure".to_string(),
                    hierarchy: None,
                    level: None,
                    semi_additive: None,
                    required_dimension: None,
                    is_calc: false,
                    ..Default::default()
                },
                ColumnEntry {
                    unique_name: "sales.balance".to_string(),
                    label: "Balance".to_string(),
                    kind: "measure".to_string(),
                    hierarchy: None,
                    level: None,
                    semi_additive: Some(SemiAdditiveInfo {
                        trigger_hierarchies: vec![
                            "time.calendar".to_string(),
                        ],
                    }),
                    required_dimension: Some("account.account_type".to_string()),
                    is_calc: false,
                    ..Default::default()
                },
                // dimension level
                ColumnEntry {
                    unique_name: "time.calendar.[Year]".to_string(),
                    label: "Year".to_string(),
                    kind: "level".to_string(),
                    hierarchy: Some("time.calendar".to_string()),
                    level: Some("Year".to_string()),
                    semi_additive: None,
                    required_dimension: None,
                    is_calc: false,
                    ..Default::default()
                },
                ColumnEntry {
                    unique_name: "time.calendar.[Month]".to_string(),
                    label: "Month".to_string(),
                    kind: "level".to_string(),
                    hierarchy: Some("time.calendar".to_string()),
                    level: Some("Month".to_string()),
                    semi_additive: None,
                    required_dimension: None,
                    is_calc: false,
                    ..Default::default()
                },
                // calc measure
                ColumnEntry {
                    unique_name: "sales.margin_pct".to_string(),
                    label: "Margin %".to_string(),
                    kind: "measure".to_string(),
                    hierarchy: None,
                    level: None,
                    semi_additive: None,
                    required_dimension: None,
                    is_calc: true,
                    ..Default::default()
                },
            ],
            describe_model: Some(DescribeModelOutput {
                calc_groups: vec![CalcGroupEntry {
                    group_name: "Time Intelligence".to_string(),
                    member_name: "YTD".to_string(),
                    unique_name: "calc.time_intel.YTD".to_string(),
                    mdx: "Aggregate(PeriodsToDate([Time].[Calendar].[Year]))".to_string(),
                }],
            }),
            ..CatalogSnapshot::default()
        }
    }

    // AC1: valid MQO binds every ref, exits 0
    #[test]
    fn ac1_valid_mqo_binds_and_exits_0() {
        let mqo = minimal_mqo("Revenue");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert_eq!(bound.measures.len(), 1);
                assert_eq!(bound.measures[0].unique_name, "sales.revenue");
                assert!(!bound.measures[0].is_calc);
                assert!(!bound.measures[0].semi_additive);
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // AC1 (case-insensitive): lower-case ref resolves
    #[test]
    fn ac1_case_insensitive_resolution() {
        let mqo = minimal_mqo("revenue");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert_eq!(bound.measures[0].unique_name, "sales.revenue");
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // AC2: fabricated name → not_found (exit 4)
    #[test]
    fn ac2_fabricated_measure_not_found() {
        let mqo = minimal_mqo("NonExistentMeasureXYZ");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::NotFound(refs) => {
                assert!(refs.iter().any(|r| r.contains("NonExistentMeasureXYZ")));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // AC2: not_found never guesses — the unique_name is not in the output
    #[test]
    fn ac2_not_found_no_guess() {
        let mqo = minimal_mqo("REVENUETYPO");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        // Must be NotFound, not Bound
        assert!(
            matches!(result, BindResult::NotFound(_)),
            "must not guess a close match"
        );
    }

    // AC3: ambiguous label (same label in two entries) → candidate set (exit 3)
    #[test]
    fn ac3_ambiguous_label_returns_candidates() {
        let mut snapshot = fixture_snapshot();
        // Add a second measure with the same label "Revenue" but different unique_name
        snapshot.columns.push(ColumnEntry {
            unique_name: "other_model.revenue".to_string(),
            label: "Revenue".to_string(),
            kind: "measure".to_string(),
            hierarchy: None,
            level: None,
            semi_additive: None,
            required_dimension: None,
            is_calc: false,
                    ..Default::default()
                },);
        let mqo = minimal_mqo("Revenue");
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Ambiguous(items) => {
                assert!(!items.is_empty());
                let item = &items[0];
                // candidates should contain both unique_names
                assert!(item["candidates"].as_array().unwrap().len() >= 2);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    // AC4a: calc-group member resolved from describe_model, MDX carried through
    #[test]
    fn ac4_calc_group_member_resolved_with_mdx() {
        let mut mqo = minimal_mqo("Revenue");
        mqo.filters.push(Filter::CalcGroupMember {
            calc_group: "Time Intelligence".to_string(),
            member: "YTD".to_string(),
        });
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert_eq!(bound.calc_group_members.len(), 1);
                let cg = &bound.calc_group_members[0];
                assert_eq!(cg.unique_name, "calc.time_intel.YTD");
                assert!(!cg.mdx.is_empty());
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // AC4b: calc-group member not found → not_found
    #[test]
    fn ac4_missing_calc_group_member_not_found() {
        let mut mqo = minimal_mqo("Revenue");
        mqo.filters.push(Filter::CalcGroupMember {
            calc_group: "Time Intelligence".to_string(),
            member: "NonExistentMember".to_string(),
        });
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        assert!(
            matches!(result, BindResult::NotFound(_)),
            "missing calc-group member must be not_found"
        );
    }

    // AC5: semi-additive measure flagged with trigger hierarchies
    #[test]
    fn ac5_semi_additive_flagged_with_trigger_hierarchies() {
        let mqo = minimal_mqo("Balance");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                let m = &bound.measures[0];
                assert_eq!(m.unique_name, "sales.balance");
                assert!(m.semi_additive);
                assert!(!m.trigger_hierarchies.is_empty());
                assert_eq!(m.trigger_hierarchies[0], "time.calendar");
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // AC5: non-semi-additive measure has empty trigger_hierarchies
    #[test]
    fn ac5_non_semi_additive_has_empty_triggers() {
        let mqo = minimal_mqo("Revenue");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert!(!bound.measures[0].semi_additive);
                assert!(bound.measures[0].trigger_hierarchies.is_empty());
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // Dimension binding by hierarchy+level label
    #[test]
    fn dimension_binding_by_hierarchy_and_level() {
        let mut mqo = minimal_mqo("Revenue");
        mqo.dimensions.push(LevelSelection {
            hierarchy: "time.calendar".to_string(),
            level: "year".to_string(), // case-insensitive
        });
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert_eq!(bound.dimensions.len(), 1);
                assert_eq!(bound.dimensions[0].unique_name, "time.calendar.[Year]");
                assert_eq!(bound.dimensions[0].hierarchy, "time.calendar");
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // Unknown dimension → not_found
    #[test]
    fn unknown_dimension_not_found() {
        let mut mqo = minimal_mqo("Revenue");
        mqo.dimensions.push(LevelSelection {
            hierarchy: "time.calendar".to_string(),
            level: "FakeLevel".to_string(),
        });
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        assert!(matches!(result, BindResult::NotFound(_)));
    }

    // required_dimension carried through for semi-additive measure
    #[test]
    fn required_dimension_carried_through() {
        let mqo = minimal_mqo("Balance");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert_eq!(
                    bound.measures[0].required_dimension,
                    Some("account.account_type".to_string())
                );
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // calc measure is_calc flag
    #[test]
    fn calc_measure_is_calc_flag() {
        let mqo = minimal_mqo("Margin %");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert!(bound.measures[0].is_calc);
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // Multiple not-found refs are all reported
    #[test]
    fn multiple_not_found_all_reported() {
        let mut mqo = minimal_mqo("FakeA");
        mqo.measures.push(MeasureRef {
            unique_name: "FakeB".to_string(),
        });
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::NotFound(refs) => {
                assert!(refs.len() >= 2);
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // Both not-found and ambiguous in same MQO → ambiguous takes precedence
    #[test]
    fn not_found_takes_precedence_over_empty() {
        let mqo = minimal_mqo("FakeMeasure");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        assert!(matches!(result, BindResult::NotFound(_)));
    }

    // unique_name passthrough: if ref exactly matches a unique_name, bind it
    #[test]
    fn unique_name_passthrough() {
        let mqo = minimal_mqo("sales.revenue");
        let snapshot = fixture_snapshot();
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(bound) => {
                assert_eq!(bound.measures[0].unique_name, "sales.revenue");
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    // ── PRD-mqo-binder-no-silent-member-grounding — AC tests ──────────────────

    fn flag_snapshot() -> CatalogSnapshot {
        // A fully-enumerated flag dimension: only one level "Flag" with domain {Y, N}.
        // No high-card siblings → safe to reject out-of-domain members.
        CatalogSnapshot {
            columns: vec![
                ColumnEntry {
                    unique_name: "sales.revenue".to_string(),
                    label: "Revenue".to_string(),
                    kind: "measure".to_string(),
                    is_calc: false,
                    ..Default::default()
                },
                ColumnEntry {
                    unique_name: "flag_dimension.[Flag]".to_string(),
                    label: "Flag".to_string(),
                    kind: "level".to_string(),
                    hierarchy: Some("flag_dimension".to_string()),
                    level: Some("Flag".to_string()),
                    domain: Some(vec!["Y".to_string(), "N".to_string()]),
                    ..Default::default()
                },
            ],
            ..CatalogSnapshot::default()
        }
    }

    fn geo_snapshot() -> CatalogSnapshot {
        // geography_dimension: Store State (enumerated, {AL,GA,...}) + Store City (no domain).
        // Presence of un-enumerated Store City means guard MUST skip (safe).
        CatalogSnapshot {
            columns: vec![
                ColumnEntry {
                    unique_name: "sales.revenue".to_string(),
                    label: "Revenue".to_string(),
                    kind: "measure".to_string(),
                    is_calc: false,
                    ..Default::default()
                },
                ColumnEntry {
                    unique_name: "geography_dimension.[Store State]".to_string(),
                    label: "Store State".to_string(),
                    kind: "level".to_string(),
                    hierarchy: Some("geography_dimension".to_string()),
                    level: Some("Store State".to_string()),
                    domain: Some(vec!["AL".to_string(), "GA".to_string(), "TX".to_string()]),
                    ..Default::default()
                },
                ColumnEntry {
                    unique_name: "geography_dimension.[Store City]".to_string(),
                    label: "Store City".to_string(),
                    kind: "level".to_string(),
                    hierarchy: Some("geography_dimension".to_string()),
                    level: Some("Store City".to_string()),
                    domain: None, // high-card
                    ..Default::default()
                },
            ],
            ..CatalogSnapshot::default()
        }
    }

    // AC-1: Member filter with an out-of-domain value on a fully-enumerated dim → MemberUnbound.
    #[test]
    fn ac1_member_unbound_on_fully_enumerated_dim() {
        let mut mqo = minimal_mqo("Revenue");
        mqo.filters.push(Filter::Member {
            hierarchy: "flag_dimension".to_string(),
            members: vec!["MAYBE".to_string()],
        });
        let result = bind(&mqo, &flag_snapshot());
        match result {
            BindResult::MemberUnbound(errs) => {
                assert_eq!(errs.len(), 1);
                assert_eq!(errs[0].member, "MAYBE");
                assert!(errs[0].candidate_levels.contains(&"Flag".to_string()));
                assert!(errs[0].note.contains("not in the domain"));
            }
            other => panic!("expected MemberUnbound, got {other:?}"),
        }
    }

    // AC-2: Valid member on a fully-enumerated dim → Bound (no error).
    #[test]
    fn ac2_valid_member_in_domain_is_bound() {
        let mut mqo = minimal_mqo("Revenue");
        mqo.filters.push(Filter::Member {
            hierarchy: "flag_dimension".to_string(),
            members: vec!["Y".to_string()],
        });
        let result = bind(&mqo, &flag_snapshot());
        assert!(
            matches!(result, BindResult::Bound(_)),
            "in-domain member must bind successfully: {result:?}"
        );
    }

    // AC-3 (conservative guard): Member not in the enumerated state domain,
    // but a high-card Store City sibling exists → MUST NOT reject.
    #[test]
    fn ac3_safe_skip_with_highcard_sibling() {
        let mut mqo = minimal_mqo("Revenue");
        mqo.filters.push(Filter::Member {
            hierarchy: "geography_dimension".to_string(),
            members: vec!["California".to_string()], // not in {AL,GA,TX}
        });
        let result = bind(&mqo, &geo_snapshot());
        assert!(
            matches!(result, BindResult::Bound(_)),
            "high-card sibling → must NOT reject: {result:?}"
        );
    }

    // AC-4: No domain data (live mode) → no false reject.
    #[test]
    fn ac4_no_domain_data_no_false_reject() {
        // fixture_snapshot() has no domain fields on any level.
        let mut mqo = minimal_mqo("Revenue");
        mqo.filters.push(Filter::Member {
            hierarchy: "time.calendar".to_string(),
            members: vec!["not-a-real-year".to_string()],
        });
        let result = bind(&mqo, &fixture_snapshot());
        assert!(
            matches!(result, BindResult::Bound(_)),
            "no domain data (live mode) → no reject: {result:?}"
        );
    }

    // AC-4 extra: ref errors take precedence over member errors.
    #[test]
    fn ac4_ref_error_wins_over_member_error() {
        let mut mqo = minimal_mqo("NonExistentMeasure"); // will be not_found
        mqo.filters.push(Filter::Member {
            hierarchy: "flag_dimension".to_string(),
            members: vec!["MAYBE".to_string()], // would be member_unbound
        });
        let result = bind(&mqo, &flag_snapshot());
        assert!(
            matches!(result, BindResult::NotFound(_)),
            "not_found takes precedence over member_unbound: {result:?}"
        );
    }
}
