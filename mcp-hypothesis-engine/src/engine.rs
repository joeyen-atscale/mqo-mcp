// mcp-hypothesis-engine — core engine logic

use std::collections::{HashMap, HashSet, VecDeque};

use mcp_concept_graph::{ConceptGraph, EdgeKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// DatasetSummary mean extraction
// ---------------------------------------------------------------------------

pub fn extract_mean(summary: &Value, column: &str) -> Option<f64> {
    // Format 1: { "columns": { "<column>": { "mean": <f64> } } }
    if let Some(cols) = summary.get("columns") {
        if let Some(obj) = cols.as_object() {
            if let Some(col_val) = obj.get(column) {
                if let Some(mean) = col_val.get("mean").and_then(|v| v.as_f64()) {
                    return Some(mean);
                }
            }
        }
        // Format 2: { "columns": [ { "name": "<column>", "mean": <f64> } ] }
        if let Some(arr) = cols.as_array() {
            for entry in arr {
                let name = entry
                    .get("name")
                    .or_else(|| entry.get("unique_name"))
                    .and_then(|v| v.as_str());
                if name == Some(column) {
                    if let Some(mean) = entry.get("mean").and_then(|v| v.as_f64()) {
                        return Some(mean);
                    }
                }
            }
        }
    }
    // Format 3: flat top-level { "<column>": { "mean": <f64> } }
    if let Some(col_val) = summary.get(column) {
        if let Some(mean) = col_val.get("mean").and_then(|v| v.as_f64()) {
            return Some(mean);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// BFS over inbound causal edges
// ---------------------------------------------------------------------------

fn is_causal(kind: &EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::DerivesFrom | EdgeKind::AggregatesVia | EdgeKind::FiltersBy
    )
}

/// A single candidate derivation path from target → leaf component.
#[derive(Debug, Clone)]
pub struct CandidatePath {
    /// Node ids from target (inclusive) to leaf (inclusive).
    pub path: Vec<String>,
    /// Edge kinds along the path (length = path.len() - 1).
    pub edge_kinds: Vec<EdgeKind>,
}

/// BFS over outgoing causal edges from the target, following the derivation
/// tree (e.g. TSS --DerivesFrom--> SSA means TSS derives from SSA; the edge
/// points outward from the metric to its components).
/// Returns all paths up to `max_depth` hops (path = [target, component, ...]).
pub fn bfs_inbound(graph: &ConceptGraph, target: &str, max_depth: u8) -> Vec<CandidatePath> {
    if max_depth == 0 {
        return vec![];
    }

    struct Frame {
        node: String,
        path: Vec<String>,
        kinds: Vec<EdgeKind>,
        visited: HashSet<String>,
    }

    let mut queue: VecDeque<Frame> = VecDeque::new();
    let mut init_visited = HashSet::new();
    init_visited.insert(target.to_string());
    queue.push_back(Frame {
        node: target.to_string(),
        path: vec![target.to_string()],
        kinds: vec![],
        visited: init_visited,
    });

    let mut results: Vec<CandidatePath> = vec![];

    while let Some(Frame { node, path, kinds, visited }) = queue.pop_front() {
        let depth = path.len() as u8 - 1;
        // Follow outgoing causal edges: target -> component
        for edge in graph.edges_from(&node) {
            if !is_causal(&edge.kind) {
                continue;
            }
            let dst = &edge.to;
            if visited.contains(dst) {
                continue;
            }
            let mut new_path = path.clone();
            new_path.push(dst.clone());
            let mut new_kinds = kinds.clone();
            new_kinds.push(edge.kind.clone());
            let mut new_visited = visited.clone();
            new_visited.insert(dst.clone());

            results.push(CandidatePath {
                path: new_path.clone(),
                edge_kinds: new_kinds.clone(),
            });

            if depth + 1 < max_depth {
                queue.push_back(Frame {
                    node: dst.clone(),
                    path: new_path,
                    kinds: new_kinds,
                    visited: new_visited,
                });
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Corroboration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corroboration {
    Corroborated,
    StructuralOnly,
}

pub fn compute_delta(mean_a: f64, mean_b: f64) -> f64 {
    if mean_a == 0.0 {
        if mean_b == 0.0 { 0.0 } else { f64::INFINITY }
    } else {
        (mean_b - mean_a) / mean_a.abs()
    }
}

fn same_direction(a: f64, b: f64) -> bool {
    a != 0.0 && b != 0.0 && a.signum() == b.signum()
}

// ---------------------------------------------------------------------------
// Confidence
// ---------------------------------------------------------------------------

pub fn confidence(corroboration: &Corroboration, depth: usize) -> &'static str {
    match (corroboration, depth) {
        (Corroboration::Corroborated, d) if d <= 2 => "high",
        (Corroboration::Corroborated, _) => "medium",
        (Corroboration::StructuralOnly, d) if d <= 2 => "medium",
        _ => "low",
    }
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Hypothesis {
    pub rank: usize,
    pub explanation: String,
    pub path: Vec<String>,
    pub path_edge_kinds: Vec<String>,
    pub corroboration: Corroboration,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_delta_fraction: Option<f64>,
    pub probe_mqo: Value,
    pub confidence: String,
}

#[derive(Debug, Serialize)]
pub struct HypothesisSet {
    pub target: String,
    pub target_delta_fraction: f64,
    pub evidence_type: String,
    pub analysis_note: String,
    pub hypotheses: Vec<Hypothesis>,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub fn run_engine(
    graph: &ConceptGraph,
    target: &str,
    target_delta: f64,
    summary_a: &Value,
    summary_b: &Value,
    max_depth: u8,
    top_k: usize,
) -> HypothesisSet {
    let paths = bfs_inbound(graph, target, max_depth);

    let mut delta_cache: HashMap<String, Option<f64>> = HashMap::new();
    let mut candidates: Vec<(CandidatePath, Corroboration, Option<f64>)> = Vec::new();

    for cpath in paths {
        let component = cpath.path.last().unwrap().clone();

        let comp_delta = *delta_cache.entry(component.clone()).or_insert_with(|| {
            let mean_a = extract_mean(summary_a, &component);
            let mean_b = extract_mean(summary_b, &component);
            match (mean_a, mean_b) {
                (Some(a), Some(b)) => Some(compute_delta(a, b)),
                _ => None,
            }
        });

        let corroboration = match comp_delta {
            Some(d) if same_direction(d, target_delta) => Corroboration::Corroborated,
            _ => Corroboration::StructuralOnly,
        };

        candidates.push((cpath, corroboration, comp_delta));
    }

    // Sort: corroborated first, larger |delta| first, shorter path first
    candidates.sort_by(|(pa, ca, da), (pb, cb, db)| {
        let ca_ord = if *ca == Corroboration::Corroborated { 0u8 } else { 1u8 };
        let cb_ord = if *cb == Corroboration::Corroborated { 0u8 } else { 1u8 };
        if ca_ord != cb_ord {
            return ca_ord.cmp(&cb_ord);
        }
        let mag_a = da.map(|d| d.abs()).unwrap_or(0.0);
        let mag_b = db.map(|d| d.abs()).unwrap_or(0.0);
        if (mag_b - mag_a).abs() > 1e-12 {
            return mag_b.partial_cmp(&mag_a).unwrap_or(std::cmp::Ordering::Equal);
        }
        pa.path.len().cmp(&pb.path.len())
    });

    let hypotheses: Vec<Hypothesis> = candidates
        .into_iter()
        .take(top_k)
        .enumerate()
        .map(|(i, (cpath, corroboration, comp_delta))| {
            let component = cpath.path.last().unwrap().clone();
            let depth = cpath.path.len() - 1;
            let edge_strs: Vec<String> = cpath
                .edge_kinds
                .iter()
                .map(|k| edge_kind_str(k).to_string())
                .collect();

            let direction_word = if target_delta < 0.0 { "fell" } else { "rose" };
            let comp_direction = match comp_delta {
                Some(d) if d < 0.0 => "fell",
                Some(d) if d > 0.0 => "rose",
                _ => "changed",
            };

            let explanation = match comp_delta {
                Some(d) => format!(
                    "{} {} because component {} {} {:.1}%",
                    target,
                    direction_word,
                    component,
                    comp_direction,
                    d.abs() * 100.0
                ),
                None => format!(
                    "{} {} because component {} is a structural derivation source (no data delta available)",
                    target, direction_word, component
                ),
            };

            let probe_mqo = json!({
                "measures": [{ "unique_name": component }],
                "dimensions": [],
                "filters": []
            });

            let conf = confidence(&corroboration, depth).to_string();

            Hypothesis {
                rank: i + 1,
                explanation,
                path: cpath.path,
                path_edge_kinds: edge_strs,
                corroboration,
                component_delta_fraction: comp_delta,
                probe_mqo,
                confidence: conf,
            }
        })
        .collect();

    HypothesisSet {
        target: target.to_string(),
        target_delta_fraction: round6(target_delta),
        evidence_type: "structural".to_string(),
        analysis_note: "Hypotheses are structural derivation paths with probe queries. Statistical causation requires additional analysis.".to_string(),
        hypotheses,
    }
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

pub fn edge_kind_str(k: &EdgeKind) -> &'static str {
    match k {
        EdgeKind::DerivesFrom => "DerivesFrom",
        EdgeKind::AggregatesVia => "AggregatesVia",
        EdgeKind::FiltersBy => "FiltersBy",
        EdgeKind::TimeShifts => "TimeShifts",
        EdgeKind::RelatedTo => "RelatedTo",
        EdgeKind::LevelOf => "LevelOf",
        EdgeKind::ParentOf => "ParentOf",
    }
}
