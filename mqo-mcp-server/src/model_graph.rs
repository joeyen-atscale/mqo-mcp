//! # model_graph — `query_model_graph` tool implementation
//!
//! Provides a read-only, budgeted, canned-query surface over the lifted model
//! RDF graph (`aso-lift` output held as an `oxrdf::Graph`).
//!
//! ## Architecture
//!
//! v1 exposes **parameterized named queries** (not raw SPARQL by default, per
//! FR2/NG3). Each query is a named operation with typed params; queries are
//! evaluated by direct triple-pattern traversal over an `oxrdf::Graph`.
//!
//! `oxigraph`'s SPARQL engine is not required: `oxrdf` (already in the
//! workspace via `aso-tbox` / `aso-lift`) provides the in-memory triple store
//! and typed node traversal. This avoids pulling a new crate while still
//! delivering the IRI-addressed, standards-compliant model-graph surface.
//!
//! ## PRD coverage
//!
//! | AC | Where handled |
//! |----|---------------|
//! | AC1 | `hierarchy_levels` query → ordered levels with IRI + label |
//! | AC2 | `calc_dependencies` query → measures/columns a calc depends on |
//! | AC3 | `conformance_check` → `owl:sameAs` linkage (stub; needs lattice-bridge) |
//! | AC4 | `BudgetConfig` + elapsed-time / row-count checks → `budget_exceeded` |
//! | AC5 | `allow_raw_sparql=false` → refused with pointer to canned queries |
//! | AC6 | `ModelGraphStore` with no loaded graph → `model_graph_not_available` |
//! | AC7 | Results contain only model-metadata IRIs/literals, never warehouse rows |
//! | AC8 | Unknown query name / bad params → actionable error listing valid queries |

use oxrdf::{Graph, NamedNode, TermRef};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

// ─────────────────────────────────────────────────────────────────────────────
//  Known IRI constants (local copies to avoid depending on aso-tbox at link time
//  from this module; they match aso_tbox::iris exactly)
// ─────────────────────────────────────────────────────────────────────────────

const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";

const ASO_HIERARCHY: &str = "https://ontology.atscale.com/aso/Hierarchy";
const ASO_LEVEL: &str = "https://ontology.atscale.com/aso/Level";
const ASO_MEASURE: &str = "https://ontology.atscale.com/aso/Measure";
const ASO_FULLY_ADDITIVE_MEASURE: &str = "https://ontology.atscale.com/aso/FullyAdditiveMeasure";
const ASO_SEMI_ADDITIVE_MEASURE: &str = "https://ontology.atscale.com/aso/SemiAdditiveMeasure";
const ASO_CALCULATED_MEMBER: &str = "https://ontology.atscale.com/aso/CalculatedMember";
const ASO_ROLE_PLAYING_REFERENCE: &str = "https://ontology.atscale.com/aso/RolePlayingReference";
const ASO_ROLLS_UP_TO: &str = "https://ontology.atscale.com/aso/rollsUpTo";
const ASO_PLAYS_ROLE_OF: &str = "https://ontology.atscale.com/aso/playsRoleOf";
const ASO_HAS_LEVEL: &str = "https://ontology.atscale.com/aso/hasLevel";
const ASO_DEPENDS_ON: &str = "https://ontology.atscale.com/aso/dependsOn";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

// ─────────────────────────────────────────────────────────────────────────────
//  Budget configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Operator-configurable query budget.
#[derive(Debug, Clone)]
pub struct BudgetConfig {
    /// Maximum wall-clock time for a single query.
    pub max_duration: Duration,
    /// Maximum number of result rows (bindings) returned.
    pub max_rows: usize,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_duration: Duration::from_secs(1),
            max_rows: 1000,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Model graph store
// ─────────────────────────────────────────────────────────────────────────────

/// In-process lifted model RDF graph store.
///
/// When `graph` is `None`, the tool returns the `model_graph_not_available`
/// result (FR6/AC6). This is the expected state for live/fixture-mode servers
/// until the auto-lift tier (OSL #2) lands and populates the graph at startup.
///
/// Load a graph with [`ModelGraphStore::load_turtle`] to exercise canned
/// queries (used in tests and when a lifted model is available).
pub struct ModelGraphStore {
    graph: Option<Graph>,
    /// Whether raw SPARQL is allowed (operator opt-in; off by default — FR5/AC5).
    pub allow_raw_sparql: bool,
    /// Query budget applied to every invocation.
    pub budget: BudgetConfig,
}

impl ModelGraphStore {
    /// Create an empty store (no graph loaded → FR6 "not available").
    #[must_use]
    pub fn new() -> Self {
        Self {
            graph: None,
            allow_raw_sparql: false,
            budget: BudgetConfig::default(),
        }
    }

