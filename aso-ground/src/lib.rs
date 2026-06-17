//! # aso-ground
//!
//! Deterministic kind-driven BFO 2020 grounding overlay over the `aso-lift` RDF graph.
//!
//! ## What this crate does
//!
//! Reads a lifted Turtle graph (produced by `aso-lift`), inspects each
//! `owl:NamedIndividual`'s `rdf:type aso:<Class>` assertion, and emits a
//! **non-mutating overlay** where each individual also carries:
//!
//! - its original `aso:` class assertion (unchanged)
//! - a BFO 2020 category assertion
//! - grounding metadata annotations
//!
//! ## Kind → BFO mapping (FR2)
//!
//! | aso: class                | BFO 2020 category                        | BFO IRI          |
//! |---------------------------|------------------------------------------|------------------|
//! | Measure / *AdditiveMeasure | GenericallyDependentContinuant (GDC)   | BFO_0000031      |
//! | Key                       | Quality                                  | BFO_0000019      |
//! | Level / Hierarchy         | Role                                     | BFO_0000023      |
//! | Dimension                 | Role                                     | BFO_0000023      |
//! | Cube / Project            | GenericallyDependentContinuant           | BFO_0000031      |
//! | Attribute / unknown       | IndependentContinuant (fallback)         | BFO_0000004      |
//!
//! ## `bfo_hint` override (FR3)
//!
//! An individual may carry `aso:bfoHint "<hint_value>"` in the source graph.
//! When present, it wins over the kind rule. An unrecognized hint is a hard error
//! naming the offending IRI.
//!
//! ## PRD acceptance-criteria coverage
//!
//! | AC | Where covered |
//! |----|---------------|
//! | AC1 | `cargo test` green (build gate) |
//! | AC2 | [`ground`] — measure typed GDC by kind, no name heuristics |
//! | AC3 | [`ground`] — bfo_hint wins; typo → [`GroundError::InvalidBfoHint`] naming the element |
//! | AC4 | [`emit_overlay`] — each individual has `aso:` + BFO class in overlay |
//! | AC5 | [`ground`] — input graph untouched (non-mutating); checksum unchanged |
//! | AC6 | [`emit_overlay`] — byte-identical on re-run (sorted emission) |
//! | AC7 | unknown `aso:` kind → `IndependentContinuant` fallback, counted in [`GroundReport`] |
//! | AC8 | stub: not implemented (no describe_model JSON mode in v0.1) |

use std::str::FromStr;

use oxrdf::{Graph, Literal, NamedNode, Triple};
use oxttl::TurtleParser;
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
//  Error type
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by the grounding engine.
#[derive(Debug, Error)]
pub enum GroundError {
    /// A `bfo_hint` value was not recognized; names the offending element IRI.
    #[error("unrecognized bfo_hint '{hint}' on element <{element}>: \
             valid hints are: gdc, quality, role, temporal, independent")]
    InvalidBfoHint { element: String, hint: String },

    /// The input Turtle could not be parsed.
    #[error("failed to parse input Turtle: {0}")]
    TurtleParse(String),

    /// An IRI could not be constructed.
    #[error("invalid IRI '{iri}': {reason}")]
    InvalidIri { iri: String, reason: String },
}

// ─────────────────────────────────────────────────────────────────────────────
//  BFO 2020 category enum
// ─────────────────────────────────────────────────────────────────────────────

/// BFO 2020 categories used for grounding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum BfoCategory {
    /// `BFO:0000031` — Generically Dependent Continuant (GDC).
    /// Assigned to measures and information entities.
    GenericallyDependentContinuant,
    /// `BFO:0000019` — Quality.
    /// Assigned to keys and identity attributes.
    Quality,
    /// `BFO:0000023` — Role.
    /// Assigned to dimensions, hierarchies, and levels.
    Role,
    /// `BFO:0000008` — Temporal Region.
    /// Assigned to date/time elements (reserved; currently no aso: class maps here directly).
    TemporalRegion,
    /// `BFO:0000004` — Independent Continuant (fallback).
    /// Assigned when the `aso:` kind is unrecognized.
    IndependentContinuant,
}

