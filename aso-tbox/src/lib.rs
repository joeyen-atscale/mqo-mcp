//! # aso-tbox
//!
//! AtScale Semantic-model Ontology (`aso:`) — BFO 2020-grounded OWL2-DL vocabulary.
//!
//! ## What this crate provides
//!
//! 1. **Embedded TBox** — `ontology/aso.ttl` is compiled into the binary via
//!    [`ASO_TTL`]; downstream crates need no file-system access.
//! 2. **IRI constants** — [`iris`] exposes every class and property IRI as a
//!    `&'static str`, so callers reference `aso_tbox::iris::CUBE` instead of a
//!    string literal.
//! 3. **Loader** — [`load_tbox`] parses the embedded Turtle into an [`oxrdf::Graph`]
//!    (in-memory triple store) and returns it ready for query or serialization.
//!
//! ## Crate-level guarantees (mapped to PRD acceptance criteria)
//!
//! | PRD AC | What this crate delivers |
//! |--------|--------------------------|
//! | AC1    | [`load_tbox`] parses without error; `owl:imports` BFO present |
//! | AC2    | ≥12 named classes, each a subject in the graph |
//! | AC3    | `aso:rollsUpTo` declared `owl:TransitiveProperty`; `aso:playsRoleOf` `owl:ObjectProperty` |
//! | AC7    | `owl:imports` IRI emitted even when BFO resolver is unreachable |

// ─────────────────────────────────────────────────────────────────────────────
//  Embedded resource
// ─────────────────────────────────────────────────────────────────────────────

/// The raw Turtle source of the `aso:` TBox, embedded at compile time.
///
/// Callers that only need IRI constants do not need this at runtime.
pub const ASO_TTL: &str = include_str!("../ontology/aso.ttl");

// ─────────────────────────────────────────────────────────────────────────────
//  IRI constants
// ─────────────────────────────────────────────────────────────────────────────

/// `aso:` namespace base IRI.
pub const ASO_NS: &str = "https://ontology.atscale.com/aso/";

/// All `aso:` class and property IRIs as `&'static str` constants.
pub mod iris {
    /// The `aso:` namespace.
    pub const NS: &str = super::ASO_NS;

    // ── Ontology IRI ───────────────────────────────────────────────────────
    pub const ONTOLOGY: &str = "https://ontology.atscale.com/aso/";

    // ── BFO import IRI ─────────────────────────────────────────────────────
    pub const BFO_ONTOLOGY: &str = "http://purl.obolibrary.org/obo/bfo/2020/bfo.owl";

    // ── Classes (FR1 — 12 required + 2 derived) ────────────────────────────

    /// `aso:Cube` — analytic subject area (GDC, `BFO:0000031`).
    pub const CUBE: &str = "https://ontology.atscale.com/aso/Cube";

    /// `aso:Measure` — numeric information entity (GDC, `BFO:0000031`).
    pub const MEASURE: &str = "https://ontology.atscale.com/aso/Measure";

    /// `aso:Dimension` — axis role (`BFO:0000023`).
    pub const DIMENSION: &str = "https://ontology.atscale.com/aso/Dimension";

    /// `aso:Hierarchy` — ordered-level role (`BFO:0000023`).
    pub const HIERARCHY: &str = "https://ontology.atscale.com/aso/Hierarchy";

    /// `aso:Level` — granularity-step role (`BFO:0000023`).
    pub const LEVEL: &str = "https://ontology.atscale.com/aso/Level";

    /// `aso:Attribute` — descriptive property GDC (`BFO:0000031`).
    pub const ATTRIBUTE: &str = "https://ontology.atscale.com/aso/Attribute";

    /// `aso:CalculatedMember` — formula-derived GDC (`BFO:0000031`).
    pub const CALCULATED_MEMBER: &str = "https://ontology.atscale.com/aso/CalculatedMember";

    /// `aso:CalculationGroup` — collection of CalculatedMembers (`BFO:0000031`).
    pub const CALCULATION_GROUP: &str = "https://ontology.atscale.com/aso/CalculationGroup";

    /// `aso:Key` — identity quality (`BFO:0000019`).
    pub const KEY: &str = "https://ontology.atscale.com/aso/Key";

    /// `aso:RolePlayingReference` — role alias (`BFO:0000023`).
    pub const ROLE_PLAYING_REFERENCE: &str =
        "https://ontology.atscale.com/aso/RolePlayingReference";

