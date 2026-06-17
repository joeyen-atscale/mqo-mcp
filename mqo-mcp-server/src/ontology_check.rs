//! # `ontology_check` — `validate_query_ontology` MCP tool
//!
//! Advisory-first pre-execution validation of an agent's proposed MQO against
//! the loaded `aso:` graph.  Returns a structured findings array so the agent
//! can self-correct in one retry rather than executing a semantically invalid
//! query.
//!
//! ## Design
//!
//! This is the **advisory** (warn-only) tier of the Ontology-Based Query Check
//! (OBQC) described in PRD-swa-ontology-query-check.  It reads the same lifted
//! graph that `query_model_graph` uses (`ModelGraphStore`) and performs
//! existence/type checks against the `aso:` schema.
//!
//! Three checks in v1:
//! 1. **`entity_existence`** — every measure/dimension label or `unique_name`
//!    referenced in the query must appear in the graph.
//! 2. **`type_mismatch`** — a named entity must be used in the role its `rdf:type`
//!    permits (measure vs dimension).
//! 3. **`semi_additive_sum_over_time`** — a measure typed `aso:SemiAdditiveMeasure`
//!    cannot be summed over a Time/Date dimension (advisory warning).
//!
//! ## Response shape
//!
//! ```json
//! {
//!   "conforms": true | false,
//!   "findings": [
//!     { "rule_id": "entity_existence", "severity": "error", "entity": "...", "message": "..." }
//!   ]
//! }
//! ```
//!
//! Empty `findings` = ontologically valid.  When no graph is loaded a single
//! `info` finding is returned and `conforms` is `true` (fail-open).

use oxrdf::{Graph, NamedNode, TermRef};
use serde_json::{json, Value};

// ── IRI constants (local copies matching aso_tbox::iris) ─────────────────────

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

const ASO_MEASURE: &str = "https://ontology.atscale.com/aso/Measure";
const ASO_FULLY_ADDITIVE_MEASURE: &str = "https://ontology.atscale.com/aso/FullyAdditiveMeasure";
const ASO_SEMI_ADDITIVE_MEASURE: &str = "https://ontology.atscale.com/aso/SemiAdditiveMeasure";
const ASO_CALCULATED_MEMBER: &str = "https://ontology.atscale.com/aso/CalculatedMember";

const ASO_HIERARCHY: &str = "https://ontology.atscale.com/aso/Hierarchy";
const ASO_LEVEL: &str = "https://ontology.atscale.com/aso/Level";
const ASO_DIMENSION: &str = "https://ontology.atscale.com/aso/Dimension";

// ─────────────────────────────────────────────────────────────────────────────
//  Public store type
// ─────────────────────────────────────────────────────────────────────────────

/// Store holding the lifted RDF graph used for ontology-based query checks.
///
/// Wraps the same `oxrdf::Graph` that `ModelGraphStore` uses; separated into
/// its own struct so the tool has a distinct field on `Server` and tests can
/// load a fixture independently.
///
/// When `graph` is `None`, the tool returns a single `info` finding:
/// `"ontology_graph_not_available"` and `conforms: true` (fail-open, FR7).
pub struct OntologyCheckStore {
    graph: Option<Graph>,
}

impl OntologyCheckStore {
    /// Create an empty store (no graph → fail-open advisory mode).
    #[must_use]
    pub fn new() -> Self {
        Self { graph: None }
    }

    /// Load a Turtle string into the store (same API as `ModelGraphStore`).
    ///
    /// Replaces any previously loaded graph.  Returns the triple count on
    /// success or an error string on parse failure.
    ///
    /// # Errors
    ///
    /// Returns an error string if the Turtle cannot be parsed.
    pub fn load_turtle(&mut self, turtle: &str) -> Result<usize, String> {
        use oxttl::TurtleParser;
        let parser = TurtleParser::new()
            .with_base_iri("https://models.atscale.com")
            .map_err(|e| format!("parser init: {e}"))?;
        let mut graph = Graph::new();
        for result in parser.for_slice(turtle.as_bytes()) {
            match result {
                Ok(triple) => {
                    graph.insert(&triple);
                }
                Err(e) => return Err(format!("parse error: {e}")),
            }
        }
        let count = graph.len();
        self.graph = Some(graph);
        Ok(count)
    }

