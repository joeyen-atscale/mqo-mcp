//! # grounding — `describe_grounding` MCP tool implementation
//!
//! Exposes the formal BFO grounding produced by `aso-ground` to the agent as a
//! targeted, token-budgeted lookup surface.  Given a list of entity names or
//! IRIs the tool returns, per entity:
//!
//! - `aso_class` — the `aso:` type IRI (e.g. `aso:FullyAdditiveMeasure`)
//! - `bfo_category` — BFO 2020 category label + IRI
//! - `grounding_source` — how the category was determined
//!   (`kind-determined` | `hint-overridden` | `fallback`)
//! - `aristotelian_definition` — a short canonical definition derived from the
//!   BFO category (v1: generated from kind, not yet from a glossary file)
//! - `skos_labels` — stub object (v1: SKOS glossary is OSL #6, not yet landed)
//!
//! ## AC coverage
//!
//! | AC | Description | Where covered |
//! |----|-------------|---------------|
//! | AC1 | Known entity → aso_class + BFO category returned | `GroundingStore::lookup` + tests |
//! | AC2 | Unknown entity → actionable error | `GroundingStore::lookup` not-found path |
//! | AC3 | No grounding store → `grounding_not_available` | `Server::describe_grounding` None arm |
//! | AC4 | Invalid IRI → error | `GroundingStore::lookup` IRI parse error path |
//! | Tool count | 25 → 26 | `describe_grounding_input_schema` + `core_tool_descriptors` |

use aso_ground::{ground, BfoCategory, GroundedElement, GroundingMethod};
use serde_json::{json, Value};

// ─────────────────────────────────────────────────────────────────────────────
//  GroundingStore
// ─────────────────────────────────────────────────────────────────────────────

/// In-process grounding store backed by `aso-ground`.
///
/// When `elements` is empty (`None`), the tool returns
/// `grounding_not_available` — the expected state until the `aso-ground`
/// overlay (OSL #3) has been applied to a model.
pub struct GroundingStore {
    /// Grounded elements, keyed by IRI for O(log n) lookup.
    ///
    /// `None` = no grounding artifacts loaded.
    elements: Option<Vec<GroundedElement>>,
    /// Optional label → IRI index for name-based lookup.
    label_index: Vec<(String, usize)>, // (normalized_label, index into elements)
}

impl GroundingStore {
    /// Create an empty store (no grounding loaded → FR3 "not available").
    #[must_use]
    pub fn new() -> Self {
        Self {
            elements: None,
            label_index: Vec::new(),
        }
    }

    /// Load grounding from a Turtle string.
    ///
    /// Calls `aso_ground::ground()` on the Turtle, stores the results,
    /// and builds a label index for name-based lookup.
    ///
    /// Returns the count of grounded elements on success, or an error string.
    pub fn load_turtle(&mut self, turtle: &str) -> Result<usize, String> {
        let grounded = ground(turtle).map_err(|e| format!("grounding error: {e}"))?;
        let count = grounded.len();

        // Build label index from rdfs:label.
        // Since aso-ground only gives us IRI/aso_class/bfo_category/method,
        // we extract labels from the element IRIs' local names as a fallback.
        let mut label_idx: Vec<(String, usize)> = Vec::with_capacity(count);
        for (i, el) in grounded.iter().enumerate() {
            let local = local_name_from_iri(&el.iri);
            label_idx.push((normalize_label(&local), i));
        }
        label_idx.sort_by(|a, b| a.0.cmp(&b.0));

        self.label_index = label_idx;
        self.elements = Some(grounded);
        Ok(count)
    }

    /// Load grounding from an already-grounded element slice.
    ///
    /// Used by tests that build elements directly without Turtle round-tripping.
    #[cfg(test)]
    pub fn load_elements(&mut self, elements: Vec<GroundedElement>) {
        let count = elements.len();
        let mut label_idx: Vec<(String, usize)> = Vec::with_capacity(count);
        for (i, el) in elements.iter().enumerate() {
            let local = local_name_from_iri(&el.iri);
            label_idx.push((normalize_label(&local), i));
        }
        label_idx.sort_by(|a, b| a.0.cmp(&b.0));
        self.label_index = label_idx;
        self.elements = Some(elements);
    }

