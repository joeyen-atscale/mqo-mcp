#![deny(unsafe_code)]
#![deny(missing_docs)]

//! # mqo-graph-traversal
//!
//! Traversal engine for the AtScale semantic model concept graph.
//!
//! All Gen-8 downstream tools (grounding-eval, hallucination-guard, gap-miner,
//! causal-tracer) delegate graph traversal to this crate so that edge semantics
//! are defined once and shared consistently.
//!
//! ## Core API
//!
//! - [`build_graph`] — parse a `describe_model` JSON string into a [`ConceptGraph`].
//! - [`related_measures`] — find all measures reachable within `depth` hops.
//! - [`causal_paths`] — return ranked derivation paths with `evidence_type: Structural`.
//! - [`suggest_next_questions`] — return adjacent measure nodes not yet in context.
//!
//! ## Quick start
//!
//! ```
//! use mqo_graph_traversal::{build_graph, related_measures, causal_paths, suggest_next_questions};
//!
//! let json = r#"{
//!   "measures": [
//!     {"unique_name": "revenue", "name": "Revenue"},
//!     {"unique_name": "cost", "name": "Cost"}
//!   ],
//!   "dimensions": [],
//!   "calculated_members": [
//!     {"unique_name": "profit", "name": "Profit",
//!      "formula_refs": [{"unique_name": "revenue"}, {"unique_name": "cost"}]}
//!   ]
//! }"#;
//!
//! let graph = build_graph(json).expect("valid JSON");
//! let related = related_measures(&graph, "profit", 1);
//! assert!(!related.is_empty());
//! ```

use std::collections::{HashMap, HashSet, VecDeque};

pub use mqo_concept_graph::{
    ConceptEdge, ConceptGraph, ConceptGraphError, ConceptGraphSnapshot, ConceptNode, EdgeKind,
    EdgeSnapshot, NodeKind,
};
use serde::{Deserialize, Serialize};

// ─── build_graph ─────────────────────────────────────────────────────────────

/// Parse an AtScale `describe_model` JSON string and return a [`ConceptGraph`].
///
/// # Panics
///
/// Panics if `json` is empty or is not valid JSON.  For a non-panicking
/// variant, call [`ConceptGraph::from_describe_model`] directly and handle the
/// [`ConceptGraphError`].
///
/// # Examples
///
/// ```
/// use mqo_graph_traversal::build_graph;
///
/// let graph = build_graph(r#"{"measures":[{"unique_name":"m1","name":"Rev"}],"dimensions":[],"calculated_members":[]}"#).unwrap();
/// assert_eq!(graph.node_count(), 1);
/// ```
pub fn build_graph(json: &str) -> Result<ConceptGraph, ConceptGraphError> {
    ConceptGraph::from_describe_model(json)
}

// ─── related_measures ────────────────────────────────────────────────────────

/// A measure node reachable from a starting measure, together with its graph distance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedMeasure {
    /// The `unique_name` of the reachable measure node.
    pub unique_name: String,
    /// Human-readable display name of the measure.
    pub display_name: String,
    /// Number of hops from the start node.
    pub distance: usize,
}

