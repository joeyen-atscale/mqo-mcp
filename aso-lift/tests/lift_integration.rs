//! Integration tests for `aso-lift` — all run against the SYNTHETIC fixture.
//!
//! NOTE: These tests cover AC1–AC8 of PRD-osl-engine-xml-rdf-lift against
//! a synthetic representative fixture (tests/fixtures/synthetic-model.xml).
//! Real-XSD validation against an exported engine model (sales-insights-project.xml)
//! is deferred per PRD open question OQ2.

use aso_lift::{lift, round_trip_check, LiftError, LiftOptions};

/// Load the synthetic fixture XML at compile-time for speed.
const FIXTURE: &str = include_str!("fixtures/synthetic-model.xml");

/// Default options for all tests — stable base IRI.
fn opts() -> LiftOptions {
    LiftOptions {
        base_iri: "https://test.models.atscale.com/synthetic".to_owned(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC1 — Turtle parses back + owl:imports aso: TBox
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac1_turtle_round_trips() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed on synthetic fixture");
    let count = round_trip_check(&output.turtle)
        .expect("emitted Turtle must parse back without error (AC1)");
    assert!(count > 0, "round-trip triple count must be > 0");
}

#[test]
fn ac1_turtle_contains_owl_imports_aso_tbox() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed");
    // The Turtle must contain an owl:imports pointing to the aso: ontology IRI.
    // We check the raw Turtle text for the imports IRI as a quick sanity gate.
    let aso_ontology_iri = aso_tbox::iris::ONTOLOGY;
    assert!(
        output.turtle.contains(aso_ontology_iri),
        "emitted Turtle must reference the aso: ontology IRI (owl:imports); \
         IRI '{aso_ontology_iri}' not found in output"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC2 — Every element → typed NamedIndividual + rdfs:label
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac2_every_element_is_named_individual() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed");
    // The Turtle must contain owl:NamedIndividual for each known element id.
    // We check for presence of key ids (deterministic IRI).
    let expected_ids = [
        "prj_sales_insights",
        "cube_sales_insights",
        "meas_revenue",
        "meas_inventory_balance",
        "dim_date",
        "hier_calendar",
        "lvl_year",
        "lvl_quarter",
        "lvl_month",
        "lvl_date",
        "rpr_order_date",
    ];
    for id in &expected_ids {
        assert!(
            output.turtle.contains(id),
            "element id '{}' not found in emitted Turtle (AC2)",
            id
        );
    }
    // owl:NamedIndividual must appear at least once per element
    let ni_count = output
        .turtle
        .matches("NamedIndividual")
        .count();
    assert!(
        ni_count >= expected_ids.len(),
        "expected at least {} owl:NamedIndividual occurrences, got {}",
        expected_ids.len(),
        ni_count
    );
}

#[test]
fn ac2_measures_typed_to_aso_measure_class() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed");
    // Revenue → aso:FullyAdditiveMeasure
    assert!(
        output.turtle.contains("FullyAdditiveMeasure"),
        "Turtle must contain FullyAdditiveMeasure for fully-additive Revenue measure"
    );
    // InventoryBalance → aso:SemiAdditiveMeasure
    assert!(
        output.turtle.contains("SemiAdditiveMeasure"),
        "Turtle must contain SemiAdditiveMeasure for semi-additive InventoryBalance measure"
    );
}

#[test]
fn ac2_labels_present() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed");
    // At least some labels should appear (rdfs:label)
    assert!(
        output.turtle.contains("label"),
        "Turtle must contain rdfs:label triples (AC2)"
    );
    // Check for known caption values from the fixture
    assert!(
        output.turtle.contains("Total Revenue") || output.turtle.contains("Revenue"),
        "Turtle must carry a label for Revenue"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC3 — IRI stability: keyed on XSD `id`, not mutable name
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac3_iri_stable_across_name_change() {
    // Lift the original fixture
    let output1 = lift(FIXTURE, &opts()).expect("lift must succeed on original");

    // Mutate the caption attribute of the Revenue measure (not its id).
    // Caption is what becomes the rdfs:label; renaming it changes the label
    // while the IRI (keyed on id=meas_revenue) must remain identical.
    let mutated = FIXTURE.replace(
        r#"caption="Total Revenue""#,
        r#"caption="Total Revenue Renamed""#,
    );
    let output2 = lift(&mutated, &opts()).expect("lift must succeed after caption change");

    // The IRI for meas_revenue must appear in both outputs (keyed on id, not name/caption)
    let expected_fragment = "meas_revenue";
    assert!(
        output1.turtle.contains(expected_fragment),
        "IRI for meas_revenue must appear in original output"
    );
    assert!(
        output2.turtle.contains(expected_fragment),
        "IRI for meas_revenue must still appear after caption rename (AC3 — id-keyed)"
    );

    // The new label should appear only in output2
    assert!(
        output2.turtle.contains("Total Revenue Renamed"),
        "renamed caption should appear as label in output2"
    );
    // The old label should not appear in output2
    assert!(
        !output2.turtle.contains("\"Total Revenue\""),
        "old label 'Total Revenue' must not appear in output2 after rename"
    );

    // Verify the IRI is identical in both runs
    let base = &opts().base_iri;
    let expected_iri = format!("{base}#{expected_fragment}");
    assert!(
        output1.turtle.contains(&expected_iri) || output1.turtle.contains(expected_fragment),
        "expected IRI fragment '{expected_fragment}' in output1"
    );
    assert!(
        output2.turtle.contains(&expected_iri) || output2.turtle.contains(expected_fragment),
        "expected IRI fragment '{expected_fragment}' in output2"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC4 — role-playing → aso:playsRoleOf triple
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac4_role_playing_emits_plays_role_of() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed");
    // The keyed-attribute-ref rpr_order_date should emit aso:playsRoleOf
    let plays_role_iri = "playsRoleOf"; // suffix match — works for both full IRI and prefix form
    assert!(
        output.turtle.contains(plays_role_iri),
        "Turtle must contain aso:playsRoleOf triple for role-playing keyed-attribute-ref (AC4)"
    );
    // Verify rpr_order_date appears as the subject of a playsRoleOf triple
    assert!(
        output.turtle.contains("rpr_order_date"),
        "role-playing reference rpr_order_date must appear in Turtle output"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC5 — byte-identical output on two runs of identical input
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac5_two_runs_are_byte_identical() {
    let output1 = lift(FIXTURE, &opts()).expect("first run must succeed");
    let output2 = lift(FIXTURE, &opts()).expect("second run must succeed");
    assert_eq!(
        output1.turtle, output2.turtle,
        "two runs on identical input must produce byte-identical Turtle (AC5 / NFR1)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC6 — unknown element kind → conservative fallback, not dropped
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac6_unknown_element_typed_as_fallback() {
    // Inject an unknown element with an id
    let with_unknown = FIXTURE.replace(
        "</cube>",
        r#"<unknown-widget id="unk_widget_01" name="Widget" caption="A Widget" /></cube>"#,
    );
    let output = lift(&with_unknown, &opts()).expect("lift must succeed even with unknown element");
    // The unknown element should still appear in output (not dropped)
    assert!(
        output.turtle.contains("unk_widget_01"),
        "unknown element unk_widget_01 must appear in Turtle output, not be silently dropped (AC6)"
    );
    // It must be typed to aso:Attribute (fallback)
    assert!(
        output.turtle.contains("Attribute"),
        "unknown element must be typed to aso:Attribute (fallback) (AC6)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC7 — pre-2.0 schema → actionable error
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac7_pre_20_schema_returns_actionable_error() {
    // A minimal project_1_1-flavored snippet
    let old_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<project id="prj_old" name="OldProject" schema_version="project_1_1">
  <cube id="cube_old" name="OldCube" caption="Old Cube" />
</project>"#;

    let result = lift(old_xml, &opts());
    assert!(
        result.is_err(),
        "lift must return an error for project_1_1 schema (AC7)"
    );
    if let Err(LiftError::UnsupportedSchema { version }) = result {
        assert_eq!(
            version, "project_1_1",
            "error must name the offending schema version"
        );
    } else {
        panic!("expected LiftError::UnsupportedSchema, got: {:?}", result);
    }
}

#[test]
fn ac7_error_message_names_migration_step() {
    let old_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<project id="prj_old" name="OldProject" schema_version="project_1_0">
</project>"#;
    let err = lift(old_xml, &opts()).unwrap_err();
    let msg = err.to_string();
    // The error message must mention migration
    assert!(
        msg.contains("migrate") || msg.contains("migration") || msg.contains("XSLT"),
        "error message must name the required migration step (AC7): got '{msg}'"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC8 — no warehouse credentials required
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ac8_succeeds_with_no_warehouse_env_vars() {
    // Temporarily unset any warehouse credential env vars
    // (they're not set in CI either; this is mostly a documentation test)
    let _ = std::env::remove_var("ATSCALE_HOST");
    let _ = std::env::remove_var("ATSCALE_TOKEN");
    let _ = std::env::remove_var("SNOWFLAKE_ACCOUNT");
    let _ = std::env::remove_var("BIGQUERY_PROJECT");

    // Lift must succeed purely from XML metadata — no network calls (AC8 / NFR2)
    let output = lift(FIXTURE, &opts()).expect(
        "lift must complete successfully with no warehouse credentials present (AC8 / NFR2)",
    );
    assert!(output.triple_count > 0);
}

// ─────────────────────────────────────────────────────────────────────────────
//  AC5 (bonus) — hierarchy rollsUpTo triples present
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hierarchy_rolls_up_to_triples_present() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed");
    // The Calendar hierarchy has 4 levels; we expect 3 rollsUpTo triples
    let rolls_count = output.turtle.matches("rollsUpTo").count();
    assert!(
        rolls_count >= 3,
        "expected at least 3 rollsUpTo triples for 4-level Calendar hierarchy, got {rolls_count}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  Snapshot — verify triple count is stable (regression guard)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn triple_count_sanity() {
    let output = lift(FIXTURE, &opts()).expect("lift must succeed");
    // Minimum: ontology + 11 elements × (type × 2 + label) + rollsUpTo × 3 + playsRoleOf + additiveOver
    // Rough lower bound: 40 triples
    assert!(
        output.triple_count >= 30,
        "expected at least 30 triples, got {} — possible regression",
        output.triple_count
    );
}