    /// Run the ontology check against an agent-supplied query description.
    ///
    /// `args` shape:
    /// ```json
    /// {
    ///   "measures":    ["Revenue", "Profit Margin"],
    ///   "dimensions":  ["Brand", "Store State Name"],
    ///   "sql":         "SELECT ..."   // optional; checked for entity names if present
    /// }
    /// ```
    ///
    /// Returns:
    /// ```json
    /// { "conforms": bool, "findings": [...] }
    /// ```
    #[must_use]
    pub fn check(&self, args: &Value) -> Value {
        // Fail-open: no graph → single info finding, conforms=true.
        let Some(graph) = &self.graph else {
            return json!({
                "conforms": true,
                "findings": [{
                    "rule_id": "ontology_graph_not_available",
                    "severity": "info",
                    "entity": null,
                    "message": "No lifted ontology graph is available for this model. \
                                The auto-lift tier (OSL #2) has not been deployed. \
                                Ontology-based query validation is advisory-only; \
                                the query may proceed."
                }]
            });
        };

        // Index all known entities by label (case-insensitive) and type.
        let index = build_entity_index(graph);

        let mut findings: Vec<Value> = Vec::new();

        // Check measures
        if let Some(measures) = args.get("measures").and_then(Value::as_array) {
            for m in measures {
                let Some(name) = m.as_str().or_else(|| m.get("name").and_then(Value::as_str)) else { continue };
                check_entity(name, EntityRole::Measure, &index, &mut findings);
            }
        }

        // Check dimensions (may be strings or objects with a "hierarchy"/"level" key)
        if let Some(dims) = args.get("dimensions").and_then(Value::as_array) {
            for d in dims {
                let Some(name) = d.as_str()
                    .or_else(|| d.get("level").and_then(Value::as_str))
                    .or_else(|| d.get("hierarchy").and_then(Value::as_str)) else { continue };
                check_entity(name, EntityRole::Dimension, &index, &mut findings);
            }
        }

        // Check SQL string for semi-additive measures used with time dimensions
        // (heuristic: detect "SUM(" patterns over known semi-additive measures)
        if findings.is_empty() {
            // Only do the SQL check when no existence errors already surfaced
            if let Some(sql) = args.get("sql").and_then(Value::as_str) {
                check_semi_additive_sql(sql, &index, &mut findings);
            }
        }

        // Also check semi-additive measures referenced in the measures array
        // against time dimensions in the dimensions array
        check_semi_additive_cross(args, &index, &mut findings);

        let conforms = findings
            .iter()
            .all(|f| f.get("severity").and_then(Value::as_str) != Some("error"));

        json!({
            "conforms": conforms,
            "findings": findings
        })
    }
}

impl Default for OntologyCheckStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Entity index helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Classification of what role a graph entity plays.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EntityKind {
    Measure,
    SemiAdditiveMeasure,
    Dimension, // Hierarchy or Level or Dimension
}

struct EntityEntry {
    iri: String,
    kind: EntityKind,
}

/// A map of lowercase-label → entity entry for all known entities in the graph.
struct EntityIndex {
    by_label: std::collections::HashMap<String, EntityEntry>,
}

