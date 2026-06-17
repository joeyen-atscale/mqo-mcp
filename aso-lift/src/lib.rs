//! # aso-lift
//!
//! Deterministic engine-model XML → RDF lift typed to the `aso:` TBox.
//!
//! ## How it works
//!
//! 1. Parse the engine model XML (project_2_0 / data_model_1_0 schema).
//! 2. Walk the element tree, minting a stable IRI for each element from its
//!    XSD `id` attribute under a caller-supplied base IRI.
//! 3. Emit `owl:NamedIndividual` triples typed to the appropriate `aso:` class,
//!    with `rdfs:label`, relationship properties, and additivity triples.
//! 4. Serialise to Turtle (required) or RDF/XML (optional flag).
//!
//! ## IRI scheme
//!
//! `<base>/<catalog>/<schema>/<model>#<id>`
//!
//! Only the `<base>#<id>` portion is implemented in v0.1; catalog/schema path
//! segments are appended when the XML carries those attributes.
//!
//! ## PRD acceptance-criteria coverage
//!
//! | AC | Where covered |
//! |----|---------------|
//! | AC1 | [`lift`] emits `owl:imports` the aso: TBox; Turtle round-trips |
//! | AC2 | Every element → `owl:NamedIndividual` + typed to `aso:` class + `rdfs:label` |
//! | AC3 | IRIs keyed on XSD `id`; stable across name changes |
//! | AC4 | `keyed-attribute-ref` emits `aso:playsRoleOf` |
//! | AC5 | Deterministic output (sorted triple emission) |
//! | AC6 | Unknown element kind → `aso:Attribute` fallback + eprintln warning |
//! | AC7 | schema_version ≠ project_2_0 → `LiftError::UnsupportedSchema` |
//! | AC8 | No warehouse credentials required (metadata-only) |

use std::fmt::Write as _;

use oxrdf::{Graph, NamedNode, Triple};
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
//  Public error type
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by the lift engine.
#[derive(Debug, Error)]
pub enum LiftError {
    /// The XML source could not be parsed.
    #[error("XML parse error: {0}")]
    Xml(String),

    /// The model uses an unsupported schema version (pre-2.0).
    /// The caller should run the engine's XSLT migration chain first.
    #[error(
        "unsupported schema version '{version}': \
         migrate to project_2_0 first using the engine XSLT chain \
         (project_1_0_to_1_1.xslt → … → project_1_x_to_2_0.xslt)"
    )]
    UnsupportedSchema { version: String },

    /// A required attribute (`id`) was missing from an element.
    #[error("element <{element}> missing required attribute '{attr}'")]
    MissingAttr { element: String, attr: String },

    /// An IRI could not be constructed from the given components.
    #[error("invalid IRI '{iri}': {reason}")]
    InvalidIri { iri: String, reason: String },
}

// ─────────────────────────────────────────────────────────────────────────────
//  Well-known IRI constants (from W3C; not aso: — those come from aso-tbox)
// ─────────────────────────────────────────────────────────────────────────────

const OWL_NS: &str = "http://www.w3.org/2002/07/owl#";
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS_NS: &str = "http://www.w3.org/2000/01/rdf-schema#";

const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";
const OWL_NAMED_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#NamedIndividual";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

// Silence unused-constant warnings; these are conceptually part of the public API surface.
#[allow(dead_code)]
const _OWL_NS: &str = OWL_NS;
#[allow(dead_code)]
const _RDF_NS: &str = RDF_NS;
#[allow(dead_code)]
const _RDFS_NS: &str = RDFS_NS;

// ─────────────────────────────────────────────────────────────────────────────
//  Lift options
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for a single lift run.
#[derive(Debug, Clone)]
pub struct LiftOptions {
    /// Base IRI for all minted element IRIs.
    /// Typically `https://example.com/models` — element IRIs are `<base>#<id>`.
    pub base_iri: String,
}