/// Return all [`RelatedMeasure`]s reachable from `start_measure` within `depth` hops,
/// in ascending order by graph distance (closest first).
///
/// Traversal is undirected (edges are followed in both directions) so that a measure
/// connected to a shared dimension level is treated as related regardless of edge
/// orientation.
///
/// Returns an **empty `Vec`** (not an error) when `start_measure` is not found in the
/// graph.
///
/// # Arguments
///
/// * `graph`         — the concept graph to traverse.
/// * `start_measure` — `unique_name` of the starting measure.
/// * `depth`         — maximum number of hops to follow.
pub fn related_measures(
    graph: &ConceptGraph,
    start_measure: &str,
    depth: usize,
) -> Vec<RelatedMeasure> {
    // Collect all nodes and edges into local adjacency structures so we can do
    // undirected BFS without borrowing the graph across iterator chains.
    let node_map: HashMap<String, &ConceptNode> =
        graph.nodes().map(|n| (n.unique_name.clone(), n)).collect();

    // Build undirected adjacency list: unique_name → set of neighbor unique_names
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for (src, tgt, _kind) in graph.edges() {
        adj.entry(src.to_owned())
            .or_default()
            .push(tgt.to_owned());
        adj.entry(tgt.to_owned())
            .or_default()
            .push(src.to_owned());
    }

    if !node_map.contains_key(start_measure) {
        return vec![];
    }

    // BFS up to `depth` hops; collect Measure nodes only (excluding the start).
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    visited.insert(start_measure.to_owned());
    queue.push_back((start_measure.to_owned(), 0));

    let mut results: Vec<RelatedMeasure> = Vec::new();

    while let Some((current, dist)) = queue.pop_front() {
        if dist >= depth {
            continue;
        }
        if let Some(neighbors) = adj.get(&current) {
            for nbr in neighbors {
                if visited.contains(nbr) {
                    continue;
                }
                visited.insert(nbr.clone());
                let new_dist = dist + 1;
                if let Some(node) = node_map.get(nbr) {
                    if node.kind == NodeKind::Measure || node.kind == NodeKind::Calc {
                        results.push(RelatedMeasure {
                            unique_name: node.unique_name.clone(),
                            display_name: node.display_name.clone(),
                            distance: new_dist,
                        });
                    }
                }
                queue.push_back((nbr.clone(), new_dist));
            }
        }
    }

    results.sort_by_key(|r| r.distance);
    results
}

// ─── causal_paths ────────────────────────────────────────────────────────────

/// Evidence type for a causal path.
///
/// All paths returned by [`causal_paths`] are structurally derived (no
/// statistical inference), so the only variant in v1 is [`EvidenceType::Structural`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceType {
    /// The path is derived from the explicit derivation-edge topology of the
    /// concept graph, not from statistical inference.
    Structural,
}

/// A single step in a causal path: an edge connecting two named nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathStep {
    /// `unique_name` of the source node at this step.
    pub from: String,
    /// `unique_name` of the target node at this step.
    pub to: String,
    /// The edge kind connecting `from` to `to`.
    pub edge_kind: EdgeKind,
}

/// A ranked causal derivation path from a root node to a target measure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalPath {
    /// Ordered list of steps from root to target.
    pub steps: Vec<PathStep>,
    /// How the path was derived.  Always [`EvidenceType::Structural`] in v1.
    pub evidence_type: EvidenceType,
    /// Path length (number of edges).  Shorter paths rank higher.
    pub length: usize,
}