fn build_entity_index(graph: &Graph) -> EntityIndex {
    let mut by_label: std::collections::HashMap<String, EntityEntry> =
        std::collections::HashMap::new();

    let measure_classes = [
        (ASO_FULLY_ADDITIVE_MEASURE, EntityKind::Measure),
        (ASO_MEASURE, EntityKind::Measure),
        (ASO_CALCULATED_MEMBER, EntityKind::Measure),
        (ASO_SEMI_ADDITIVE_MEASURE, EntityKind::SemiAdditiveMeasure),
    ];
    let dim_classes = [
        (ASO_HIERARCHY, EntityKind::Dimension),
        (ASO_LEVEL, EntityKind::Dimension),
        (ASO_DIMENSION, EntityKind::Dimension),
    ];

    for (class_iri, kind) in measure_classes.iter().chain(dim_classes.iter()) {
        for iri in subjects_of_type(graph, class_iri) {
            let label = label_of(graph, &iri).unwrap_or_default();
            if !label.is_empty() {
                by_label.insert(
                    label.to_lowercase(),
                    EntityEntry {
                        iri: iri.clone(),
                        kind: kind.clone(),
                    },
                );
            }
            // Also index by the IRI's fragment (the local name)
            if let Some(frag) = iri.split('#').next_back() {
                by_label.entry(frag.to_lowercase()).or_insert(EntityEntry {
                    iri: iri.clone(),
                    kind: kind.clone(),
                });
            }
        }
    }

    EntityIndex { by_label }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Check helpers
// ─────────────────────────────────────────────────────────────────────────────

/// What role is the agent trying to use this entity as?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityRole {
    Measure,
    Dimension,
}

/// Check a single entity name against the index, appending findings as needed.
fn check_entity(name: &str, role: EntityRole, index: &EntityIndex, findings: &mut Vec<Value>) {
    let key = name.to_lowercase();
    match index.by_label.get(&key) {
        None => {
            findings.push(json!({
                "rule_id": "entity_existence",
                "severity": "error",
                "entity": name,
                "message": format!(
                    "Entity '{}' was not found in the ontology graph for this model. \
                     Check spelling or use describe_model to list available entities.",
                    name
                )
            }));
        }
        Some(entry) => {
            // Type mismatch check
            let type_ok = match &role {
                EntityRole::Measure => matches!(
                    entry.kind,
                    EntityKind::Measure | EntityKind::SemiAdditiveMeasure
                ),
                EntityRole::Dimension => entry.kind == EntityKind::Dimension,
            };
            if !type_ok {
                let expected = match &role {
                    EntityRole::Measure => "measure",
                    EntityRole::Dimension => "dimension/level/hierarchy",
                };
                let actual = match &entry.kind {
                    EntityKind::Measure | EntityKind::SemiAdditiveMeasure => "measure",
                    EntityKind::Dimension => "dimension/level/hierarchy",
                };
                findings.push(json!({
                    "rule_id": "type_mismatch",
                    "severity": "error",
                    "entity": name,
                    "message": format!(
                        "Entity '{}' (IRI: {}) is typed as {} in the ontology but is being \
                         used as a {}.  Use a {} instead.",
                        name, entry.iri, actual, expected, expected
                    )
                }));
            }
        }
    }
}

/// Detect semi-additive measure names in an SQL `SUM()` expression (heuristic).
fn check_semi_additive_sql(sql: &str, index: &EntityIndex, findings: &mut Vec<Value>) {
    let sql_lower = sql.to_lowercase();
    // Heuristic: look for SUM( ... ) — if a semi-additive measure label appears nearby
    // we flag it as a warning.
    if !sql_lower.contains("sum(") {
        return;
    }
    for (label_key, entry) in &index.by_label {
        if entry.kind == EntityKind::SemiAdditiveMeasure
            && sql_lower.contains(label_key.as_str())
        {
            findings.push(json!({
                "rule_id": "semi_additive_sum_over_time",
                "severity": "warning",
                "entity": label_key,
                "message": format!(
                    "Measure '{}' is typed aso:SemiAdditiveMeasure. \
                     Summing a semi-additive measure over time typically produces \
                     semantically incorrect results. \
                     Use the valid aggregation for this measure (e.g. LASTPERIOD, AVG) \
                     or consult describe_model for the allowed aggregation.",
                    label_key
                )
            }));
        }
    }
}