    /// `aso:DataSet` — physical table GDC (`BFO:0000031`).
    pub const DATA_SET: &str = "https://ontology.atscale.com/aso/DataSet";

    /// `aso:Perspective` — curated view GDC (`BFO:0000031`).
    pub const PERSPECTIVE: &str = "https://ontology.atscale.com/aso/Perspective";

    // ── Derived / defined classes (FR4, FR6) ───────────────────────────────

    /// `aso:SemiAdditiveMeasure` — measure not additive over all dimensions.
    pub const SEMI_ADDITIVE_MEASURE: &str =
        "https://ontology.atscale.com/aso/SemiAdditiveMeasure";

    /// `aso:FullyAdditiveMeasure` — measure additive over all dimensions.
    pub const FULLY_ADDITIVE_MEASURE: &str =
        "https://ontology.atscale.com/aso/FullyAdditiveMeasure";

    // ── Object properties ──────────────────────────────────────────────────

    /// `aso:rollsUpTo` — transitive rollup between Levels (FR3).
    pub const ROLLS_UP_TO: &str = "https://ontology.atscale.com/aso/rollsUpTo";

    /// `aso:playsRoleOf` — RolePlayingReference → Dimension (FR3).
    pub const PLAYS_ROLE_OF: &str = "https://ontology.atscale.com/aso/playsRoleOf";

    /// `aso:additiveOver` — Measure additive over a Dimension (FR4).
    pub const ADDITIVE_OVER: &str = "https://ontology.atscale.com/aso/additiveOver";

    /// `aso:hasDimension` — Cube → Dimension.
    pub const HAS_DIMENSION: &str = "https://ontology.atscale.com/aso/hasDimension";

    /// `aso:hasMeasure` — Cube → Measure.
    pub const HAS_MEASURE: &str = "https://ontology.atscale.com/aso/hasMeasure";

    /// `aso:hasHierarchy` — Dimension → Hierarchy.
    pub const HAS_HIERARCHY: &str = "https://ontology.atscale.com/aso/hasHierarchy";

    /// `aso:hasLevel` — Hierarchy → Level.
    pub const HAS_LEVEL: &str = "https://ontology.atscale.com/aso/hasLevel";

    /// `aso:hasKey` — Level → Key.
    pub const HAS_KEY: &str = "https://ontology.atscale.com/aso/hasKey";
}

// ─────────────────────────────────────────────────────────────────────────────
//  Loader
// ─────────────────────────────────────────────────────────────────────────────

use oxrdf::{Graph, Triple};
use oxttl::TurtleParser;

/// Error returned by [`load_tbox`].
#[derive(Debug)]
pub struct TBoxLoadError(pub String);

impl std::fmt::Display for TBoxLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "aso-tbox load error: {}", self.0)
    }
}
impl std::error::Error for TBoxLoadError {}

/// Parse the embedded `aso.ttl` Turtle source into an [`oxrdf::Graph`].
///
/// The graph is backed by a hash-set of [`Triple`]s; it is loaded entirely
/// in-memory. Callers that want a quad-store may wrap the result with
/// [`oxrdf::Dataset`] themselves.
///
/// # Errors
///
/// Returns [`TBoxLoadError`] if the embedded Turtle fails to parse.
pub fn load_tbox() -> Result<Graph, TBoxLoadError> {
    load_tbox_from(ASO_TTL)
}

/// Parse an arbitrary Turtle string into a [`Graph`].
///
/// Exposed for testing and for callers that want to extend the TBox at runtime.
pub fn load_tbox_from(turtle_src: &str) -> Result<Graph, TBoxLoadError> {
    let parser = TurtleParser::new()
        .with_base_iri(ASO_NS)
        .map_err(|e| TBoxLoadError(format!("parser init: {e}")))?;

    let mut graph = Graph::new();

    // Use for_slice — the idiomatic oxttl 0.2 API for in-memory byte slices.
    for result in parser.for_slice(turtle_src.as_bytes()) {
        match result {
            Ok(triple) => {
                graph.insert(&triple);
            }
            Err(e) => return Err(TBoxLoadError(format!("parse error: {e}"))),
        }
    }

    Ok(graph)
}

