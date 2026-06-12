#![deny(unsafe_code)]
#![deny(missing_docs)]

//! # mqo-concept-graph
//!
//! Converts an AtScale `describe_model` JSON payload into an in-memory attributed
//! property graph.  All Gen-8 downstream tools (causal-tracer, narrative-builder,
//! nl-model-extender) share this single representation so that semantic topology is
//! extracted once, consistently, and deterministically — with no LLM calls.
//!
//! ## Quick start
//!
//! ```
//! use mqo_concept_graph::{ConceptGraph, EdgeKind};
//!
//! let json = r#"{
//!   "measures": [{"unique_name": "m1", "name": "Revenue"}],
//!   "dimensions": [{"unique_name": "d1", "name": "Date", "hierarchies": [{"unique_name": "h1", "name": "Calendar", "levels": [{"unique_name": "l1", "name": "Year"}]}]}],
//!   "calculated_members": []
//! }"#;
//!
//! let graph = ConceptGraph::from_describe_model(json).unwrap();
//! assert!(graph.node_count() > 0);
//! ```

use std::collections::HashMap;

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors returned by [`ConceptGraph::from_describe_model`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConceptGraphError {
    /// The input string is not valid JSON, or is empty.
    InvalidJson(String),
    /// The JSON is valid but does not contain the expected `describe_model` structure.
    InvalidStructure(String),
}

impl std::fmt::Display for ConceptGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConceptGraphError::InvalidJson(msg) => write!(f, "invalid JSON: {msg}"),
            ConceptGraphError::InvalidStructure(msg) => write!(f, "invalid structure: {msg}"),
        }
    }
}

impl std::error::Error for ConceptGraphError {}

// ─── Node types ──────────────────────────────────────────────────────────────

/// A node in the concept graph.  Every node has a stable `unique_name` identifier
/// (matching the AtScale `unique_name` field in `describe_model` output) and a
/// human-readable display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConceptNode {
    /// The stable unique name used as the primary key.  Corresponds to
    /// `unique_name` in AtScale `describe_model` output.
    pub unique_name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// The semantic kind of this node.
    pub kind: NodeKind,
}

/// Semantic kind of a [`ConceptNode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    /// A base or aggregate measure defined on the model.
    Measure,
    /// A leaf-level or non-leaf level within a dimension hierarchy.
    DimensionLevel,
    /// A calculated member / calculated measure derived from other nodes.
    Calc,
    /// A named hierarchy within a dimension.
    Hierarchy,
}

// ─── Edge types ──────────────────────────────────────────────────────────────

/// The semantic kind of a directed edge in the concept graph.
///
/// The five variants cover all primary relationships expressed in AtScale
/// `describe_model` output, plus a catch-all for future extensibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeKind {
    /// A measure is aggregated through (grouped by) a dimension level or hierarchy.
    AggregatesVia,
    /// A time-intelligence measure is shifted relative to another time period.
    TimeShifts,
    /// One node's output is filtered by another node (e.g. a filter measure).
    FiltersBy,
    /// A calculated node is derived from one or more base nodes.
    DerivesFrom,
    /// A generic relationship used as a catch-all for future edge semantics.
    RelatedTo,
}

/// An edge in the concept graph connecting two [`ConceptNode`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConceptEdge {
    /// The semantic kind of this edge.
    pub kind: EdgeKind,
}

// ─── Serializable snapshot ────────────────────────────────────────────────────

/// A serializable snapshot of a single edge, used for JSON round-trip serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSnapshot {
    /// The `unique_name` of the source node.
    pub from: String,
    /// The `unique_name` of the target node.
    pub to: String,
    /// The semantic kind of the edge.
    pub kind: EdgeKind,
}

/// A serializable snapshot of the full graph (nodes + edges), enabling JSON
/// round-trip serialization without depending on petgraph's internal serde format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptGraphSnapshot {
    /// All nodes in the graph.
    pub nodes: Vec<ConceptNode>,
    /// All edges in the graph.
    pub edges: Vec<EdgeSnapshot>,
}

// ─── The graph ───────────────────────────────────────────────────────────────

