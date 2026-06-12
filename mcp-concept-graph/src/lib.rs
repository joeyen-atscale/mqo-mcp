// mcp-concept-graph — attributed property graph of the AtScale semantic model.
//
// Derived from describe_model JSON; no network; no external graph DB.
// Pure Rust, pure in-memory adjacency list with typed edges.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("invalid describe_model JSON: {0}")]
    InvalidJson(String),
    #[error("JSON parse error: {0}")]
    ParseError(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// NodeKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Measure,
    DimensionLevel,
    Hierarchy,
    Calc,
    DateRole,
}

// ---------------------------------------------------------------------------
// EdgeKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    AggregatesVia,
    DerivesFrom,
    TimeShifts,
    FiltersBy,
    RelatedTo,
    LevelOf,
    ParentOf,
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
    pub model_name: String,
    pub cluster_name: Option<String>,
    /// Arbitrary extra attributes: expression, format_string, folder, etc.
    pub attributes: HashMap<String, Value>,
}

impl Node {
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: NodeKind) -> Self {
        Node {
            id: id.into(),
            label: label.into(),
            kind,
            model_name: String::new(),
            cluster_name: None,
            attributes: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Edge
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    /// 1.0 by default; used for path ranking (lower = preferred).
    pub weight: f32,
}

impl Edge {
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: EdgeKind) -> Self {
        Edge {
            from: from.into(),
            to: to.into(),
            kind,
            weight: 1.0,
        }
    }

    pub fn with_weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }
}

// ---------------------------------------------------------------------------
// ConceptGraph
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConceptGraph {
    /// All nodes keyed by id.
    nodes: HashMap<String, Node>,
    /// Outgoing adjacency: from_id → edges.
    adj: HashMap<String, Vec<Edge>>,
    /// Incoming adjacency: to_id → edges.
    rev: HashMap<String, Vec<Edge>>,
}

impl ConceptGraph {
    pub fn new() -> Self {
        ConceptGraph::default()
    }

    // -----------------------------------------------------------------------
    // Construction helpers
    // -----------------------------------------------------------------------

    pub fn add_node(&mut self, node: Node) {
        let id = node.id.clone();
        self.nodes.insert(id.clone(), node);
        self.adj.entry(id.clone()).or_default();
        self.rev.entry(id).or_default();
    }