    /// Describe a set of entities by name or IRI.
    ///
    /// Accepts a JSON value:
    /// ```json
    /// {
    ///   "entities": ["Store Sales Increase", "https://models.atscale.com#measure-revenue"],
    ///   "max_entities": 50
    /// }
    /// ```
    ///
    /// Returns:
    /// - Success: `{"results": [...], "total_requested": N, "total_returned": M}`
    /// - No grounding: `{"status": "grounding_not_available", "detail": "..."}`
    #[must_use]
    pub fn lookup(&self, args: &Value) -> Value {
        let elements = match &self.elements {
            Some(e) => e,
            None => {
                return json!({
                    "status": "grounding_not_available",
                    "detail": "No grounding artifacts are loaded for this model. \
                               Grounding is produced by the aso-ground overlay (OSL #3). \
                               Load a grounded Turtle fixture or enable the auto-lift + overlay tier."
                });
            }
        };

        // Parse entity references
        let entity_refs: Vec<&str> = match args.get("entities").and_then(Value::as_array) {
            Some(arr) => arr.iter().filter_map(Value::as_str).collect(),
            None => {
                // Empty entity list → usage hint, not an error (AC-edge)
                return json!({
                    "status": "ok",
                    "results": [],
                    "total_requested": 0,
                    "total_returned": 0,
                    "usage_hint": "Pass an 'entities' array of entity names or IRIs to look up grounding. \
                                   Example: {\"entities\": [\"Store Sales Increase\", \"Sales Amount\"]}"
                });
            }
        };

        if entity_refs.is_empty() {
            return json!({
                "status": "ok",
                "results": [],
                "total_requested": 0,
                "total_returned": 0,
                "usage_hint": "Empty 'entities' list. Pass entity names or IRIs to look up their formal grounding."
            });
        }

        let max_entities = args
            .get("max_entities")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(50)
            .max(1)
            .min(200);

        let total_requested = entity_refs.len();
        let to_serve = entity_refs.iter().take(max_entities);

        let mut results: Vec<Value> = Vec::new();
        let mut dropped: Vec<&str> = Vec::new();

        // Entities beyond max_entities are dropped (token-budget, FR2).
        if total_requested > max_entities {
            dropped.extend(entity_refs[max_entities..].iter().copied());
        }

        for entity_ref in to_serve {
            let entry = self.resolve_entity(entity_ref, elements);
            results.push(entry);
        }

        let mut resp = json!({
            "status": "ok",
            "results": results,
            "total_requested": total_requested,
            "total_returned": results.len()
        });

        if !dropped.is_empty() {
            resp["truncated"] = json!(true);
            resp["dropped_entities"] = json!(dropped);
            resp["truncation_note"] = json!(format!(
                "Response truncated at {} entities (max_entities budget). \
                 {} entities were not returned: {:?}. \
                 Narrow the request to retrieve them.",
                max_entities,
                dropped.len(),
                dropped
            ));
        }

        resp
    }