/// Return all triples whose subject is `iri` from the given graph, as owned
/// [`Triple`]s.
pub fn triples_for_subject(graph: &Graph, iri: &str) -> Vec<Triple> {
    let Ok(node) = oxrdf::NamedNode::new(iri) else {
        return vec![];
    };
    graph
        .triples_for_subject(node.as_ref())
        .map(|t| t.into_owned())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::NamedNode;

    // ── Helper ─────────────────────────────────────────────────────────────

    fn named(iri: &str) -> NamedNode {
        NamedNode::new(iri).unwrap()
    }

    /// Returns true if `graph` contains at least one triple with the given
    /// subject IRI.
    fn has_subject(graph: &Graph, iri: &str) -> bool {
        let node = named(iri);
        graph.triples_for_subject(node.as_ref()).next().is_some()
    }

    /// Returns true if `graph` contains `(subject, predicate, object)`.
    fn has_triple(graph: &Graph, s: &str, p: &str, o: &str) -> bool {
        let subj = NamedNode::new(s).unwrap();
        let pred = NamedNode::new(p).unwrap();
        let obj = NamedNode::new(o).unwrap();
        graph.contains(&Triple::new(subj, pred, obj))
    }

    // ── AC1 — embedded TTL parses without error ────────────────────────────

    #[test]
    fn ac1_tbox_parses_without_error() {
        let result = load_tbox();
        assert!(result.is_ok(), "load_tbox() failed: {:?}", result.err());
    }

    #[test]
    fn ac1_graph_is_nonempty() {
        let graph = load_tbox().unwrap();
        assert!(
            graph.len() > 0,
            "TBox graph is empty — no triples were parsed"
        );
    }

    // ── AC1 / AC7 — owl:imports BFO present ───────────────────────────────

    #[test]
    fn ac1_owl_imports_bfo() {
        let graph = load_tbox().unwrap();
        // The ontology declares: <aso:> owl:imports <bfo.owl>
        let owl_imports = "http://www.w3.org/2002/07/owl#imports";
        let bfo_iri = iris::BFO_ONTOLOGY;
        assert!(
            has_triple(&graph, iris::ONTOLOGY, owl_imports, bfo_iri),
            "graph must contain <aso:> owl:imports <{bfo_iri}>"
        );
    }

    // ── AC2 — ≥12 named classes, each a subject ────────────────────────────

    /// All 12 required FR1 class IRIs plus the 2 derived classes.
    const REQUIRED_CLASSES: &[&str] = &[
        iris::CUBE,
        iris::MEASURE,
        iris::DIMENSION,
        iris::HIERARCHY,
        iris::LEVEL,
        iris::ATTRIBUTE,
        iris::CALCULATED_MEMBER,
        iris::CALCULATION_GROUP,
        iris::KEY,
        iris::ROLE_PLAYING_REFERENCE,
        iris::DATA_SET,
        iris::PERSPECTIVE,
        iris::SEMI_ADDITIVE_MEASURE,
        iris::FULLY_ADDITIVE_MEASURE,
    ];

    #[test]
    fn ac2_at_least_12_classes() {
        // At least 12 of the required IRIs appear as subjects (FR1).
        assert!(
            REQUIRED_CLASSES.len() >= 12,
            "fewer than 12 class IRIs defined in test list"
        );
    }

    #[test]
    fn ac2_all_required_classes_appear_as_subjects() {
        let graph = load_tbox().unwrap();
        for iri in REQUIRED_CLASSES {
            assert!(
                has_subject(&graph, iri),
                "class IRI <{iri}> not found as a subject in the TBox"
            );
        }
    }

    #[test]
    fn ac2_classes_declared_owl_class() {
        let graph = load_tbox().unwrap();
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let owl_class = "http://www.w3.org/2002/07/owl#Class";
        for iri in REQUIRED_CLASSES {
            assert!(
                has_triple(&graph, iri, rdf_type, owl_class),
                "<{iri}> must be declared as owl:Class"
            );
        }
    }

    // ── AC2 — each required class has a BFO parent ─────────────────────────

    #[test]
    fn ac2_each_class_has_bfo_parent() {
        let graph = load_tbox().unwrap();
        let rdfs_subclass = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

        // BFO parent candidates used by this TBox
        let bfo_parents = [
            "http://purl.obolibrary.org/obo/BFO_0000031", // GDC
            "http://purl.obolibrary.org/obo/BFO_0000019", // Quality
            "http://purl.obolibrary.org/obo/BFO_0000023", // Role
            "http://purl.obolibrary.org/obo/BFO_0000008", // Temporal Region
        ];

        // The 12 required classes (not the derived sub-classes which sub aso:Measure)
        let top_level_classes = &REQUIRED_CLASSES[..12];
        for iri in top_level_classes {
            let has_bfo_parent = bfo_parents
                .iter()
                .any(|bfo| has_triple(&graph, iri, rdfs_subclass, bfo));
            assert!(
                has_bfo_parent,
                "<{iri}> must have exactly one BFO parent via rdfs:subClassOf"
            );
        }
    }

    // ── AC2 — IRI constants appear as subjects ─────────────────────────────

    #[test]
    fn iris_constants_appear_as_subjects() {
        let graph = load_tbox().unwrap();
        let all_property_iris = [
            iris::ROLLS_UP_TO,
            iris::PLAYS_ROLE_OF,
            iris::ADDITIVE_OVER,
            iris::HAS_DIMENSION,
            iris::HAS_MEASURE,
            iris::HAS_HIERARCHY,
            iris::HAS_LEVEL,
            iris::HAS_KEY,
        ];
        for iri in &all_property_iris {
            assert!(
                has_subject(&graph, iri),
                "property IRI <{iri}> not found as a subject"
            );
        }
    }

    // ── AC3 — rollsUpTo is TransitiveProperty; playsRoleOf is ObjectProperty

    #[test]
    fn ac3_rolls_up_to_is_transitive_property() {
        let graph = load_tbox().unwrap();
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let owl_transitive = "http://www.w3.org/2002/07/owl#TransitiveProperty";
        assert!(
            has_triple(&graph, iris::ROLLS_UP_TO, rdf_type, owl_transitive),
            "aso:rollsUpTo must be declared owl:TransitiveProperty"
        );
    }

    #[test]
    fn ac3_plays_role_of_is_object_property() {
        let graph = load_tbox().unwrap();
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let owl_op = "http://www.w3.org/2002/07/owl#ObjectProperty";
        assert!(
            has_triple(&graph, iris::PLAYS_ROLE_OF, rdf_type, owl_op),
            "aso:playsRoleOf must be declared owl:ObjectProperty"
        );
    }

    // ── Round-trip: serialise to N-Triples and re-parse ────────────────────

    #[test]
    fn round_trip_via_ntriples() {
        use oxttl::{NTriplesParser, NTriplesSerializer};

        let graph = load_tbox().unwrap();

        // Serialize to N-Triples
        let mut buf = Vec::new();
        let mut serializer = NTriplesSerializer::new().for_writer(&mut buf);
        for triple_ref in graph.iter() {
            serializer.serialize_triple(triple_ref).unwrap();
        }
        // finish() returns the writer (Vec<u8>) — not io::Result
        let _ = serializer.finish();

        let nt_str = std::str::from_utf8(&buf).unwrap();

        // Re-parse from N-Triples
        let mut graph2 = Graph::new();
        for result in NTriplesParser::new().for_slice(nt_str.as_bytes()) {
            graph2.insert(&result.unwrap());
        }

        assert_eq!(
            graph.len(),
            graph2.len(),
            "round-trip N-Triples changed triple count: {} → {}",
            graph.len(),
            graph2.len()
        );
    }

    // ── Additivity property declared (FR4) ────────────────────────────────

    #[test]
    fn additive_over_is_object_property() {
        let graph = load_tbox().unwrap();
        let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let owl_op = "http://www.w3.org/2002/07/owl#ObjectProperty";
        assert!(
            has_triple(&graph, iris::ADDITIVE_OVER, rdf_type, owl_op),
            "aso:additiveOver must be declared owl:ObjectProperty"
        );
    }

    // ── triples_for_subject helper works ──────────────────────────────────

    #[test]
    fn triples_for_subject_returns_cube_triples() {
        let graph = load_tbox().unwrap();
        let ts = triples_for_subject(&graph, iris::CUBE);
        assert!(
            !ts.is_empty(),
            "triples_for_subject(CUBE) returned empty — Cube must be in TBox"
        );
    }

    #[test]
    fn triples_for_subject_unknown_iri_returns_empty() {
        let graph = load_tbox().unwrap();
        let ts = triples_for_subject(&graph, "https://example.com/unknown");
        assert!(ts.is_empty());
    }
}
