//! CI corpus check for the engine-validation gate
//! (PRD-mqo-dax-engine-validation-gate, FR-3 / FR-4 / AC-1 / AC-2 / AC-3).
//!
//! These tests pin the gate against a committed regression fixture (the captured
//! pre-C1 malformed projection DAX) and against well-formed projection DAX, so a
//! regression that re-introduces ungrounded or unquoted output turns `cargo test`
//! red — the capability cannot be marked shipped while emitting unparseable DAX.

use mqo_dax_compiler::{validate_dax_output, DaxValidationError};

/// AC-1 fixture: the captured pre-C1 malformed projection DAX. This is a
/// *committed regression fixture* (a string constant, not generated). It carries
/// both failure modes: a `/* ungrounded: Ship Mode Type */` marker AND the
/// unquoted, space-bearing `Ship Mode Type[…]` table identifier.
const PRE_C1_DAX: &str = "EVALUATE\nSUMMARIZECOLUMNS('atscale_catalogs'[Carrier], KEEPFILTERS(FILTER(ALL(Ship Mode Type[Ship Mode Type] /* ungrounded: Ship Mode Type */), Ship Mode Type[Ship Mode Type] IN {\"EXPRESS\"})))";

/// AC-1 / AC-3 fixture: the expected post-C1 grounded projection DAX — the table
/// name is single-quoted and there is no ungrounded marker.
const WELL_FORMED_DAX: &str = "EVALUATE\nSUMMARIZECOLUMNS('ship_mode'[Carrier], KEEPFILTERS(FILTER(ALL('ship_mode'[Ship Mode Type]), 'ship_mode'[Ship Mode Type] IN {\"EXPRESS\"})))";

/// AC-1 / AC-6: the pre-C1 fixture MUST be rejected, and the message MUST name
/// the offending `Ship Mode Type` token.
#[test]
fn pre_c1_fixture_is_rejected_and_names_token() {
    let err = validate_dax_output(PRE_C1_DAX).expect_err("pre-C1 DAX must be rejected by the gate");
    let msg = err.to_string();
    assert!(
        msg.contains("Ship Mode Type"),
        "rejection message must name the offending token, got: {msg}"
    );
}

/// AC-1: the post-C1 grounded projection DAX MUST pass the gate.
#[test]
fn well_formed_projection_passes() {
    assert_eq!(validate_dax_output(WELL_FORMED_DAX), Ok(()));
}

/// AC-2 (regression direction): re-introducing an ungrounded ref into otherwise
/// well-formed DAX makes the gate red — exactly what keeps the build honest.
#[test]
fn reintroduced_ungrounded_ref_goes_red() {
    let regressed = "EVALUATE\nSUMMARIZECOLUMNS('ship_mode'[Carrier] /* ungrounded: ship_mode.Carrier */)";
    let err = validate_dax_output(regressed).expect_err("re-introduced ungrounded ref must fail");
    assert_eq!(
        err,
        DaxValidationError::UngroundedRef {
            token: "ship_mode.Carrier".to_string()
        }
    );
}

/// AC-3 / FR-4: a corpus of well-formed DAX (copied from the measure-query
/// acceptance suite shapes) MUST produce 0 false rejections.
#[test]
fn measure_query_corpus_has_no_false_rejections() {
    let corpus = [
        // bare measure-only ROW
        "EVALUATE\nROW(\"Revenue\", [Revenue])",
        // measure + single-word dimension
        "EVALUATE\nSUMMARIZECOLUMNS(Calendar[Year], \"Revenue\", [Revenue])",
        // multiple measures
        "EVALUATE\nSUMMARIZECOLUMNS(Calendar[Year], \"Revenue\", [Revenue], \"Units\", [Units Sold])",
        // member filter with KEEPFILTERS, grounded table
        "EVALUATE\nSUMMARIZECOLUMNS('region'[Region], KEEPFILTERS(FILTER(ALL('region'[Region]), 'region'[Region] IN {\"North\", \"South\"})))",
        // range filter
        "EVALUATE\nSUMMARIZECOLUMNS(Calendar[Year], \"Revenue\", [Revenue])\nORDER BY [Revenue] DESC",
        // TOPN
        "EVALUATE\nTOPN(10, SUMMARIZECOLUMNS(Calendar[Year], \"Revenue\", [Revenue]), [Revenue], DESC)",
        // legitimately quoted multi-word table identifier (edge case PRD §4)
        "EVALUATE\nSUMMARIZECOLUMNS('Ship Mode Type'[Carrier], \"Revenue\", [Revenue])",
        // CALCULATE time-intel
        "EVALUATE\nROW(\"YoY\", CALCULATE([Revenue], SAMEPERIODLASTYEAR(Calendar[Date])))",
    ];

    for (i, dax) in corpus.iter().enumerate() {
        assert_eq!(
            validate_dax_output(dax),
            Ok(()),
            "false rejection on well-formed corpus case #{i}: {dax}"
        );
    }
}