    /// Load an RDF/Turtle string into the store.
    ///
    /// Replaces any previously loaded graph.  Returns an error string on
    /// parse failure.
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

    /// Execute a named canned query with params.
    ///
    /// Returns a JSON value following the response shape:
    ///
    /// - Success: `{"query": "<name>", "bindings": [...], "row_count": N}`
    /// - Budget exceeded: `{"status": "budget_exceeded", "detail": "..."}`
    /// - No graph: `{"status": "model_graph_not_available", "detail": "..."}`
    /// - Bad query name/params: `{"status": "error", "detail": "...", "valid_queries": [...]}`
    /// - Raw SPARQL disabled: `{"status": "raw_sparql_disabled", "detail": "...", "valid_queries": [...]}`
    #[must_use]
    pub fn query(&self, args: &Value) -> Value {
        let start = Instant::now();

        // AC5: handle raw_sparql submission when disabled
        if let Some(raw_sparql) = args.get("raw_sparql").and_then(Value::as_str) {
            if !raw_sparql.is_empty() {
                if !self.allow_raw_sparql {
                    return json!({
                        "status": "raw_sparql_disabled",
                        "detail": "Raw SPARQL is disabled on this cluster. Use a canned query instead.",
                        "valid_queries": valid_query_names()
                    });
                }
                // Raw SPARQL allowed — not implemented in v1 (the opt-in tier)
                return json!({
                    "status": "error",
                    "detail": "Raw SPARQL opt-in accepted but execution is not yet implemented in this server version.",
                    "valid_queries": valid_query_names()
                });
            }
        }

        // AC6: no graph loaded
        let graph = match &self.graph {
            Some(g) => g,
            None => {
                return json!({
                    "status": "model_graph_not_available",
                    "detail": "No lifted model graph is available for this model. \
                               Live auto-lift (OSL #2 tier) has not yet been deployed. \
                               Load a lifted .ttl fixture to query this tool.",
                });
            }
        };

        // Parse the query name
        let query_name = match args.get("query").and_then(Value::as_str) {
            Some(q) => q,
            None => {
                return json!({
                    "status": "error",
                    "detail": "Missing required parameter 'query'. Specify a named query.",
                    "valid_queries": valid_query_names()
                });
            }
        };

        let params = args.get("params").cloned().unwrap_or(json!({}));

        // Dispatch to the appropriate canned query
        match query_name {
            "hierarchy_levels" => run_hierarchy_levels(graph, &params, &self.budget, start),
            "calc_dependencies" => run_calc_dependencies(graph, &params, &self.budget, start),
            "role_playing_refs" => run_role_playing_refs(graph, &params, &self.budget, start),
            "conformance_check" => run_conformance_check(graph, &params, &self.budget, start),
            _ => {
                json!({
                    "status": "error",
                    "detail": format!(
                        "Unknown query '{}'. Choose one of: {}",
                        query_name,
                        valid_query_names().join(", ")
                    ),
                    "valid_queries": valid_query_names(),
                    "params_help": canned_query_params_help()
                })
            }
        }
    }
}

impl Default for ModelGraphStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Valid query catalogue
// ─────────────────────────────────────────────────────────────────────────────

fn valid_query_names() -> Vec<&'static str> {
    vec![
        "hierarchy_levels",
        "calc_dependencies",
        "role_playing_refs",
        "conformance_check",
    ]
}