/// Cross-check: any semi-additive measure in the measures array + a time/date
/// dimension in the dimensions array → advisory warning.
fn check_semi_additive_cross(args: &Value, index: &EntityIndex, findings: &mut Vec<Value>) {
    let measures = args
        .get("measures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let dims = args
        .get("dimensions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Collect semi-additive measure names referenced in the query
    let semi_measures: Vec<String> = measures
        .iter()
        .filter_map(|m| m.as_str().or_else(|| m.get("name").and_then(Value::as_str)))
        .filter(|name| {
            let entry = index.by_label.get(&name.to_lowercase());
            entry.is_some_and(|e| e.kind == EntityKind::SemiAdditiveMeasure)
        })
        .map(str::to_string)
        .collect();

    if semi_measures.is_empty() {
        return;
    }

    // Detect time/date dimensions (heuristic: label contains "date", "time", "calendar", "month",
    // "year", "quarter", "week")
    let time_keywords = ["date", "time", "calendar", "month", "year", "quarter", "week", "period"];
    let has_time_dim = dims.iter().any(|d| {
        let name = d
            .as_str()
            .or_else(|| d.get("level").and_then(Value::as_str))
            .or_else(|| d.get("hierarchy").and_then(Value::as_str))
            .unwrap_or("")
            .to_lowercase();
        time_keywords.iter().any(|kw| name.contains(kw))
    });

    if !has_time_dim {
        return;
    }

    for measure_name in &semi_measures {
        // Avoid duplicate warnings if check_semi_additive_sql already fired
        let already_warned = findings.iter().any(|f| {
            f.get("rule_id").and_then(Value::as_str) == Some("semi_additive_sum_over_time")
                && f.get("entity").and_then(Value::as_str) == Some(measure_name.as_str())
        });
        if !already_warned {
            findings.push(json!({
                "rule_id": "semi_additive_sum_over_time",
                "severity": "warning",
                "entity": measure_name,
                "message": format!(
                    "Measure '{}' is typed aso:SemiAdditiveMeasure and is combined with a \
                     time/date dimension. Summing a semi-additive measure over time produces \
                     semantically incorrect results. Use the valid aggregation for this measure \
                     (e.g. LASTPERIOD, AVG) or consult describe_model.",
                    measure_name
                )
            }));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Graph helpers (local copies — same approach as model_graph.rs)
// ─────────────────────────────────────────────────────────────────────────────

fn label_of(graph: &Graph, iri: &str) -> Option<String> {
    let node = NamedNode::new(iri).ok()?;
    let pred = NamedNode::new(RDFS_LABEL).ok()?;
    graph
        .objects_for_subject_predicate(node.as_ref(), pred.as_ref())
        .next()
        .and_then(|term| {
            if let TermRef::Literal(lit) = term {
                Some(lit.value().to_string())
            } else {
                None
            }
        })
}

fn subjects_of_type(graph: &Graph, class_iri: &str) -> Vec<String> {
    let Ok(type_pred) = NamedNode::new(RDF_TYPE) else {
        return vec![];
    };
    let Ok(class_node) = NamedNode::new(class_iri) else {
        return vec![];
    };
    let class_term: TermRef<'_> = TermRef::NamedNode(class_node.as_ref());
    graph
        .subjects_for_predicate_object(type_pred.as_ref(), class_term)
        .filter_map(|s| {
            if let oxrdf::NamedOrBlankNodeRef::NamedNode(n) = s {
                Some(n.as_str().to_string())
            } else {
                None
            }
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
//  Input schema (called from mcp.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// JSON Schema for the `validate_query_ontology` tool's input.
#[must_use]
pub fn validate_query_ontology_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "measures": {
                "type": "array",
                "items": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "object", "properties": { "name": { "type": "string" } }, "additionalProperties": true }
                    ]
                },
                "description": "Measure names (labels or unique_names) referenced in the query."
            },
            "dimensions": {
                "type": "array",
                "items": {
                    "oneOf": [
                        { "type": "string" },
                        {
                            "type": "object",
                            "properties": {
                                "hierarchy": { "type": "string" },
                                "level": { "type": "string" }
                            },
                            "additionalProperties": true
                        }
                    ]
                },
                "description": "Dimension/level/hierarchy names referenced in the query."
            },
            "sql": {
                "type": "string",
                "description": "Optional SQL string to scan for semi-additive SUM() patterns."
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

    // ── Shared fixture: same synthetic graph as model_graph.rs tests ──────────
    //
    // One cube "Sales" with:
    //   - FullyAdditiveMeasure "Revenue"
    //   - SemiAdditiveMeasure "Inventory Level"
    //   - Hierarchy "Brand" with levels [Brand, SKU]
    //   - Level "Sold Calendar Month" (time dimension)

    fn fixture_ttl() -> &'static str {
        r#"
@prefix aso:  <https://ontology.atscale.com/aso/> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

<https://models.atscale.com#cube-sales>
    rdf:type owl:NamedIndividual, aso:Cube ;
    rdfs:label "Sales" .

<https://models.atscale.com#measure-revenue>
    rdf:type owl:NamedIndividual, aso:FullyAdditiveMeasure ;
    rdfs:label "Revenue" .

<https://models.atscale.com#measure-inventory>
    rdf:type owl:NamedIndividual, aso:SemiAdditiveMeasure ;
    rdfs:label "Inventory Level" .

<https://models.atscale.com#hier-brand>
    rdf:type owl:NamedIndividual, aso:Hierarchy ;
    rdfs:label "Brand" ;
    aso:hasLevel <https://models.atscale.com#level-brand> ;
    aso:hasLevel <https://models.atscale.com#level-sku> .

<https://models.atscale.com#level-brand>
    rdf:type owl:NamedIndividual, aso:Level ;
    rdfs:label "Brand" .

<https://models.atscale.com#level-sku>
    rdf:type owl:NamedIndividual, aso:Level ;
    rdfs:label "SKU" ;
    aso:rollsUpTo <https://models.atscale.com#level-brand> .

<https://models.atscale.com#level-sold-calendar-month>
    rdf:type owl:NamedIndividual, aso:Level ;
    rdfs:label "Sold Calendar Month" .
"#
    }

    fn fixture_store() -> OntologyCheckStore {
        let mut store = OntologyCheckStore::new();
        store
            .load_turtle(fixture_ttl())
            .expect("fixture TTL must parse");
        store
    }

    // ── AC1: valid query → empty findings ─────────────────────────────────────

    #[test]
    fn ac1_valid_query_returns_empty_findings() {
        let store = fixture_store();
        let result = store.check(&json!({
            "measures": ["Revenue"],
            "dimensions": ["Brand"]
        }));

        assert_eq!(
            result.get("conforms").and_then(Value::as_bool),
            Some(true),
            "valid query must conform: {result}"
        );

        let findings = result
            .get("findings")
            .and_then(Value::as_array)
            .expect("findings must be an array");
        assert!(findings.is_empty(), "valid query must have no findings: {findings:?}");
    }

    // ── AC2: unknown measure → error finding ──────────────────────────────────

    #[test]
    fn ac2_unknown_measure_returns_error_finding() {
        let store = fixture_store();
        let result = store.check(&json!({
            "measures": ["NonExistentMeasure"],
            "dimensions": []
        }));

        assert_eq!(
            result.get("conforms").and_then(Value::as_bool),
            Some(false),
            "unknown entity must not conform: {result}"
        );

        let findings = result
            .get("findings")
            .and_then(Value::as_array)
            .expect("findings must be an array");
        assert!(!findings.is_empty(), "must have at least one finding");

        let finding = &findings[0];
        assert_eq!(
            finding.get("rule_id").and_then(Value::as_str),
            Some("entity_existence"),
            "rule_id must be entity_existence: {finding}"
        );
        assert_eq!(
            finding.get("severity").and_then(Value::as_str),
            Some("error"),
            "severity must be error: {finding}"
        );
        assert_eq!(
            finding.get("entity").and_then(Value::as_str),
            Some("NonExistentMeasure"),
            "entity must name the offending measure: {finding}"
        );
    }

    // ── AC3: no graph → single info "not available" ───────────────────────────

    #[test]
    fn ac3_no_graph_returns_info_not_available() {
        let store = OntologyCheckStore::new(); // no graph loaded
        let result = store.check(&json!({
            "measures": ["Revenue"],
            "dimensions": ["Brand"]
        }));

        assert_eq!(
            result.get("conforms").and_then(Value::as_bool),
            Some(true),
            "no-graph must fail-open (conforms=true): {result}"
        );

        let findings = result
            .get("findings")
            .and_then(Value::as_array)
            .expect("findings must be an array");
        assert_eq!(findings.len(), 1, "must have exactly one finding: {findings:?}");

        let f = &findings[0];
        assert_eq!(
            f.get("severity").and_then(Value::as_str),
            Some("info"),
            "severity must be info: {f}"
        );
        assert_eq!(
            f.get("rule_id").and_then(Value::as_str),
            Some("ontology_graph_not_available"),
            "rule_id must be ontology_graph_not_available: {f}"
        );
    }

    // ── AC4: multiple issues → multiple findings ──────────────────────────────

    #[test]
    fn ac4_multiple_issues_produce_multiple_findings() {
        let store = fixture_store();
        let result = store.check(&json!({
            "measures": ["UnknownMeasureA", "UnknownMeasureB"],
            "dimensions": ["UnknownDimA"]
        }));

        assert_eq!(
            result.get("conforms").and_then(Value::as_bool),
            Some(false),
            "must not conform when multiple issues exist: {result}"
        );

        let findings = result
            .get("findings")
            .and_then(Value::as_array)
            .expect("findings must be an array");
        assert!(
            findings.len() >= 3,
            "must have at least 3 findings (two measures + one dimension): found {} — {findings:?}",
            findings.len()
        );

        // All must be entity_existence errors
        for f in findings {
            assert_eq!(
                f.get("rule_id").and_then(Value::as_str),
                Some("entity_existence"),
                "all findings must be entity_existence: {f}"
            );
            assert_eq!(
                f.get("severity").and_then(Value::as_str),
                Some("error"),
                "all findings must be error severity: {f}"
            );
        }
    }

    // ── Semi-additive + time dimension → warning ──────────────────────────────

    #[test]
    fn semi_additive_with_time_dim_produces_warning() {
        let store = fixture_store();
        let result = store.check(&json!({
            "measures": ["Inventory Level"],
            "dimensions": ["Sold Calendar Month"]
        }));

        let findings = result
            .get("findings")
            .and_then(Value::as_array)
            .expect("findings array");

        let has_semi_warning = findings.iter().any(|f| {
            f.get("rule_id").and_then(Value::as_str) == Some("semi_additive_sum_over_time")
                && f.get("severity").and_then(Value::as_str) == Some("warning")
        });
        assert!(
            has_semi_warning,
            "semi-additive measure + time dim must produce semi_additive_sum_over_time warning: {findings:?}"
        );
    }

    // ── Type mismatch: using a dimension as a measure ─────────────────────────

    #[test]
    fn type_mismatch_dimension_used_as_measure_produces_error() {
        let store = fixture_store();
        // "Brand" is a Hierarchy (dimension), not a measure
        let result = store.check(&json!({
            "measures": ["Brand"],
            "dimensions": []
        }));

        let findings = result
            .get("findings")
            .and_then(Value::as_array)
            .expect("findings array");

        let has_type_mismatch = findings.iter().any(|f| {
            f.get("rule_id").and_then(Value::as_str) == Some("type_mismatch")
                && f.get("severity").and_then(Value::as_str) == Some("error")
        });
        assert!(
            has_type_mismatch,
            "dimension used as measure must produce type_mismatch error: {findings:?}"
        );
    }
}