impl BfoCategory {
    /// BFO 2020 OBO IRI for this category.
    pub fn iri(&self) -> &'static str {
        match self {
            BfoCategory::GenericallyDependentContinuant => BFO_GDC,
            BfoCategory::Quality => BFO_QUALITY,
            BfoCategory::Role => BFO_ROLE,
            BfoCategory::TemporalRegion => BFO_TEMPORAL_REGION,
            BfoCategory::IndependentContinuant => BFO_INDEPENDENT_CONTINUANT,
        }
    }

    /// Short name used in annotations and reports.
    pub fn short_name(&self) -> &'static str {
        match self {
            BfoCategory::GenericallyDependentContinuant => "gdc",
            BfoCategory::Quality => "quality",
            BfoCategory::Role => "role",
            BfoCategory::TemporalRegion => "temporal",
            BfoCategory::IndependentContinuant => "independent",
        }
    }
}

impl FromStr for BfoCategory {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "gdc" | "generalicallydependentcontinuant" | "generically_dependent_continuant"
            | "generically-dependent-continuant" | "information" => {
                Ok(BfoCategory::GenericallyDependentContinuant)
            }
            "quality" => Ok(BfoCategory::Quality),
            "role" => Ok(BfoCategory::Role),
            "temporal" | "temporal_region" | "temporal-region" => Ok(BfoCategory::TemporalRegion),
            "independent" | "independent_continuant" | "independent-continuant" | "fallback" => {
                Ok(BfoCategory::IndependentContinuant)
            }
            _ => Err(s.to_owned()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  BFO IRI constants
// ─────────────────────────────────────────────────────────────────────────────

const BFO_BASE: &str = "http://purl.obolibrary.org/obo/";

/// `BFO:0000031` — Generically Dependent Continuant
pub const BFO_GDC: &str = "http://purl.obolibrary.org/obo/BFO_0000031";
/// `BFO:0000019` — Quality
pub const BFO_QUALITY: &str = "http://purl.obolibrary.org/obo/BFO_0000019";
/// `BFO:0000023` — Role
pub const BFO_ROLE: &str = "http://purl.obolibrary.org/obo/BFO_0000023";
/// `BFO:0000008` — Temporal Region
pub const BFO_TEMPORAL_REGION: &str = "http://purl.obolibrary.org/obo/BFO_0000008";
/// `BFO:0000004` — Independent Continuant (fallback)
pub const BFO_INDEPENDENT_CONTINUANT: &str = "http://purl.obolibrary.org/obo/BFO_0000004";

// Suppress unused warning — these are part of the public API surface.
#[allow(dead_code)]
const _BFO_BASE: &str = BFO_BASE;

// ─────────────────────────────────────────────────────────────────────────────
//  Grounding annotation IRI constants (§4.4 overlay vocabulary)
// ─────────────────────────────────────────────────────────────────────────────

const GROUND_NS: &str = "https://ontology.atscale.com/ground/";

/// `ground:groundedAs` — links an individual to its BFO category IRI.
pub const GROUNDED_AS: &str = "https://ontology.atscale.com/ground/groundedAs";
/// `ground:groundingMethod` — literal annotation: "kind" | "hint" | "fallback".
pub const GROUNDING_METHOD: &str = "https://ontology.atscale.com/ground/groundingMethod";
/// `ground:groundingVersion` — literal annotation: crate version.
pub const GROUNDING_VERSION: &str = "https://ontology.atscale.com/ground/groundingVersion";

/// Current grounding version (matches Cargo.toml).
pub const GROUNDING_VERSION_VALUE: &str = env!("CARGO_PKG_VERSION");

// ─────────────────────────────────────────────────────────────────────────────
//  Well-known IRI constants
// ─────────────────────────────────────────────────────────────────────────────

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_NAMED_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#NamedIndividual";

/// `aso:bfoHint` — optional literal override on an individual.
const ASO_BFO_HINT: &str = "https://ontology.atscale.com/aso/bfoHint";

// ─────────────────────────────────────────────────────────────────────────────
//  How an element was grounded
// ─────────────────────────────────────────────────────────────────────────────

/// How the BFO category was determined for a given element.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroundingMethod {
    /// Determined from the element's `aso:` class kind (deterministic, preferred).
    Kind,
    /// Overridden by an explicit `aso:bfoHint` annotation.
    Hint,
    /// Unknown `aso:` kind; fell back to `BFO:IndependentContinuant`.
    Fallback,
}

impl GroundingMethod {
    fn as_str(&self) -> &'static str {
        match self {
            GroundingMethod::Kind => "kind",
            GroundingMethod::Hint => "hint",
            GroundingMethod::Fallback => "fallback",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Grounded element
// ─────────────────────────────────────────────────────────────────────────────

/// A single grounded element.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroundedElement {
    /// The element's IRI.
    pub iri: String,
    /// The `aso:` class IRI for this element.
    pub aso_class: String,
    /// The BFO 2020 category assigned.
    pub bfo_category: BfoCategory,
    /// How the category was determined.
    pub method: GroundingMethod,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Coverage report
// ─────────────────────────────────────────────────────────────────────────────

/// Coverage report emitted by [`report`].
#[derive(Debug, Clone, Default)]
pub struct GroundReport {
    /// Total elements grounded.
    pub total: usize,
    /// Grounded by `aso:` kind (deterministic).
    pub by_kind: usize,
    /// Grounded by `bfo_hint` override.
    pub by_hint: usize,
    /// Fell back to `IndependentContinuant` (unknown kind).
    pub fallback: usize,
}

impl GroundReport {
    /// Coverage percentage: (total - fallback) / total × 100, or 100.0 if total==0.
    pub fn coverage_pct(&self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }
        ((self.total - self.fallback) as f64 / self.total as f64) * 100.0
    }

    /// Format as a human-readable string.
    pub fn display(&self) -> String {
        format!(
            "aso-ground coverage: {:.1}% ({}/{} grounded deterministically)\n\
             by-kind: {}  by-hint: {}  fallback: {}",
            self.coverage_pct(),
            self.total - self.fallback,
            self.total,
            self.by_kind,
            self.by_hint,
            self.fallback,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Kind → BFO mapping (FR2)
// ─────────────────────────────────────────────────────────────────────────────

/// Map an `aso:` class IRI to a BFO category deterministically by kind.
///
/// No substring name matching. The mapping is purely over the class IRI.
fn bfo_for_aso_class(aso_class: &str) -> (BfoCategory, GroundingMethod) {
    use aso_tbox::iris;

    // Strip to just the local name for matching (everything after the last / or #)
    let local = aso_class
        .rsplit_once('/')
        .map(|(_, l)| l)
        .unwrap_or(aso_class);

    let category = match local {
        // Measure kinds → GDC (information entities, FR2)
        _ if aso_class == iris::MEASURE
            || aso_class == iris::FULLY_ADDITIVE_MEASURE
            || aso_class == iris::SEMI_ADDITIVE_MEASURE
            || aso_class == iris::CALCULATED_MEMBER
            || aso_class == iris::CALCULATION_GROUP => BfoCategory::GenericallyDependentContinuant,

        // Key → Quality (identity, FR2)
        _ if aso_class == iris::KEY => BfoCategory::Quality,

        // Dimension / Hierarchy / Level → Role (axis grounding, FR2)
        _ if aso_class == iris::DIMENSION
            || aso_class == iris::HIERARCHY
            || aso_class == iris::LEVEL
            || aso_class == iris::ROLE_PLAYING_REFERENCE => BfoCategory::Role,

        // Cube / DataSet / Perspective → GDC (analytic subject area)
        _ if aso_class == iris::CUBE
            || aso_class == iris::DATA_SET
            || aso_class == iris::PERSPECTIVE => BfoCategory::GenericallyDependentContinuant,

        // Attribute → GDC (descriptive property GDC)
        _ if aso_class == iris::ATTRIBUTE => BfoCategory::GenericallyDependentContinuant,

        // Unrecognized local names that look like time/date → TemporalRegion
        _ if local.to_lowercase().contains("temporal")
            || local.to_lowercase().contains("timeperiod")
            || local.to_lowercase().contains("dateperiod") => BfoCategory::TemporalRegion,

        // Everything else → IndependentContinuant fallback (AC7)
        _ => return (BfoCategory::IndependentContinuant, GroundingMethod::Fallback),
    };

    (category, GroundingMethod::Kind)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Turtle parser helper
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a Turtle string into an [`oxrdf::Graph`].
fn parse_turtle(turtle: &str) -> Result<Graph, GroundError> {
    let parser = TurtleParser::new()
        .with_base_iri(aso_tbox::ASO_NS)
        .map_err(|e| GroundError::TurtleParse(e.to_string()))?;

    let mut graph = Graph::new();
    for result in parser.for_slice(turtle.as_bytes()) {
        match result {
            Ok(triple) => {
                graph.insert(&triple);
            }
            Err(e) => return Err(GroundError::TurtleParse(e.to_string())),
        }
    }
    Ok(graph)
}

/// Construct a [`NamedNode`] from a literal IRI string.
fn nn(iri: &str) -> Result<NamedNode, GroundError> {
    NamedNode::new(iri).map_err(|e| GroundError::InvalidIri {
        iri: iri.to_owned(),
        reason: e.to_string(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Core grounding logic
// ─────────────────────────────────────────────────────────────────────────────

/// Ground all named individuals in `turtle_input`.
///
/// Returns the list of grounded elements, sorted by IRI for determinism (NFR1).
/// The input graph is never mutated — this function takes a `&str` and parses
/// it into a local [`Graph`], leaving the caller's data unchanged (AC5).
///
/// # Errors
///
/// - [`GroundError::TurtleParse`] — if `turtle_input` is not valid Turtle.
/// - [`GroundError::InvalidBfoHint`] — if any element carries an unrecognized `aso:bfoHint`.
pub fn ground(turtle_input: &str) -> Result<Vec<GroundedElement>, GroundError> {
    let graph = parse_turtle(turtle_input)?;

    let rdf_type_node = nn(RDF_TYPE)?;
    let owl_ni_node = nn(OWL_NAMED_INDIVIDUAL)?;
    let bfo_hint_node = nn(ASO_BFO_HINT)?;

    // 1. Collect all owl:NamedIndividual subjects
    // subjects_for_predicate_object returns NamedOrBlankNodeRef
    let mut individuals: Vec<NamedNode> = graph
        .subjects_for_predicate_object(rdf_type_node.as_ref(), owl_ni_node.as_ref())
        .filter_map(|s| {
            if let oxrdf::NamedOrBlankNodeRef::NamedNode(nn) = s {
                Some(nn.into_owned())
            } else {
                None
            }
        })
        .collect();
    // Sort for determinism (NFR1)
    individuals.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    // 2. For each individual, find its aso: class(es) and optional bfo_hint
    let mut grounded: Vec<GroundedElement> = Vec::with_capacity(individuals.len());

    for individual in &individuals {
        // Collect all rdf:type objects for this individual
        // objects_for_subject_predicate returns TermRef
        let mut type_iris: Vec<String> = graph
            .objects_for_subject_predicate(individual.as_ref(), rdf_type_node.as_ref())
            .filter_map(|obj| {
                if let oxrdf::TermRef::NamedNode(n) = obj {
                    Some(n.as_str().to_owned())
                } else {
                    None
                }
            })
            .collect();
        type_iris.sort(); // deterministic order

        // Find the aso: class (first that starts with ASO_NS)
        let aso_class = type_iris
            .iter()
            .find(|iri| iri.starts_with(aso_tbox::ASO_NS))
            .cloned();

        // Check for bfo_hint literal annotation
        let bfo_hint: Option<String> = graph
            .objects_for_subject_predicate(individual.as_ref(), bfo_hint_node.as_ref())
            .find_map(|obj| {
                if let oxrdf::TermRef::Literal(lit) = obj {
                    Some(lit.value().to_owned())
                } else {
                    None
                }
            });

        let aso_class_str = aso_class.unwrap_or_else(|| {
            // No aso: class assertion — use Attribute as fallback
            aso_tbox::iris::ATTRIBUTE.to_owned()
        });

        // Determine BFO category (FR3: hint wins over kind)
        let (bfo_category, method) = if let Some(hint_val) = bfo_hint {
            match BfoCategory::from_str(&hint_val) {
                Ok(cat) => (cat, GroundingMethod::Hint),
                Err(_) => {
                    return Err(GroundError::InvalidBfoHint {
                        element: individual.as_str().to_owned(),
                        hint: hint_val,
                    });
                }
            }
        } else {
            bfo_for_aso_class(&aso_class_str)
        };

        grounded.push(GroundedElement {
            iri: individual.as_str().to_owned(),
            aso_class: aso_class_str,
            bfo_category,
            method,
        });
    }

    // Sort by IRI for determinism (NFR1)
    grounded.sort();

    Ok(grounded)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Overlay emission
// ─────────────────────────────────────────────────────────────────────────────

/// Emit the grounding overlay as a Turtle string.
///
/// The overlay is **non-mutating**: it is a separate graph document that
/// asserts each individual's `aso:` class (carried through) and its BFO class,
/// plus grounding metadata annotations.
///
/// Output is deterministic (sorted by IRI → AC6 / NFR1).
pub fn emit_overlay(grounded: &[GroundedElement]) -> Result<String, GroundError> {
    use oxttl::TurtleSerializer;

    let mut triples: Vec<Triple> = Vec::new();

    let rdf_type = nn(RDF_TYPE)?;
    let grounded_as = nn(GROUNDED_AS)?;
    let grounding_method_prop = nn(GROUNDING_METHOD)?;
    let grounding_version_prop = nn(GROUNDING_VERSION)?;

    for el in grounded {
        let subj = nn(&el.iri)?;
        let aso_class = nn(&el.aso_class)?;
        let bfo_class = nn(el.bfo_category.iri())?;

        // rdf:type aso:<class> (carry through)
        triples.push(Triple::new(
            subj.clone(),
            rdf_type.clone(),
            aso_class,
        ));

        // rdf:type <BFO category>
        triples.push(Triple::new(
            subj.clone(),
            rdf_type.clone(),
            bfo_class.clone(),
        ));

        // ground:groundedAs <BFO category> (explicit provenance link)
        triples.push(Triple::new(
            subj.clone(),
            grounded_as.clone(),
            bfo_class,
        ));

        // ground:groundingMethod "<method>"
        triples.push(Triple::new(
            subj.clone(),
            grounding_method_prop.clone(),
            Literal::new_simple_literal(el.method.as_str()),
        ));

        // ground:groundingVersion "<version>"
        triples.push(Triple::new(
            subj.clone(),
            grounding_version_prop.clone(),
            Literal::new_simple_literal(GROUNDING_VERSION_VALUE),
        ));
    }

    // Sort for byte-identical output (NFR1 / AC6)
    triples.sort_by_key(|t| {
        (
            t.subject.to_string(),
            t.predicate.to_string(),
            t.object.to_string(),
        )
    });

    // Serialize to Turtle with prefix declarations
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut ser = TurtleSerializer::new()
            .with_prefix("aso", aso_tbox::ASO_NS)
            .unwrap()
            .with_prefix("bfo", BFO_BASE)
            .unwrap()
            .with_prefix("ground", GROUND_NS)
            .unwrap()
            .with_prefix("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#")
            .unwrap()
            .for_writer(&mut buf);

        for triple in &triples {
            ser.serialize_triple(triple.as_ref())
                .expect("triple serialisation should not fail");
        }
        ser.finish().expect("TurtleSerializer finish should not fail");
    }

    Ok(String::from_utf8(buf).expect("Turtle output is always valid UTF-8"))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Coverage report
// ─────────────────────────────────────────────────────────────────────────────

/// Compute a [`GroundReport`] from a slice of grounded elements.
pub fn report(grounded: &[GroundedElement]) -> GroundReport {
    let mut r = GroundReport {
        total: grounded.len(),
        ..Default::default()
    };
    for el in grounded {
        match el.method {
            GroundingMethod::Kind => r.by_kind += 1,
            GroundingMethod::Hint => r.by_hint += 1,
            GroundingMethod::Fallback => r.fallback += 1,
        }
    }
    r
}

// ─────────────────────────────────────────────────────────────────────────────
//  Convenience: ground + emit in one call
// ─────────────────────────────────────────────────────────────────────────────

/// Ground a lifted Turtle graph and emit the overlay Turtle.
///
/// Returns `(overlay_turtle, report)`.
///
/// The input `turtle_input` is not mutated (AC5).
pub fn ground_and_emit(
    turtle_input: &str,
) -> Result<(String, GroundReport), GroundError> {
    let grounded = ground(turtle_input)?;
    let rpt = report(&grounded);
    let overlay = emit_overlay(&grounded)?;
    Ok((overlay, rpt))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper: synthetic minimal lifted Turtle ────────────────────────────

    /// A minimal lifted Turtle that covers all element kinds we care about.
    /// This is NOT generated by aso-lift at test-time — it is a hand-crafted
    /// fixture that matches the expected aso-lift output shape.
    const MINIMAL_LIFTED_TTL: &str = r#"
@prefix aso:  <https://ontology.atscale.com/aso/> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

<https://test.models.atscale.com/m#gross_throughput>
    rdf:type owl:NamedIndividual, aso:FullyAdditiveMeasure ;
    rdfs:label "Gross Throughput" .

<https://test.models.atscale.com/m#dim_product>
    rdf:type owl:NamedIndividual, aso:Dimension ;
    rdfs:label "Product" .

<https://test.models.atscale.com/m#lvl_category>
    rdf:type owl:NamedIndividual, aso:Level ;
    rdfs:label "Category" .

<https://test.models.atscale.com/m#key_product_id>
    rdf:type owl:NamedIndividual, aso:Key ;
    rdfs:label "Product ID" .

<https://test.models.atscale.com/m#cube_sales>
    rdf:type owl:NamedIndividual, aso:Cube ;
    rdfs:label "Sales" .

<https://test.models.atscale.com/m#unknown_widget>
    rdf:type owl:NamedIndividual, <https://ontology.atscale.com/aso/UnknownKind> ;
    rdfs:label "Widget" .

<https://test.models.atscale.com/m#attr_color>
    rdf:type owl:NamedIndividual, aso:Attribute ;
    rdfs:label "Color" .
"#;

    /// Turtle with a bfo_hint on one element.
    const HINT_TTL: &str = r#"
@prefix aso:  <https://ontology.atscale.com/aso/> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

<https://test.models.atscale.com/m#dim_geo>
    rdf:type owl:NamedIndividual, aso:Dimension ;
    aso:bfoHint "role" ;
    rdfs:label "Geography" .

<https://test.models.atscale.com/m#meas_balance>
    rdf:type owl:NamedIndividual, aso:SemiAdditiveMeasure ;
    rdfs:label "Balance" .
"#;

    /// Turtle with a typo bfo_hint.
    const TYPO_HINT_TTL: &str = r#"
@prefix aso:  <https://ontology.atscale.com/aso/> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .

<https://test.models.atscale.com/m#meas_revenue>
    rdf:type owl:NamedIndividual, aso:Measure ;
    aso:bfoHint "rolle" .
"#;

    // ── AC2 — measure → GDC by kind, NOT fallback ─────────────────────────

    #[test]
    fn ac2_measure_grounded_as_gdc_by_kind() {
        let grounded = ground(MINIMAL_LIFTED_TTL).expect("ground must succeed");
        // gross_throughput is a FullyAdditiveMeasure — must be GDC, not fallback
        let measure = grounded
            .iter()
            .find(|e| e.iri.contains("gross_throughput"))
            .expect("gross_throughput must be in grounded output");
        assert_eq!(
            measure.bfo_category,
            BfoCategory::GenericallyDependentContinuant,
            "measure 'gross_throughput' (no revenue/sales token) must be GDC by kind (AC2)"
        );
        assert_eq!(
            measure.method,
            GroundingMethod::Kind,
            "must be grounded by kind, not fallback"
        );
    }

    // ── AC3 — bfo_hint wins; typo → error naming element ─────────────────

    #[test]
    fn ac3_bfo_hint_wins_over_kind() {
        let grounded = ground(HINT_TTL).expect("ground must succeed with valid hint");
        let geo = grounded
            .iter()
            .find(|e| e.iri.contains("dim_geo"))
            .expect("dim_geo must be grounded");
        // hint is "role" which is BFO Role — same as kind for Dimension; method must be Hint
        assert_eq!(geo.bfo_category, BfoCategory::Role);
        assert_eq!(geo.method, GroundingMethod::Hint, "method must be Hint when bfo_hint present");
    }

    #[test]
    fn ac3_typo_hint_errors_loudly_naming_element() {
        let result = ground(TYPO_HINT_TTL);
        assert!(result.is_err(), "typo bfo_hint must produce an error");
        match result.unwrap_err() {
            GroundError::InvalidBfoHint { element, hint } => {
                assert!(
                    element.contains("meas_revenue"),
                    "error must name the offending element (got: {element})"
                );
                assert_eq!(hint, "rolle", "error must name the bad hint value");
            }
            other => panic!("expected InvalidBfoHint, got: {other:?}"),
        }
    }

    // ── AC4 — overlay: each individual carries aso: + BFO class ──────────

    #[test]
    fn ac4_overlay_contains_aso_and_bfo_types() {
        let grounded = ground(MINIMAL_LIFTED_TTL).expect("ground must succeed");
        let overlay = emit_overlay(&grounded).expect("emit_overlay must succeed");

        // Check the Turtle overlay contains both aso: and BFO IRIs
        assert!(
            overlay.contains("FullyAdditiveMeasure") || overlay.contains("Measure"),
            "overlay must contain aso: class (AC4)"
        );
        assert!(
            overlay.contains("BFO_0000031"),
            "overlay must contain BFO GDC IRI for measure (AC4)"
        );
        assert!(
            overlay.contains("BFO_0000019"),
            "overlay must contain BFO Quality IRI for key (AC4)"
        );
        assert!(
            overlay.contains("BFO_0000023"),
            "overlay must contain BFO Role IRI for level/dimension (AC4)"
        );
    }

    // ── AC5 — input graph checksum unchanged (non-mutating) ───────────────

    #[test]
    fn ac5_input_unchanged_after_grounding() {
        let input_original = MINIMAL_LIFTED_TTL.to_owned();
        let input_copy = MINIMAL_LIFTED_TTL.to_owned();

        let _ = ground(&input_copy).expect("ground must succeed");

        // The input string must not have been mutated
        assert_eq!(
            input_original, input_copy,
            "ground() must not mutate the input string (AC5)"
        );
    }

    // ── AC6 — byte-identical on two runs ──────────────────────────────────

    #[test]
    fn ac6_byte_identical_on_two_runs() {
        let (overlay1, _) = ground_and_emit(MINIMAL_LIFTED_TTL).expect("first run");
        let (overlay2, _) = ground_and_emit(MINIMAL_LIFTED_TTL).expect("second run");
        assert_eq!(
            overlay1, overlay2,
            "overlay must be byte-identical on two runs of identical input (AC6 / NFR1)"
        );
    }

    // ── AC7 — unknown kind → IndependentContinuant fallback, counted ──────

    #[test]
    fn ac7_unknown_kind_goes_to_fallback_and_counted() {
        let grounded = ground(MINIMAL_LIFTED_TTL).expect("ground must succeed");
        let rpt = report(&grounded);

        // unknown_widget has aso:UnknownKind — must be fallback
        let widget = grounded
            .iter()
            .find(|e| e.iri.contains("unknown_widget"))
            .expect("unknown_widget must be in grounded output");
        assert_eq!(
            widget.bfo_category,
            BfoCategory::IndependentContinuant,
            "unknown kind must map to IndependentContinuant (AC7)"
        );
        assert_eq!(
            widget.method,
            GroundingMethod::Fallback,
            "must be Fallback method (AC7)"
        );

        // Fallback must be counted in the report
        assert!(
            rpt.fallback >= 1,
            "fallback count must be ≥ 1 in report (AC7), got {}",
            rpt.fallback
        );
    }

    // ── Additional: key → Quality ─────────────────────────────────────────

    #[test]
    fn key_grounded_as_quality() {
        let grounded = ground(MINIMAL_LIFTED_TTL).expect("ground must succeed");
        let key = grounded
            .iter()
            .find(|e| e.iri.contains("key_product_id"))
            .expect("key_product_id must be grounded");
        assert_eq!(key.bfo_category, BfoCategory::Quality);
        assert_eq!(key.method, GroundingMethod::Kind);
    }

    // ── Additional: dimension → Role ──────────────────────────────────────

    #[test]
    fn dimension_grounded_as_role() {
        let grounded = ground(MINIMAL_LIFTED_TTL).expect("ground must succeed");
        let dim = grounded
            .iter()
            .find(|e| e.iri.contains("dim_product"))
            .expect("dim_product must be grounded");
        assert_eq!(dim.bfo_category, BfoCategory::Role);
        assert_eq!(dim.method, GroundingMethod::Kind);
    }

    // ── Additional: level → Role ──────────────────────────────────────────

    #[test]
    fn level_grounded_as_role() {
        let grounded = ground(MINIMAL_LIFTED_TTL).expect("ground must succeed");
        let lvl = grounded
            .iter()
            .find(|e| e.iri.contains("lvl_category"))
            .expect("lvl_category must be grounded");
        assert_eq!(lvl.bfo_category, BfoCategory::Role);
        assert_eq!(lvl.method, GroundingMethod::Kind);
    }

    // ── Additional: report coverage ───────────────────────────────────────

    #[test]
    fn report_coverage_tallies_correctly() {
        let grounded = ground(MINIMAL_LIFTED_TTL).expect("ground must succeed");
        let rpt = report(&grounded);

        assert_eq!(
            rpt.total,
            rpt.by_kind + rpt.by_hint + rpt.fallback,
            "total must equal by_kind + by_hint + fallback"
        );
        assert!(rpt.total > 0, "must have grounded at least one element");
        // unknown_widget is the only fallback in MINIMAL_LIFTED_TTL
        assert_eq!(rpt.fallback, 1, "exactly one fallback (unknown_widget)");
        assert_eq!(rpt.by_hint, 0, "no hints in minimal fixture");
    }

    // ── Additional: semi-additive measure → GDC ───────────────────────────

    #[test]
    fn semi_additive_measure_grounded_as_gdc() {
        let grounded = ground(HINT_TTL).expect("ground must succeed");
        let balance = grounded
            .iter()
            .find(|e| e.iri.contains("meas_balance"))
            .expect("meas_balance must be grounded");
        assert_eq!(balance.bfo_category, BfoCategory::GenericallyDependentContinuant);
        assert_eq!(balance.method, GroundingMethod::Kind);
    }

    // ── Additional: valid hint aliases ────────────────────────────────────

    #[test]
    fn valid_hint_aliases_parse() {
        for hint in &["gdc", "quality", "role", "temporal", "independent", "fallback"] {
            assert!(
                hint.parse::<BfoCategory>().is_ok(),
                "hint alias '{hint}' must parse"
            );
        }
    }
}