/// Return ranked [`CausalPath`]s from causal ancestors (root nodes) to
/// `target_measure`.
///
/// A path is constructed by following [`EdgeKind::DerivesFrom`] and
/// [`EdgeKind::FiltersBy`] edges *outward* from the target until leaf roots are
/// reached (nodes with no further derivation edges).  The resulting paths are then
/// presented in forward order: root → … → target.
///
/// Paths are ranked by ascending length (shortest first).
///
/// Returns an **empty `Vec`** when `target_measure` is not in the graph or has no
/// derivation edges leading from it to roots.
///
/// # Arguments
///
/// * `graph`          — the concept graph to inspect.
/// * `target_measure` — `unique_name` of the measure to find derivation paths for.
pub fn causal_paths(graph: &ConceptGraph, target_measure: &str) -> Vec<CausalPath> {
    // Build a forward-edge map for DerivesFrom / FiltersBy edges only:
    // source → [(target, edge_kind)]
    // Edge semantics: `profit DerivesFrom revenue` means profit --DerivesFrom--> revenue
    // in the petgraph; the "root" is revenue (no further derivation edges outward),
    // and the path is revenue → profit.
    let mut fwd_adj: HashMap<String, Vec<(String, EdgeKind)>> = HashMap::new();
    for (src, tgt, kind) in graph.edges() {
        if kind == EdgeKind::DerivesFrom || kind == EdgeKind::FiltersBy {
            fwd_adj
                .entry(src.to_owned())
                .or_default()
                .push((tgt.to_owned(), kind));
        }
    }

    if graph.get_node(target_measure).is_none() {
        return vec![];
    }

    // Check whether the target has any outgoing derivation edges at all.
    if fwd_adj.get(target_measure).map(|v| v.is_empty()).unwrap_or(true)
        && !fwd_adj.contains_key(target_measure)
    {
        return vec![];
    }

    // DFS forward from target_measure following derivation edges.
    // We build paths from target outward; each path ends when no further
    // derivation edges exist.  Then reverse each path so it reads root → target.
    //
    // State: (current_node, steps_so_far_from_target)
    let mut paths: Vec<CausalPath> = Vec::new();
    // Each stack frame: (current, accumulated_forward_steps, per-path visited set)
    let mut stack: Vec<(String, Vec<PathStep>, HashSet<String>)> = vec![(
        target_measure.to_owned(),
        vec![],
        {
            let mut s = HashSet::new();
            s.insert(target_measure.to_owned());
            s
        },
    )];

    while let Some((current, steps_so_far, visited)) = stack.pop() {
        let successors = fwd_adj.get(&current);
        let has_successors = successors.map(|v| !v.is_empty()).unwrap_or(false);

        if !has_successors {
            // Leaf root reached: emit if we traversed at least one step.
            if !steps_so_far.is_empty() {
                // steps_so_far is target→...→root; reverse to get root→...→target
                let mut steps = steps_so_far.clone();
                steps.reverse();
                let length = steps.len();
                paths.push(CausalPath {
                    steps,
                    evidence_type: EvidenceType::Structural,
                    length,
                });
            }
            continue;
        }

        if let Some(succs) = successors {
            for (succ, kind) in succs {
                if visited.contains(succ) {
                    // Cycle: emit what we have so far.
                    if !steps_so_far.is_empty() {
                        let mut steps = steps_so_far.clone();
                        steps.reverse();
                        let length = steps.len();
                        paths.push(CausalPath {
                            steps,
                            evidence_type: EvidenceType::Structural,
                            length,
                        });
                    }
                    continue;
                }
                let mut new_steps = steps_so_far.clone();
                // Store the step in reverse (target-direction first); we'll reverse the whole
                // vec at the end.  Step direction: from succ (ancestor) to current (descendant).
                new_steps.push(PathStep {
                    from: succ.clone(),
                    to: current.clone(),
                    edge_kind: *kind,
                });
                let mut new_visited = visited.clone();
                new_visited.insert(succ.clone());
                stack.push((succ.clone(), new_steps, new_visited));
            }
        }
    }

    // Sort by ascending length.
    paths.sort_by_key(|p| p.length);
    paths
}

// ─── suggest_next_questions ──────────────────────────────────────────────────

/// A candidate next question: a measure node adjacent to the current context
/// but not yet included in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextQuestion {
    /// `unique_name` of the suggested measure.
    pub unique_name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// The `unique_name` of the context node that links to this suggestion.
    pub connected_via: String,
}