    /// Resolve a single entity reference (IRI or name) against the grounded elements.
    fn resolve_entity<'a>(&self, entity_ref: &str, elements: &'a [GroundedElement]) -> Value {
        // Validate that if it looks like an IRI it is parseable.
        if entity_ref.starts_with("http://") || entity_ref.starts_with("https://") {
            // IRI-based lookup: find by exact IRI match.
            if let Some(idx) = elements.iter().position(|e| e.iri == entity_ref) {
                return grounded_element_to_json(&elements[idx]);
            }
            // Validate the IRI is at least structurally sound.
            if entity_ref.contains(' ') {
                return json!({
                    "entity": entity_ref,
                    "status": "error",
                    "detail": format!(
                        "Invalid IRI '{}': IRIs must not contain spaces. \
                         Did you mean to pass an entity name instead?",
                        entity_ref
                    )
                });
            }
            return json!({
                "entity": entity_ref,
                "status": "ungrounded",
                "detail": format!(
                    "No grounding found for IRI <{}>. \
                     The entity is not present in the loaded grounding artifacts. \
                     Check the IRI or use a name-based lookup.",
                    entity_ref
                )
            });
        }

        // Name-based lookup: normalize and search label index.
        let normalized = normalize_label(entity_ref);
        if let Some(&(_, idx)) = self
            .label_index
            .iter()
            .find(|(label, _)| label == &normalized)
        {
            return grounded_element_to_json(&elements[idx]);
        }

        // Substring fallback: find elements whose local name contains the normalized query.
        let candidates: Vec<usize> = self
            .label_index
            .iter()
            .filter(|(label, _)| label.contains(normalized.as_str()))
            .map(|(_, idx)| *idx)
            .collect();

        if candidates.len() == 1 {
            return grounded_element_to_json(&elements[candidates[0]]);
        }

        if candidates.len() > 1 {
            let suggestions: Vec<&str> = candidates
                .iter()
                .take(5)
                .map(|&i| elements[i].iri.as_str())
                .collect();
            return json!({
                "entity": entity_ref,
                "status": "ambiguous",
                "detail": format!(
                    "Name '{}' matches {} grounded entities. Narrow the query or use an IRI. \
                     Candidates (up to 5): {:?}",
                    entity_ref, candidates.len(), suggestions
                ),
                "candidate_iris": suggestions
            });
        }

        json!({
            "entity": entity_ref,
            "status": "ungrounded",
            "detail": format!(
                "No grounding found for '{}'. \
                 The entity does not appear in the loaded grounding artifacts. \
                 Use describe_model to list available entity names, then retry.",
                entity_ref
            )
        })
    }
}

impl Default for GroundingStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Conversion helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Convert a [`GroundedElement`] into the JSON response shape for one entity.
fn grounded_element_to_json(el: &GroundedElement) -> Value {
    let bfo_label = bfo_label(&el.bfo_category);
    let grounding_source = grounding_source_label(&el.method);
    let definition = aristotelian_definition(&el.aso_class, &el.bfo_category);

    json!({
        "entity": el.iri,
        "status": "grounded",
        "aso_class": el.aso_class,
        "aso_class_local": local_name_from_iri(&el.aso_class),
        "bfo_category": {
            "iri": el.bfo_category.iri(),
            "label": bfo_label
        },
        "grounding_source": grounding_source,
        "aristotelian_definition": definition,
        "skos_labels": {
            "note": "SKOS labels are produced by the OSL #6 glossary component (not yet deployed). \
                     When available, prefLabel/altLabel/exactMatch will appear here."
        }
    })
}

/// BFO 2020 human-readable label for the category.
fn bfo_label(cat: &BfoCategory) -> &'static str {
    match cat {
        BfoCategory::GenericallyDependentContinuant => "Generically Dependent Continuant",
        BfoCategory::Quality => "Quality",
        BfoCategory::Role => "Role",
        BfoCategory::TemporalRegion => "Temporal Region",
        BfoCategory::IndependentContinuant => "Independent Continuant",
    }
}

/// Human-readable grounding source label.
fn grounding_source_label(method: &GroundingMethod) -> &'static str {
    match method {
        GroundingMethod::Kind => "kind-determined",
        GroundingMethod::Hint => "hint-overridden",
        GroundingMethod::Fallback => "fallback",
    }
}