/// An immutable attributed property graph derived from an AtScale `describe_model`
/// JSON payload.
///
/// Internally backed by [`petgraph::graph::DiGraph`].  Constructed once via
/// [`ConceptGraph::from_describe_model`]; no mutation after construction (v1).
#[derive(Debug)]
pub struct ConceptGraph {
    /// The underlying directed graph storing nodes and typed edges.
    graph: DiGraph<ConceptNode, ConceptEdge>,
    /// Map from `unique_name` to `NodeIndex` for O(1) lookup.
    index: HashMap<String, NodeIndex>,
}

impl ConceptGraph {
    // ── Construction ─────────────────────────────────────────────────────────

    /// Parse an AtScale `describe_model` JSON string and return a fully-built
    /// concept graph.
    ///
    /// # Errors
    ///
    /// Returns [`ConceptGraphError::InvalidJson`] for empty or non-JSON input,
    /// and [`ConceptGraphError::InvalidStructure`] for JSON that lacks the
    /// expected top-level object shape.
    pub fn from_describe_model(json: &str) -> Result<Self, ConceptGraphError> {
        if json.trim().is_empty() {
            return Err(ConceptGraphError::InvalidJson(
                "input is empty".to_string(),
            ));
        }

        let root: Value = serde_json::from_str(json)
            .map_err(|e| ConceptGraphError::InvalidJson(e.to_string()))?;

        if !root.is_object() {
            return Err(ConceptGraphError::InvalidStructure(
                "top-level value must be a JSON object".to_string(),
            ));
        }

        let mut graph: DiGraph<ConceptNode, ConceptEdge> = DiGraph::new();
        let mut index: HashMap<String, NodeIndex> = HashMap::new();

        // ── 1. Add measure nodes ─────────────────────────────────────────────
        if let Some(measures) = root.get("measures").and_then(Value::as_array) {
            for m in measures {
                let uname = str_field(m, "unique_name")?;
                let dname = str_field(m, "name").unwrap_or_else(|_| uname.clone());
                let node = ConceptNode {
                    unique_name: uname.clone(),
                    display_name: dname,
                    kind: NodeKind::Measure,
                };
                let idx = graph.add_node(node);
                index.insert(uname, idx);
            }
        }

        // ── 2. Add dimension levels and hierarchies ──────────────────────────
        if let Some(dimensions) = root.get("dimensions").and_then(Value::as_array) {
            for dim in dimensions {
                let hierarchies = dim
                    .get("hierarchies")
                    .and_then(Value::as_array)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);

                for hier in hierarchies {
                    let h_uname = str_field(hier, "unique_name")?;
                    let h_dname =
                        str_field(hier, "name").unwrap_or_else(|_| h_uname.clone());
                    let h_node = ConceptNode {
                        unique_name: h_uname.clone(),
                        display_name: h_dname,
                        kind: NodeKind::Hierarchy,
                    };
                    let h_idx = graph.add_node(h_node);
                    index.insert(h_uname.clone(), h_idx);

                    let levels = hier
                        .get("levels")
                        .and_then(Value::as_array)
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]);

                    for level in levels {
                        let l_uname = str_field(level, "unique_name")?;
                        let l_dname =
                            str_field(level, "name").unwrap_or_else(|_| l_uname.clone());
                        let l_node = ConceptNode {
                            unique_name: l_uname.clone(),
                            display_name: l_dname,
                            kind: NodeKind::DimensionLevel,
                        };
                        let l_idx = graph.add_node(l_node);
                        index.insert(l_uname.clone(), l_idx);

                        // Hierarchy → Level (RelatedTo, structural containment)
                        graph.add_edge(
                            h_idx,
                            l_idx,
                            ConceptEdge { kind: EdgeKind::RelatedTo },
                        );
                    }
                }
            }
        }

        // ── 3. Add calculated member nodes ──────────────────────────────────
        if let Some(calcs) = root
            .get("calculated_members")
            .and_then(Value::as_array)
        {
            for calc in calcs {
                let uname = str_field(calc, "unique_name")?;
                let dname = str_field(calc, "name").unwrap_or_else(|_| uname.clone());
                let node = ConceptNode {
                    unique_name: uname.clone(),
                    display_name: dname,
                    kind: NodeKind::Calc,
                };
                let idx = graph.add_node(node);
                index.insert(uname, idx);
            }
        }

        // ── 4. Add typed semantic edges from the optional `edges` array ──────
        //
        // Fixture format:
        //   "edges": [{ "from": "m1", "to": "l1", "kind": "aggregates_via" }, ...]
        if let Some(edges) = root.get("edges").and_then(Value::as_array) {
            for e in edges {
                let from_name = str_field(e, "from")?;
                let to_name = str_field(e, "to")?;
                let kind_str = str_field(e, "kind")?;

                let kind = parse_edge_kind(&kind_str)?;

                let from_idx = index.get(&from_name).copied().ok_or_else(|| {
                    ConceptGraphError::InvalidStructure(format!(
                        "edge references unknown node '{from_name}'"
                    ))
                })?;
                let to_idx = index.get(&to_name).copied().ok_or_else(|| {
                    ConceptGraphError::InvalidStructure(format!(
                        "edge references unknown node '{to_name}'"
                    ))
                })?;

                graph.add_edge(from_idx, to_idx, ConceptEdge { kind });
            }
        }

        // ── 5. Derive AggregatesVia / TimeShifts from measure fields ─────────
        if let Some(measures) = root.get("measures").and_then(Value::as_array) {
            for m in measures {
                let m_uname = match str_field(m, "unique_name") {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                let m_idx = match index.get(&m_uname).copied() {
                    Some(i) => i,
                    None => continue,
                };
                if let Some(links) = m.get("dimension_links").and_then(Value::as_array) {
                    for link in links {
                        let l_uname = match str_field(link, "level_unique_name") {
                            Ok(u) => u,
                            Err(_) => continue,
                        };
                        if let Some(&l_idx) = index.get(&l_uname) {
                            graph.add_edge(
                                m_idx,
                                l_idx,
                                ConceptEdge { kind: EdgeKind::AggregatesVia },
                            );
                        }
                    }
                }
                if let Some(links) = m.get("time_shift_links").and_then(Value::as_array) {
                    for link in links {
                        let t_uname = match str_field(link, "target_unique_name") {
                            Ok(u) => u,
                            Err(_) => continue,
                        };
                        if let Some(&t_idx) = index.get(&t_uname) {
                            graph.add_edge(
                                m_idx,
                                t_idx,
                                ConceptEdge { kind: EdgeKind::TimeShifts },
                            );
                        }
                    }
                }
            }
        }

        // ── 6. Derive DerivesFrom / FiltersBy from calculated_members ─────────
        if let Some(calcs) = root
            .get("calculated_members")
            .and_then(Value::as_array)
        {
            for calc in calcs {
                let c_uname = match str_field(calc, "unique_name") {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                let c_idx = match index.get(&c_uname).copied() {
                    Some(i) => i,
                    None => continue,
                };
                if let Some(refs) = calc.get("formula_refs").and_then(Value::as_array) {
                    for r in refs {
                        let r_uname = match str_field(r, "unique_name") {
                            Ok(u) => u,
                            Err(_) => continue,
                        };
                        if let Some(&r_idx) = index.get(&r_uname) {
                            graph.add_edge(
                                c_idx,
                                r_idx,
                                ConceptEdge { kind: EdgeKind::DerivesFrom },
                            );
                        }
                    }
                }
                if let Some(refs) = calc.get("filter_refs").and_then(Value::as_array) {
                    for r in refs {
                        let r_uname = match str_field(r, "unique_name") {
                            Ok(u) => u,
                            Err(_) => continue,
                        };
                        if let Some(&r_idx) = index.get(&r_uname) {
                            graph.add_edge(
                                c_idx,
                                r_idx,
                                ConceptEdge { kind: EdgeKind::FiltersBy },
                            );
                        }
                    }
                }
            }
        }

        Ok(ConceptGraph { graph, index })
    }

    /// Reconstruct a [`ConceptGraph`] from a [`ConceptGraphSnapshot`] (the
    /// serializable intermediate form).  This is the deserialization half of the
    /// JSON round-trip.
    pub fn from_snapshot(snap: ConceptGraphSnapshot) -> Result<Self, ConceptGraphError> {
        let mut graph: DiGraph<ConceptNode, ConceptEdge> = DiGraph::new();
        let mut index: HashMap<String, NodeIndex> = HashMap::new();

        for node in snap.nodes {
            let uname = node.unique_name.clone();
            let idx = graph.add_node(node);
            index.insert(uname, idx);
        }

        for edge in snap.edges {
            let from_idx = index.get(&edge.from).copied().ok_or_else(|| {
                ConceptGraphError::InvalidStructure(format!(
                    "snapshot edge references unknown node '{}'",
                    edge.from
                ))
            })?;
            let to_idx = index.get(&edge.to).copied().ok_or_else(|| {
                ConceptGraphError::InvalidStructure(format!(
                    "snapshot edge references unknown node '{}'",
                    edge.to
                ))
            })?;
            graph.add_edge(from_idx, to_idx, ConceptEdge { kind: edge.kind });
        }

        Ok(ConceptGraph { graph, index })
    }

    // ── Query API ────────────────────────────────────────────────────────────

    /// Returns the total number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Returns the total number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Look up a node by its `unique_name`.  Returns `None` if not found.
    pub fn get_node(&self, unique_name: &str) -> Option<&ConceptNode> {
        let idx = self.index.get(unique_name)?;
        self.graph.node_weight(*idx)
    }

    /// Returns an iterator over the outgoing neighbors of `node_id` reachable via
    /// edges of exactly `kind`.  Each yielded item is a reference to the target
    /// [`ConceptNode`].
    ///
    /// Returns an empty iterator if `node_id` is unknown.
    pub fn neighbors<'g>(
        &'g self,
        node_id: &str,
        kind: EdgeKind,
    ) -> impl Iterator<Item = &'g ConceptNode> + 'g {
        let idx_opt = self.index.get(node_id).copied();
        let graph = &self.graph;

        let neighbors: Vec<&'g ConceptNode> = match idx_opt {
            None => vec![],
            Some(idx) => graph
                .edges(idx)
                .filter(move |e| e.weight().kind == kind)
                .filter_map(move |e| graph.node_weight(e.target()))
                .collect(),
        };

        neighbors.into_iter()
    }

    /// Returns an iterator over *all* nodes in the graph.
    pub fn nodes(&self) -> impl Iterator<Item = &ConceptNode> {
        self.graph.node_weights()
    }

    /// Returns an iterator over all edges as `(source_unique_name, target_unique_name, EdgeKind)`.
    pub fn edges(&self) -> impl Iterator<Item = (&str, &str, EdgeKind)> {
        self.graph.edge_references().filter_map(|e| {
            let src = self.graph.node_weight(e.source())?.unique_name.as_str();
            let tgt = self.graph.node_weight(e.target())?.unique_name.as_str();
            Some((src, tgt, e.weight().kind))
        })
    }

    // ── Serialization ────────────────────────────────────────────────────────

    /// Serialize the graph to a stable [`ConceptGraphSnapshot`] that can be
    /// converted to JSON with `serde_json::to_string`.
    pub fn to_snapshot(&self) -> ConceptGraphSnapshot {
        let nodes: Vec<ConceptNode> = self.graph.node_weights().cloned().collect();
        let edges: Vec<EdgeSnapshot> = self
            .graph
            .edge_references()
            .filter_map(|e| {
                let from = self.graph.node_weight(e.source())?.unique_name.clone();
                let to = self.graph.node_weight(e.target())?.unique_name.clone();
                Some(EdgeSnapshot {
                    from,
                    to,
                    kind: e.weight().kind,
                })
            })
            .collect();
        ConceptGraphSnapshot { nodes, edges }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Extract a string field from a JSON object, returning a [`ConceptGraphError`]
/// if the field is missing or not a string.
fn str_field(obj: &Value, field: &str) -> Result<String, ConceptGraphError> {
    obj.get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ConceptGraphError::InvalidStructure(format!(
                "expected string field '{field}' in {obj}"
            ))
        })
}

/// Parse an edge-kind string into an [`EdgeKind`] variant.
fn parse_edge_kind(s: &str) -> Result<EdgeKind, ConceptGraphError> {
    match s {
        "aggregates_via" | "AggregatesVia" => Ok(EdgeKind::AggregatesVia),
        "time_shifts" | "TimeShifts" => Ok(EdgeKind::TimeShifts),
        "filters_by" | "FiltersBy" => Ok(EdgeKind::FiltersBy),
        "derives_from" | "DerivesFrom" => Ok(EdgeKind::DerivesFrom),
        "related_to" | "RelatedTo" => Ok(EdgeKind::RelatedTo),
        other => Err(ConceptGraphError::InvalidStructure(format!(
            "unknown edge kind '{other}'"
        ))),
    }
}