/// Return candidate [`NextQuestion`]s — measure nodes that are adjacent to any
/// node in `context_measures` but not already in `context_measures`.
///
/// Returns an **empty `Vec`** when the context already covers all reachable
/// measure neighbors (or the graph has no nodes).
///
/// # Arguments
///
/// * `graph`            — the concept graph to inspect.
/// * `context_measures` — slice of `unique_name`s already in context.
pub fn suggest_next_questions(
    graph: &ConceptGraph,
    context_measures: &[&str],
) -> Vec<NextQuestion> {
    let context_set: HashSet<&str> = context_measures.iter().copied().collect();

    // Build undirected adjacency list (all edge types).
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for (src, tgt, _kind) in graph.edges() {
        adj.entry(src.to_owned())
            .or_default()
            .push(tgt.to_owned());
        adj.entry(tgt.to_owned())
            .or_default()
            .push(src.to_owned());
    }

    let mut seen_suggestions: HashSet<String> = HashSet::new();
    let mut results: Vec<NextQuestion> = Vec::new();

    for &ctx_name in context_measures {
        if graph.get_node(ctx_name).is_none() {
            continue;
        }
        if let Some(neighbors) = adj.get(ctx_name) {
            for nbr in neighbors {
                if context_set.contains(nbr.as_str()) {
                    continue;
                }
                if seen_suggestions.contains(nbr) {
                    continue;
                }
                if let Some(node) = graph.get_node(nbr) {
                    if node.kind == NodeKind::Measure || node.kind == NodeKind::Calc {
                        seen_suggestions.insert(nbr.clone());
                        results.push(NextQuestion {
                            unique_name: node.unique_name.clone(),
                            display_name: node.display_name.clone(),
                            connected_via: ctx_name.to_owned(),
                        });
                    }
                }
            }
        }
    }

    results
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_json() -> &'static str {
        r#"{
            "measures": [
                {"unique_name": "revenue", "name": "Revenue"},
                {"unique_name": "cost", "name": "Cost"}
            ],
            "dimensions": [
                {"unique_name": "d_date", "name": "Date", "hierarchies": [
                    {"unique_name": "h_cal", "name": "Calendar", "levels": [
                        {"unique_name": "l_year", "name": "Year"}
                    ]}
                ]}
            ],
            "calculated_members": [
                {"unique_name": "profit", "name": "Profit",
                 "formula_refs": [{"unique_name": "revenue"}, {"unique_name": "cost"}]}
            ],
            "edges": [
                {"from": "revenue", "to": "l_year", "kind": "aggregates_via"},
                {"from": "cost",    "to": "l_year", "kind": "aggregates_via"}
            ]
        }"#
    }

    #[test]
    fn build_graph_smoke() {
        let g = build_graph(minimal_json()).unwrap();
        // measures: revenue, cost; hierarchy: h_cal; level: l_year; calc: profit = 5 nodes
        assert_eq!(g.node_count(), 5);
        // 2 explicit edges + 2 from dimension_links + 1 hier→level RelatedTo + 2 DerivesFrom = 7
        // Actually: 2 explicit aggregates_via + 1 RelatedTo (h_cal→l_year) + 2 DerivesFrom (profit→revenue, profit→cost) = 5
        assert_eq!(g.edge_count(), 5);
    }

    #[test]
    fn related_measures_basic() {
        let g = build_graph(minimal_json()).unwrap();
        // profit derives_from revenue and cost → depth 1 from profit should return both
        let related = related_measures(&g, "profit", 1);
        let names: Vec<_> = related.iter().map(|r| r.unique_name.as_str()).collect();
        assert!(names.contains(&"revenue"), "expected revenue in {names:?}");
        assert!(names.contains(&"cost"), "expected cost in {names:?}");
    }

    #[test]
    fn related_measures_missing_start() {
        let g = build_graph(minimal_json()).unwrap();
        let related = related_measures(&g, "nonexistent", 3);
        assert!(related.is_empty());
    }

    #[test]
    fn causal_paths_basic() {
        let g = build_graph(minimal_json()).unwrap();
        let paths = causal_paths(&g, "profit");
        assert!(!paths.is_empty(), "profit has DerivesFrom edges");
        for p in &paths {
            assert_eq!(p.evidence_type, EvidenceType::Structural);
        }
    }

    #[test]
    fn suggest_next_questions_basic() {
        let g = build_graph(minimal_json()).unwrap();
        // Context = only revenue; profit is adjacent via DerivesFrom
        let suggestions = suggest_next_questions(&g, &["revenue"]);
        let names: Vec<_> = suggestions
            .iter()
            .map(|q| q.unique_name.as_str())
            .collect();
        assert!(names.contains(&"profit"), "expected profit in {names:?}");
    }

    #[test]
    fn suggest_next_questions_full_context() {
        let g = build_graph(minimal_json()).unwrap();
        // All measure+calc nodes in context; no new suggestions possible
        let suggestions = suggest_next_questions(&g, &["revenue", "cost", "profit"]);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn serde_roundtrip_snapshot() {
        let g = build_graph(minimal_json()).unwrap();
        let snap = g.to_snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let snap2: ConceptGraphSnapshot = serde_json::from_str(&json).unwrap();
        let g2 = ConceptGraph::from_snapshot(snap2).unwrap();
        assert_eq!(g.node_count(), g2.node_count());
        assert_eq!(g.edge_count(), g2.edge_count());
    }
}