/// Aristotelian (genus + differentia) definition derived from BFO category and
/// `aso:` class kind.  v1 uses canned definitions per class; v2 will draw from
/// the OSL #6 SKOS glossary.
fn aristotelian_definition(aso_class: &str, bfo_category: &BfoCategory) -> String {
    let local = local_name_from_iri(aso_class);
    match local.as_str() {
        "FullyAdditiveMeasure" => {
            "A generically dependent continuant (information entity) that represents a numeric \
             quantity which can be summed accurately across all dimensional grain combinations \
             (fully additive). It bears information about business performance and is measured \
             at the fact grain."
                .to_string()
        }
        "SemiAdditiveMeasure" => {
            "A generically dependent continuant (information entity) that represents a numeric \
             quantity whose sum is meaningful only along some dimensions (semi-additive). \
             Typically additivity is restricted along the time dimension (e.g. balance measures)."
                .to_string()
        }
        "Measure" => {
            "A generically dependent continuant (information entity) that represents a numeric \
             quantity aggregated from fact data. Its additivity constraints are unspecified; \
             consult describe_model for aggregation metadata."
                .to_string()
        }
        "CalculatedMember" | "CalculationGroup" => {
            "A generically dependent continuant (information entity) that represents a derived \
             numeric value computed from other measures via a formula (e.g. MDX or DAX expression). \
             It does not aggregate raw fact rows directly."
                .to_string()
        }
        "Key" => {
            "A quality that inheres in a model entity and serves as a unique identity attribute. \
             It uniquely identifies members of a dimension level."
                .to_string()
        }
        "Level" => {
            "A role played by a dimension member set at a particular grain in an analytic \
             hierarchy. It participates in rollup relationships (aso:rollsUpTo) from finer \
             to coarser grain."
                .to_string()
        }
        "Hierarchy" => {
            "A role that organizes dimension levels into an ordered rollup structure (from finest \
             grain to coarsest). It partitions the analytical space along one axis of aggregation."
                .to_string()
        }
        "Dimension" => {
            "A role played by a set of descriptive attributes and levels that define one axis \
             of an analytic model. Dimensions impose structure on how measures are sliced and \
             aggregated."
                .to_string()
        }
        "RolePlayingReference" => {
            "A role played by a dimension that participates in an analytic model under a different \
             name/context than its base dimension (aso:playsRoleOf). Typical example: the Date \
             Dimension playing the role of 'Ship Date' or 'Return Date'."
                .to_string()
        }
        "Cube" | "DataSet" | "Perspective" => {
            "A generically dependent continuant (information entity) that represents a named \
             analytic subject area — a virtual multidimensional dataset defined by measures \
             and dimensions. It is the primary queryable unit in the semantic layer."
                .to_string()
        }
        "Attribute" => {
            "A generically dependent continuant (descriptive property) that describes a \
             dimension member. Attributes are projectable but not aggregatable as measures."
                .to_string()
        }
        _ => {
            format!(
                "A {} (BFO: {}) whose aso: class '{}' is not in the v1 definition vocabulary. \
                 Consult the AtScale ontology documentation for a precise definition.",
                bfo_label(bfo_category),
                bfo_category.iri(),
                local
            )
        }
    }
}

/// Extract the local name from an IRI (everything after the last `/` or `#`).
fn local_name_from_iri(iri: &str) -> String {
    iri.rsplit_once('/')
        .map(|(_, l)| l)
        .or_else(|| iri.rsplit_once('#').map(|(_, l)| l))
        .unwrap_or(iri)
        .to_string()
}