    pub fn add_edge(&mut self, edge: Edge) {
        // Ensure both endpoints are registered (even as phantom nodes).
        self.adj.entry(edge.from.clone()).or_default();
        self.rev.entry(edge.from.clone()).or_default();
        self.adj.entry(edge.to.clone()).or_default();
        self.rev.entry(edge.to.clone()).or_default();

        // Avoid duplicate edges.
        let adj_vec = self.adj.entry(edge.from.clone()).or_default();
        let already = adj_vec
            .iter()
            .any(|e| e.to == edge.to && e.kind == edge.kind);
        if !already {
            let rev_edge = edge.clone();
            adj_vec.push(edge.clone());
            self.rev.entry(edge.to.clone()).or_default().push(rev_edge);
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn edges_from(&self, id: &str) -> &[Edge] {
        self.adj.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn edges_to(&self, id: &str) -> &[Edge] {
        self.rev.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn nodes(&self) -> Vec<&Node> {
        self.nodes.values().collect()
    }

    pub fn edges(&self) -> Vec<&Edge> {
        self.adj.values().flatten().collect()
    }

    pub fn nodes_by_kind(&self, kind: NodeKind) -> Vec<&Node> {
        self.nodes.values().filter(|n| n.kind == kind).collect()
    }

    /// All nodes reachable in one hop, optionally filtered by edge kind.
    pub fn neighbors(&self, id: &str, kind: Option<EdgeKind>) -> Vec<&Node> {
        self.edges_from(id)
            .iter()
            .filter(|e| kind.as_ref().map_or(true, |k| &e.kind == k))
            .filter_map(|e| self.nodes.get(&e.to))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Traversal
    // -----------------------------------------------------------------------

    /// BFS up to k hops; returns all unique nodes reachable within k hops
    /// (excluding the source node itself).
    pub fn k_hop_neighbors(&self, id: &str, k: u8) -> Vec<&Node> {
        if k == 0 {
            return vec![];
        }
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(id.to_string());
        let mut frontier: Vec<String> = vec![id.to_string()];
        let mut result_ids: Vec<String> = Vec::new();

        for _ in 0..k {
            let mut next_frontier: Vec<String> = Vec::new();
            for node_id in &frontier {
                for e in self.edges_from(node_id) {
                    if visited.insert(e.to.clone()) {
                        result_ids.push(e.to.clone());
                        next_frontier.push(e.to.clone());
                    }
                }
            }
            frontier = next_frontier;
            if frontier.is_empty() {
                break;
            }
        }

        result_ids
            .iter()
            .filter_map(|rid| self.nodes.get(rid))
            .collect()
    }

    /// BFS shortest path (unweighted). Returns the sequence of node ids
    /// from `from` to `to` inclusive, or `None` if unreachable.
    pub fn shortest_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        if from == to {
            return Some(vec![from.to_string()]);
        }
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(from.to_string());
        let mut queue: VecDeque<Vec<String>> = VecDeque::new();
        queue.push_back(vec![from.to_string()]);

        while let Some(path) = queue.pop_front() {
            let current = path.last().unwrap();
            for e in self.edges_from(current) {
                if e.to == to {
                    let mut full_path = path.clone();
                    full_path.push(e.to.clone());
                    return Some(full_path);
                }
                if visited.insert(e.to.clone()) {
                    let mut new_path = path.clone();
                    new_path.push(e.to.clone());
                    queue.push_back(new_path);
                }
            }
        }
        None
    }

    /// Induced subgraph over the given node ids.
    /// Only includes edges where both endpoints are in `ids`.
    pub fn subgraph(&self, ids: &[&str]) -> ConceptGraph {
        let id_set: HashSet<&str> = ids.iter().copied().collect();
        let mut g = ConceptGraph::new();

        for &id in &id_set {
            if let Some(n) = self.nodes.get(id) {
                g.add_node(n.clone());
            }
        }
        for &id in &id_set {
            for e in self.edges_from(id) {
                if id_set.contains(e.to.as_str()) {
                    g.add_edge(e.clone());
                }
            }
        }
        g
    }

    // -----------------------------------------------------------------------
    // JSON round-trip (independent from from_describe_model)
    // -----------------------------------------------------------------------

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "nodes": self.nodes.values().collect::<Vec<_>>(),
            "edges": self.adj.values().flatten().collect::<Vec<_>>(),
        })
    }

    pub fn from_json(json: &Value) -> Self {
        let mut g = ConceptGraph::new();

        if let Some(nodes) = json.get("nodes").and_then(|v| v.as_array()) {
            for n in nodes {
                if let Ok(node) = serde_json::from_value::<Node>(n.clone()) {
                    g.add_node(node);
                }
            }
        }
        if let Some(edges) = json.get("edges").and_then(|v| v.as_array()) {
            for e in edges {
                if let Ok(edge) = serde_json::from_value::<Edge>(e.clone()) {
                    g.add_edge(edge);
                }
            }
        }
        g
    }

    // -----------------------------------------------------------------------
    // from_describe_model
    // -----------------------------------------------------------------------

    /// Build a ConceptGraph from a describe_model JSON response.
    ///
    /// Parsing rules:
    /// - measures[]       → Node { kind: Measure }
    /// - dimensions[]     → hierarchy nodes + level nodes;
    ///                      LevelOf edges (level → hierarchy),
    ///                      ParentOf edges (parent level → child level)
    /// - calcs / calculated_members → Node { kind: Calc };
    ///                      DerivesFrom edges for [Measures].[X] refs
    /// - date_roles / time_dims → Node { kind: DateRole };
    ///                      TimeShifts edges to associated measures
    /// - folder co-location → bidirectional RelatedTo edges (weight 0.5)
    pub fn from_describe_model(json: &Value) -> Result<Self, GraphError> {
        let mut g = ConceptGraph::new();

        let model_name = json
            .get("name")
            .or_else(|| json.get("model_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // ---- measures ----
        let mut measure_by_name: HashMap<String, String> = HashMap::new(); // display_name → unique_name
        if let Some(measures) = json.get("measures").and_then(|v| v.as_array()) {
            for m in measures {
                let unique_name = str_field(m, "unique_name");
                let label = str_field(m, "name").or_else(|| unique_name.clone());
                let (unique_name, label) = match (unique_name, label) {
                    (Some(u), Some(l)) => (u, l),
                    _ => continue,
                };
                let mut node = Node::new(&unique_name, &label, NodeKind::Measure);
                node.model_name = model_name.clone();
                if let Some(expr) = str_field(m, "expression") {
                    node.attributes.insert("expression".into(), Value::String(expr));
                }
                if let Some(folder) = str_field(m, "folder") {
                    node.attributes.insert("folder".into(), Value::String(folder));
                }
                if let Some(fmt) = str_field(m, "format_string") {
                    node.attributes.insert("format_string".into(), Value::String(fmt));
                }
                measure_by_name.insert(label.clone(), unique_name.clone());
                g.add_node(node);
            }
        }

        // ---- dimensions → hierarchies → levels ----
        if let Some(dimensions) = json.get("dimensions").and_then(|v| v.as_array()) {
            for dim in dimensions {
                let hierarchies = dim
                    .get("hierarchies")
                    .and_then(|v| v.as_array())
                    .map(|a| a.as_slice())
                    .unwrap_or(&[]);

                // If no explicit hierarchies array, treat the dimension itself as a hierarchy.
                let hierarchy_items: Vec<&Value>;
                let single;
                let hier_slice: &[&Value] = if hierarchies.is_empty() {
                    single = vec![dim];
                    &single
                } else {
                    hierarchy_items = hierarchies.iter().collect();
                    &hierarchy_items
                };

                for hier in hier_slice {
                    let hier_unique_name = str_field(hier, "unique_name");
                    let hier_label = str_field(hier, "name").or_else(|| hier_unique_name.clone());
                    let (hier_id, hier_label) = match (hier_unique_name, hier_label) {
                        (Some(u), Some(l)) => (u, l),
                        _ => continue,
                    };

                    let mut hier_node = Node::new(&hier_id, &hier_label, NodeKind::Hierarchy);
                    hier_node.model_name = model_name.clone();
                    g.add_node(hier_node);

                    // Levels within the hierarchy.
                    let levels = hier
                        .get("levels")
                        .and_then(|v| v.as_array())
                        .map(|a| a.as_slice())
                        .unwrap_or(&[]);

                    let mut prev_level_id: Option<String> = None;
                    for level in levels {
                        let level_unique_name = str_field(level, "unique_name");
                        let level_label =
                            str_field(level, "name").or_else(|| level_unique_name.clone());
                        let (level_id, level_label) = match (level_unique_name, level_label) {
                            (Some(u), Some(l)) => (u, l),
                            _ => continue,
                        };

                        let mut level_node =
                            Node::new(&level_id, &level_label, NodeKind::DimensionLevel);
                        level_node.model_name = model_name.clone();
                        g.add_node(level_node);

                        // LevelOf: level → hierarchy
                        g.add_edge(Edge::new(&level_id, &hier_id, EdgeKind::LevelOf));

                        // ParentOf: previous level → this level
                        if let Some(prev) = prev_level_id.take() {
                            g.add_edge(Edge::new(&prev, &level_id, EdgeKind::ParentOf));
                        }
                        prev_level_id = Some(level_id);
                    }
                }
            }
        }

        // ---- calcs / calculated_members ----
        let calcs_iter = json
            .get("calculated_members")
            .or_else(|| json.get("calcs"))
            .and_then(|v| v.as_array());
        if let Some(calcs) = calcs_iter {
            for calc in calcs {
                let unique_name = str_field(calc, "unique_name");
                let label = str_field(calc, "name").or_else(|| unique_name.clone());
                let (calc_id, calc_label) = match (unique_name, label) {
                    (Some(u), Some(l)) => (u, l),
                    _ => continue,
                };
                let expr = str_field(calc, "expression");
                let mut node = Node::new(&calc_id, &calc_label, NodeKind::Calc);
                node.model_name = model_name.clone();
                if let Some(ref e) = expr {
                    node.attributes.insert("expression".into(), Value::String(e.clone()));
                }
                g.add_node(node);

                // DerivesFrom edges: parse [Measures].[X] patterns.
                if let Some(ref expression) = expr {
                    for measure_name in extract_measure_refs(expression) {
                        // Try to find by display name first, then direct match.
                        let target_id = measure_by_name
                            .get(&measure_name)
                            .cloned()
                            .unwrap_or_else(|| measure_name.clone());
                        if g.nodes.contains_key(&target_id) {
                            g.add_edge(Edge::new(&calc_id, &target_id, EdgeKind::DerivesFrom));
                        }
                    }
                }
            }
        }

        // ---- date_roles / time_dims ----
        let date_roles_iter = json
            .get("date_roles")
            .or_else(|| json.get("time_dims"))
            .and_then(|v| v.as_array());
        if let Some(date_roles) = date_roles_iter {
            for dr in date_roles {
                let unique_name = str_field(dr, "unique_name");
                let label = str_field(dr, "name").or_else(|| unique_name.clone());
                let (dr_id, dr_label) = match (unique_name, label) {
                    (Some(u), Some(l)) => (u, l),
                    _ => continue,
                };
                let mut node = Node::new(&dr_id, &dr_label, NodeKind::DateRole);
                node.model_name = model_name.clone();
                g.add_node(node);

                // TimeShifts → associated measures.
                if let Some(assocs) = dr.get("measure_associations").and_then(|v| v.as_array()) {
                    for assoc in assocs {
                        let measure_id = assoc
                            .as_str()
                            .map(String::from)
                            .or_else(|| str_field(assoc, "unique_name"))
                            .or_else(|| str_field(assoc, "name"));
                        if let Some(mid) = measure_id {
                            let target = measure_by_name.get(&mid).cloned().unwrap_or(mid);
                            if g.nodes.contains_key(&target) {
                                g.add_edge(Edge::new(&dr_id, &target, EdgeKind::TimeShifts));
                            }
                        }
                    }
                }
            }
        }

        // ---- folder co-location → bidirectional RelatedTo (weight 0.5) ----
        let mut folder_map: HashMap<String, Vec<String>> = HashMap::new();
        for node in g.nodes.values() {
            if let Some(Value::String(folder)) = node.attributes.get("folder") {
                if !folder.is_empty() {
                    folder_map
                        .entry(folder.clone())
                        .or_default()
                        .push(node.id.clone());
                }
            }
        }
        for (_folder, members) in &folder_map {
            for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let a = &members[i];
                    let b = &members[j];
                    g.add_edge(
                        Edge::new(a, b, EdgeKind::RelatedTo).with_weight(0.5),
                    );
                    g.add_edge(
                        Edge::new(b, a, EdgeKind::RelatedTo).with_weight(0.5),
                    );
                }
            }
        }

        Ok(g)
    }

    /// Convenience wrapper that parses a JSON string first.
    pub fn from_describe_model_str(s: &str) -> Result<Self, GraphError> {
        let json: Value = serde_json::from_str(s)?;
        Self::from_describe_model(&json)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract a string field from a JSON object.
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(String::from)
}

/// Parse `[Measures].[X]` patterns from a calc expression.
/// Returns the display name X (not the full MDX reference).
fn extract_measure_refs(expression: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let pat = "[Measures].[";
    let mut search = expression;
    while let Some(start) = search.find(pat) {
        let rest = &search[start + pat.len()..];
        if let Some(end) = rest.find(']') {
            refs.push(rest[..end].to_string());
        }
        search = &search[start + pat.len()..];
    }
    refs
}