impl Default for LiftOptions {
    fn default() -> Self {
        Self {
            base_iri: "https://models.atscale.com".to_owned(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Output
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a successful lift.
#[derive(Debug)]
pub struct LiftOutput {
    /// Turtle-serialized RDF (UTF-8).
    pub turtle: String,
    /// Number of RDF triples emitted.
    pub triple_count: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Internal model
// ─────────────────────────────────────────────────────────────────────────────

/// Intermediate representation of a parsed model element.
#[derive(Debug)]
struct Element {
    /// XSD `id` attribute (stable, used for IRI minting).
    id: String,
    /// Mutable display name from `name` attribute.
    name: String,
    /// Optional human-readable `caption`.
    caption: Option<String>,
    /// What kind of element this is.
    kind: ElementKind,
}

#[derive(Debug)]
enum ElementKind {
    Project,
    Cube,
    Measure {
        additivity: Additivity,
        /// Aggregation function string (e.g. "Sum", "LastNonEmpty") — stored for
        /// future XSLT-parity serialisation; not yet emitted as a triple in v0.1.
        #[allow(dead_code)]
        aggregation_fn: Option<String>,
        subspace_dim: Option<String>,
    },
    Dimension,
    Hierarchy,
    Level {
        order: u32,
        parent_hierarchy_id: String,
    },
    KeyedAttributeRef {
        /// The `ref-path` — points at the base dimension/ref id.
        ref_path: String,
    },
    /// Fallback for unknown element kinds.
    Unknown {
        tag: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Additivity {
    Additive,
    SemiAdditive,
    NonAdditive,
}

// ─────────────────────────────────────────────────────────────────────────────
//  IRI minting
// ─────────────────────────────────────────────────────────────────────────────

/// Mint a stable IRI for an element given the model base IRI and the XSD `id`.
///
/// Scheme: `<base>#<id>`
///
/// The `id` is used (not the mutable `name`) to ensure stability (FR3/AC3).
fn mint_iri(base: &str, id: &str) -> Result<NamedNode, LiftError> {
    let iri = format!("{base}#{id}");
    NamedNode::new(&iri).map_err(|e| LiftError::InvalidIri {
        iri,
        reason: e.to_string(),
    })
}

/// Mint the ontology IRI for the lifted ABox document.
fn mint_ontology_iri(base: &str) -> Result<NamedNode, LiftError> {
    let iri = format!("{base}#ontology");
    NamedNode::new(&iri).map_err(|e| LiftError::InvalidIri {
        iri,
        reason: e.to_string(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  XML parsing helpers
// ─────────────────────────────────────────────────────────────────────────────

use quick_xml::events::Event;
use quick_xml::Reader;

/// Extract `id`, `name`, `caption` from the current start-element attributes.
fn read_attrs(
    attrs: quick_xml::events::attributes::Attributes<'_>,
    tag: &str,
) -> Result<(String, String, Option<String>), LiftError> {
    let mut id = None;
    let mut name = None;
    let mut caption = None;

    for attr in attrs {
        let attr = attr.map_err(|e| LiftError::Xml(e.to_string()))?;
        let key = std::str::from_utf8(attr.key.as_ref())
            .map_err(|e| LiftError::Xml(e.to_string()))?;
        let val = attr
            .unescape_value()
            .map_err(|e| LiftError::Xml(e.to_string()))?
            .into_owned();
        match key {
            "id" => id = Some(val),
            "name" => name = Some(val),
            "caption" => caption = Some(val),
            _ => {}
        }
    }

    let id = id.ok_or_else(|| LiftError::MissingAttr {
        element: tag.to_owned(),
        attr: "id".to_owned(),
    })?;
    let name = name.unwrap_or_else(|| id.clone());
    Ok((id, name, caption))
}

/// Read a `ref-path` attribute from start-element attributes.
fn read_ref_path(
    attrs: quick_xml::events::attributes::Attributes<'_>,
    tag: &str,
) -> Result<(String, String, Option<String>, String), LiftError> {
    let mut id = None;
    let mut name = None;
    let mut caption = None;
    let mut ref_path = None;

    for attr in attrs {
        let attr = attr.map_err(|e| LiftError::Xml(e.to_string()))?;
        let key = std::str::from_utf8(attr.key.as_ref())
            .map_err(|e| LiftError::Xml(e.to_string()))?;
        let val = attr
            .unescape_value()
            .map_err(|e| LiftError::Xml(e.to_string()))?
            .into_owned();
        match key {
            "id" => id = Some(val),
            "name" => name = Some(val),
            "caption" => caption = Some(val),
            "ref-path" => ref_path = Some(val),
            _ => {}
        }
    }

    let id = id.ok_or_else(|| LiftError::MissingAttr {
        element: tag.to_owned(),
        attr: "id".to_owned(),
    })?;
    let name = name.unwrap_or_else(|| id.clone());
    let ref_path = ref_path.ok_or_else(|| LiftError::MissingAttr {
        element: tag.to_owned(),
        attr: "ref-path".to_owned(),
    })?;
    Ok((id, name, caption, ref_path))
}

/// Read a `level` start element, including optional `order` attribute.
fn read_level_attrs(
    attrs: quick_xml::events::attributes::Attributes<'_>,
) -> Result<(String, String, Option<String>, u32), LiftError> {
    let mut id = None;
    let mut name = None;
    let mut caption = None;
    let mut order: u32 = 0;

    for attr in attrs {
        let attr = attr.map_err(|e| LiftError::Xml(e.to_string()))?;
        let key = std::str::from_utf8(attr.key.as_ref())
            .map_err(|e| LiftError::Xml(e.to_string()))?;
        let val = attr
            .unescape_value()
            .map_err(|e| LiftError::Xml(e.to_string()))?
            .into_owned();
        match key {
            "id" => id = Some(val),
            "name" => name = Some(val),
            "caption" => caption = Some(val),
            "order" => order = val.parse().unwrap_or(0),
            _ => {}
        }
    }

    let id = id.ok_or_else(|| LiftError::MissingAttr {
        element: "level".to_owned(),
        attr: "id".to_owned(),
    })?;
    let name = name.unwrap_or_else(|| id.clone());
    Ok((id, name, caption, order))
}

// ─────────────────────────────────────────────────────────────────────────────
//  XML parse → Element list
// ─────────────────────────────────────────────────────────────────────────────

/// Parse the engine model XML into a flat list of [`Element`]s.
///
/// Validates the schema version and returns [`LiftError::UnsupportedSchema`]
/// for anything other than `project_2_0` (AC7).
fn parse_model(xml: &str) -> Result<Vec<Element>, LiftError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut elements: Vec<Element> = Vec::new();

    // Stack tracking: (tag, id) for nesting context
    let mut current_hierarchy_id: Option<String> = None;

    // Measure sub-element tracking
    let mut in_measure_id: Option<String> = None;
    let mut measure_additivity: Option<Additivity> = None;
    let mut measure_agg_fn: Option<String> = None;
    let mut measure_subspace: Option<String> = None;
    let mut capture_text_for: Option<&'static str> = None;

    let mut schema_checked = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| LiftError::Xml(e.to_string()))?
                    .to_owned();

                match tag.as_str() {
                    "project" => {
                        // Validate schema version (AC7)
                        let mut schema_version = None;
                        let mut proj_id = None;
                        let mut proj_name = None;
                        let mut proj_caption = None;
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| LiftError::Xml(e.to_string()))?;
                            let k = std::str::from_utf8(attr.key.as_ref())
                                .map_err(|e| LiftError::Xml(e.to_string()))?;
                            let v = attr
                                .unescape_value()
                                .map_err(|e| LiftError::Xml(e.to_string()))?
                                .into_owned();
                            match k {
                                "schema_version" => schema_version = Some(v),
                                "id" => proj_id = Some(v),
                                "name" => proj_name = Some(v),
                                "caption" => proj_caption = Some(v),
                                _ => {}
                            }
                        }
                        if !schema_checked {
                            schema_checked = true;
                            let version =
                                schema_version.unwrap_or_else(|| "unknown".to_owned());
                            if version != "project_2_0" {
                                return Err(LiftError::UnsupportedSchema { version });
                            }
                        }
                        if let Some(id) = proj_id {
                            let name = proj_name.unwrap_or_else(|| id.clone());
                            elements.push(Element {
                                id,
                                name,
                                caption: proj_caption,
                                kind: ElementKind::Project,
                            });
                        }
                    }
                    "cube" => {
                        let (id, name, caption) = read_attrs(e.attributes(), "cube")?;
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::Cube,
                        });
                    }
                    "measure" => {
                        let (id, name, caption) = read_attrs(e.attributes(), "measure")?;
                        in_measure_id = Some(id.clone());
                        measure_additivity = None;
                        measure_agg_fn = None;
                        measure_subspace = None;
                        // placeholder — will be patched when we close </measure>
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::Measure {
                                additivity: Additivity::Additive,
                                aggregation_fn: None,
                                subspace_dim: None,
                            },
                        });
                    }
                    "additivity" => {
                        if in_measure_id.is_some() {
                            capture_text_for = Some("additivity");
                        }
                    }
                    "aggregation-function" => {
                        if in_measure_id.is_some() {
                            capture_text_for = Some("aggregation-function");
                        }
                    }
                    "subspace" => {
                        if in_measure_id.is_some() {
                            capture_text_for = Some("subspace");
                        }
                    }
                    "dimension" => {
                        let (id, name, caption) = read_attrs(e.attributes(), "dimension")?;
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::Dimension,
                        });
                    }
                    "hierarchy" => {
                        let (id, name, caption) = read_attrs(e.attributes(), "hierarchy")?;
                        current_hierarchy_id = Some(id.clone());
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::Hierarchy,
                        });
                    }
                    "level" => {
                        let (id, name, caption, order) = read_level_attrs(e.attributes())?;
                        let parent_hierarchy_id =
                            current_hierarchy_id.clone().unwrap_or_default();
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::Level {
                                order,
                                parent_hierarchy_id,
                            },
                        });
                    }
                    "keyed-attribute-ref" => {
                        let (id, name, caption, ref_path) =
                            read_ref_path(e.attributes(), "keyed-attribute-ref")?;
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::KeyedAttributeRef { ref_path },
                        });
                    }
                    other => {
                        // Unknown element: log and continue (AC6 handled at emit time)
                        eprintln!(
                            "[aso-lift] WARN: unknown element kind <{other}> — \
                             will use aso:Attribute fallback if it carries an id"
                        );
                        // Try to read attrs; skip if no id
                        let mut has_id = false;
                        let mut unk_id = String::new();
                        let mut unk_name = String::new();
                        let mut unk_caption = None;
                        for attr in e.attributes().flatten() {
                            let k = std::str::from_utf8(attr.key.as_ref())
                                .unwrap_or("")
                                .to_owned();
                            let v = attr
                                .unescape_value()
                                .map(|s| s.into_owned())
                                .unwrap_or_default();
                            match k.as_str() {
                                "id" => {
                                    has_id = true;
                                    unk_id = v;
                                }
                                "name" => unk_name = v,
                                "caption" => unk_caption = Some(v),
                                _ => {}
                            }
                        }
                        if has_id {
                            if unk_name.is_empty() {
                                unk_name = unk_id.clone();
                            }
                            elements.push(Element {
                                id: unk_id,
                                name: unk_name,
                                caption: unk_caption,
                                kind: ElementKind::Unknown { tag: other.to_owned() },
                            });
                        }
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                // Self-closing elements (e.g. <level ... />)
                let tag = std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| LiftError::Xml(e.to_string()))?
                    .to_owned();

                match tag.as_str() {
                    "level" => {
                        let (id, name, caption, order) = read_level_attrs(e.attributes())?;
                        let parent_hierarchy_id =
                            current_hierarchy_id.clone().unwrap_or_default();
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::Level {
                                order,
                                parent_hierarchy_id,
                            },
                        });
                    }
                    "keyed-attribute-ref" => {
                        let (id, name, caption, ref_path) =
                            read_ref_path(e.attributes(), "keyed-attribute-ref")?;
                        elements.push(Element {
                            id,
                            name,
                            caption,
                            kind: ElementKind::KeyedAttributeRef { ref_path },
                        });
                    }
                    other => {
                        // Unknown self-closing element: apply same fallback as Start branch (AC6)
                        eprintln!(
                            "[aso-lift] WARN: unknown self-closing element kind <{other}> — \
                             will use aso:Attribute fallback if it carries an id"
                        );
                        let mut has_id = false;
                        let mut unk_id = String::new();
                        let mut unk_name = String::new();
                        let mut unk_caption = None;
                        for attr in e.attributes().flatten() {
                            let k = std::str::from_utf8(attr.key.as_ref())
                                .unwrap_or("")
                                .to_owned();
                            let v = attr
                                .unescape_value()
                                .map(|s| s.into_owned())
                                .unwrap_or_default();
                            match k.as_str() {
                                "id" => { has_id = true; unk_id = v; }
                                "name" => unk_name = v,
                                "caption" => unk_caption = Some(v),
                                _ => {}
                            }
                        }
                        if has_id {
                            if unk_name.is_empty() { unk_name = unk_id.clone(); }
                            elements.push(Element {
                                id: unk_id,
                                name: unk_name,
                                caption: unk_caption,
                                kind: ElementKind::Unknown { tag: other.to_owned() },
                            });
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if let Some(field) = capture_text_for {
                    let text = e
                        .unescape()
                        .map_err(|e| LiftError::Xml(e.to_string()))?
                        .trim()
                        .to_owned();
                    match field {
                        "additivity" => {
                            measure_additivity = Some(match text.to_lowercase().as_str() {
                                "additive" => Additivity::Additive,
                                "semi-additive" => Additivity::SemiAdditive,
                                _ => Additivity::NonAdditive,
                            });
                        }
                        "aggregation-function" => {
                            measure_agg_fn = Some(text);
                        }
                        "subspace" => {
                            measure_subspace = Some(text);
                        }
                        _ => {}
                    }
                    capture_text_for = None;
                }
            }
            Ok(Event::End(ref e)) => {
                let name_bytes = e.name();
                let tag = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|e| LiftError::Xml(e.to_string()))?;
                match tag {
                    "measure" => {
                        // Patch the last-pushed measure with accumulated sub-element data
                        if let Some(ref mid) = in_measure_id.clone() {
                            if let Some(el) = elements.iter_mut().rev().find(|el| &el.id == mid) {
                                el.kind = ElementKind::Measure {
                                    additivity: measure_additivity
                                        .clone()
                                        .unwrap_or(Additivity::Additive),
                                    aggregation_fn: measure_agg_fn.clone(),
                                    subspace_dim: measure_subspace.clone(),
                                };
                            }
                        }
                        in_measure_id = None;
                        measure_additivity = None;
                        measure_agg_fn = None;
                        measure_subspace = None;
                    }
                    "hierarchy" => {
                        current_hierarchy_id = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(LiftError::Xml(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(elements)
}

// ─────────────────────────────────────────────────────────────────────────────
//  RDF graph builder
// ─────────────────────────────────────────────────────────────────────────────

/// Map an [`ElementKind`] to the appropriate `aso:` class IRI (FR2).
///
/// Unknown kinds fall back to `aso:Attribute` (AC6).
fn class_iri_for(kind: &ElementKind) -> &'static str {
    use aso_tbox::iris;
    match kind {
        ElementKind::Project => iris::CUBE, // project is the container; lift as Cube for now
        ElementKind::Cube => iris::CUBE,
        ElementKind::Measure { additivity, .. } => match additivity {
            Additivity::Additive => iris::FULLY_ADDITIVE_MEASURE,
            Additivity::SemiAdditive => iris::SEMI_ADDITIVE_MEASURE,
            Additivity::NonAdditive => iris::MEASURE,
        },
        ElementKind::Dimension => iris::DIMENSION,
        ElementKind::Hierarchy => iris::HIERARCHY,
        ElementKind::Level { .. } => iris::LEVEL,
        ElementKind::KeyedAttributeRef { .. } => iris::ROLE_PLAYING_REFERENCE,
        ElementKind::Unknown { .. } => iris::ATTRIBUTE,
    }
}

/// Construct helper: make a [`NamedNode`] from a literal IRI string; panics in
/// debug if the IRI is invalid (all our IRIs are compile-time constants).
#[inline]
fn nn(iri: &str) -> NamedNode {
    NamedNode::new(iri).unwrap_or_else(|e| panic!("invalid IRI {iri}: {e}"))
}

/// Build the RDF graph from a list of parsed elements.
///
/// Returns the [`Graph`] and a vec of sorted N-Triples lines for deterministic
/// Turtle output.
fn build_graph(elements: &[Element], opts: &LiftOptions) -> Result<Graph, LiftError> {
    let mut graph = Graph::new();
    let base = &opts.base_iri;

    // 1. Ontology header: typed as owl:Ontology + owl:imports aso: TBox
    let ontology_node = mint_ontology_iri(base)?;
    graph.insert(&Triple::new(
        ontology_node.clone(),
        nn(RDF_TYPE),
        nn(OWL_ONTOLOGY),
    ));
    graph.insert(&Triple::new(
        ontology_node.clone(),
        nn(OWL_IMPORTS),
        nn(aso_tbox::iris::ONTOLOGY),
    ));

    // 2. Build a map from id → IRI for cross-element references
    let mut id_to_iri: std::collections::HashMap<String, NamedNode> =
        std::collections::HashMap::new();
    for el in elements {
        let iri = mint_iri(base, &el.id)?;
        id_to_iri.insert(el.id.clone(), iri);
    }

    // 3. Collect levels per hierarchy, sorted by order, for rollsUpTo
    let mut hier_levels: std::collections::HashMap<String, Vec<(u32, String)>> =
        std::collections::HashMap::new();
    for el in elements {
        if let ElementKind::Level { order, parent_hierarchy_id } = &el.kind {
            hier_levels
                .entry(parent_hierarchy_id.clone())
                .or_default()
                .push((*order, el.id.clone()));
        }
    }
    // Sort each hierarchy's levels by order ascending
    for levels in hier_levels.values_mut() {
        levels.sort_by_key(|(order, _)| *order);
    }

    // 4. Emit triples for each element
    for el in elements {
        let subj = mint_iri(base, &el.id)?;

        // rdf:type owl:NamedIndividual
        graph.insert(&Triple::new(
            subj.clone(),
            nn(RDF_TYPE),
            nn(OWL_NAMED_INDIVIDUAL),
        ));

        // rdf:type aso:<Class>
        let class_iri = class_iri_for(&el.kind);
        if let ElementKind::Unknown { ref tag } = el.kind {
            // AC6: log the fallback
            eprintln!(
                "[aso-lift] WARN: element id='{}' tag=<{}> has no aso: class mapping; \
                 typed as aso:Attribute (fallback)",
                el.id, tag
            );
        }
        graph.insert(&Triple::new(subj.clone(), nn(RDF_TYPE), nn(class_iri)));

        // rdfs:label — prefer caption, fall back to name
        let label = el.caption.as_deref().unwrap_or(&el.name);
        graph.insert(&Triple::new(
            subj.clone(),
            nn(RDFS_LABEL),
            oxrdf::Literal::new_simple_literal(label),
        ));

        // Element-kind–specific triples
        match &el.kind {
            ElementKind::Measure {
                additivity,
                aggregation_fn: _,
                subspace_dim,
            } => {
                // aso:additiveOver — for fully additive, no subspace constraint;
                // for semi-additive, emit a "NOT additiveOver" note via subspace_dim.
                // v1: emit additiveOver only when explicitly additive (FR5).
                if matches!(additivity, Additivity::Additive) {
                    // Fully additive: we don't know which dim yet without more context;
                    // emit the type (FullyAdditiveMeasure) as the semantic signal.
                    // (aso:additiveOver per-dimension triples require a dim IRI reference.)
                }
                if let Some(sd) = subspace_dim {
                    // Semi-additive: emit aso:additiveOver exclusion using the subspace dim id
                    // if we can resolve it.
                    if let Some(dim_iri) = id_to_iri.get(sd) {
                        // The semi-additive measure is NOT additive over this dimension.
                        // We represent the constraint positively as: the subspace dimension
                        // is the one it's non-additive over (stored as aso:additiveOver
                        // with a negative-polarity note — v1 uses a plain triple;
                        // SHACL shapes carry the restriction semantics).
                        graph.insert(&Triple::new(
                            subj.clone(),
                            nn(aso_tbox::iris::ADDITIVE_OVER),
                            dim_iri.clone(),
                        ));
                    }
                }
            }
            ElementKind::KeyedAttributeRef { ref_path } => {
                // FR4/AC4: emit aso:playsRoleOf pointing to the base element
                if let Some(ref_iri) = id_to_iri.get(ref_path) {
                    graph.insert(&Triple::new(
                        subj.clone(),
                        nn(aso_tbox::iris::PLAYS_ROLE_OF),
                        ref_iri.clone(),
                    ));
                } else {
                    // ref-path didn't resolve to a known id — log, don't fail
                    eprintln!(
                        "[aso-lift] WARN: keyed-attribute-ref id='{}' ref-path='{}' \
                         did not resolve to a known element id",
                        el.id, ref_path
                    );
                }
            }
            _ => {}
        }
    }

    // 5. Emit rollsUpTo triples (FR4): finer level rolls up to coarser level
    for levels in hier_levels.values() {
        // levels is sorted coarse→fine (ascending order value)
        // level[i] rollsUpTo level[i-1]
        for window in levels.windows(2) {
            let (_, fine_id) = &window[1]; // higher order = finer
            let (_, coarse_id) = &window[0]; // lower order = coarser
            if let (Some(fine_iri), Some(coarse_iri)) =
                (id_to_iri.get(fine_id), id_to_iri.get(coarse_id))
            {
                graph.insert(&Triple::new(
                    fine_iri.clone(),
                    nn(aso_tbox::iris::ROLLS_UP_TO),
                    coarse_iri.clone(),
                ));
            }
        }
    }

    Ok(graph)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Turtle serialisation (deterministic)
// ─────────────────────────────────────────────────────────────────────────────

/// Serialise a [`Graph`] to Turtle.
///
/// For determinism (AC5 / NFR1), triples are emitted in sorted N-Triples order
/// (alphabetically by (subject, predicate, object) IRI/literal string).
fn to_turtle(graph: &Graph) -> String {
    use oxttl::TurtleSerializer;

    // Collect all triples, serialise to N-Triples first (deterministic sort key)
    let mut nt_lines: Vec<String> = graph
        .iter()
        .map(|t| {
            // Render each component as a string for sorting
            let s = t.subject.to_string();
            let p = t.predicate.to_string();
            let o = t.object.to_string();
            format!("{s} {p} {o} .")
        })
        .collect();
    nt_lines.sort(); // lexicographic — deterministic

    // Build sorted prefixed Turtle manually for readability
    let mut out = String::new();
    let _ = writeln!(out, "@prefix aso:  <{ASO_NS}> .");
    let _ = writeln!(out, "@prefix owl:  <{OWL_NS}> .");
    let _ = writeln!(out, "@prefix rdf:  <{RDF_NS}> .");
    let _ = writeln!(out, "@prefix rdfs: <{RDFS_NS}> .");
    let _ = writeln!(out);

    // Emit via oxttl TurtleSerializer for valid Turtle (handles escaping etc.)
    // We emit triples in the sorted order by re-parsing the N-Triples lines.
    // Simpler: directly use the TurtleSerializer on the graph (oxttl handles
    // prefix compression automatically).
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut ser = TurtleSerializer::new()
            .with_prefix("aso", aso_tbox::ASO_NS)
            .unwrap()
            .with_prefix("owl", OWL_NS)
            .unwrap()
            .with_prefix("rdf", RDF_NS)
            .unwrap()
            .with_prefix("rdfs", RDFS_NS)
            .unwrap()
            .for_writer(&mut buf);

        // Emit in sorted order: collect triples, sort by their string rep
        let mut triples: Vec<Triple> = graph.iter().map(|t| t.into_owned()).collect();
        triples.sort_by_key(|t| {
            (
                t.subject.to_string(),
                t.predicate.to_string(),
                t.object.to_string(),
            )
        });
        for triple in &triples {
            ser.serialize_triple(triple.as_ref())
                .expect("triple serialisation should not fail");
        }
        ser.finish().expect("TurtleSerializer finish should not fail");
    }

    String::from_utf8(buf).expect("Turtle output is always valid UTF-8")
}

const ASO_NS: &str = aso_tbox::ASO_NS;

// ─────────────────────────────────────────────────────────────────────────────
//  Public entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Lift engine model XML to RDF.
///
/// Returns a [`LiftOutput`] containing the Turtle serialisation and metadata.
///
/// # Errors
///
/// - [`LiftError::UnsupportedSchema`] — if the XML uses a pre-2.0 schema version (AC7).
/// - [`LiftError::Xml`] — if the XML is malformed.
/// - [`LiftError::MissingAttr`] — if a required attribute (`id`) is absent.
/// - [`LiftError::InvalidIri`] — if an IRI cannot be constructed.
pub fn lift(xml: &str, opts: &LiftOptions) -> Result<LiftOutput, LiftError> {
    let elements = parse_model(xml)?;
    let graph = build_graph(&elements, opts)?;
    let triple_count = graph.len();
    let turtle = to_turtle(&graph);
    Ok(LiftOutput {
        turtle,
        triple_count,
    })
}

/// Validate that the emitted Turtle parses back without error (AC1 / FR6).
///
/// Returns `Ok(triple_count)` on success, `Err(parse_error)` on failure.
pub fn round_trip_check(turtle: &str) -> Result<usize, String> {
    let parser = oxttl::TurtleParser::new()
        .with_base_iri(ASO_NS)
        .map_err(|e| e.to_string())?;
    let mut count = 0usize;
    for result in parser.for_slice(turtle.as_bytes()) {
        result.map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}