/// Normalize a label for lookup: lowercase + collapse whitespace.
fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tool descriptor helper (called from mcp.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// JSON schema for the `describe_grounding` tool's input.
#[must_use]
pub fn describe_grounding_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "entities": {
                "type": "array",
                "items": { "type": "string" },
                "description": "List of entity names or IRIs to ground. \
                                Names are matched case-insensitively against model element labels. \
                                IRIs are matched exactly. Example: [\"Store Sales Increase\", \"Sales Amount\"]."
            },
            "max_entities": {
                "type": "integer",
                "minimum": 1,
                "maximum": 200,
                "description": "Maximum number of entities to return (token budget). \
                                Default 50. Entities beyond the cap are listed in 'dropped_entities'. \
                                Reduce this value for context-budget-sensitive sessions."
            }
        },
        "additionalProperties": false
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aso_ground::{BfoCategory, GroundedElement, GroundingMethod};

    // ── Fixture helper ────────────────────────────────────────────────────────

    fn fixture_store() -> GroundingStore {
        let mut store = GroundingStore::new();
        store.load_elements(vec![
            GroundedElement {
                iri: "https://models.atscale.com#measure-store-sales-increase".to_string(),
                aso_class: "https://ontology.atscale.com/aso/FullyAdditiveMeasure".to_string(),
                bfo_category: BfoCategory::GenericallyDependentContinuant,
                method: GroundingMethod::Kind,
            },
            GroundedElement {
                iri: "https://models.atscale.com#measure-sales-amount".to_string(),
                aso_class: "https://ontology.atscale.com/aso/Measure".to_string(),
                bfo_category: BfoCategory::GenericallyDependentContinuant,
                method: GroundingMethod::Kind,
            },
            GroundedElement {
                iri: "https://models.atscale.com#dim-store".to_string(),
                aso_class: "https://ontology.atscale.com/aso/Dimension".to_string(),
                bfo_category: BfoCategory::Role,
                method: GroundingMethod::Kind,
            },
        ]);
        store
    }

    // ── AC1: known entity → aso_class + BFO category returned ────────────────

    #[test]
    fn ac1_known_entity_returns_aso_and_bfo() {
        let store = fixture_store();
        let result = store.lookup(&serde_json::json!({
            "entities": ["https://models.atscale.com#measure-store-sales-increase"]
        }));

        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("ok"),
            "status must be ok: {result}"
        );

        let results = result
            .get("results")
            .and_then(Value::as_array)
            .expect("results must be an array");
        assert_eq!(results.len(), 1, "one entity requested → one result");

        let r = &results[0];
        assert_eq!(
            r.get("status").and_then(Value::as_str),
            Some("grounded"),
            "entity must be grounded: {r}"
        );
        assert!(
            r.get("aso_class").and_then(Value::as_str).is_some(),
            "aso_class must be present"
        );
        assert!(
            r.get("bfo_category").is_some(),
            "bfo_category must be present"
        );
        assert_eq!(
            r.get("bfo_category")
                .and_then(|b| b.get("label"))
                .and_then(Value::as_str),
            Some("Generically Dependent Continuant"),
            "measure must be GDC"
        );
        assert_eq!(
            r.get("grounding_source").and_then(Value::as_str),
            Some("kind-determined"),
            "grounding_source must be kind-determined"
        );
        assert!(
            r.get("aristotelian_definition").and_then(Value::as_str).is_some(),
            "aristotelian_definition must be present"
        );
    }

    // ── AC2: unknown entity → actionable error ────────────────────────────────

    #[test]
    fn ac2_unknown_entity_returns_actionable_error() {
        let store = fixture_store();
        let result = store.lookup(&serde_json::json!({
            "entities": ["https://models.atscale.com#entity-does-not-exist"]
        }));

        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("ok"),
            "outer status must be ok even when entity is unknown: {result}"
        );

        let results = result
            .get("results")
            .and_then(Value::as_array)
            .expect("results must be an array");
        let r = &results[0];
        assert_eq!(
            r.get("status").and_then(Value::as_str),
            Some("ungrounded"),
            "unknown entity must have status 'ungrounded': {r}"
        );
        let detail = r.get("detail").and_then(Value::as_str).unwrap_or("");
        assert!(
            !detail.is_empty(),
            "unknown entity must include actionable detail: {r}"
        );
        // Must not be a fabricated category (AC2: no aso_class/bfo_category on an unknown)
        assert!(
            r.get("aso_class").is_none(),
            "unknown entity must not have a fabricated aso_class"
        );
    }

    // ── AC3: no grounding store → "not available" ─────────────────────────────

    #[test]
    fn ac3_no_grounding_store_returns_not_available() {
        let store = GroundingStore::new(); // empty, no elements loaded
        let result = store.lookup(&serde_json::json!({
            "entities": ["Sales Amount"]
        }));

        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("grounding_not_available"),
            "empty store must return grounding_not_available: {result}"
        );
        let detail = result.get("detail").and_then(Value::as_str).unwrap_or("");
        assert!(
            !detail.is_empty(),
            "grounding_not_available must include detail text"
        );
    }

    // ── AC4: invalid IRI → error ──────────────────────────────────────────────

    #[test]
    fn ac4_invalid_iri_returns_error() {
        let store = fixture_store();
        // An IRI with spaces is invalid
        let result = store.lookup(&serde_json::json!({
            "entities": ["https://models.atscale.com/invalid IRI with spaces"]
        }));

        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("ok"),
            "outer status must be ok: {result}"
        );
        let results = result
            .get("results")
            .and_then(Value::as_array)
            .expect("results must be an array");
        let r = &results[0];
        assert_eq!(
            r.get("status").and_then(Value::as_str),
            Some("error"),
            "invalid IRI must have status 'error': {r}"
        );
    }

    // ── Empty entity list → usage hint, not an error ──────────────────────────

    #[test]
    fn empty_entity_list_returns_usage_hint() {
        let store = fixture_store();
        let result = store.lookup(&serde_json::json!({"entities": []}));

        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("ok"),
            "empty list must return ok status: {result}"
        );
        assert_eq!(
            result.get("total_returned").and_then(Value::as_u64),
            Some(0),
            "total_returned must be 0"
        );
        assert!(
            result.get("usage_hint").is_some(),
            "empty list must include usage_hint"
        );
    }

    // ── Token-budget truncation ───────────────────────────────────────────────

    #[test]
    fn max_entities_truncates_result() {
        let store = fixture_store();
        // Request 3 entities but cap at 1
        let result = store.lookup(&serde_json::json!({
            "entities": [
                "https://models.atscale.com#measure-store-sales-increase",
                "https://models.atscale.com#measure-sales-amount",
                "https://models.atscale.com#dim-store"
            ],
            "max_entities": 1
        }));

        assert_eq!(
            result.get("total_requested").and_then(Value::as_u64),
            Some(3),
            "total_requested must be 3"
        );
        assert_eq!(
            result.get("total_returned").and_then(Value::as_u64),
            Some(1),
            "total_returned must be 1 (capped)"
        );
        assert_eq!(
            result.get("truncated").and_then(Value::as_bool),
            Some(true),
            "truncated must be true"
        );
        let dropped = result
            .get("dropped_entities")
            .and_then(Value::as_array)
            .expect("dropped_entities must be present");
        assert_eq!(dropped.len(), 2, "2 entities must be in dropped list");
    }

    // ── BFO IRI present in result ─────────────────────────────────────────────

    #[test]
    fn bfo_iri_present_in_result() {
        let store = fixture_store();
        let result = store.lookup(&serde_json::json!({
            "entities": ["https://models.atscale.com#measure-store-sales-increase"]
        }));
        let results = result
            .get("results")
            .and_then(Value::as_array)
            .expect("results array");
        let r = &results[0];
        let bfo_iri = r
            .get("bfo_category")
            .and_then(|b| b.get("iri"))
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(
            bfo_iri.contains("BFO_"),
            "bfo_category.iri must contain a BFO IRI: {bfo_iri}"
        );
    }

    // ── Dimension entity → Role ───────────────────────────────────────────────

    #[test]
    fn dimension_entity_returns_role_category() {
        let store = fixture_store();
        let result = store.lookup(&serde_json::json!({
            "entities": ["https://models.atscale.com#dim-store"]
        }));
        let results = result
            .get("results")
            .and_then(Value::as_array)
            .expect("results array");
        let r = &results[0];
        assert_eq!(
            r.get("bfo_category")
                .and_then(|b| b.get("label"))
                .and_then(Value::as_str),
            Some("Role"),
            "dimension must have BFO Role: {r}"
        );
        assert_eq!(
            r.get("grounding_source").and_then(Value::as_str),
            Some("kind-determined"),
        );
    }

    // ── Describe grounding input schema is well-formed ────────────────────────

    #[test]
    fn describe_grounding_input_schema_is_valid() {
        let schema = describe_grounding_input_schema();
        assert_eq!(
            schema.get("type").and_then(Value::as_str),
            Some("object"),
            "schema type must be 'object'"
        );
        assert!(
            schema.get("properties").is_some(),
            "schema must have properties"
        );
        assert!(
            schema
                .get("properties")
                .and_then(|p| p.get("entities"))
                .is_some(),
            "schema must have 'entities' property"
        );
    }
}