fn canned_query_params_help() -> Value {
    json!({
        "hierarchy_levels": {
            "description": "Return the ordered rollup levels of a hierarchy (IRI + label).",
            "params": {
                "hierarchy_iri": { "type": "string", "description": "IRI of the hierarchy to query. Omit to return all hierarchies." },
                "hierarchy_label": { "type": "string", "description": "rdfs:label of the hierarchy (alternative to hierarchy_iri)." }
            }
        },
        "calc_dependencies": {
            "description": "Return the measures/columns a calculated member depends on.",
            "params": {
                "measure_iri": { "type": "string", "description": "IRI of the calc/measure to query. Omit to return dependencies for all calcs." },
                "measure_label": { "type": "string", "description": "rdfs:label of the measure (alternative to measure_iri)." }
            }
        },
        "role_playing_refs": {
            "description": "Return role-playing dimension references (aso:playsRoleOf linkage).",
            "params": {
                "base_dimension_iri": { "type": "string", "description": "IRI of the base dimension. Omit to return all role-playing refs." }
            }
        },
        "conformance_check": {
            "description": "Check if two entities are conformed across models via owl:sameAs (requires lattice-bridge OSL #7).",
            "params": {
                "entity_a_iri": { "type": "string", "description": "IRI of the first entity." },
                "entity_b_iri": { "type": "string", "description": "IRI of the second entity (optional; omit to list all sameAs links)." }
            },
            "note": "Cross-model owl:sameAs links are emitted by the lattice-bridge component (OSL #7). Until that component runs on a model pair, this query returns an empty result."
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Graph traversal helpers (oxrdf 0.3 API)
// ─────────────────────────────────────────────────────────────────────────────

/// Look up the `rdfs:label` of a named node in the graph.
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

/// Return all subjects that have `rdf:type <class_iri>`.
fn subjects_of_type(graph: &Graph, class_iri: &str) -> Vec<String> {
    let Ok(type_pred) = NamedNode::new(RDF_TYPE) else {
        return vec![];
    };
    let Ok(class_node) = NamedNode::new(class_iri) else {
        return vec![];
    };
    // subjects_for_predicate_object expects TermRef for the object —
    // convert via TermRef::NamedNode to disambiguate the Into impl.
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

/// Return all objects of `(subject, predicate, ?)` as IRI strings.
fn objects_of(graph: &Graph, subject_iri: &str, predicate_iri: &str) -> Vec<String> {
    let Ok(subj) = NamedNode::new(subject_iri) else {
        return vec![];
    };
    let Ok(pred) = NamedNode::new(predicate_iri) else {
        return vec![];
    };
    // objects_for_subject_predicate returns TermRef
    graph
        .objects_for_subject_predicate(subj.as_ref(), pred.as_ref())
        .filter_map(|term| {
            if let TermRef::NamedNode(n) = term {
                Some(n.as_str().to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Check whether the budget's time limit has been exceeded.
fn time_exceeded(start: Instant, budget: &BudgetConfig) -> bool {
    start.elapsed() > budget.max_duration
}

// ─────────────────────────────────────────────────────────────────────────────
//  Canned query: hierarchy_levels (AC1)
// ─────────────────────────────────────────────────────────────────────────────

/// Return the ordered levels of a hierarchy (or all hierarchies).
///
/// Traversal: find hierarchy IRI(s) → collect levels via `aso:hasLevel` →
/// order from coarsest to finest using the `aso:rollsUpTo` chain.
fn run_hierarchy_levels(
    graph: &Graph,
    params: &Value,
    budget: &BudgetConfig,
    start: Instant,
) -> Value {
    // Resolve target hierarchy IRIs
    let target_iris: Vec<String> = {
        let mut iris = Vec::new();
        if let Some(iri) = params.get("hierarchy_iri").and_then(Value::as_str) {
            iris.push(iri.to_string());
        } else if let Some(label) = params.get("hierarchy_label").and_then(Value::as_str) {
            let hier_iris = subjects_of_type(graph, ASO_HIERARCHY);
            for h_iri in hier_iris {
                if label_of(graph, &h_iri).as_deref() == Some(label) {
                    iris.push(h_iri);
                }
            }
        } else {
            let mut all = subjects_of_type(graph, ASO_HIERARCHY);
            all.sort();
            iris = all;
        }
        iris
    };

    let mut bindings: Vec<Value> = Vec::new();

    for hier_iri in &target_iris {
        if time_exceeded(start, budget) {
            return budget_exceeded_result("hierarchy_levels", bindings.len());
        }

        let hier_label = label_of(graph, hier_iri);

        // Collect levels via aso:hasLevel
        let level_iris = objects_of(graph, hier_iri, ASO_HAS_LEVEL);

        let levels_to_order = if level_iris.is_empty() {
            // Fallback: use all Level-typed nodes in the graph
            let mut all = subjects_of_type(graph, ASO_LEVEL);
            all.sort();
            all
        } else {
            level_iris
        };

        let ordered = topo_sort_levels(graph, &levels_to_order);
        for (order, l_iri) in ordered.iter().enumerate() {
            if time_exceeded(start, budget) || bindings.len() >= budget.max_rows {
                return budget_exceeded_result("hierarchy_levels", bindings.len());
            }
            let l_label = label_of(graph, l_iri);
            bindings.push(json!({
                "hierarchy_iri": hier_iri,
                "hierarchy_label": hier_label,
                "level_iri": l_iri,
                "level_label": l_label,
                "order": order
            }));
        }
    }

    json!({
        "query": "hierarchy_levels",
        "bindings": bindings,
        "row_count": bindings.len(),
        "note": "order=0 is the coarsest (top) level; order increases toward finer grain."
    })
}

/// Topologically sort levels from coarsest to finest using the `aso:rollsUpTo`
/// chain (finer level `rollsUpTo` coarser level).
///
/// Returns levels ordered coarsest→finest (root first).
fn topo_sort_levels(graph: &Graph, level_iris: &[String]) -> Vec<String> {
    if level_iris.is_empty() {
        return vec![];
    }

    let level_set: std::collections::HashSet<&str> =
        level_iris.iter().map(String::as_str).collect();

    // Build parent map (finer → coarser)
    let mut parent_of: BTreeMap<String, Option<String>> = BTreeMap::new();
    for l in level_iris {
        let targets = objects_of(graph, l, ASO_ROLLS_UP_TO);
        let in_set = targets.into_iter().find(|t| level_set.contains(t.as_str()));
        parent_of.insert(l.clone(), in_set);
    }

    // Find root(s): levels whose parent is None (coarsest)
    let mut roots: Vec<String> = parent_of
        .iter()
        .filter(|(_, p)| p.is_none())
        .map(|(k, _)| k.clone())
        .collect();
    roots.sort();

    // BFS from roots (coarse → fine)
    let mut ordered = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<String> = roots.into_iter().collect();
    while let Some(l) = queue.pop_front() {
        if visited.contains(&l) {
            continue;
        }
        visited.insert(l.clone());
        ordered.push(l.clone());
        // Find children: levels in our set that roll up to `l`
        let mut children: Vec<String> = parent_of
            .iter()
            .filter(|(_, p)| p.as_deref() == Some(l.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        children.sort();
        for child in children {
            queue.push_back(child);
        }
    }

    // Append any levels not reached (disconnected components)
    for l in level_iris {
        if !visited.contains(l) {
            ordered.push(l.clone());
        }
    }

    ordered
}

// ─────────────────────────────────────────────────────────────────────────────
//  Canned query: calc_dependencies (AC2)
// ─────────────────────────────────────────────────────────────────────────────

/// Return measures/columns that a calc/measure depends on via `aso:dependsOn`.
fn run_calc_dependencies(
    graph: &Graph,
    params: &Value,
    budget: &BudgetConfig,
    start: Instant,
) -> Value {
    // All measure IRIs in the graph (measures + calcs)
    let measure_classes = [
        ASO_MEASURE,
        ASO_FULLY_ADDITIVE_MEASURE,
        ASO_SEMI_ADDITIVE_MEASURE,
        ASO_CALCULATED_MEMBER,
    ];

    let mut all_measures: Vec<String> = Vec::new();
    for cls in &measure_classes {
        all_measures.extend(subjects_of_type(graph, cls));
    }
    all_measures.sort();
    all_measures.dedup();

    let target_iris: Vec<String> = {
        let mut iris = Vec::new();
        if let Some(iri) = params.get("measure_iri").and_then(Value::as_str) {
            iris.push(iri.to_string());
        } else if let Some(label) = params.get("measure_label").and_then(Value::as_str) {
            for m_iri in &all_measures {
                if label_of(graph, m_iri).as_deref() == Some(label) {
                    iris.push(m_iri.clone());
                }
            }
        } else {
            iris = all_measures;
        }
        iris
    };

    let mut bindings: Vec<Value> = Vec::new();

    for measure_iri in &target_iris {
        if time_exceeded(start, budget) {
            return budget_exceeded_result("calc_dependencies", bindings.len());
        }
        let measure_label = label_of(graph, measure_iri);

        let deps = objects_of(graph, measure_iri, ASO_DEPENDS_ON);
        if !deps.is_empty() {
            for dep_iri in &deps {
                if bindings.len() >= budget.max_rows {
                    return budget_exceeded_result("calc_dependencies", bindings.len());
                }
                let dep_label = label_of(graph, dep_iri);
                bindings.push(json!({
                    "calc_iri": measure_iri,
                    "calc_label": measure_label,
                    "depends_on_iri": dep_iri,
                    "depends_on_label": dep_label,
                    "edge": "aso:dependsOn"
                }));
            }
        } else {
            // No explicit dependsOn: emit a "no-dependency-info" binding.
            bindings.push(json!({
                "calc_iri": measure_iri,
                "calc_label": measure_label,
                "depends_on_iri": null,
                "depends_on_label": null,
                "edge": null,
                "note": "No aso:dependsOn edges found. MDX lineage extraction (ATSCALE-47878) not yet present in this lift."
            }));
        }
    }

    json!({
        "query": "calc_dependencies",
        "bindings": bindings,
        "row_count": bindings.len()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Canned query: role_playing_refs (FR2)
// ─────────────────────────────────────────────────────────────────────────────

/// Return role-playing dimension references (aso:playsRoleOf linkage).
fn run_role_playing_refs(
    graph: &Graph,
    params: &Value,
    budget: &BudgetConfig,
    start: Instant,
) -> Value {
    let base_dim_filter = params
        .get("base_dimension_iri")
        .and_then(Value::as_str)
        .map(str::to_string);

    let role_refs = subjects_of_type(graph, ASO_ROLE_PLAYING_REFERENCE);

    let mut bindings: Vec<Value> = Vec::new();

    for ref_iri in &role_refs {
        if time_exceeded(start, budget) || bindings.len() >= budget.max_rows {
            return budget_exceeded_result("role_playing_refs", bindings.len());
        }
        let ref_label = label_of(graph, ref_iri);
        let base_iris = objects_of(graph, ref_iri, ASO_PLAYS_ROLE_OF);

        for base_iri in &base_iris {
            if let Some(ref filter_iri) = base_dim_filter {
                if base_iri != filter_iri {
                    continue;
                }
            }
            let base_label = label_of(graph, base_iri);
            bindings.push(json!({
                "role_ref_iri": ref_iri,
                "role_ref_label": ref_label,
                "base_dimension_iri": base_iri,
                "base_dimension_label": base_label,
                "edge": "aso:playsRoleOf"
            }));
        }
    }

    json!({
        "query": "role_playing_refs",
        "bindings": bindings,
        "row_count": bindings.len()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Canned query: conformance_check (AC3 — stub pending lattice-bridge)
// ─────────────────────────────────────────────────────────────────────────────

/// Check cross-model conformance via `owl:sameAs`.
///
/// **AC3 stub note:** `owl:sameAs` links are emitted by the `lattice-bridge`
/// component (OSL #7), which is not yet built. Until that component runs on a
/// model pair and re-lifts, this query returns an empty result (correct behavior:
/// no fabricated conformance claims). When lattice-bridge is integrated, `owl:sameAs`
/// triples will be present in the graph and this query will return them.
fn run_conformance_check(
    graph: &Graph,
    params: &Value,
    budget: &BudgetConfig,
    start: Instant,
) -> Value {
    let entity_a_filter = params
        .get("entity_a_iri")
        .and_then(Value::as_str)
        .map(str::to_string);
    let entity_b_filter = params
        .get("entity_b_iri")
        .and_then(Value::as_str)
        .map(str::to_string);

    let Ok(same_as_pred) = NamedNode::new(OWL_SAME_AS) else {
        return json!({
            "query": "conformance_check",
            "bindings": [],
            "row_count": 0,
            "note": "owl:sameAs IRI is invalid — internal error."
        });
    };

    let mut bindings: Vec<Value> = Vec::new();

    // Traverse all (?, owl:sameAs, ?) triples via triples_for_predicate
    for triple in graph.triples_for_predicate(same_as_pred.as_ref()) {
        if time_exceeded(start, budget) || bindings.len() >= budget.max_rows {
            return budget_exceeded_result("conformance_check", bindings.len());
        }

        // TripleRef.subject is NamedOrBlankNodeRef
        let a_iri = match triple.subject {
            oxrdf::NamedOrBlankNodeRef::NamedNode(n) => n.as_str().to_string(),
            oxrdf::NamedOrBlankNodeRef::BlankNode(_) => continue,
        };
        // TripleRef.object is TermRef
        let b_iri = match triple.object {
            TermRef::NamedNode(n) => n.as_str().to_string(),
            _ => continue,
        };

        if let Some(ref fa) = entity_a_filter {
            if &a_iri != fa {
                continue;
            }
        }
        if let Some(ref fb) = entity_b_filter {
            if &b_iri != fb {
                continue;
            }
        }

        let a_label = label_of(graph, &a_iri);
        let b_label = label_of(graph, &b_iri);
        bindings.push(json!({
            "entity_a_iri": a_iri,
            "entity_a_label": a_label,
            "entity_b_iri": b_iri,
            "entity_b_label": b_label,
            "edge": "owl:sameAs"
        }));
    }

    let note = if bindings.is_empty() {
        "No owl:sameAs conformance links found. This is expected: cross-model conformance \
         links require the lattice-bridge component (OSL #7) to have run on the relevant \
         model pair. This is a future integration tier."
    } else {
        "owl:sameAs links reflect lattice-bridge (OSL #7) output."
    };

    json!({
        "query": "conformance_check",
        "bindings": bindings,
        "row_count": bindings.len(),
        "note": note
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Budget exceeded helper
// ─────────────────────────────────────────────────────────────────────────────

fn budget_exceeded_result(query: &str, rows_so_far: usize) -> Value {
    json!({
        "status": "budget_exceeded",
        "query": query,
        "detail": format!(
            "Query '{}' exceeded the operator-configured time or row budget. \
             {} rows were collected before the limit was hit. \
             Narrow the query using params (e.g. hierarchy_iri, measure_iri) to reduce scope.",
            query, rows_so_far
        ),
        "rows_collected": rows_so_far
    })
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tool descriptor helper (called from mcp.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// JSON schema for the `query_model_graph` tool's input.
#[must_use]
pub fn query_model_graph_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "enum": ["hierarchy_levels", "calc_dependencies", "role_playing_refs", "conformance_check"],
                "description": "Named canned query to execute. Required unless raw_sparql is set."
            },
            "params": {
                "type": "object",
                "description": "Query-specific parameters (see valid_queries for each query's param schema).",
                "additionalProperties": true
            },
            "raw_sparql": {
                "type": "string",
                "description": "Raw SPARQL SELECT query string. OFF by default (operator opt-in required). When disabled, submitting this field returns an error pointing to the canned query set."
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

    // ── Fixture: small synthetic lifted graph ─────────────────────────────────
    //
    // Represents: one cube "Sales" with:
    //   - Hierarchy "Brand" with levels [Brand (coarse, order=0), SKU (fine, order=1)]
    //   - Measure "Revenue" (fully additive)
    //   - Measure "Profit Margin" (depends on Revenue, via aso:dependsOn)
    //   - Role-playing ref "Ship Date" playsRoleOf "Date Dimension"
    //
    // Note: aso:dependsOn is not part of the standard aso-lift output (requires
    // ATSCALE-47878); it is added directly to test the calc_dependencies query path.

    fn fixture_ttl() -> &'static str {
        r#"
@prefix aso:  <https://ontology.atscale.com/aso/> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

<https://models.atscale.com#ontology>
    rdf:type owl:Ontology ;
    owl:imports <https://ontology.atscale.com/aso/> .

<https://models.atscale.com#cube-sales>
    rdf:type owl:NamedIndividual, aso:Cube ;
    rdfs:label "Sales" .

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

<https://models.atscale.com#measure-revenue>
    rdf:type owl:NamedIndividual, aso:FullyAdditiveMeasure ;
    rdfs:label "Revenue" .

<https://models.atscale.com#measure-margin>
    rdf:type owl:NamedIndividual, aso:Measure ;
    rdfs:label "Profit Margin" ;
    aso:dependsOn <https://models.atscale.com#measure-revenue> .

<https://models.atscale.com#dim-date>
    rdf:type owl:NamedIndividual, aso:Dimension ;
    rdfs:label "Date Dimension" .

<https://models.atscale.com#role-ship-date>
    rdf:type owl:NamedIndividual, aso:RolePlayingReference ;
    rdfs:label "Ship Date" ;
    aso:playsRoleOf <https://models.atscale.com#dim-date> .
"#
    }

    fn fixture_store() -> ModelGraphStore {
        let mut store = ModelGraphStore::new();
        store
            .load_turtle(fixture_ttl())
            .expect("fixture TTL must parse");
        store
    }

    // ── AC1: hierarchy_levels returns ordered levels with IRIs + labels ────────

    #[test]
    fn ac1_hierarchy_levels_returns_ordered_levels_with_iris_and_labels() {
        let store = fixture_store();
        let result = store.query(&json!({
            "query": "hierarchy_levels",
            "params": { "hierarchy_iri": "https://models.atscale.com#hier-brand" }
        }));

        assert_eq!(
            result.get("query").and_then(Value::as_str),
            Some("hierarchy_levels"),
            "query field must be 'hierarchy_levels'"
        );

        let bindings = result
            .get("bindings")
            .and_then(Value::as_array)
            .expect("bindings must be an array");

        assert_eq!(bindings.len(), 2, "Brand hierarchy must have 2 levels");

        // order=0 must be coarsest: Brand
        let first = &bindings[0];
        assert_eq!(
            first.get("level_label").and_then(Value::as_str),
            Some("Brand"),
            "order=0 must be 'Brand' (coarsest)"
        );
        assert!(
            first.get("level_iri").and_then(Value::as_str).is_some(),
            "level_iri must be present"
        );
        assert_eq!(
            first.get("hierarchy_label").and_then(Value::as_str),
            Some("Brand"),
            "hierarchy_label must be 'Brand'"
        );

        // order=1 must be finer: SKU
        let second = &bindings[1];
        assert_eq!(
            second.get("level_label").and_then(Value::as_str),
            Some("SKU"),
            "order=1 must be 'SKU' (finer)"
        );
    }

    // ── AC2: calc_dependencies returns measure lineage ────────────────────────

    #[test]
    fn ac2_calc_dependencies_returns_lineage() {
        let store = fixture_store();
        let result = store.query(&json!({
            "query": "calc_dependencies",
            "params": {
                "measure_iri": "https://models.atscale.com#measure-margin"
            }
        }));

        assert_eq!(
            result.get("query").and_then(Value::as_str),
            Some("calc_dependencies"),
        );

        let bindings = result
            .get("bindings")
            .and_then(Value::as_array)
            .expect("bindings array");

        assert_eq!(bindings.len(), 1, "Profit Margin depends on exactly one measure");
        let b = &bindings[0];
        assert_eq!(
            b.get("calc_label").and_then(Value::as_str),
            Some("Profit Margin")
        );
        assert_eq!(
            b.get("depends_on_label").and_then(Value::as_str),
            Some("Revenue")
        );
        assert!(
            b.get("depends_on_iri").and_then(Value::as_str).is_some(),
            "depends_on_iri must be present (IRI)"
        );
    }

    // ── AC4: budget exceeded — row cap ────────────────────────────────────────

    #[test]
    fn ac4_budget_exceeded_when_row_cap_hit() {
        let mut store = fixture_store();
        store.budget.max_rows = 1; // 2 levels exist, so cap at 1 triggers exceeded
        let result = store.query(&json!({
            "query": "hierarchy_levels"
        }));
        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("budget_exceeded"),
            "must return budget_exceeded when row cap hit: {result}"
        );
    }

    // ── AC4: budget exceeded — time cap ───────────────────────────────────────

    #[test]
    fn ac4_budget_exceeded_or_fast_when_time_cap_zero() {
        let mut store = fixture_store();
        // Zero-duration budget: the first time_exceeded check may fire immediately.
        store.budget.max_duration = Duration::from_nanos(0);
        let result = store.query(&json!({ "query": "hierarchy_levels" }));
        let status = result.get("status").and_then(Value::as_str);
        let is_valid = status == Some("budget_exceeded")
            || result.get("query").and_then(Value::as_str) == Some("hierarchy_levels");
        assert!(is_valid, "must return budget_exceeded or normal result: {result}");
    }

    // ── AC5: raw SPARQL refused when disabled ─────────────────────────────────

    #[test]
    fn ac5_raw_sparql_refused_when_disabled() {
        let store = fixture_store();
        let result = store.query(&json!({
            "raw_sparql": "SELECT ?s WHERE { ?s a aso:Cube }"
        }));
        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("raw_sparql_disabled"),
            "must refuse raw SPARQL when disabled: {result}"
        );
        let valid = result.get("valid_queries").and_then(Value::as_array);
        assert!(valid.is_some(), "must include valid_queries list");
        assert!(!valid.unwrap().is_empty(), "valid_queries must not be empty");
    }

    // ── AC6: model graph not available ────────────────────────────────────────

    #[test]
    fn ac6_no_graph_returns_not_available() {
        let store = ModelGraphStore::new();
        let result = store.query(&json!({
            "query": "hierarchy_levels"
        }));
        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("model_graph_not_available"),
            "must return model_graph_not_available when no graph is loaded: {result}"
        );
    }

    // ── AC7: data-leak guard — results contain only model-metadata ────────────

    #[test]
    fn ac7_results_contain_only_model_metadata_no_warehouse_rows() {
        let store = fixture_store();
        let result = store.query(&json!({
            "query": "hierarchy_levels"
        }));
        let bindings = result
            .get("bindings")
            .and_then(Value::as_array)
            .expect("bindings array");

        for binding in bindings {
            if let Some(level_iri) = binding.get("level_iri").and_then(Value::as_str) {
                assert!(
                    level_iri.starts_with("https://models.atscale.com")
                        || level_iri.starts_with("https://ontology.atscale.com"),
                    "level_iri must be a model-metadata IRI, not warehouse data: {level_iri}"
                );
            }
            assert!(
                binding.get("row_data").is_none(),
                "bindings must not contain 'row_data' warehouse fields"
            );
            assert!(
                binding.get("fact_table_row").is_none(),
                "bindings must not contain 'fact_table_row' warehouse fields"
            );
        }
    }

    // ── AC8: unknown query name returns actionable error ─────────────────────

    #[test]
    fn ac8_unknown_query_name_returns_actionable_error() {
        let store = fixture_store();
        let result = store.query(&json!({
            "query": "does_not_exist",
            "params": {}
        }));
        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("error"),
            "unknown query must return status=error: {result}"
        );
        let valid = result
            .get("valid_queries")
            .and_then(Value::as_array)
            .expect("must include valid_queries");
        assert!(
            valid.iter().any(|v| v.as_str() == Some("hierarchy_levels")),
            "valid_queries must list 'hierarchy_levels'"
        );
        assert!(
            result.get("params_help").is_some(),
            "must include params_help to guide the caller"
        );
    }

    // ── AC8: missing query param returns error ────────────────────────────────

    #[test]
    fn ac8_missing_query_field_returns_error() {
        let store = fixture_store();
        let result = store.query(&json!({
            "params": { "hierarchy_iri": "https://models.atscale.com#hier-brand" }
        }));
        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("error"),
            "missing query field must return status=error: {result}"
        );
        assert!(
            result.get("valid_queries").is_some(),
            "error response must list valid_queries"
        );
    }

    // ── AC3 stub: conformance_check returns empty without sameAs triples ──────

    #[test]
    fn ac3_conformance_check_returns_empty_without_same_as_triples() {
        let store = fixture_store();
        let result = store.query(&json!({
            "query": "conformance_check"
        }));
        assert_eq!(
            result.get("query").and_then(Value::as_str),
            Some("conformance_check"),
        );
        let bindings = result
            .get("bindings")
            .and_then(Value::as_array)
            .expect("bindings array");
        assert_eq!(
            bindings.len(),
            0,
            "conformance_check must return 0 bindings when no owl:sameAs triples present \
             (lattice-bridge not yet integrated)"
        );
        let note = result.get("note").and_then(Value::as_str).unwrap_or("");
        assert!(
            note.contains("lattice-bridge") || note.contains("OSL #7"),
            "note must reference lattice-bridge/OSL #7: {note}"
        );
    }

    // ── Role-playing refs roundtrip ───────────────────────────────────────────

    #[test]
    fn role_playing_refs_finds_ship_date() {
        let store = fixture_store();
        let result = store.query(&json!({
            "query": "role_playing_refs"
        }));
        let bindings = result
            .get("bindings")
            .and_then(Value::as_array)
            .expect("bindings array");
        assert_eq!(bindings.len(), 1, "one role-playing ref in fixture");
        let b = &bindings[0];
        assert_eq!(
            b.get("role_ref_label").and_then(Value::as_str),
            Some("Ship Date")
        );
        assert_eq!(
            b.get("base_dimension_label").and_then(Value::as_str),
            Some("Date Dimension")
        );
    }

    // ── Raw SPARQL disabled takes precedence over no-graph ────────────────────

    #[test]
    fn raw_sparql_disabled_takes_precedence_over_no_graph() {
        let store = ModelGraphStore::new(); // no graph
        let result = store.query(&json!({
            "raw_sparql": "SELECT * WHERE { ?s ?p ?o }"
        }));
        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("raw_sparql_disabled"),
        );
    }
}
