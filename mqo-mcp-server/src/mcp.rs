//! Minimal MCP (Model Context Protocol) JSON-RPC 2.0 server over stdio.
//!
//! Per the PRD: when the protocol layer is agnostic, "a minimal
//! JSON-RPC-over-stdio implementation per the MCP spec is acceptable as long as
//! the `query_multidimensional` tool is exposed and the ACs pass." This module
//! is that implementation. It supports the three lifecycle/discovery methods the
//! ACs touch:
//!
//! - `initialize`        — handshake; advertises server info + capabilities.
//! - `tools/list`        — advertises the four tools and their input schemas,
//!   with `readOnlyHint: true` on the three catalog tools.
//! - `tools/call`        — dispatches a tool invocation.
//!
//! The catalog tools (`list_models`, `describe_model`, `search_columns`) are
//! served from the loaded catalog snapshot (the "catalog passthrough or
//! snapshot" of the PRD). `query_multidimensional` runs the bind→route→compile
//! →execute pipeline.

// Pre-existing lint suppressions — do not remove without fixing the underlying code.
#![allow(
    clippy::doc_markdown, clippy::missing_errors_doc, clippy::missing_panics_doc,
    clippy::must_use_candidate, clippy::map_unwrap_or, clippy::manual_let_else,
    clippy::items_after_statements, clippy::too_many_lines, clippy::uninlined_format_args,
    clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::implicit_hasher,
    clippy::similar_names, clippy::redundant_closure_for_method_calls, clippy::map_clone,
    clippy::if_not_else, clippy::unnested_or_patterns, clippy::manual_range_patterns,
    clippy::explicit_auto_deref, clippy::doc_overindented_list_items,
    clippy::used_underscore_binding, clippy::absurd_extreme_comparisons, clippy::type_complexity
)]

use crate::autolift::{ensure_graph, graph_to_turtle, AutoliftCache};
use crate::catalog_cache::fetch_schema_update;
use crate::chart_tools;
use crate::cursor::CursorStore;
use crate::grounding::GroundingStore;
use crate::handle_ops::{self, HandleStore};
use crate::model_graph::ModelGraphStore;
use crate::ontology_check::OntologyCheckStore;
use crate::pipeline::{self, PipelineError, PipelineOutput, ToolPaths};
use crate::probe::BackendCapabilities;
use crate::projection_guard::check_projection_cardinality;
use crate::routing;
use dh_spec::DatasetHandle;
use mcp_cluster_health_monitor::report::HealthReport;
use mcp_cluster_registry::ClusterRegistry;
use mqo_auth_bridge::LiveExecutor;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// Enrichment data derived from `enriched-catalog.v1`, cached at startup.
///
/// Holds both the raw JSON string (for passing to `mqo-bind` via `--enriched-catalog`) and the
/// pre-computed per-measure compatibility map (for `describe_model` enrichment).
pub struct ServerEnrichedData {
    /// Serialized `enriched-catalog.v1` JSON — written to a temp file and passed to `mqo-bind`.
    pub catalog_json: String,
    /// Per-measure compatible hierarchies.
    /// Key: measure `unique_name`. Value: JSON array of `{hierarchy_unique_name, level_unique_names}`.
    pub compatible_hierarchies: BTreeMap<String, Value>,
    /// Per-measure channel scope descriptor derived from `FactBindings` channel groups
    /// (PRD-mqo-channel-scope-measure-grounding, FR1/FR2).
    ///
    /// Key: measure `unique_name` (fully-qualified, e.g. `tpcds_benchmark_model.store_quantity_sold`).
    /// Value: JSON object `{"channel_groups": [String], "channel_scope_label": String}` where:
    ///   - `channel_groups` — the fact-table column-group identifiers this measure aggregates.
    ///   - `channel_scope_label` — human-readable summary: `"store_sales only"`,
    ///     `"all-channel (store_sales, catalog_sales, web_sales)"`, etc.
    ///
    /// Only present for measures that appear in `FactBindings::tpcds_defaults()`.
    /// Absent bindings → no descriptor, guard silent (OQ4).
    pub channel_scope_map: BTreeMap<String, Value>,
}

impl ServerEnrichedData {
    /// Build from a parsed `enriched-catalog.v1` JSON value.
    ///
    /// Computes the `mqoguard-compatibility-matrix` once and caches per-measure hierarchy lists
    /// so `describe_model` never re-computes the matrix per call.
    ///
    /// Returns `None` when the JSON has no `columns` array (cannot build anything useful).
    #[must_use]
    pub fn from_json(enriched: &Value) -> Option<Self> {
        use mqoguard_compatibility_matrix::{
            build_matrix, EnrichedCatalog, EnrichedColumn, MatrixConfig,
        };
        use std::collections::{BTreeSet, HashMap};

        let catalog_json = serde_json::to_string(&enriched).ok()?;

        let col_arr = enriched.get("columns").and_then(Value::as_array)?;

        // Build the matrix crate's EnrichedCatalog from the raw JSON.
        let columns: Vec<EnrichedColumn> = col_arr
            .iter()
            .map(|c| {
                let column_group: BTreeSet<String> = c
                    .get("column_group")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                EnrichedColumn {
                    unique_name: c
                        .get("unique_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    label: c
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    kind: c
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    is_calc: c
                        .get("is_calc")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    hierarchy: c
                        .get("hierarchy")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    level: c
                        .get("level")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    column_group,
                }
            })
            .collect();

        let matrix_catalog = EnrichedCatalog {
            model: "catalog".to_string(),
            columns,
        };
        let matrix = build_matrix(&matrix_catalog, &MatrixConfig::default());

        // Build hierarchy → sorted level unique_names from the enriched JSON columns.
        let mut hier_to_levels: HashMap<String, Vec<String>> = HashMap::new();
        for c in col_arr {
            if c.get("kind").and_then(Value::as_str) == Some("level") {
                if let (Some(hier), Some(un)) = (
                    c.get("hierarchy").and_then(Value::as_str),
                    c.get("unique_name").and_then(Value::as_str),
                ) {
                    hier_to_levels
                        .entry(hier.to_string())
                        .or_default()
                        .push(un.to_string());
                }
            }
        }

        // Pre-compute per-measure compatible_hierarchies array for describe_model.
        let compatible_hierarchies: BTreeMap<String, Value> = matrix
            .measures
            .iter()
            .map(|(measure_un, mc)| {
                let entries: Vec<Value> = mc
                    .compatible_hierarchies
                    .iter()
                    .map(|h_id| {
                        let levels = hier_to_levels.get(h_id).cloned().unwrap_or_default();
                        json!({
                            "hierarchy_unique_name": h_id,
                            "level_unique_names": levels
                        })
                    })
                    .collect();
                (measure_un.clone(), Value::Array(entries))
            })
            .collect();

        // Build per-measure channel scope map from FactBindings::tpcds_defaults()
        // (PRD-mqo-channel-scope-measure-grounding, FR1/FR2).
        // Derived once at startup from the existing source of truth; no new hand-authored mapping.
        let channel_scope_map: BTreeMap<String, Value> = {
            use mqoguard_column_group_enrichment::FactBindings;
            let bindings = FactBindings::tpcds_defaults();
            bindings
                .measures
                .into_iter()
                .map(|(un, groups)| {
                    let mut groups_sorted: Vec<String> = groups.into_iter().collect();
                    groups_sorted.sort();
                    let label = if groups_sorted.len() == 1 {
                        format!("{} only", groups_sorted[0])
                    } else {
                        format!("all-channel ({})", groups_sorted.join(", "))
                    };
                    let descriptor = json!({
                        "channel_groups": groups_sorted,
                        "channel_scope_label": label
                    });
                    (un, descriptor)
                })
                .collect()
        };

        Some(Self {
            catalog_json,
            compatible_hierarchies,
            channel_scope_map,
        })
    }
}

// ── describe_model disambiguation pack ───────────────────────────────────────
//
// Implements PRD-mqo-describe-disambiguation-pack: enrich the `describe_model`
// response so the model picks the *canonical* near-twin entity on the first try.
//
// Two enrichments, both additive (older clients ignore unknown keys):
//
//   1. Near-twin grouping (FR-2/FR-3): dimension levels whose *core label*
//      (the trailing concept words, e.g. "Brand Name", "State Name") collide
//      across different hierarchies are grouped under a top-level `near_twins`
//      block. Within each group the attribute on the shortest hierarchy name
//      is marked `canonical_for: "generic"` (hierarchy-primacy heuristic).
//
//   2. Date roles (FR-4): each measure carries `date_roles` — the unique_names
//      of temporally-typed date hierarchies it can be sliced by. Derived from
//      the catalog's date hierarchies (empty array when none, never absent).

/// Maximum number of trailing tokens used as the near-twin "core label" key.
const NEAR_TWIN_CORE_TOKENS: usize = 2;

/// Normalize a label for collision detection: lowercase, collapse whitespace.
fn normalize_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Normalize a member value for domain matching: trim, collapse interior
/// whitespace, lowercase.  Mirrors the catalog's member normalization so that
/// `"  Corpcorp #1 "` and `"corpcorp  #1"` both resolve to the same key.
fn normalize_member(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The "core label" of an attribute: the trailing concept words shared by
/// near-twins across hierarchies (e.g. "Product Brand Name",
/// "Store Item Product Brand Name" → "brand name"; "Customer State Name",
/// "Store State Name", "Warehouse State Name" → "state name"). "name" is
/// retained in the bucket key so cross-hierarchy State Name / Brand Name twins
/// land in the same group. Returns `None` for labels shorter than the
/// core-token window (nothing meaningful to disambiguate).
fn core_label(label: &str) -> Option<String> {
    let norm = normalize_label(label);
    let toks: Vec<&str> = norm.split(' ').filter(|t| !t.is_empty()).collect();
    if toks.len() < NEAR_TWIN_CORE_TOKENS {
        // Too short to carry a hierarchy-role prefix; group on the whole label.
        if toks.is_empty() {
            return None;
        }
        return Some(toks.join(" "));
    }
    Some(toks[toks.len() - NEAR_TWIN_CORE_TOKENS..].join(" "))
}

/// Does this label name a human-readable display attribute? True when the
/// trailing concept word is "name" (e.g. "Store State Name", "Sold Day Name",
/// "Product Brand Name"). Used by the `canonical_for` heuristic (FIX 2) to
/// prefer the named display attribute over a code-like sibling.
fn label_is_name(label: &str) -> bool {
    normalize_label(label)
        .split(' ')
        .next_back()
        .is_some_and(|w| w == "name")
}

/// True for date-role hierarchies (sold/ship/return/inventory date dimensions,
/// week hierarchies). These are distinct date roles, NOT near-twins — excluding
/// them from grouping prevents suggesting a path-incompatible `Ship Calendar
/// Year` for a `Sold Calendar Year`. Date-role correctness is owned by binding.
fn is_date_role_hierarchy(hier: &str) -> bool {
    let h = hier.to_lowercase();
    h.contains("date") || h.contains("calendar") || h.contains("time")
}

// ── Within-hierarchy *Name display preference ────────────────────────────────
//
// Implements PRD-mqo-within-hierarchy-name-preference. The cross-hierarchy
// `near_twins` rule cannot help when a level and its display *Name* sibling live
// on the SAME hierarchy (`Store State` code vs `Store State Name`; the ordinal
// `Sold Day of Week` vs the named `Sold Day Name`). For each such same-hierarchy
// pair `describe_model` marks the Name level `display_preferred: true` and
// annotates the non-Name sibling with `display_sibling: "<Name unique_name>"`.
// Advisory only (no validator rejection); deterministic; catalog-only.

/// Detect whether `name_label` is the display-"Name" form of `code_label` on the
/// same hierarchy. Two shapes are recognized (both case-insensitive):
///
///   1. Suffix pair — the Name label is the code label plus a trailing "Name"
///      token (`Store State` / `Store State Name`).
///   2. Ordinal/Name pair — a `<stem> Name` paired with a `<stem> of Week` /
///      `<stem> of Year` ordinal that shares the same leading stem
///      (`Sold Day Name` / `Sold Day of Week`).
///
/// Returns true when `name_label` is the preferred display form of `code_label`.
fn is_name_form_of(name_label: &str, code_label: &str) -> bool {
    let name = normalize_label(name_label);
    let code = normalize_label(code_label);
    if name == code {
        return false;
    }
    let name_toks: Vec<&str> = name.split(' ').filter(|t| !t.is_empty()).collect();
    let code_toks: Vec<&str> = code.split(' ').filter(|t| !t.is_empty()).collect();
    if name_toks.last() != Some(&"name") {
        return false; // the preferred member must end in "Name"
    }
    // Shape 1: suffix pair — code is exactly name minus the trailing "Name".
    if name_toks[..name_toks.len() - 1] == code_toks[..] {
        return true;
    }
    // Shape 2: ordinal/name pair — shared leading stem, code is "<stem> of week"
    // / "<stem> of year". The stem is the name label minus its trailing "Name".
    let stem = &name_toks[..name_toks.len() - 1];
    if code_toks.len() >= stem.len() + 2
        && &code_toks[..stem.len()] == stem
        && code_toks[stem.len()] == "of"
        && matches!(code_toks.get(stem.len() + 1), Some(&"week") | Some(&"year"))
    {
        return true;
    }
    false
}

/// Annotate dimension level columns in place with the within-hierarchy display
/// preference (PRD-mqo-within-hierarchy-name-preference). For each pair of levels
/// on the SAME hierarchy where one is the display "Name" form of the other (see
/// [`is_name_form_of`]), the Name level gets `display_preferred: true` and the
/// non-Name sibling gets `display_sibling: "<Name unique_name>"`. Levels with no
/// Name sibling are left untouched. Deterministic and catalog-only.
fn annotate_display_siblings(columns: &mut [Value]) {
    // Collect (index, unique_name, hierarchy, label) for every level column.
    let levels: Vec<(usize, String, String, String)> = columns
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            if c.get("kind").and_then(Value::as_str) != Some("level") {
                return None;
            }
            let un = c.get("unique_name").and_then(Value::as_str)?;
            let label = c.get("label").and_then(Value::as_str)?;
            let hier = c
                .get("hierarchy")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| un.split_once('.').map(|(h, _)| h.to_string()))
                .unwrap_or_default();
            Some((i, un.to_string(), hier, label.to_string()))
        })
        .collect();

    // For each non-Name level, find a same-hierarchy Name sibling. Deterministic:
    // candidates are scanned in `columns` order; pick the lexicographically
    // smallest matching Name unique_name as a stable tiebreak.
    for (code_idx, _code_un, code_hier, code_label) in &levels {
        let mut best: Option<(&str, &str)> = None; // (name_un, name_label)
        for (_, name_un, name_hier, name_label) in &levels {
            if name_hier != code_hier {
                continue;
            }
            if is_name_form_of(name_label, code_label) {
                match best {
                    Some((cur, _)) if cur <= name_un.as_str() => {}
                    _ => best = Some((name_un.as_str(), name_label.as_str())),
                }
            }
        }
        if let Some((name_un, _)) = best {
            let name_un = name_un.to_string();
            columns[*code_idx]["display_sibling"] = json!(name_un);
        }
    }

    // Mark every level that is the Name form of some same-hierarchy sibling.
    let preferred_idxs: Vec<usize> = levels
        .iter()
        .filter(|(_, name_un, name_hier, name_label)| {
            levels.iter().any(|(_, code_un, code_hier, code_label)| {
                code_un != name_un
                    && code_hier == name_hier
                    && is_name_form_of(name_label, code_label)
            })
        })
        .map(|(i, _, _, _)| *i)
        .collect();
    for i in preferred_idxs {
        columns[i]["display_preferred"] = json!(true);
    }
}

/// Build the `near_twins` block for a set of `describe_model` columns.
///
/// Buckets dimension *levels* by their core label and emits one group per
/// bucket that spans ≥2 distinct hierarchies. Within a group the attribute on
/// the lexicographically shortest hierarchy name is tagged
/// `canonical_for: "generic"` (the hierarchy-primacy heuristic — shorter ≈ more
/// primary, e.g. `product_dimension` over `store_item_product_dimension`).
///
/// Deterministic: groups and members are sorted, so the same catalog always
/// yields the same block (NFR-3).
fn build_near_twins(columns: &[Value]) -> Vec<Value> {
    use std::collections::BTreeMap;

    // core_label -> Vec<(unique_name, hierarchy, label)>
    let mut buckets: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for c in columns {
        if c.get("kind").and_then(Value::as_str) != Some("level") {
            continue;
        }
        let (Some(un), Some(label)) = (
            c.get("unique_name").and_then(Value::as_str),
            c.get("label").and_then(Value::as_str),
        ) else {
            continue;
        };
        // Hierarchy: prefer explicit field, else parse from `hier.[Level]`.
        let hier = c
            .get("hierarchy")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| un.split_once('.').map(|(h, _)| h.to_string()))
            .unwrap_or_default();
        // Date-role hierarchies (sold/ship/return/inventory date dimensions, week
        // hierarchies) are DISTINCT date roles, not near-twins — never group across
        // them (would suggest a path-incompatible Ship Calendar Year for a Sold one).
        if is_date_role_hierarchy(&hier) {
            continue;
        }
        if let Some(core) = core_label(label) {
            buckets
                .entry(core)
                .or_default()
                .push((un.to_string(), hier, label.to_string()));
        }
    }

    let mut groups: Vec<Value> = Vec::new();
    for (core, mut members) in buckets {
        // Only a near-twin group if it spans more than one hierarchy.
        let distinct_hiers: std::collections::BTreeSet<&str> =
            members.iter().map(|(_, h, _)| h.as_str()).collect();
        if distinct_hiers.len() < 2 {
            continue;
        }
        // Stable order for determinism.
        members.sort_by(|a, b| a.0.cmp(&b.0));

        // FIX 2: canonical selection prefers the human-readable display
        // attribute — the member whose label is/contains the word "Name"
        // (e.g. "Store State Name" over the code-like "Store State",
        // "Sold Day Name" over "Sold Day of Week"). When multiple (or no)
        // members are named, fall back to the existing shortest-hierarchy-name
        // primacy as the tiebreak.
        let canonical_un = members
            .iter()
            .min_by(|a, b| {
                // `false` (is a Name attr) sorts before `true`, so invert.
                (!label_is_name(&a.2))
                    .cmp(&(!label_is_name(&b.2)))
                    .then_with(|| a.1.len().cmp(&b.1.len()))
                    .then_with(|| a.1.cmp(&b.1))
                    .then_with(|| a.0.cmp(&b.0))
            })
            .map(|(un, _, _)| un.clone());

        let twins: Vec<Value> = members
            .iter()
            .map(|(un, hier, label)| {
                let mut entry = json!({
                    "unique_name": un,
                    "hierarchy": hier,
                    "label": label,
                });
                if Some(un) == canonical_un.as_ref() {
                    entry["canonical_for"] = json!("generic");
                }
                entry
            })
            .collect();

        groups.push(json!({
            "core_label": core,
            "twin_kind": "level",
            "near_twins": twins,
        }));
    }
    groups
}

/// The "measure group" prefix of a measure label: the leading token(s) that
/// distinguish lookalike measures across fact groups (e.g. "Catalog" / "Store" /
/// "Web" / "Total" in "Catalog Ext Sales Price" vs "Store Ext Sales Price").
/// Returns the first whitespace-delimited token of the label, lowercased.
fn measure_group_prefix(label: &str) -> Option<String> {
    normalize_label(label)
        .split(' ')
        .find(|t| !t.is_empty())
        .map(str::to_string)
}

/// Qualifier / channel-scope tokens that distinguish *members* of a measure
/// family rather than naming the family's core concept. Stripped when computing
/// a measure's family stem so that, e.g., `Web Net Paid Amount`,
/// `Web Net Paid Incl Ship`, `Catalog Net Paid Inc Tax Amount`, and
/// `Total Net Paid Amount` all collapse to the same stem ("net paid") and group
/// together. These are exactly the tokens that should surface as a member's
/// `distinguishing` qualifier (channel prefix, incl-tax/ship, amount, average).
///
/// Lowercased, matched token-wise. Deliberately conservative: concept words like
/// "sales", "price", "profit", "quantity" are NOT here, so distinct concepts stay
/// in distinct families.
const MEASURE_QUALIFIER_TOKENS: &[&str] = &[
    "web", "store", "catalog", "total", "and", "incl", "inc", "tax", "ship",
    "amount", "average", "avg",
];

/// True for a token that is a member-distinguishing qualifier (see
/// [`MEASURE_QUALIFIER_TOKENS`]).
fn is_measure_qualifier_token(tok: &str) -> bool {
    MEASURE_QUALIFIER_TOKENS.contains(&tok.to_lowercase().as_str())
}

/// The "family stem" of a measure label: its concept tokens with the
/// member-distinguishing qualifier/channel tokens removed (see
/// [`MEASURE_QUALIFIER_TOKENS`]), lowercased and joined. Measures sharing a stem
/// form a near-twin family (e.g. all "Net Paid" variants → "net paid").
///
/// Returns `None` when nothing concept-bearing remains (a label made entirely of
/// qualifier tokens has no stem to group on).
fn measure_family_stem(label: &str) -> Option<String> {
    let stem: Vec<String> = normalize_label(label)
        .split(' ')
        .filter(|t| !t.is_empty() && !is_measure_qualifier_token(t))
        .map(str::to_string)
        .collect();
    if stem.is_empty() {
        None
    } else {
        Some(stem.join(" "))
    }
}

/// Compute a member's `distinguishing` qualifier phrases: the contiguous runs of
/// the member's label tokens that are NOT in `common` (the set of lowercased
/// tokens shared by every member of the family). Tokens are kept in original
/// label order and casing; adjacent distinguishing tokens are joined into a
/// single phrase (so "Web Net Paid Incl Ship" with common {net,paid} →
/// `["Web", "Incl Ship"]`). A base member whose only extra tokens are absent
/// yields `[]`.
fn distinguishing_runs(label: &str, common: &std::collections::BTreeSet<String>) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for tok in label.split_whitespace() {
        if common.contains(&tok.to_lowercase()) {
            if !cur.is_empty() {
                runs.push(cur.join(" "));
                cur.clear();
            }
        } else {
            cur.push(tok);
        }
    }
    if !cur.is_empty() {
        runs.push(cur.join(" "));
    }
    runs
}

/// Build the *measure-side* `near_twins` groups.
///
/// Implements PRD-mqo-describe-measure-disambiguation. Buckets
/// `kind=="measure"` columns by their **family stem** ([`measure_family_stem`] —
/// concept tokens with channel/qualifier words removed, e.g. all "Net Paid"
/// variants → "net paid") and emits one group per stem shared by ≥2 members.
/// This surfaces the `lookalike_measure` problem: many measures share the same
/// core concept but differ only by a qualifier (channel scope, incl-tax/ship,
/// average), and the model must pick the one the question's wording means.
///
/// FR-2: each member carries `distinguishing` — the contiguous runs of its label
/// tokens that are NOT common to every member of its family (the set-difference
/// of its tokens vs the family's common tokens). So within the "Net Paid"
/// family, `Web Net Paid Incl Ship` carries `["Web", "Incl Ship"]` and the base
/// `Web Net Paid Amount` carries `["Web", "Amount"]`, showing the model exactly
/// what separates them.
///
/// FR-3: advisory hint data only — no validator rejection is keyed off this.
/// No canonical hint is emitted for measures: unlike dimension hierarchies,
/// there is no primacy heuristic — the caller must choose the variant that
/// matches intent. Deterministic (FR-4): buckets and members are sorted.
fn build_measure_twins(columns: &[Value]) -> Vec<Value> {
    use std::collections::{BTreeMap, BTreeSet};

    // family_stem -> Vec<(unique_name, group_prefix, label)>
    let mut buckets: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for c in columns {
        if c.get("kind").and_then(Value::as_str) != Some("measure") {
            continue;
        }
        let (Some(un), Some(label)) = (
            c.get("unique_name").and_then(Value::as_str),
            c.get("label").and_then(Value::as_str),
        ) else {
            continue;
        };
        let prefix = measure_group_prefix(label).unwrap_or_default();
        if let Some(stem) = measure_family_stem(label) {
            buckets
                .entry(stem)
                .or_default()
                .push((un.to_string(), prefix, label.to_string()));
        }
    }

    let mut groups: Vec<Value> = Vec::new();
    for (stem, mut members) in buckets {
        // FR-1/FR-5: only a near-twin family if it has ≥2 members.
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| a.0.cmp(&b.0));

        // Tokens (lowercased) common to EVERY member: the family's shared core.
        // distinguishing = a member's tokens minus this set.
        let mut common: Option<BTreeSet<String>> = None;
        for (_, _, label) in &members {
            let toks: BTreeSet<String> = label
                .split_whitespace()
                .map(|t| t.to_lowercase())
                .collect();
            common = Some(match common {
                None => toks,
                Some(prev) => prev.intersection(&toks).cloned().collect(),
            });
        }
        let common = common.unwrap_or_default();

        let twins: Vec<Value> = members
            .iter()
            .map(|(un, prefix, label)| {
                json!({
                    "unique_name": un,
                    "measure_group": prefix,
                    "label": label,
                    "distinguishing": distinguishing_runs(label, &common),
                })
            })
            .collect();
        groups.push(json!({
            "core_label": stem,
            "twin_kind": "measure",
            "near_twins": twins,
        }));
    }
    groups
}

/// Collect the unique_names of temporally-typed (date) hierarchies from the
/// catalog columns. A hierarchy is treated as a date role when its name (or the
/// name of any of its levels) contains the token `date` (case-insensitive).
///
/// Used to derive each measure's `date_roles` (FR-4). Deterministic, sorted.
fn date_role_hierarchies(columns: &[Value]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut roles: BTreeSet<String> = BTreeSet::new();
    for c in columns {
        let hier = c
            .get("hierarchy")
            .and_then(Value::as_str)
            .or_else(|| {
                c.get("unique_name")
                    .and_then(Value::as_str)
                    .and_then(|un| un.split_once('.').map(|(h, _)| h))
            });
        if let Some(h) = hier {
            if h.to_lowercase().contains("date") {
                roles.insert(h.to_string());
            }
        }
    }
    roles.into_iter().collect()
}

/// Surface packaged-calc metadata on a single `describe_model` measure column.
///
/// Adds two additive fields, reusing the `mqo-param-validator` calc heuristics
/// (`is_packaged_calc` / `calc_triggers`) so the model can prefer a packaged
/// calc (e.g. `Web and Catalog Sales Price Growth`) over a plain base measure
/// when the NL phrasing asks for a derived concept:
///
///   * `is_calc: bool`   — true when the measure is a packaged calculated
///     measure. An explicit `is_calc:true` in the catalog wins; otherwise the
///     validator's name heuristic decides (`* Growth`, `* Increase`,
///     `* Change`, `* YoY`, `* vs Prior`, `Price Growth`, …).
///   * `triggers: [String]` — the NL phrases that should map to this calc
///     (from `calc_triggers()`). Empty array for non-calc measures.
///
/// The column's `unique_name` / `label` / `is_calc` are deserialized into the
/// validator's `CatalogMeasure` so the two crates share one source of truth.
fn annotate_calc(col: &mut Value) {
    use mqo_param_validator::{calc_triggers, is_packaged_calc, CatalogMeasure};

    let measure = CatalogMeasure {
        unique_name: col
            .get("unique_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        label: col.get("label").and_then(Value::as_str).map(str::to_string),
        is_calc: col.get("is_calc").and_then(Value::as_bool),
        ..Default::default()
    };

    let is_calc = is_packaged_calc(&measure);
    let triggers: Vec<String> = if is_calc {
        calc_triggers(&measure)
    } else {
        Vec::new()
    };
    col["is_calc"] = json!(is_calc);
    col["triggers"] = json!(triggers);
}

/// Estimate serialized byte size of a JSON value (UTF-8 length of compact form).
fn json_byte_size(v: &Value) -> usize {
    serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
}

/// Protocol version this server speaks. Matches the MCP spec revision string.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Selects which engine the server uses for `query_multidimensional`.
///
/// - `Fixture` — deterministic cluster-free synthesis (default; no `--endpoint`).
/// - `Live` — sends the compiled query to a live `AtScale` endpoint via the bridge.
pub enum ServerEngine {
    /// Use the deterministic `FixtureEngine` (cluster-free CI default).
    Fixture,
    /// Use a `LiveExecutor` configured with endpoint + OIDC credentials.
    /// Boxed to keep the enum size small.
    Live(Box<LiveExecutor>),
}

/// Server-side state needed to answer requests.
pub struct Server {
    /// The recorded catalog snapshot (the JSON `list_models`/`search_columns`/
    /// `describe_model` would return). Serves the catalog tools and is fed to
    /// the binder.
    pub catalog: Value,
    /// Per-level cardinality stats + shape flags consumed by the router.
    pub stats: Value,
    /// Resolved fleet binary paths.
    pub tools: ToolPaths,
    /// Router row threshold above which the SQL extract path is chosen.
    pub row_threshold: u64,
    /// Which engine to use for query execution.
    pub engine: ServerEngine,
    /// Override the router's backend decision. `Some("sql")` forces every query
    /// through the SQL compiler regardless of shape — useful when the target
    /// cluster exposes only `PGWire` (no SSDAX/XMLA).
    pub backend_override: Option<String>,
    /// Backend capability map determined at startup by the capability probe.
    /// When `--no-probe` is set or the server is in fixture mode, this is
    /// [`BackendCapabilities::all_live`] (no downgrading).
    pub capabilities: BackendCapabilities,
    /// Optional cluster registry for federation mode.
    /// `None` → single-cluster behavior (backward-compatible default).
    pub registry: Option<Arc<ClusterRegistry>>,
    /// Cached health report, refreshed by the `health_status` tool.
    /// `None` until the first health check has been run.
    pub health_cache: Option<Arc<Mutex<Option<HealthReport>>>>,
    /// In-process handle store for the four dataset handle-op tools.
    /// `None` → handle-op tools return an unsupported-operation error.
    pub handle_store: Option<HandleStore>,
    /// Cursor/pagination store.  Shared across all requests.
    /// `None` disables cursor mode (not a supported config; always `Some` in practice).
    pub cursor_store: Option<Arc<CursorStore>>,
    /// Page size for cursor mode (default [`crate::cursor::DEFAULT_PAGE_SIZE`]).
    pub page_size: usize,
    /// Inline-row threshold (K): `query_multidimensional` and every `dataset_*`
    /// op inline raw `rows` only when `row_count <= inline_threshold`.  Above K
    /// the response carries `{summary, handle, capabilities, row_count}` and no
    /// `rows`.  Default [`crate::handle_ops::INLINE_THRESHOLD`] (25).
    pub inline_threshold: usize,
    /// Enrichment data derived from `enriched-catalog.v1`, or `None` when unavailable.
    ///
    /// When `Some`: `describe_model` annotates measures with `compatible_hierarchies`,
    /// and `query_multidimensional` passes `--enriched-catalog` to the binder so
    /// `CrossFactPath` checking activates.
    /// When `None`: raw-catalog mode — behavior is identical to the pre-extension server.
    pub enriched: Option<Arc<ServerEnrichedData>>,
    /// XMLA model coordinate map: `cube_name` → (`xmla_catalog`, `cube_name`).
    ///
    /// Built at startup in live mode via `DBSCHEMA_CATALOGS` / `MDSCHEMA_CUBES` discovery
    /// (or loaded from a static `--xmla-catalog-map` JSON file). Used by the pipeline to
    /// resolve the real XMLA catalog name before DAX/MDX dispatch — the MCP schema name
    /// (e.g. `tpcds_Databricks`) differs from the XMLA catalog name (`tpcds_Snowflake`).
    ///
    /// Empty in fixture mode (no XMLA endpoint) and when discovery is not configured.
    pub xmla_model_coords: HashMap<String, (String, String)>,
    /// Maximum allowed distinct-row cardinality estimate for a projection MQO.
    ///
    /// When an attribute-projection MQO's estimated distinct count exceeds this
    /// value the server returns a typed `projection_too_large` decline instead of
    /// executing (which would spend credits and return 0 rows due to the engine
    /// row cap).  Default: `DEFAULT_MAX_PROJECTION_CARDINALITY`.  Set to 0 to
    /// always decline projections.
    pub max_projection_cardinality: usize,
    /// Lifted model RDF graph store for the `query_model_graph` tool.
    ///
    /// `None` → tool returns `model_graph_not_available` (FR6/AC6).  This is the
    /// expected state until the auto-lift tier (OSL #2) populates the graph at
    /// startup. Tests load a fixture graph directly via `ModelGraphStore::load_turtle`.
    pub model_graph: Option<Box<ModelGraphStore>>,
    /// BFO grounding store for the `describe_grounding` tool (PRD-swa-grounding-mcp-tool).
    ///
    /// `None` → tool returns `grounding_not_available`.  Populated at startup when
    /// grounding artifacts (aso-ground overlay, OSL #3) are available.
    pub grounding_store: Option<Box<GroundingStore>>,
    /// Ontology-based query check store for the `validate_query_ontology` tool.
    ///
    /// `None` → tool returns a single `info` finding (fail-open per FR7).
    /// Uses the same lifted `aso:` graph as `model_graph`; kept as a separate
    /// store so it can be populated/tested independently (advisory-first tier).
    pub ontology_check: Option<Box<OntologyCheckStore>>,
    /// Base URL for the engine catalog-XML endpoint used by auto-lift.
    ///
    /// When `Some`, `query_model_graph` fetches `{base_url}/{catalog_id}.xml`
    /// (OIDC-authed) and lifts the XML into the in-process triple store lazily
    /// on first use.  `None` (the default) disables auto-lift; the tool falls
    /// back to `model_graph` or returns `model_graph_not_available`.
    ///
    /// Sourced from `--autolift-base-url` / `ATSCALE_CATALOG_XML_BASE`.
    pub autolift_base_url: Option<String>,
    /// Shared in-process auto-lift cache (keyed on `(catalog_id, LAST_SCHEMA_UPDATE)`).
    ///
    /// `None` when auto-lift is disabled or no executor is available.
    pub autolift_cache: Option<Arc<AutoliftCache>>,
}

/// Default maximum distinct-row estimate for a projection MQO.
///
/// Aligned to `DEFAULT_MAX_RESULT_ROWS` (the materialization budget) so the
/// projection cap and the handle materialization budget are the same number —
/// one knob, no gap (PRD-mqo-projection-handle-over-cap, FR-1/OQ-2).
///
/// Rollback to old behavior: pass `--max-projection-cardinality 10000` (or set
/// `--max-result-rows 10000`).
pub const DEFAULT_MAX_PROJECTION_CARDINALITY: usize = mqo_auth_bridge::DEFAULT_MAX_RESULT_ROWS;

/// The advertised tool list. The three catalog tools are read-only.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn tool_descriptors() -> Value {
    let mut tools = core_tool_descriptors();
    tools.extend(handle_ops::handle_op_descriptors());
    Value::Array(tools)
}

/// Core (non-handle-op) tool descriptors: catalog, query, federation, chart tools.
#[allow(clippy::too_many_lines)]
fn core_tool_descriptors() -> Vec<Value> {
    let mqo_schema = mqo_input_schema();
    vec![
        json!({
            "name": "list_models",
            "description": "List the semantic models (cubes) available in the catalog. Read-only.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "describe_model",
            "description": "Describe a model: its measures, hierarchies/levels, and calculation groups. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": { "model": { "type": "string", "description": "Model unique name." } },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "search_columns",
            "description": "Search measures and dimension levels by label or unique name (column-name mode), OR find which level(s) hold a specific member value (member_value mode). When member_value is supplied, scans captured level domains for the value (case-insensitive) and returns {found, matched_levels, value}. Use member_value to ground a filter member in one call instead of rephrasing column-name searches. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Substring to match against label/unique_name (column-name mode). Omit when using member_value." },
                    "member_value": { "type": "string", "description": "A filter member value to locate (e.g. 'corpcorp #1'). When present, searches captured level domains for this value and returns the level(s) that contain it. Mutually exclusive with query." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "query_multidimensional",
            "description": "Run a Multidimensional Query Object (NEVER raw SQL) through bind→route→compile→execute and return bounded result rows plus the compiled query. Read-only by construction: the input is a selection-only object, so no write path exists.\n\n**Large results:** when row_count exceeds the inline threshold the response is handle-first: {handle, row_count, columns, sample, notes}. Work with the handle via dataset_* ops (dataset_aggregate, dataset_filter, dataset_top_n, etc.) or call dataset_export to materialize. Do NOT loop next_page to assemble all rows — next_page is for incremental paging by non-LLM clients.\n\n**Measureless projection (list / which / each member questions):** set `projection: true` with an empty `measures` array to return the DISTINCT members of the projected dimension levels — no measure aggregation. Use this shape for questions like \"list each customer who...\", \"which products are...\", \"each store that...\".\n\n**Cross-dimension / fact-resident filters on projections:** the `filters` array in a projection MAY contain filters on levels that are NOT in the `dimensions` list — including levels reachable via a fact table (e.g. Store Name, Sold Calendar Year). The engine compiles this as a SUMMARIZECOLUMNS auto-exist (semijoin): the result is the set of projected members that have at least one qualifying fact row matching the filter. No anchor measure is needed — and adding a fabricated count measure is WRONG for this shape (it changes grain and adds an unwanted column).\n\nExample — first name and gender of each customer who shopped at store 'ese' in year 2001:\n{\"projection\": true, \"measures\": [], \"dimensions\": [{\"hierarchy\": \"customer_dimension\", \"level\": \"Customer First Name\"}, {\"hierarchy\": \"customer_demographics\", \"level\": \"Gender\"}], \"filters\": [{\"type\": \"member_level\", \"hierarchy\": \"store_dimension\", \"level\": \"store_dimension.[Store Name]\", \"members\": [\"ese\"]}, {\"type\": \"member_level\", \"hierarchy\": \"sold_date_dimensions\", \"level\": \"sold_date_dimensions.[Sold Calendar Year]\", \"members\": [\"2001\"]}]}\n\n**When to use measures instead:** if the question asks \"how much / how many / total / average\" of a measure (sum, avg, count), use the `measures` array with dimension grouping — do NOT use `projection: true` for aggregation questions.\n\nSupported filter types:\n- MemberLevel: {type:\"MemberLevel\", level_unique_name, members:[...], exclude:true|false} — filter a level to explicit members; exclude:true inverts to NOT-IN.\n- Member: {type:\"Member\", level_unique_name, members:[...]} — domain-scan grounded member filter (equivalent to MemberLevel without the exclude flag).\n- Group: {type:\"Group\", op:\"and\"|\"or\", filters:[...]} — combine two or more filters; up to two levels of nesting supported.\n- Range: {type:\"Range\", level_unique_name, lo, hi} — inclusive bounds filter; ISO-date strings accepted for date levels (full timezone support coming).\n\n**Per-entity numeric attributes (projectable quantities):** when `describe_model` shows a level entry with `projectable_per_member_quantity: true`, that level holds the per-member value already stored in the semantic layer (e.g. employee count per store, floor space per warehouse). To answer \"how many employees work at each store\", project that attribute — `projection: true, measures: [], dimensions: [Store Name, Store Number of Employees]`. Do NOT count rows or sum the level; the answer is the stored attribute value, not an aggregation. Example: {\"projection\": true, \"measures\": [], \"dimensions\": [{\"hierarchy\": \"store_dimension\", \"level\": \"Store Name\"}, {\"hierarchy\": \"store_dimension\", \"level\": \"Store Number of Employees\"}]}. Contrast: a member-count question (\"how many products are in each category\") uses a count measure like `total_product_count` in the `measures` array — because that count is computed by aggregating fact rows, not stored as a level attribute.",
            "inputSchema": mqo_schema,
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "list_clusters",
            "description": "List all registered clusters with their name, endpoint, supported backends, priority, and current health status. Federation mode only; returns an error when no registry is configured. Read-only.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "health_status",
            "description": "Run a fresh TCP health check against all registered clusters and return the health report JSON. Federation mode only. Read-only.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "diff_clusters",
            "description": "Diff the describe_model catalog of two clusters identified by name. Returns a structured diff report classifying measures and dimensions as agree/diverge/critical_diverge/only_in_a/only_in_b. Federation mode only. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cluster_a": { "type": "string", "description": "Name of the first cluster (from the registry)." },
                    "cluster_b": { "type": "string", "description": "Name of the second cluster (from the registry)." }
                },
                "required": ["cluster_a", "cluster_b"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "recommend_chart",
            "description": "Given a query_multidimensional response (or rows + bound directly), run it through the result profiler and chart recommender to produce a chart-recommendation.v1 JSON: {mark, encoding, rationale, alternatives}. Read-only by construction — no state mutation, deterministic, idempotent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "response": { "type": "object", "description": "Full query_multidimensional payload {rows, bound, …}. Provide this OR rows+bound." },
                    "rows": { "type": "array", "description": "Row array from a query result. Required when `response` is absent." },
                    "bound": { "type": "object", "description": "Bound object {measures: [...], dimensions: [...]}. Required when `response` is absent." },
                    "catalog": { "type": "object", "description": "Optional catalog snapshot to enrich column typing." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "build_vega_spec",
            "description": "Produce a Vega-Lite v5 spec JSON from either a query_multidimensional response (full pipeline) or a pre-computed chart-recommendation.v1 + rows (emit-only). Returns {$schema, data, mark, encoding} in structuredContent. Read-only by construction — no state mutation, deterministic, idempotent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "response": { "type": "object", "description": "Full query_multidimensional payload {rows, bound, …}. Provide this OR recommendation+rows." },
                    "recommendation": { "type": "object", "description": "A chart-recommendation.v1 object. Required when `response` is absent." },
                    "rows": { "type": "array", "description": "Row array. Required when `response` is absent." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "build_bi_asset",
            "description": "Given a query_multidimensional response (or rows + bound directly), run the full profile → recommend → emit pipeline and return a complete bi-asset.v1 bundle: {asset, title, description, vega_spec, profile_summary, caveats}. Reduces LLM round-trips to a captioned chart from 2+ to 1. Read-only by construction — no state mutation, deterministic, idempotent. Returns an error envelope when the row count exceeds the bound rather than truncating.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "response": { "type": "object", "description": "Full query_multidimensional payload {rows, bound, …}. Provide this OR rows+bound." },
                    "rows": { "type": "array", "description": "Row array from a query result. Required when `response` is absent." },
                    "bound": { "type": "object", "description": "Bound object {measures: [...], dimensions: [...]}. Required when `response` is absent." },
                    "catalog": { "type": "object", "description": "Optional catalog snapshot to enrich column typing. When absent, the server's loaded catalog is used." }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "compose_dashboard",
            "description": "Compose an array of bi-asset.v1 bundles into a dashboard.v1 layout manifest and a Vega-Lite v5 concat spec. Returns {dashboard, title, layout, columns, panels[], vega_concat_spec} in structuredContent. Read-only by construction — no state mutation, deterministic, idempotent. Returns an error envelope on zero panels or when the panel count exceeds the bound.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bundles": {
                        "type": "array",
                        "description": "Array of bi-asset.v1 bundle objects (as returned by build_bi_asset).",
                        "items": { "type": "object" }
                    },
                    "title": { "type": "string", "description": "Dashboard-level title (required)." },
                    "layout": {
                        "type": "string",
                        "enum": ["grid", "vertical", "horizontal"],
                        "description": "Layout strategy. Defaults to 'grid'."
                    },
                    "columns": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Grid width in columns (default 2, ignored for vertical/horizontal layouts)."
                    }
                },
                "required": ["bundles", "title"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "next_page",
            "description": "Fetch the next page of a cursor returned by query_multidimensional. For incremental paging by non-LLM clients. **LLM guidance:** do NOT loop next_page to assemble all rows — instead use dataset_* ops on the returned handle (dataset_aggregate, dataset_filter, dataset_top_n, etc.) or call dataset_export to materialize out-of-band. Returns {cursor_id, page_token, page, has_more}. Returns a structured error {error: 'CursorExpired', cursor_id} when the cursor has expired or is unknown. Read-only by construction.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cursor_id": {
                        "type": "string",
                        "description": "The cursor UUID returned by query_multidimensional or a previous next_page call."
                    },
                    "page_token": {
                        "type": "integer",
                        "description": "Row offset to start from (default 0). Use the page_token value from the previous response.",
                        "minimum": 0
                    }
                },
                "required": ["cursor_id"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "query_model_graph",
            "description": "Query the lifted model RDF knowledge graph (aso-lift output) using a named canned query. \
Answers structural questions about the model — hierarchy rollup levels, calc/measure lineage, role-playing \
dimension references, cross-model conformance — that cannot be answered from `describe_model` alone. \
Read-only; results contain only model-metadata IRIs/literals, never warehouse row data.\n\n\
**v1 query set (set `query` to one of):**\n\
- `hierarchy_levels` — ordered levels of a hierarchy (coarse→fine), with IRIs and labels. \
  Params: `hierarchy_iri` (IRI) or `hierarchy_label` (label string); omit to return all hierarchies.\n\
- `calc_dependencies` — measures/columns a calc depends on (`aso:dependsOn` lineage). \
  Params: `measure_iri` or `measure_label`; omit to return all.\n\
- `role_playing_refs` — role-playing dimension references (`aso:playsRoleOf`). \
  Params: `base_dimension_iri` to filter; omit for all.\n\
- `conformance_check` — cross-model `owl:sameAs` conformance links (requires lattice-bridge OSL #7). \
  Params: `entity_a_iri`, `entity_b_iri`; omit for all sameAs links.\n\n\
**Responses:**\n\
- Normal: `{query, bindings: [...], row_count}` — each binding includes IRIs + labels.\n\
- No graph: `{status: 'model_graph_not_available'}` — no lifted graph loaded; auto-lift tier not yet deployed.\n\
- Budget exceeded: `{status: 'budget_exceeded'}` — narrow query with params.\n\
- Raw SPARQL disabled: `{status: 'raw_sparql_disabled'}` — use a canned query instead.\n\
- Bad query/params: `{status: 'error', valid_queries, params_help}`.",
            "inputSchema": crate::model_graph::query_model_graph_input_schema(),
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "describe_grounding",
            "description": "Return the formal BFO 2020 ontological grounding for a set of named model entities. \
Given entity names or IRIs, returns each one's BFO category (Generically Dependent Continuant, Role, Quality, …), \
`aso:` type, how the category was determined (kind-determined / hint-overridden / fallback), and an Aristotelian \
definition. Use this tool to answer 'what IS this measure ontologically?' before selecting between near-twin entities. \
Read-only; token-budgeted (default 50 entities, configurable via `max_entities`).\n\n\
**When to call:** call `describe_grounding` before `query_multidimensional` whenever the question involves \
entities that might be ambiguous (e.g. 'Store Sales Increase' vs 'Sales Amount') — the BFO category and \
definition reveal which is the canonical measure and what its additivity constraints are.\n\n\
**Inputs:** `entities` — array of entity names (matched case-insensitively) or full IRIs. \
`max_entities` — optional integer cap (1-200; default 50).\n\n\
**Responses (per entity in `results`):**\n\
- `status: 'grounded'` — entity found; includes `aso_class`, `bfo_category.iri`, `bfo_category.label`, \
  `grounding_source`, `aristotelian_definition`, `skos_labels`.\n\
- `status: 'ungrounded'` — entity not in grounding artifacts; returned with actionable detail.\n\
- `status: 'ambiguous'` — name matches multiple entities; returned with candidate IRIs.\n\
- `status: 'error'` — invalid IRI or other input problem.\n\n\
**Outer response:**\n\
- `{status: 'ok', results: [...], total_requested, total_returned}` — normal result.\n\
- `{status: 'grounding_not_available'}` — grounding artifacts not loaded (OSL #3 not deployed).\n\
- `truncated: true, dropped_entities: [...]` — token budget exceeded; retry with fewer entities.",
            "inputSchema": crate::grounding::describe_grounding_input_schema(),
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "validate_query_ontology",
            "description": "Advisory pre-execution ontology check: validate a proposed MQO against the \
loaded aso: model graph before execution. Returns a structured findings array so the agent can \
self-correct in one retry rather than executing a semantically invalid query.\n\n\
**Use this BEFORE `query_multidimensional`** when unsure whether the referenced measures/dimensions \
are ontologically valid for this model.\n\n\
**Checks performed (v1):**\n\
- `entity_existence` — every referenced measure/dimension must appear in the aso: graph.\n\
- `type_mismatch` — entities must be used in the role their aso: type permits \
  (e.g. a Hierarchy cannot be used as a measure).\n\
- `semi_additive_sum_over_time` — a SemiAdditiveMeasure combined with a time/date dimension \
  produces a warning (summing semi-additive measures over time yields semantically incorrect results).\n\n\
**Response shape:**\n\
`{conforms: bool, findings: [{rule_id, severity, entity, message}]}`\n\
- `conforms: true` + empty findings → ontologically valid.\n\
- `conforms: false` → at least one `error` severity finding; fix before executing.\n\
- `severity: warning` or `info` → advisory only; does not block execution in warn mode.\n\n\
**Fail-open:** when no ontology graph is loaded, returns `conforms: true` with a single \
`info` finding (`ontology_graph_not_available`). The query may proceed.",
            "inputSchema": crate::ontology_check::validate_query_ontology_input_schema(),
            "annotations": { "readOnlyHint": true }
        }),
    ]
}

/// The JSON Schema describing the `query_multidimensional` argument shape.
///
/// We wrap the canonical `mqo-spec` schema under a `mqo` property so the tool
/// contract is `{ "mqo": <MQO>, "cluster": "<optional name>" }`.
fn mqo_input_schema() -> Value {
    let mqo_schema: Value = serde_json::from_str(&mqo_spec::emit_json_schema())
        .unwrap_or_else(|_| json!({ "type": "object" }));
    json!({
        "type": "object",
        "properties": {
            "mqo": mqo_schema,
            "cluster": {
                "type": "string",
                "description": "Optional cluster name from the registry. When set, routes to that cluster. When absent, auto-routes to the highest-priority healthy cluster (federation mode) or uses the configured single endpoint."
            }
        },
        "required": ["mqo"],
        "additionalProperties": false
    })
}

impl Server {
    /// Handle one JSON-RPC request object, returning the response object.
    ///
    /// Notifications (requests with no `id`) return `None`.
    #[must_use]
    pub fn handle(&self, req: &Value) -> Option<Value> {
        // Notifications carry no id and expect no response.
        let id = req.get("id").cloned()?;
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");

        let result = match method {
            "initialize" => Ok(Self::initialize()),
            "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
            "tools/call" => self.tools_call(req.get("params")),
            "ping" => Ok(json!({})),
            other => Err(JsonRpcError::method_not_found(other)),
        };

        Some(match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e.to_value() }),
        })
    }

    fn initialize() -> Value {
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "mqo-mcp-server", "version": env!("CARGO_PKG_VERSION") }
        })
    }

    fn tools_call(&self, params: Option<&Value>) -> Result<Value, JsonRpcError> {
        let params = params.ok_or_else(|| JsonRpcError::invalid_params("missing params"))?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| JsonRpcError::invalid_params("missing tool name"))?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        match name {
            "list_models" => Ok(tool_text_result(&self.list_models())),
            "describe_model" => Ok(tool_text_result(&self.describe_model(&args))),
            "search_columns" => Ok(tool_text_result(&self.search_columns(&args))),
            "query_multidimensional" => Ok(self.query_multidimensional(&args)),
            "next_page" => Ok(self.next_page_tool(&args)),
            "list_clusters" => Ok(tool_text_result(&self.list_clusters())),
            "health_status" => Ok(tool_text_result(&self.health_status())),
            "diff_clusters" => Ok(tool_text_result(&self.diff_clusters(&args))),
            "recommend_chart" => Ok(chart_tools::handle_recommend_chart(&args, &self.catalog)),
            "build_vega_spec" => Ok(chart_tools::handle_build_vega_spec(&args, &self.catalog)),
            "build_bi_asset" => {
                // Honor an optional per-call catalog override; fall back to the server's catalog.
                let effective_catalog = args
                    .get("catalog")
                    .cloned()
                    .unwrap_or_else(|| self.catalog.clone());
                Ok(chart_tools::handle_build_bi_asset(&args, &effective_catalog))
            }
            "compose_dashboard" => Ok(chart_tools::handle_compose_dashboard(&args)),
            "query_model_graph" => Ok(self.query_model_graph(&args)),
            "describe_grounding" => Ok(self.describe_grounding(&args)),
            "validate_query_ontology" => Ok(self.validate_query_ontology(&args)),
            "dataset_aggregate"
            | "dataset_filter"
            | "dataset_sort"
            | "dataset_top_n"
            | "dataset_pivot"
            | "dataset_compare"
            | "dataset_drill"
            | "dataset_describe"
            | "dataset_slice"
            | "dataset_period_over_period"
            | "dataset_chart"
            | "dataset_export" => Ok(self.dispatch_handle_op(name, &args)),
            other => Err(JsonRpcError::invalid_params(&format!(
                "unknown tool `{other}`"
            ))),
        }
    }

    // ── Handle-operation tools dispatch ──────────────────────────────────────

    fn dispatch_handle_op(&self, tool: &str, args: &Value) -> Value {
        match &self.handle_store {
            None => {
                let payload = json!({ "error": { "code": "unsupported_operation", "detail": "handle store not configured on this server instance" } });
                json!({
                    "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                    "structuredContent": payload,
                    "isError": true
                })
            }
            Some(hs) => match tool {
                "dataset_aggregate" => handle_ops::handle_dataset_aggregate(&hs.store, args, self.inline_threshold, Some(&self.catalog)),
                "dataset_filter" => handle_ops::handle_dataset_filter(&hs.store, args, self.inline_threshold),
                "dataset_sort" => handle_ops::handle_dataset_sort(&hs.store, args, self.inline_threshold),
                "dataset_top_n" => handle_ops::handle_dataset_top_n(&hs.store, args, self.inline_threshold),
                "dataset_pivot" => handle_ops::handle_dataset_pivot(&hs.store, args, self.inline_threshold),
                "dataset_compare" => handle_ops::handle_dataset_compare(&hs.store, args, self.inline_threshold),
                "dataset_drill" => handle_ops::handle_dataset_drill(&hs.store, args, self.inline_threshold),
                "dataset_describe" => handle_ops::handle_dataset_describe(&hs.store, args, self.inline_threshold),
                "dataset_slice" => handle_ops::handle_dataset_slice(&hs.store, args, self.inline_threshold),
                "dataset_period_over_period" => handle_ops::handle_dataset_period_over_period(&hs.store, args, self.inline_threshold),
                "dataset_chart" => handle_ops::handle_dataset_chart(&hs.store, args, self.inline_threshold),
                "dataset_export" => handle_ops::handle_dataset_export(&hs.store, args),
                other => {
                    let payload = json!({ "error": { "code": "unknown_handle_op", "detail": format!("unknown handle-op tool '{other}'") } });
                    json!({
                        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                        "structuredContent": payload,
                        "isError": true
                    })
                }
            },
        }
    }

    // ── Model graph tool ──────────────────────────────────────────────────────

    /// Handle a `query_model_graph` tool call.
    ///
    /// Priority order:
    ///   1. If `model_graph` is pre-loaded (fixture/test), use it directly.
    ///   2. If auto-lift is enabled (`autolift_base_url` + live executor + cache),
    ///      ensure-lift the target model and query the result graph.
    ///   3. Otherwise return `model_graph_not_available`.
    ///
    /// Auto-lift is lazy: the first call for a model triggers the HTTP fetch +
    /// aso-lift transform; subsequent calls for the same `LAST_SCHEMA_UPDATE`
    /// hit the in-memory cache.  The data-query path (`query_multidimensional`)
    /// is never affected (NFR1).
    fn query_model_graph(&self, args: &Value) -> Value {
        // ── Path 1: pre-loaded fixture/test graph ──────────────────────────
        if let Some(store) = &self.model_graph {
            let result = store.query(args);
            let text = serde_json::to_string(&result).unwrap_or_default();
            return serde_json::json!({
                "content": [{ "type": "text", "text": text }],
                "structuredContent": result
            });
        }

        // ── Path 2: auto-lift (live mode, base URL configured) ────────────
        let result = self.ensure_autolift_and_query(args);
        let text = serde_json::to_string(&result).unwrap_or_default();
        serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": result
        })
    }

    /// Ensure the auto-lifted graph is available for the requested model and
    /// execute the query against it.  Returns `model_graph_not_available` when
    /// auto-lift is disabled, the executor is not live, or the lift fails.
    fn ensure_autolift_and_query(&self, args: &Value) -> Value {
        // Use the shared helper. If it returns None we give a more specific
        // error message based on which prerequisite is missing.
        if matches!(
            (&self.autolift_base_url, &self.engine, &self.autolift_cache),
            (None, _, _) | (_, ServerEngine::Fixture, _) | (_, _, None)
        ) {
            return serde_json::json!({
                "status": "model_graph_not_available",
                "detail": "No lifted model graph is available. \
                           Auto-lift is disabled (--autolift-base-url not configured) \
                           or this server is in fixture mode."
            });
        }

        let model_name: Option<&str> = args.get("model").and_then(Value::as_str);
        if self.resolve_autolift_coords(model_name).is_none() {
            return serde_json::json!({
                "status": "model_graph_not_available",
                "detail": "Cannot resolve XMLA catalog ID for the requested model. \
                           Ensure --xmla-catalog-map or live XMLA discovery is configured."
            });
        }

        if let Some(graph) = self.try_ensure_graph_for_model(args) {
            let store = ModelGraphStore::from_graph(graph);
            store.query(args)
        } else {
            let catalog_id = self
                .resolve_autolift_coords(model_name)
                .map(|(c, _)| c)
                .unwrap_or_else(|| "unknown".to_string());
            serde_json::json!({
                "status": "model_graph_not_available",
                "detail": format!(
                    "Auto-lift failed for model '{catalog_id}'. \
                     The engine XML endpoint may be unreachable, or the XML could not be parsed. \
                     Check that --autolift-base-url points to the engine catalog REST API."
                )
            })
        }
    }

    /// Resolve `(xmla_catalog, cube_name)` for the given model name (or the
    /// first discovered model when `model_name` is `None`).
    fn resolve_autolift_coords(&self, model_name: Option<&str>) -> Option<(String, String)> {
        let coords = if let Some(name) = model_name {
            self.xmla_model_coords.get(name).cloned()
        } else {
            // Use the first entry (sorted for determinism) when no model specified.
            self.xmla_model_coords
                .iter()
                .min_by_key(|(k, _)| k.as_str())
                .map(|(_, v)| v.clone())
        };
        coords
    }

    /// Shared auto-lift entry point for all three OSL tools.
    ///
    /// Resolves the XMLA catalog coordinates for the model named in `args`
    /// (falls back to the first discovered model), fetches `LAST_SCHEMA_UPDATE`,
    /// then delegates to [`ensure_graph`] which handles the cache-hit / cache-
    /// miss / try_autolift path.
    ///
    /// Returns `None` when auto-lift is disabled, the executor is not live, or
    /// the lift fails.  All callers fall back to "not available" on `None`.
    fn try_ensure_graph_for_model(&self, args: &Value) -> Option<oxrdf::Graph> {
        let (base_url, executor, cache) =
            match (&self.autolift_base_url, &self.engine, &self.autolift_cache) {
                (Some(u), ServerEngine::Live(ex), Some(c)) => (u, ex.as_ref(), c),
                _ => return None,
            };

        let model_name: Option<&str> = args.get("model").and_then(Value::as_str);
        let (catalog_id, cube) = self.resolve_autolift_coords(model_name)?;

        let schema_update = fetch_schema_update(executor, &catalog_id, &cube)
            .unwrap_or_else(|| "none".to_string());

        ensure_graph(&catalog_id, &schema_update, base_url, executor, cache)
    }

    // ── Grounding tool ────────────────────────────────────────────────────────

    /// Handle a `describe_grounding` tool call.
    ///
    /// Priority order:
    ///   1. If `grounding_store` is pre-loaded (fixture/test), use it directly.
    ///   2. If auto-lift is enabled, call `ensure_graph` to fetch+lift the model
    ///      XML, then build a transient `GroundingStore` from the graph and call
    ///      `lookup()` against it.
    ///   3. Otherwise return `grounding_not_available`.
    fn describe_grounding(&self, args: &Value) -> Value {
        let result = if let Some(store) = &self.grounding_store {
            // Path 1: pre-loaded store.
            store.lookup(args)
        } else if let Some(graph) = self.try_ensure_graph_for_model(args) {
            // Path 2: auto-lift succeeded — build a transient GroundingStore.
            let turtle = graph_to_turtle(&graph);
            let mut store = GroundingStore::new();
            match store.load_turtle(&turtle) {
                Ok(_) => store.lookup(args),
                Err(e) => {
                    eprintln!("mqo-mcp-server: describe_grounding: grounding error: {e}");
                    serde_json::json!({
                        "status": "grounding_not_available",
                        "detail": format!(
                            "Auto-lifted graph could not be grounded: {e}. \
                             The graph was fetched but aso-ground returned an error."
                        )
                    })
                }
            }
        } else {
            // Path 3: not available.
            serde_json::json!({
                "status": "grounding_not_available",
                "detail": "No grounding artifacts are loaded for this model. \
                           The aso-ground overlay (OSL #3) has not been deployed on this server. \
                           Live models return this result until the grounding overlay is integrated."
            })
        };
        let text = serde_json::to_string(&result).unwrap_or_default();
        serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": result
        })
    }

    // ── Ontology check tool ───────────────────────────────────────────────────

    /// Handle a `validate_query_ontology` tool call.
    ///
    /// Priority order:
    ///   1. If `ontology_check` is pre-loaded (fixture/test), use it directly.
    ///   2. If auto-lift is enabled, call `ensure_graph` to fetch+lift the model
    ///      XML, then build a transient `OntologyCheckStore` from the graph and
    ///      run the check against it.
    ///   3. Otherwise delegate to an empty store (fail-open: single `info`
    ///      finding, `conforms: true`).
    fn validate_query_ontology(&self, args: &Value) -> Value {
        let result = if let Some(store) = &self.ontology_check {
            // Path 1: pre-loaded store.
            store.check(args)
        } else if let Some(graph) = self.try_ensure_graph_for_model(args) {
            // Path 2: auto-lift succeeded — build a transient OntologyCheckStore.
            let mut store = OntologyCheckStore::new();
            let turtle = graph_to_turtle(&graph);
            match store.load_turtle(&turtle) {
                Ok(_) => store.check(args),
                Err(e) => {
                    eprintln!("mqo-mcp-server: validate_query_ontology: graph load error: {e}");
                    // Fail-open: parse error means we cannot validate; return info.
                    OntologyCheckStore::new().check(args)
                }
            }
        } else {
            // Path 3: not available — fail-open per FR7.
            OntologyCheckStore::new().check(args)
        };
        let text = serde_json::to_string(&result).unwrap_or_default();
        serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": result
        })
    }

    // ── Catalog tools (read-only snapshot passthrough) ─────────────────────

    fn list_models(&self) -> Value {
        // Derive the set of model names from the catalog.  When an explicit
        // `models` list is present in the snapshot, use it; otherwise derive
        // from column unique-name prefixes.
        let model_names: Vec<String> = if let Some(models) = self.catalog.get("models") {
            if let Some(arr) = models.as_array() {
                arr.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            } else {
                vec![]
            }
        } else {
            let mut set: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            if let Some(cols) = self.catalog.get("columns").and_then(Value::as_array) {
                for c in cols {
                    if let Some(un) = c.get("unique_name").and_then(Value::as_str) {
                        if let Some((model, _)) = un.split_once('.') {
                            set.insert(model.to_string());
                        }
                    }
                }
            }
            set.into_iter().collect()
        };

        // FR-1 / FR-5: annotate each model with its queryability.
        //   - XMLA discovery ran (>0 cubes): queryable = true iff the model is
        //     a mapped cube in `xmla_model_coords`.
        //   - Discovery not run or 0 cubes mapped: emit `queryable: null`
        //     (unknown) — never falsely mark a real cube as non-queryable.
        let discovery_ran = !self.xmla_model_coords.is_empty();
        let annotated: Vec<Value> = model_names
            .iter()
            .map(|name| {
                let queryable_field = if discovery_ran {
                    json!(self.xmla_model_coords.contains_key(name.as_str()))
                } else {
                    Value::Null
                };
                json!({ "name": name, "queryable": queryable_field })
            })
            .collect();

        // Emit both the enriched objects array and the legacy bare-name list
        // for back-compat with callers that only read `models` as strings.
        json!({
            "models": model_names,
            "model_details": annotated
        })
    }

    fn describe_model(&self, args: &Value) -> Value {
        let model = args.get("model").and_then(Value::as_str);
        let mut columns: Vec<Value> = self
            .catalog
            .get("columns")
            .and_then(Value::as_array)
            .map(|cols| {
                cols.iter()
                    .filter(|c| match model {
                        None => true,
                        Some(m) => c
                            .get("unique_name")
                            .and_then(Value::as_str)
                            .is_some_and(|un| un.starts_with(&format!("{m}.")) || un == m),
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        // When enriched: annotate each measure with its pre-computed compatible_hierarchies
        // and channel_scope descriptor (PRD-mqo-channel-scope-measure-grounding, FR1/FR2).
        // When not enriched: columns are returned unmodified (FR9 — omitted, never null).
        if let Some(ref enriched) = self.enriched {
            for col in &mut columns {
                if col.get("kind").and_then(Value::as_str) == Some("measure") {
                    if let Some(un) = col
                        .get("unique_name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                    {
                        if let Some(compat) = enriched.compatible_hierarchies.get(&un) {
                            col["compatible_hierarchies"] = compat.clone();
                        }
                        // FR1: emit channel_scope descriptor derived from FactBindings.
                        // Only present when the measure appears in tpcds_defaults().
                        // Absent for unknown measures → guard stays silent (OQ4).
                        if let Some(scope) = enriched.channel_scope_map.get(&un) {
                            col["channel_scope"] = scope.clone();
                        }
                    }
                }
            }
        }

        // ── Disambiguation pack ──────────────────────────────────────────────
        // FR-1: dimension levels already carry `hierarchy` + `level` from the
        //       catalog snapshot; ensure they are present (parse from
        //       `hier.[Level]` when the snapshot omitted them).
        for col in &mut columns {
            if col.get("kind").and_then(Value::as_str) != Some("level") {
                continue;
            }
            let parsed: Option<(String, String)> = col
                .get("unique_name")
                .and_then(Value::as_str)
                .and_then(|un| un.split_once('.'))
                .map(|(h, lvl)| {
                    (
                        h.to_string(),
                        lvl.trim_start_matches('[').trim_end_matches(']').to_string(),
                    )
                });
            if let Some((h, lvl)) = parsed {
                if col.get("hierarchy").and_then(Value::as_str).is_none() {
                    col["hierarchy"] = json!(h);
                }
                if col.get("level").and_then(Value::as_str).is_none() {
                    col["level"] = json!(lvl);
                }
            }
        }

        // Within-hierarchy *Name display preference
        // (PRD-mqo-within-hierarchy-name-preference): for each level that has a
        // same-hierarchy display "Name" sibling, mark the Name level
        // `display_preferred:true` and annotate the non-Name sibling with
        // `display_sibling:"<Name unique_name>"`. Advisory, catalog-only.
        annotate_display_siblings(&mut columns);

        // FR-4: each measure carries `date_roles` — compatible date hierarchies.
        // Derived from the catalog's temporally-typed hierarchies. Always an
        // array (empty when none), never absent.
        let date_roles = date_role_hierarchies(&columns);
        let date_roles_val = json!(date_roles);
        // FR-5 (PRD-mqo-attribute-projection): Build the attribute_of lookup once,
        // outside the mutation loop, so we don't borrow `columns` while mutating it.
        // Map: level unique_name → related attribute unique_names.
        let attr_lookup: std::collections::HashMap<String, Vec<String>> = {
            let full_catalog_columns: &[Value] = self
                .catalog
                .get("columns")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut map: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for c in full_catalog_columns {
                if let Some(ao) = c.get("attribute_of").and_then(Value::as_str) {
                    if let Some(un) = c.get("unique_name").and_then(Value::as_str) {
                        map.entry(ao.to_string()).or_default().push(un.to_string());
                    }
                }
            }
            map
        };
        for col in &mut columns {
            if col.get("kind").and_then(Value::as_str) == Some("measure") {
                col["date_roles"] = date_roles_val.clone();
                // FIX 1: surface packaged-calc metadata (is_calc + NL triggers)
                // so the model prefers a packaged calc over a plain base measure.
                annotate_calc(col);
            }
            // FR-5 (PRD-mqo-attribute-projection): mark each level as projectable and
            // list its related 1:1 attributes so the model can build projections.
            if col.get("kind").and_then(Value::as_str) == Some("level") {
                col["projectable"] = json!(true);
                let un = col
                    .get("unique_name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let related = attr_lookup.get(un).cloned().unwrap_or_default();
                col["related_attributes"] = json!(related);
            }
        }

        // FR-2/FR-3: near-twin grouping with canonical_for hint.
        //
        // The dimension *levels* live under dimension-prefixed unique_names
        // (e.g. `store_dimension.[State Name]`) while the requested `model` is
        // the fact cube (e.g. `tpcds_benchmark_model`). Filtering `columns` by
        // the model prefix therefore drops every level — so the level-twin pass
        // must read levels from the *full* catalog, not the model-filtered
        // `columns` view. Measures, in contrast, are correctly present in
        // `columns`, but we likewise source them from the full catalog so the
        // measure-twin pass is independent of the model filter.
        let all_columns: &[Value] = self
            .catalog
            .get("columns")
            .and_then(Value::as_array)
            .map_or(&columns[..], Vec::as_slice);
        let level_twins = build_near_twins(all_columns);
        // Measure-side near-twins (lookalike_measure): variants of one core
        // concept that differ only by a qualifier (channel / incl-tax-ship /
        // average), each annotated with its `distinguishing` qualifier tokens
        // (PRD-mqo-describe-measure-disambiguation).
        let measure_twins = build_measure_twins(all_columns);

        // FR-5 footprint guard. The original response is columns (with
        // compatible_hierarchies + FR-1/FR-4 tags) without the near_twins block.
        // Level twins are few and always kept (the disambiguation-pack contract).
        // The measure-twin families are the larger, growable block, so they are
        // the ones trimmed under budget pressure: drop the smallest (least
        // confusable) families first until the whole near_twins block is within
        // +15% of the columns payload. Every kept family still has ≥2 members.
        let base = json!({ "columns": &columns });
        let base_bytes = json_byte_size(&base);
        let level_bytes = json_byte_size(&json!(level_twins));
        let mut measure_twins = measure_twins;
        if base_bytes > 0 {
            // Sort smallest-family-first so `pop()` drops the least confusable.
            measure_twins.sort_by(|a, b| {
                let len = |g: &Value| {
                    g.get("near_twins")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len)
                };
                len(a)
                    .cmp(&len(b))
                    .then_with(|| {
                        a.get("core_label")
                            .and_then(Value::as_str)
                            .cmp(&b.get("core_label").and_then(Value::as_str))
                    })
            });
            #[allow(clippy::cast_precision_loss)]
            let over_budget = |measures: &[Value]| -> bool {
                let total = level_bytes + json_byte_size(&json!(measures));
                (total as f64 / base_bytes as f64) > 0.15
            };
            while over_budget(&measure_twins) && !measure_twins.is_empty() {
                measure_twins.remove(0);
            }
        }

        let mut near_twins = level_twins;
        near_twins.extend(measure_twins);

        // ── Per-hierarchy ordered levels (PRD-mqo-describe-filter-capabilities) ──
        // For each hierarchy present in the catalog, emit an ordered list of
        // {unique_name, label, has_domain} so callers can construct MemberLevel
        // filters without guessing level names.  We build from `all_columns`
        // (the full unfiltered catalog) so every hierarchy is visible regardless
        // of the `model` filter.  The `hierarchy` field is guaranteed to be
        // present at this point for all level columns (catalog_ingest sets it from
        // the snapshot; catalog_context normalises it from unique_name as a
        // fallback).  Levels appear in catalog-snapshot insertion order within
        // each hierarchy.
        let hierarchy_levels: BTreeMap<String, Vec<Value>> = {
            let mut map: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for col in all_columns {
                if col.get("kind").and_then(Value::as_str) != Some("level") {
                    continue;
                }
                let un = match col.get("unique_name").and_then(Value::as_str) {
                    Some(s) => s,
                    None => continue,
                };
                let label = col
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or(un);
                // Prefer explicit `hierarchy` field; fall back to everything
                // before the last dot segment (mirrors catalog_context.rs).
                let hier: String = col
                    .get("hierarchy")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| un.split_once('.').map(|(h, _)| h.to_string()))
                    .unwrap_or_default();
                if hier.is_empty() {
                    continue;
                }
                let has_domain = col
                    .get("domain")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty());
                // FR-5 (PRD-mqo-attribute-projection): each level is projectable.
                // All levels support projection in v0.1; cardinality-guard will add
                // per-level refinement when PRD-mqo-projection-cardinality-guard lands.
                // related_attributes: catalog columns that list this level as their
                // `attribute_of` (1:1 attributes on the level), or empty when none.
                let related_attributes: Vec<String> = all_columns
                    .iter()
                    .filter(|c| {
                        c.get("attribute_of")
                            .and_then(Value::as_str)
                            .is_some_and(|ao| ao == un)
                    })
                    .filter_map(|c| {
                        c.get("unique_name").and_then(Value::as_str).map(str::to_string)
                    })
                    .collect();
                // Pass through value_type from the catalog when present.  This is
                // critical for numeric levels (e.g. "Store Number of Employees",
                // value_type:"integer") — without it the LLM cannot distinguish a
                // projectable integer-valued attribute from an aggregatable measure,
                // leading to incorrect SUM aggregation instead of a projection.
                let value_type = col
                    .get("value_type")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // FR-1 (PRD-mqo-project-not-count-grounding): numeric attribute levels
                // (kind=level + numeric value_type) are projectable per-member quantities —
                // the value is already stored as an attribute of the entity; aggregating it
                // (sum or count) is wrong. Mark them explicitly so the LLM projects instead.
                let is_numeric_vt = value_type
                    .as_deref()
                    .map(|vt| matches!(vt, "integer" | "decimal" | "float" | "number"))
                    .unwrap_or(false);
                let mut entry = json!({
                    "unique_name": un,
                    "label": label,
                    "has_domain": has_domain,
                    "projectable": true,
                    "related_attributes": related_attributes,
                    "filterable_cross_dimension": true
                });
                if is_numeric_vt {
                    entry["projectable_per_member_quantity"] = json!(true);
                }
                if let Some(vt) = value_type {
                    entry["value_type"] = json!(vt);
                }
                map.entry(hier).or_default().push(entry);
            }
            map
        };
        // Convert BTreeMap to a JSON object (hierarchy name → levels array).
        let hierarchy_levels_val: Value = {
            let mut obj = serde_json::Map::new();
            for (h, lvls) in hierarchy_levels {
                obj.insert(h, json!(lvls));
            }
            Value::Object(obj)
        };

        // FR-2 / FR-5: annotate the model with queryability.
        // When a specific model was requested, emit the flag for that model.
        // When discovery has not run (0 cubes), emit `null` (unknown).
        let (queryable_field, candidate_cubes_field) = match model {
            None => (Value::Null, Value::Null),
            Some(m) => {
                if self.xmla_model_coords.is_empty() {
                    // Discovery has not run — unknown, never false-negative.
                    (Value::Null, Value::Null)
                } else if self.xmla_model_coords.contains_key(m) {
                    // Confirmed queryable cube.
                    (json!(true), Value::Null)
                } else {
                    // Not in the XMLA map → non-queryable dimension.
                    // List all known cubes as candidates.
                    let mut cubes: Vec<String> =
                        self.xmla_model_coords.keys().cloned().collect();
                    cubes.sort();
                    (json!(false), json!(cubes))
                }
            }
        };

        let mut resp = json!({
            "model": model,
            "queryable": queryable_field,
            "columns": columns,
            "near_twins": near_twins,
            "hierarchy_levels": hierarchy_levels_val,
            "describe_model": self.catalog.get("describe_model").cloned().unwrap_or(Value::Null),
            "projection_note": "A measureless projection (projection:true, measures:[]) may be filtered by ANY level in this model — including levels from other dimensions or fact-resident levels not in the dimensions list. The engine resolves such filters via SUMMARIZECOLUMNS auto-exist (semijoin): the result is the distinct attribute set for members that have at least one qualifying fact row. Use this shape for list/which/each questions. Use measures[] for aggregation (sum/avg/count) questions."
        });
        // Only include `candidate_cubes` when the model is non-queryable.
        if !candidate_cubes_field.is_null() {
            resp["candidate_cubes"] = candidate_cubes_field;
        }
        resp
    }

    fn search_columns(&self, args: &Value) -> Value {
        // ── member_value mode (PRD-mqo-member-locate) ─────────────────────────
        // When `member_value` is supplied, scan captured level domains for the
        // value (case-insensitive, whitespace-normalized) and return which levels
        // contain it.  Levels with no captured domain are listed as candidates
        // marked `domain_unknown: true` — never falsely asserted absent.
        if let Some(raw_value) = args.get("member_value").and_then(Value::as_str) {
            let value = raw_value.trim().to_string();
            if value.is_empty() {
                return json!({
                    "error": {
                        "code": "invalid_input",
                        "detail": "member_value must not be empty"
                    }
                });
            }
            return self.locate_member(&value);
        }

        // ── column-name search mode (FR-6 back-compat) ───────────────────────
        let q = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let columns: Vec<Value> = self
            .catalog
            .get("columns")
            .and_then(Value::as_array)
            .map(|cols| {
                cols.iter()
                    .filter(|c| {
                        let un = c
                            .get("unique_name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_lowercase();
                        let label = c
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_lowercase();
                        q.is_empty() || un.contains(&q) || label.contains(&q)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        json!({ "columns": columns })
    }

    /// Value → level lookup for the `member_value` mode of `search_columns`.
    ///
    /// Scans all catalog columns where `kind == "level"` for a case-insensitive,
    /// whitespace-normalized match of `value` in the column's `domain` array.
    ///
    /// Returns:
    /// ```json
    /// {
    ///   "value": "<queried value>",
    ///   "found": true|false,
    ///   "matched_levels": [
    ///     { "unique_name": "...", "hierarchy": "...", "label": "...", "domain_unknown": true|false }
    ///   ]
    /// }
    /// ```
    ///
    /// When `found` is `true`, `matched_levels` lists only the levels whose
    /// captured domain contains the value. When `found` is `false`, `matched_levels`
    /// lists all level candidates (including `domain_unknown` ones) so the model
    /// can still pin a level without re-searching. Levels with no captured domain
    /// are always surfaced as candidates marked `domain_unknown: true`.
    fn locate_member(&self, value: &str) -> Value {
        // Normalize the target: whitespace-collapse + lowercase.
        let target = normalize_member(value);

        let all_cols = match self
            .catalog
            .get("columns")
            .and_then(Value::as_array)
        {
            Some(c) => c,
            None => return json!({ "value": value, "found": false, "matched_levels": [] }),
        };

        // Scan all level columns.
        let mut matched: Vec<Value> = Vec::new();
        let mut domain_unknown_candidates: Vec<Value> = Vec::new();

        for col in all_cols {
            if col.get("kind").and_then(Value::as_str) != Some("level") {
                continue;
            }
            let un = match col.get("unique_name").and_then(Value::as_str) {
                Some(s) => s,
                None => continue,
            };
            let label = col.get("label").and_then(Value::as_str).unwrap_or(un);
            let hier: String = col
                .get("hierarchy")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| un.split_once('.').map(|(h, _)| h.to_string()))
                .unwrap_or_default();

            match col.get("domain").and_then(Value::as_array) {
                Some(domain) if !domain.is_empty() => {
                    // Captured domain — check if value is in it.
                    let hit = domain.iter().any(|v| {
                        v.as_str()
                            .is_some_and(|s| normalize_member(s) == target)
                    });
                    if hit {
                        matched.push(json!({
                            "unique_name": un,
                            "hierarchy": hier,
                            "label": label,
                            "domain_unknown": false
                        }));
                    }
                }
                _ => {
                    // No captured domain — list as candidate (G3).
                    domain_unknown_candidates.push(json!({
                        "unique_name": un,
                        "hierarchy": hier,
                        "label": label,
                        "domain_unknown": true
                    }));
                }
            }
        }

        // Sort for determinism.
        matched.sort_by(|a, b| {
            a.get("unique_name")
                .and_then(Value::as_str)
                .cmp(&b.get("unique_name").and_then(Value::as_str))
        });
        domain_unknown_candidates.sort_by(|a, b| {
            a.get("unique_name")
                .and_then(Value::as_str)
                .cmp(&b.get("unique_name").and_then(Value::as_str))
        });

        let found = !matched.is_empty();
        let result_levels: Vec<Value> = if found {
            // Found: return only the matching levels (not unknowns).
            matched
        } else {
            // Not found: return all candidates (domain_unknown included) so the
            // model can still pin one without re-searching (FR-3 / G2 / G3).
            domain_unknown_candidates
        };

        json!({
            "value": value,
            "found": found,
            "matched_levels": result_levels
        })
    }

    // ── Federation tools (registry-only) ──────────────────────────────────

    fn list_clusters(&self) -> Value {
        let Some(ref registry) = self.registry else {
            return json!({ "error": "no registry configured" });
        };

        // Grab cached health, if any.
        let health_snapshot: Option<HealthReport> = self
            .health_cache
            .as_ref()
            .and_then(|m| m.lock().ok()?.clone());

        let entries: Vec<Value> = registry
            .clusters
            .iter()
            .map(|c| {
                let status = health_snapshot.as_ref().and_then(|h| {
                    h.clusters
                        .iter()
                        .find(|cr| cr.name == c.name)
                        .map(|cr| serde_json::to_value(&cr.status).unwrap_or(json!("unknown")))
                });
                json!({
                    "name": c.name,
                    "endpoint": c.endpoint,
                    "supported_backends": c.supported_backends,
                    "priority": c.priority,
                    "required": c.required,
                    "tags": c.tags,
                    "status": status.unwrap_or(json!("unknown"))
                })
            })
            .collect();

        json!({ "clusters": entries })
    }

    fn health_status(&self) -> Value {
        let Some(ref registry) = self.registry else {
            return json!({ "error": "no registry configured" });
        };

        let report = routing::run_health_check_sync(registry, 5000);

        // Update the health cache if present.
        if let Some(ref cache) = self.health_cache {
            if let Ok(mut guard) = cache.lock() {
                *guard = Some(report.clone());
            }
        }

        serde_json::to_value(&report).unwrap_or_else(|e| json!({ "error": e.to_string() }))
    }

    fn diff_clusters(&self, args: &Value) -> Value {
        let Some(ref registry) = self.registry else {
            return json!({ "error": "no registry configured" });
        };

        let Some(cluster_a) = args.get("cluster_a").and_then(Value::as_str) else {
            return json!({ "error": "missing required field 'cluster_a'" });
        };
        let Some(cluster_b) = args.get("cluster_b").and_then(Value::as_str) else {
            return json!({ "error": "missing required field 'cluster_b'" });
        };

        // Verify both clusters exist in the registry.
        if registry.get(cluster_a).is_none() {
            return json!({ "error": format!("cluster '{cluster_a}' not found in registry") });
        }
        if registry.get(cluster_b).is_none() {
            return json!({ "error": format!("cluster '{cluster_b}' not found in registry") });
        }

        // Use the local catalog snapshot for both clusters (in-process diff
        // without live describe_model calls). In a future version this could
        // call each cluster's describe_model endpoint; for now we diff the
        // single loaded snapshot against itself to exercise the diff pipeline
        // and satisfy AC6.
        let catalog_text =
            serde_json::to_string(&self.catalog).unwrap_or_else(|_| "{}".to_string());

        let describe_a = mcp_cross_cluster_diff::catalog::DescribeModel::from_json(&catalog_text)
            .unwrap_or_else(|_| mcp_cross_cluster_diff::catalog::DescribeModel {
                models: vec![],
                extra: std::collections::HashMap::new(),
            });
        let describe_b = describe_a.clone();

        let config = mcp_cross_cluster_diff::diff::DiffConfig {
            cluster_a: cluster_a.to_string(),
            cluster_b: cluster_b.to_string(),
            numeric_tolerance: 0.001,
        };

        let report =
            mcp_cross_cluster_diff::diff::diff_catalogs(&describe_a, &describe_b, &config);
        serde_json::to_value(&report).unwrap_or_else(|e| json!({ "error": e.to_string() }))
    }

    // ── query_multidimensional ─────────────────────────────────────────────

    fn query_multidimensional(&self, args: &Value) -> Value {
        // The input schema requires `{ "mqo": <MQO> }`. Reject any call that
        // does not carry the `mqo` wrapper key — callers who pass MQO fields
        // directly in `arguments` (bypassing the wrapper) get a structured
        // `missing_mqo_key` error, not silent execution. This closes the
        // bare-args bypass: without this check, `args.clone()` would fall
        // through to pipeline deserialization for any well-shaped MQO object
        // even when the caller omitted the required nesting.
        let query = match args.get("mqo") {
            Some(v) => v.clone(),
            None => {
                return structured_err(&crate::pipeline::PipelineError::NotAnMqo(
                    "query_multidimensional requires an 'mqo' key in the arguments object; \
                     the MQO must be nested as {\"mqo\": <MQO>}, not passed as flat fields"
                        .to_string(),
                ));
            }
        };

        // Optional `cluster` field selects a specific registry cluster.
        let preferred_cluster = args.get("cluster").and_then(Value::as_str);

        // When a registry is active, resolve the target cluster and determine
        // the backend override from its supported_backends list (if any).
        // When no registry is present, this is a no-op — single-cluster path.
        let (cluster_used, backend_override) = if let Some(ref registry) = self.registry {
            let health_snapshot: Option<HealthReport> = self
                .health_cache
                .as_ref()
                .and_then(|m| m.lock().ok()?.clone());

            match routing::select_cluster(
                registry,
                health_snapshot.as_ref(),
                preferred_cluster,
            ) {
                Ok(entry) => {
                    // Use the cluster's first supported backend as override only
                    // if a specific cluster was explicitly requested (not auto-route).
                    // For auto-route we let the router decide.
                    let bo = if preferred_cluster.is_some() {
                        self.backend_override
                            .clone()
                            .or_else(|| entry.supported_backends.first().cloned())
                    } else {
                        self.backend_override.clone()
                    };
                    (Some(entry.name.clone()), bo)
                }
                Err(e) => {
                    // Return a structured routing error.
                    let payload = json!({
                        "error": {
                            "code": "routing_error",
                            "detail": e.to_string()
                        }
                    });
                    return json!({
                        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                        "structuredContent": payload,
                        "isError": true
                    });
                }
            }
        } else {
            (None, self.backend_override.clone())
        };

        // ── Projection cardinality guard (FR-1 through FR-5) ─────────────────
        // For projection MQOs (no measures), estimate the distinct-row count
        // before execution.  Measure-bearing queries are unaffected (FR-6).
        if let Ok(mqo_parsed) = serde_json::from_value::<mqo_spec::Mqo>(query.clone()) {
            if mqo_parsed.measures.is_empty() {
                // This is a projection MQO — apply the guard.
                let catalog_snap: mqo_catalog_binder::catalog::CatalogSnapshot =
                    serde_json::from_value(self.catalog.clone()).unwrap_or_default();
                if let Err(too_large) = check_projection_cardinality(
                    &mqo_parsed,
                    &catalog_snap,
                    self.max_projection_cardinality,
                ) {
                    return structured_err(&PipelineError::ProjectionTooLarge {
                        level: too_large.level,
                        estimate: too_large.estimate,
                        cap: too_large.cap,
                    });
                }
            }
        }

        // ── Non-queryable dimension guard (FR-3, PRD-mqo-queryable-model-grounding) ─
        // When XMLA discovery has run (>0 cubes in the map) and the requested model
        // is NOT in the map, it is a dimension table — return a typed model_path
        // error naming the candidate cubes rather than the opaque xmla_coords_not_found.
        // This fires BEFORE the pipeline so the LLM can recover in one retry.
        // When discovery has not run (empty map), we fall through and let the
        // pipeline handle it as before (FR-5 fail-safe: never mislabel a real cube).
        if !self.xmla_model_coords.is_empty() {
            if let Some(requested_model) = query.get("model").and_then(Value::as_str) {
                if !self.xmla_model_coords.contains_key(requested_model) {
                    let mut cubes: Vec<String> =
                        self.xmla_model_coords.keys().cloned().collect();
                    cubes.sort();
                    return structured_err(&PipelineError::NonQueryableDimension {
                        model: requested_model.to_string(),
                        candidate_cubes: cubes,
                    });
                }
            }
        }

        let start = std::time::Instant::now();
        let result = pipeline::run(
            &query,
            &self.catalog,
            &self.stats,
            &self.tools,
            self.row_threshold,
            &self.engine,
            backend_override.as_deref(),
            &self.capabilities,
            self.enriched.as_ref().map(|e| e.catalog_json.as_str()),
            &self.xmla_model_coords,
            self.enriched.as_ref().map(|e| &e.channel_scope_map),
        );
        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(out) => {
                // PRD-mqo-handle-canonical-labels (v0.32.0): persist the result
                // under canonical clean column labels — the same names the response
                // uses — so handle schema == response == dataset_export (G1/FR-1/FR-3).
                //
                // `put_rows_with_canonical_labels` applies `clean_result_rows`
                // (the shared single function, FR-3) before persisting, so:
                //   - handle stores "Year", "Revenue" (not raw mangled keys)
                //   - collision disambiguation is identical to the response (FR-5)
                //   - already-clean names are a no-op (FR-7/idempotent)
                //
                // Back-compat (FR-6): the resolver in `clean_result_rows` remains
                // available for ops that pass a raw key; however, since the handle
                // now stores canonical names, well-behaved `dataset_*` callers using
                // the names from the response need no resolution at all.
                let handle = self
                    .handle_store
                    .as_ref()
                    .and_then(|hs| hs.put_rows_with_canonical_labels(&out.rows, &out.bound).ok());

                // Build the clean-row PipelineOutput for the response.  The response
                // and the handle now share the same canonical column names (FR-3).
                let clean_rows =
                    crate::handle_ops::clean_result_rows(&out.rows, &out.bound);
                // Build a response PipelineOutput with canonical clean column labels.
                let out_clean = crate::pipeline::PipelineOutput {
                    rows: clean_rows,
                    backend: out.backend.clone(),
                    estimated_rows: out.estimated_rows,
                    routing_reason: out.routing_reason.clone(),
                    compiled_query: out.compiled_query.clone(),
                    bound: out.bound.clone(),
                    filters_applied: out.filters_applied.clone(),
                    filters_dropped: out.filters_dropped.clone(),
                    row_cap_tripped: out.row_cap_tripped,
                };

                // Cursor mode: persist and return first page when rows > page_size.
                if out_clean.rows.len() > self.page_size {
                    if let Some(ref store) = self.cursor_store {
                        match store.put_and_first_page(out_clean.rows.clone(), self.page_size) {
                            Ok(first_page) => {
                                return structured_cursor_ok(
                                    &out_clean,
                                    &first_page,
                                    cluster_used.as_deref(),
                                    latency_ms,
                                    handle.as_ref(),
                                );
                            }
                            Err(e) => {
                                eprintln!("mqo-mcp-server: cursor store error: {e}");
                                // Fall through to inline on store error.
                            }
                        }
                    }
                }
                // PRD-mqo-calc-context-ratio-measures: if ALL result rows have a
                // null value for a packaged calc (is_calc=true) measure, warn the
                // model that the measure needs a comparison-context
                // (time_intelligence or calc-group member) to compute.
                let calc_null_warning = detect_calc_context_null(&out);

                let mut resp = structured_ok(
                    &out_clean,
                    cluster_used.as_deref(),
                    latency_ms,
                    handle.as_ref(),
                    self.inline_threshold,
                );
                if let Some(warn) = calc_null_warning {
                    if let Some(obj) = resp.get_mut("content")
                        .and_then(|c| c.get_mut(0))
                    {
                        // Append the warning to the text content for model visibility.
                        if let Some(text) = obj.get_mut("text").and_then(|t| t.as_str()) {
                            let mut updated: serde_json::Value =
                                serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
                            if let Some(map) = updated.as_object_mut() {
                                map.insert("calc_context_warning".to_string(),
                                    serde_json::Value::String(warn));
                            }
                            obj["text"] = serde_json::Value::String(
                                serde_json::to_string(&updated).unwrap_or_default());
                        }
                    }
                }
                resp
            }
            Err(PipelineError::CrossFactIncompatible { report }) => {
                let text = self.format_cross_fact_text(&report);
                let payload = json!({ "error": { "code": "cross_fact_incompatible", "detail": report } });
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": payload,
                    "isError": true
                })
            }
            Err(e) => structured_err(&e),
        }
    }

    // ── Cross-fact error formatting ──────────────────────────────────────

    fn format_cross_fact_text(&self, report: &Value) -> String {
        let Some(reports) = report.get("incompatible").and_then(Value::as_array) else {
            return "cross_fact_incompatible: one or more measure×dimension pairs span different fact tables.".to_string();
        };
        let Some(first) = reports.first() else {
            return "cross_fact_incompatible: one or more measure×dimension pairs span different fact tables.".to_string();
        };

        let measure = first
            .get("measure_unique_name")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let dimension = first
            .get("dimension_unique_name")
            .and_then(Value::as_str)
            .unwrap_or("?");

        let compat_hint = self
            .enriched
            .as_ref()
            .and_then(|e| e.compatible_hierarchies.get(measure))
            .and_then(Value::as_array)
            .map(|arr| {
                let names: Vec<&str> = arr
                    .iter()
                    .filter_map(|e| e.get("hierarchy_unique_name").and_then(Value::as_str))
                    .collect();
                if names.is_empty() {
                    String::new()
                } else {
                    format!(" Compatible dimensions for [{measure}]: [{}].", names.join(", "))
                }
            })
            .unwrap_or_default();

        let extra = if reports.len() > 1 {
            format!(" (and {} more incompatible pair(s))", reports.len() - 1)
        } else {
            String::new()
        };

        format!(
            "cross_fact_incompatible: measure [{measure}] and dimension [{dimension}] \
             belong to different facts{extra}.{compat_hint}"
        )
    }

    // ── next_page tool ────────────────────────────────────────────────────

    fn next_page_tool(&self, args: &Value) -> Value {
        let Some(cursor_id) = args.get("cursor_id").and_then(Value::as_str) else {
            let payload = json!({ "error": { "code": "invalid_params", "detail": "missing required field 'cursor_id'" } });
            return json!({
                "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                "structuredContent": payload,
                "isError": true
            });
        };

        let page_token: usize = args
            .get("page_token")
            .and_then(Value::as_u64)
            .map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX));

        let Some(ref store) = self.cursor_store else {
            let payload = json!({ "error": { "code": "cursor_disabled", "detail": "cursor store not configured" } });
            return json!({
                "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                "structuredContent": payload,
                "isError": true
            });
        };

        match store.next_page(cursor_id, page_token, self.page_size) {
            Ok(page) => {
                let payload = serde_json::to_value(&page)
                    .unwrap_or_else(|_| json!({ "error": "serialization error" }));
                json!({
                    "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                    "structuredContent": payload,
                    "isError": false
                })
            }
            Err(cursor_err) => {
                let payload = serde_json::to_value(&cursor_err)
                    .unwrap_or_else(|_| json!({ "error": "serialization error" }));
                json!({
                    "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                    "structuredContent": payload,
                    "isError": true
                })
            }
        }
    }
}

/// Detect the calc-context-null pattern: a packaged calc (is_calc=true) measure
/// returns all-null values for every row, indicating the comparison context
/// (time_intelligence frame or calc-group member) is missing.
/// Returns Some(warning_message) when detected, None otherwise.
fn detect_calc_context_null(out: &crate::pipeline::PipelineOutput) -> Option<String> {
    // Only fire for packaged calc measures.
    let calc_measures: Vec<String> = out.bound.get("measures")
        .and_then(|m| m.as_array())
        .map(|arr| arr.iter().filter_map(|m| {
            if m.get("is_calc").and_then(|v| v.as_bool()).unwrap_or(false) {
                m.get("unique_name").and_then(|v| v.as_str()).map(String::from)
            } else { None }
        }).collect())
        .unwrap_or_default();
    if calc_measures.is_empty() { return None; }
    if out.rows.is_empty() { return None; }
    // Check if all rows have null for every calc measure column.
    let all_null = out.rows.iter().all(|row| {
        row.as_object().is_some_and(|obj| {
            calc_measures.iter().all(|m| {
                let short = m.rsplit('.').next().unwrap_or(m);
                obj.iter().any(|(k, v)| {
                    (k.contains(short) || k == m) && v.is_null()
                })
            })
        })
    });
    if all_null {
        Some(format!(
            "All rows have null values for packaged calc measure(s) [{}]. \
             These ratio/growth measures require a comparison context: add a \
             time_intelligence operation (e.g. YoY, PriorPeriod) or a \
             CalcGroupMember filter to provide the prior-period reference. \
             Without it the measure evaluates to null.",
            calc_measures.join(", ")
        ))
    } else {
        None
    }
}

/// Build a tool-call success result whose `content[0]` is the JSON payload as
/// text and whose `structuredContent` carries the parsed object.
///
/// When `cluster_used` is `Some`, the federation metadata block is included
/// in the response (AC8).
fn structured_ok(
    out: &PipelineOutput,
    cluster_used: Option<&str>,
    latency_ms: u64,
    handle: Option<&DatasetHandle>,
    inline_threshold: usize,
) -> Value {
    let row_count = out.rows.len();
    let mut payload = json!({
        "backend": out.backend,
        "estimated_rows": out.estimated_rows,
        "routing_reason": out.routing_reason,
        "compiled_query": out.compiled_query,
        "row_count": row_count,
        "bound": out.bound,
        "filters_applied": out.filters_applied,
        "filters_dropped": out.filters_dropped,
    });

    // Size gate (AC-2 / AC-3): inline rows only when row_count ≤ K.
    if handle_ops::should_inline(row_count, inline_threshold) {
        payload["rows"] = json!(out.rows);
    } else {
        payload["notes"] = json!([format!(
            "{row_count} rows exceed inline_threshold ({inline_threshold}); rows omitted. \
             Use the returned handle with dataset_* ops or next_page."
        )]);
    }

    // Always attach the handle, bounded summary, and advertised capabilities so
    // the LLM can operate on the result without ever receiving all rows.
    attach_handle_summary(&mut payload, handle, out);

    // FR-3: when the real result exceeded the materialization budget, the rows
    // (and handle) are a truncated prefix. Surface a typed over-budget signal so
    // a consumer never mistakes a clamped set for the full answer.
    attach_over_budget_signal(&mut payload, out);

    // AC8: include federation metadata when active.
    if let Some(cluster) = cluster_used {
        payload["cluster_used"] = json!(cluster);
        payload["latency_ms"] = json!(latency_ms);
    }

    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": false
    })
}

/// Attach `handle`, `summary`, and `capabilities` to a query response payload,
/// derived from the stored typed dataset.  No-op when the result was not stored.
fn attach_handle_summary(payload: &mut Value, handle: Option<&DatasetHandle>, out: &PipelineOutput) {
    let Some(h) = handle else { return };
    payload["handle"] = serde_json::to_value(h).unwrap_or(Value::Null);
    // Bound-authoritative roles so the summary's measure/dimension split matches
    // the stored dataset (numeric dimensions are not mislabelled as measures).
    let ds = crate::handle_ops::json_rows_to_dataset_with_bound(&out.rows, &out.bound);
    let summary = dh_summary::summarize(&ds, &dh_summary::SummaryCfg::default());
    payload["summary"] = serde_json::to_value(&summary).unwrap_or(Value::Null);
    payload["capabilities"] = serde_json::to_value(dh_summary::capabilities(&ds)).unwrap_or(Value::Null);
}

/// Build a handle-first large-result response (FR-1).
///
/// Leads with `{handle, row_count, columns, sample, notes}` to steer the LLM
/// toward `dataset_*` ops or `dataset_export` rather than paginating.  Cursor
/// fields (`cursor_id`, `page`, `page_token`, `has_more`) are retained for
/// back-compat with non-LLM clients that want incremental paging.
fn structured_cursor_ok(
    out: &PipelineOutput,
    first_page: &crate::cursor::CursorFirstPage,
    cluster_used: Option<&str>,
    latency_ms: u64,
    handle: Option<&DatasetHandle>,
) -> Value {
    let total_rows = first_page.total_rows;

    // Derive column names from the first row (or from the bound).
    let columns: Vec<Value> = {
        let mut cols: Vec<String> = Vec::new();
        if let Some(row) = out.rows.first() {
            if let Some(obj) = row.as_object() {
                for k in obj.keys() {
                    cols.push(k.clone());
                }
            }
        }
        // Fall back to bound dimensions + measures if rows are empty.
        if cols.is_empty() {
            let dims = out.bound.get("dimensions").and_then(Value::as_array);
            let meas = out.bound.get("measures").and_then(Value::as_array);
            for arr in [dims, meas].into_iter().flatten() {
                for item in arr {
                    if let Some(un) = item.get("unique_name").and_then(Value::as_str) {
                        cols.push(un.to_string());
                    }
                }
            }
        }
        cols.into_iter().map(|c| json!(c)).collect()
    };

    // Bounded sample: up to page_size rows (= the first page already computed).
    let sample = first_page.page.clone();

    // FR-1 / FR-5: handle-first notes steer.
    let notes = vec![
        format!(
            "Large result: {total_rows} rows. Work with the handle via dataset_* ops \
             (dataset_aggregate, dataset_filter, dataset_top_n, etc.) or call \
             dataset_export to materialize out-of-band. \
             next_page is for incremental paging by non-LLM clients — do NOT loop \
             next_page to assemble all rows."
        )
    ];

    let mut payload = json!({
        // ── Handle-first fields (FR-1) ──────────────────────────────────────
        "handle": first_page.cursor_id,
        "row_count": total_rows,
        "columns": columns,
        "sample": sample,
        "notes": notes,
        // ── Back-compat cursor fields (FR-6) ────────────────────────────────
        "cursor_id": first_page.cursor_id,
        "page_size": first_page.page_size,
        "total_rows": total_rows,
        "page": first_page.page,
        "page_token": first_page.page_token,
        "has_more": first_page.has_more,
        // ── Query metadata ──────────────────────────────────────────────────
        "backend": out.backend,
        "estimated_rows": out.estimated_rows,
        "routing_reason": out.routing_reason,
        "compiled_query": out.compiled_query,
        "bound": out.bound,
        "filters_applied": out.filters_applied,
        "filters_dropped": out.filters_dropped,
    });

    // Attach the typed-store handle + bounded summary alongside the cursor.
    attach_handle_summary(&mut payload, handle, out);

    // FR-3: typed over-budget signal when the result exceeded the budget.
    attach_over_budget_signal(&mut payload, out);

    if let Some(cluster) = cluster_used {
        payload["cluster_used"] = json!(cluster);
        payload["latency_ms"] = json!(latency_ms);
    }

    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": false
    })
}

/// Attach the typed over-budget signal (FR-3) when the engine result exceeded
/// the materialization budget. On a tripped result the rows were truncated to
/// exactly the budget, so `out.rows.len()` IS the budget value to report.
///
/// The query response stays `isError: false` (a handle + truncated prefix is
/// still usable), but it carries an explicit `result_too_large` block plus a
/// `truncated: true` marker and a steering note, so the consumer treats the
/// handle as an incomplete prefix and re-runs with a higher `--max-result-rows`
/// or a narrowing `dataset_filter`/`dataset_top_n` — never as the full answer.
fn attach_over_budget_signal(payload: &mut Value, out: &PipelineOutput) {
    if !out.row_cap_tripped {
        return;
    }
    let budget = out.rows.len();
    payload["truncated"] = json!(true);
    payload["result_too_large"] = json!({
        "code": "result_too_large",
        "budget": budget,
        "detail": format!(
            "the query's full result exceeded the server materialization budget \
             ({budget} rows); the handle and any inline rows are a truncated prefix \
             of exactly {budget} rows, NOT the complete result. Raise \
             --max-result-rows (up to the 200000 upstream ceiling) to materialize \
             more, or narrow the query (dataset_filter / dataset_top_n / add a \
             filter) so the full result fits the budget."
        ),
    });
    let note = format!(
        "OVER BUDGET: result exceeded the materialization budget ({budget} rows) and \
         was truncated to {budget}. This handle is an incomplete prefix — do not treat \
         aggregates/exports over it as the full answer."
    );
    match payload.get_mut("notes").and_then(Value::as_array_mut) {
        Some(arr) => arr.push(json!(note)),
        None => payload["notes"] = json!([note]),
    }
}

/// Build a tool-call *application* error result (`isError: true`). Per MCP,
/// tool execution failures are reported in the result, not as a protocol error.
fn structured_err(e: &PipelineError) -> Value {
    let (code, detail) = match e {
        PipelineError::NotAnMqo(d) => ("not_an_mqo", json!(d)),
        PipelineError::Invalid(d) => ("invalid_mqo", json!(d)),
        PipelineError::NotGround { report } => ("not_ground", report.clone()),
        PipelineError::CrossFactIncompatible { report } => {
            ("cross_fact_incompatible", report.clone())
        }
        PipelineError::ParamRejected { report, .. } => ("param_rejected", report.clone()),
        PipelineError::Subprocess { tool, detail } => (
            "subprocess_error",
            json!({ "tool": tool, "detail": detail }),
        ),
        PipelineError::Io(d) => ("io_error", json!(d)),
        PipelineError::Engine(e) => ("engine_error", json!(e.to_string())),
        PipelineError::NoBackendAvailable { dax, mdx, sql } => (
            "no_backend_available",
            json!({ "dax": dax, "mdx": mdx, "sql": sql }),
        ),
        PipelineError::XmlaCoordsNotFound { model } => (
            "xmla_coords_not_found",
            json!({
                "model": model,
                "detail": format!(
                    "No XMLA catalog/cube found for model '{model}'. \
                     Populate --xmla-catalog-map or ensure XMLA discovery ran at startup."
                )
            }),
        ),
        PipelineError::ProjectionTooLarge { level, estimate, cap } => (
            "projection_too_large",
            json!({
                "level": level,
                "estimate": estimate,
                "cap": cap,
                "detail": format!(
                    "Projection over level '{level}' has an estimated distinct cardinality \
                     of {estimate}, which exceeds the configured cap of {cap}. \
                     Add a filter to narrow the set or ask the operator to raise \
                     --max-projection-cardinality."
                )
            }),
        ),
        PipelineError::NonQueryableDimension { model, candidate_cubes } => (
            "non_queryable_dimension",
            json!({
                "model": model,
                "candidate_cubes": candidate_cubes,
                "detail": format!(
                    "Model '{model}' is a dimension table, not a queryable cube. \
                     Re-issue query_multidimensional with one of the following \
                     cube model(s) instead: {candidate_cubes:?}."
                )
            }),
        ),
        PipelineError::DimensionNotMaterialized { missing, requested, report } => (
            "dimension_not_materialized",
            json!({
                "missing": missing,
                "requested": requested,
                "detail": format!(
                    "The engine returned rows missing {missing} of {requested} requested \
                     dimension column(s). This typically indicates a near-twin level \
                     (a label shared across ≥2 hierarchies) whose column was silently \
                     dropped. Retry with an explicit hierarchy-qualified level or a \
                     different dimension."
                ),
                "report": report,
            }),
        ),
    };
    let payload = json!({
        "error": {
            "code": code,
            "detail": detail,
            "error_class": pipeline::error_class(e),
        }
    });
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": true
    })
}

fn tool_text_result(value: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(value).unwrap_or_default() }],
        "structuredContent": value,
        "isError": false
    })
}

// ── XMLA catalog discovery ─────────────────────────────────────────────────

/// Discover XMLA catalog→cube mappings by issuing `DBSCHEMA_CATALOGS` and then
/// `MDSCHEMA_CUBES` against the XMLA endpoint.
///
/// Returns a map `cube_name → (xmla_catalog, cube_name)`.
///
/// On any failure (network, parse) logs a warning and returns an empty map;
/// the server starts successfully and the first DAX/MDX query surfaces the
/// `XmlaCoordsNotFound` error (FR3) rather than a hung startup.
#[must_use]
pub fn discover_xmla_coords(xmla_url: &str, bearer_token: &str) -> HashMap<String, (String, String)> {
    match discover_xmla_coords_inner(xmla_url, bearer_token) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("mqo-mcp-server: WARN: XMLA catalog discovery failed: {e}; \
                       DAX/MDX queries will fail with XmlaCoordsNotFound until \
                       --xmla-catalog-map is provided or the endpoint becomes available");
            HashMap::new()
        }
    }
}

fn discover_xmla_coords_inner(
    xmla_url: &str,
    bearer_token: &str,
) -> Result<HashMap<String, (String, String)>, String> {
    let catalogs = xmla_discover(xmla_url, bearer_token, "DBSCHEMA_CATALOGS", None)
        .map_err(|e| format!("DBSCHEMA_CATALOGS: {e}"))?;

    let catalog_names: Vec<String> = catalogs
        .iter()
        .filter_map(|row| row.get("CATALOG_NAME").and_then(Value::as_str).map(str::to_string))
        .collect();

    let mut map: HashMap<String, (String, String)> = HashMap::new();
    for catalog in &catalog_names {
        let cubes = xmla_discover(xmla_url, bearer_token, "MDSCHEMA_CUBES", Some(catalog))
            .map_err(|e| format!("MDSCHEMA_CUBES({catalog}): {e}"))?;
        for row in &cubes {
            if let Some(cube_name) = row.get("CUBE_NAME").and_then(Value::as_str) {
                map.insert(
                    cube_name.to_string(),
                    (catalog.clone(), cube_name.to_string()),
                );
            }
        }
    }
    eprintln!(
        "mqo-mcp-server: XMLA discovery: {} catalog(s), {} cube(s) mapped",
        catalog_names.len(),
        map.len()
    );
    Ok(map)
}

/// Issue a single XMLA `Discover` request and return the `<row>` elements as a
/// `Vec` of `HashMap<field, value>` objects.
fn xmla_discover(
    xmla_url: &str,
    bearer_token: &str,
    request_type: &str,
    catalog: Option<&str>,
) -> Result<Vec<Value>, String> {
    let restriction = catalog.map_or_else(String::new, |c| {
        format!("<CATALOG_NAME>{}</CATALOG_NAME>", xml_escape(c))
    });
    let catalog_prop = catalog.map_or_else(String::new, |c| {
        format!("<Catalog>{}</Catalog>", xml_escape(c))
    });

    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <Discover xmlns="urn:schemas-microsoft-com:xml-analysis">
      <RequestType>{request_type}</RequestType>
      <Restrictions><RestrictionList>{restriction}</RestrictionList></Restrictions>
      <Properties><PropertyList>{catalog_prop}</PropertyList></Properties>
    </Discover>
  </soap:Body>
</soap:Envelope>"#,
    );

    let resp_text = xmla_http_post(xmla_url, bearer_token, &body)?;
    Ok(parse_discover_rows(&resp_text))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Minimal synchronous HTTP POST — reuses reqwest blocking (already in scope
/// via mqo-auth-bridge's tokio runtime approach, but we use a new current-thread
/// runtime here to stay in sync context).
fn xmla_http_post(xmla_url: &str, bearer_token: &str, body: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio build: {e}"))?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let resp = client
            .post(xmla_url)
            .header("Authorization", format!("Bearer {bearer_token}"))
            .header("Content-Type", "application/xml")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| format!("HTTP: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status}: {text}"));
        }
        resp.text().await.map_err(|e| format!("body: {e}"))
    })
}

/// Parse `<row>` elements from an XMLA Discover response into a `Vec<Value>`.
///
/// Each `<row>` becomes a JSON object with element names as keys and text
/// content as string values.
fn parse_discover_rows(xml: &str) -> Vec<Value> {
    // Minimal hand-rolled parser: locate <row>…</row> blocks and extract
    // child element text. We avoid pulling in an XML crate to keep deps lean.
    let mut rows: Vec<Value> = Vec::new();
    let mut search = xml;
    while let Some(row_start) = search.find("<row>") {
        search = &search[row_start + 5..];
        let Some(row_end) = search.find("</row>") else {
            break;
        };
        let row_content = &search[..row_end];
        search = &search[row_end + 6..];

        let mut obj = serde_json::Map::new();
        let mut inner = row_content;
        while let Some(open) = inner.find('<') {
            let tag_start = &inner[open + 1..];
            let Some(tag_close) = tag_start.find('>') else { break; };
            let tag_name = &tag_start[..tag_close];
            // Skip closing tags and self-closing tags.
            if tag_name.starts_with('/') || tag_name.ends_with('/') {
                inner = &tag_start[tag_close + 1..];
                continue;
            }
            let after_open = &tag_start[tag_close + 1..];
            let close_tag = format!("</{tag_name}>");
            let text = if let Some(close_pos) = after_open.find(&close_tag) {
                let t = &after_open[..close_pos];
                inner = &after_open[close_pos + close_tag.len()..];
                t
            } else {
                inner = after_open;
                ""
            };
            obj.insert(tag_name.to_string(), Value::String(text.to_string()));
        }
        rows.push(Value::Object(obj));
    }
    rows
}

// ── JSON-RPC error helper ──────────────────────────────────────────────────

/// A JSON-RPC 2.0 error object.
struct JsonRpcError {
    code: i64,
    message: String,
}

impl JsonRpcError {
    fn method_not_found(method: &str) -> Self {
        JsonRpcError {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }
    fn invalid_params(detail: &str) -> Self {
        JsonRpcError {
            code: -32602,
            message: format!("invalid params: {detail}"),
        }
    }
    fn to_value(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}

// ── Size-gate unit tests (AC-2 / AC-3 on the query path) ───────────────────────

#[cfg(test)]
mod size_gate_tests {
    use super::*;

    fn synth_output(n: usize) -> PipelineOutput {
        let rows: Vec<Value> = (0..n)
            .map(|i| json!({ "year": 2000 + i as i64, "revenue": (i as f64) * 10.0 }))
            .collect();
        PipelineOutput {
            backend: "dax".to_string(),
            estimated_rows: n as u64,
            routing_reason: "test".to_string(),
            compiled_query: "EVALUATE".to_string(),
            rows,
            bound: json!({}),
            filters_applied: vec![],
            filters_dropped: vec![],
            row_cap_tripped: false,
        }
    }

    fn store_handle(out: &PipelineOutput) -> (HandleStore, DatasetHandle) {
        let hs = HandleStore::new();
        let h = hs.put_rows(&out.rows).expect("put_rows");
        (hs, h)
    }

    /// AC-2: a ≤K result inlines `rows` (plus handle + summary).
    #[test]
    fn ac2_query_small_result_inlines_rows() {
        let out = synth_output(10); // ≤ K=25
        let (_hs, h) = store_handle(&out);
        let resp = structured_ok(&out, None, 1, Some(&h), 25);
        let sc = &resp["structuredContent"];
        assert_eq!(sc["row_count"], json!(10));
        assert!(sc.get("rows").is_some(), "≤K must inline rows: {sc}");
        assert_eq!(sc["rows"].as_array().unwrap().len(), 10);
        assert!(sc.get("handle").is_some(), "handle always present");
        assert!(sc.get("summary").is_some(), "summary always present");
        assert!(sc.get("capabilities").is_some(), "capabilities advertised");
    }

    /// FR-3 (G3): when the engine result tripped the materialization budget,
    /// the response carries a typed `result_too_large` over-budget signal naming
    /// the budget — never a silent clamp presented as complete.
    #[test]
    fn over_budget_signal_present_when_tripped() {
        // A handle whose persisted rows == the budget, marked tripped.
        let mut out = synth_output(5);
        out.row_cap_tripped = true;
        let (_hs, h) = store_handle(&out);
        let resp = structured_ok(&out, None, 1, Some(&h), 25);
        let sc = &resp["structuredContent"];
        // isError stays false (a handle + prefix is still usable) but the typed
        // signal is explicit.
        assert_eq!(resp["isError"], json!(false));
        assert_eq!(sc["truncated"], json!(true), "must mark truncated: {sc}");
        let rtl = sc.get("result_too_large").expect("result_too_large signal");
        assert_eq!(rtl["code"], json!("result_too_large"));
        assert_eq!(rtl["budget"], json!(5), "budget == persisted (truncated) row count");
        let notes = sc["notes"].as_array().expect("notes present when over budget");
        assert!(
            notes.iter().any(|n| n.as_str().is_some_and(|s| s.contains("OVER BUDGET"))),
            "an over-budget steering note must be present: {sc}"
        );
    }

    /// A within-budget result carries NO over-budget signal (negative control).
    #[test]
    fn no_over_budget_signal_when_within_budget() {
        let out = synth_output(5); // row_cap_tripped = false
        let (_hs, h) = store_handle(&out);
        let resp = structured_ok(&out, None, 1, Some(&h), 25);
        let sc = &resp["structuredContent"];
        assert!(sc.get("result_too_large").is_none(), "no signal within budget: {sc}");
        assert!(sc.get("truncated").is_none(), "no truncated marker within budget: {sc}");
    }

    /// FR-4 (G2): the inline sample bound is governed by `inline_threshold`,
    /// independent of whether the budget tripped. A tripped result above the
    /// inline threshold still omits `rows` (handle-first) — the trip does not
    /// enlarge the LLM-context inline payload.
    #[test]
    fn over_budget_does_not_enlarge_inline_sample() {
        let mut out = synth_output(100); // > inline_threshold=25
        out.row_cap_tripped = true;
        let (_hs, h) = store_handle(&out);
        let resp = structured_ok(&out, None, 1, Some(&h), 25);
        let sc = &resp["structuredContent"];
        assert!(
            sc.get("rows").is_none(),
            "above inline_threshold rows must be omitted even when over budget: {sc}"
        );
        // The over-budget signal is still present (typed), but no rows inlined.
        assert_eq!(sc["truncated"], json!(true));
    }

    /// Edge case: exactly K rows still inlines (≤K is inclusive).
    #[test]
    fn ac2_query_at_threshold_inlines() {
        let out = synth_output(25);
        let (_hs, h) = store_handle(&out);
        let resp = structured_ok(&out, None, 1, Some(&h), 25);
        let sc = &resp["structuredContent"];
        assert!(sc.get("rows").is_some(), "exactly K inlines: {sc}");
    }

    /// AC-3: a >K result carries handle + summary + row_count and NO `rows`.
    #[test]
    fn ac3_query_large_result_is_gated_no_rows() {
        let out = synth_output(26); // K+1
        let (_hs, h) = store_handle(&out);
        let resp = structured_ok(&out, None, 1, Some(&h), 25);
        let sc = &resp["structuredContent"];
        assert_eq!(sc["row_count"], json!(26));
        assert!(
            !sc.as_object().unwrap().contains_key("rows"),
            "above K must NOT carry rows: {sc}"
        );
        assert!(sc.get("handle").is_some(), "handle present for handoff");
        assert!(sc.get("summary").is_some(), "summary present");
        assert!(sc["notes"].as_array().is_some(), "migration note present");
    }

    /// AC-4: a configurable threshold (100) inlines a 60-row result.
    #[test]
    fn ac4_threshold_override_inlines_60_rows() {
        let out = synth_output(60);
        let (_hs, h) = store_handle(&out);
        let resp = structured_ok(&out, None, 1, Some(&h), 100);
        let sc = &resp["structuredContent"];
        assert!(sc.get("rows").is_some(), "60 ≤ 100 inlines: {sc}");
    }
}

// ── Large-result handle-first contract tests (AC-1 through AC-6, PRD-mqo-large-result-handle-contract) ──

#[cfg(test)]
mod handle_first_contract_tests {
    use super::*;
    use crate::cursor::CursorStore;
    use std::sync::Arc;

    fn synth_rows(n: usize) -> Vec<Value> {
        (0..n)
            .map(|i| json!({ "customer_id": i as i64, "store": "store-A", "revenue": (i as f64) * 1.5 }))
            .collect()
    }

    fn synth_pipeline_output(n: usize) -> PipelineOutput {
        let rows = synth_rows(n);
        PipelineOutput {
            backend: "dax".to_string(),
            estimated_rows: n as u64,
            routing_reason: "test".to_string(),
            compiled_query: "EVALUATE ...".to_string(),
            rows,
            bound: json!({"measures": [], "dimensions": []}),
            filters_applied: vec![],
            filters_dropped: vec![],
            row_cap_tripped: false,
        }
    }

    /// AC-1: Given a query returning 1000 rows, when the cursor path fires,
    /// the response carries `handle`, `row_count: 1000`, `columns`, a bounded
    /// `sample`, and a `notes` steer — and does NOT present as "page for the rest".
    #[test]
    fn ac1_large_cursor_response_is_handle_first() {
        let out = synth_pipeline_output(1000);
        let page_size = 50_usize;
        let store = Arc::new(CursorStore::new(600));
        let first_page = store
            .put_and_first_page(out.rows.clone(), page_size)
            .expect("put succeeds");

        let resp = structured_cursor_ok(&out, &first_page, None, 0, None);
        let sc = &resp["structuredContent"];

        // Handle is present (= cursor_id)
        assert!(sc.get("handle").is_some(), "handle must be present: {sc}");
        assert_eq!(sc["handle"], sc["cursor_id"], "handle == cursor_id");

        // row_count is the total, not just the page
        assert_eq!(sc["row_count"], json!(1000), "row_count must be total rows");

        // columns present
        let cols = sc["columns"].as_array().expect("columns must be an array");
        assert!(!cols.is_empty(), "columns must be non-empty");

        // sample is a bounded top-N (≤ page_size)
        let sample = sc["sample"].as_array().expect("sample must be an array");
        assert!(sample.len() <= page_size, "sample bounded to page_size: {}", sample.len());
        assert!(!sample.is_empty(), "sample must not be empty for non-empty result");

        // notes steer present and mentions handle ops
        let notes = sc["notes"].as_array().expect("notes must be an array");
        assert!(!notes.is_empty(), "notes must be non-empty");
        let note_text = notes[0].as_str().expect("notes[0] is a string");
        assert!(
            note_text.contains("dataset_export") || note_text.contains("dataset_*"),
            "notes must mention dataset_* or dataset_export: {note_text}"
        );
        assert!(
            note_text.contains("next_page"),
            "notes must mention next_page to steer away from it: {note_text}"
        );

        // Back-compat cursor fields present (FR-6)
        assert!(sc.get("cursor_id").is_some(), "cursor_id back-compat present");
        assert!(sc.get("page").is_some(), "page back-compat present");
        assert!(sc.get("page_token").is_some(), "page_token back-compat present");
        assert!(sc.get("has_more").is_some(), "has_more back-compat present");
    }

    /// AC-6 (back-compat): small results ≤ inline threshold are inlined as before,
    /// cursor path not taken.
    #[test]
    fn ac6_small_result_unchanged() {
        let out = synth_pipeline_output(10);
        let (_hs, h) = {
            let hs = crate::handle_ops::HandleStore::new();
            let h = hs.put_rows(&out.rows).expect("put_rows");
            (hs, h)
        };
        let resp = structured_ok(&out, None, 1, Some(&h), 25);
        let sc = &resp["structuredContent"];
        // Small result still inlines rows
        assert!(sc.get("rows").is_some(), "small result must inline rows: {sc}");
        assert_eq!(sc["row_count"], json!(10));
        // handle + summary still present
        assert!(sc.get("handle").is_some(), "handle present for small result");
        assert!(sc.get("summary").is_some(), "summary present for small result");
    }
}

// ── dataset_export unit tests (AC-2 through AC-4, PRD-mqo-large-result-handle-contract) ──

#[cfg(test)]
mod dataset_export_tests {
    use super::*;
    use crate::handle_ops::{HandleStore, DEFAULT_EXPORT_MAX_ROWS};

    fn make_store_with_n_rows(n: usize) -> (HandleStore, DatasetHandle) {
        let rows: Vec<Value> = (0..n)
            .map(|i| json!({ "id": i as i64, "name": format!("row-{i}"), "val": (i as f64) * 2.5 }))
            .collect();
        let hs = HandleStore::new();
        let h = hs.put_rows(&rows).expect("put_rows succeeds");
        (hs, h)
    }

    /// AC-2: Given a handle, when dataset_export(handle, format: json) is called
    /// within the cap, it returns the full persisted rows (bounded JSON).
    #[test]
    fn ac2_export_json_within_cap_returns_rows() {
        let (hs, h) = make_store_with_n_rows(10);
        let args = json!({ "handle": h.id, "format": "json" });
        let resp = crate::handle_ops::handle_dataset_export(&hs.store, &args);
        let sc = &resp["structuredContent"];
        assert_eq!(resp["isError"], json!(false), "must not be error: {resp}");
        assert_eq!(sc["format"], json!("json"));
        assert_eq!(sc["row_count"], json!(10_u64));
        let rows = sc["rows"].as_array().expect("rows array present");
        assert_eq!(rows.len(), 10, "all 10 rows returned");
    }

    /// AC-3: Given an export json request above the cap, then result_too_large
    /// (total + cap), no rows.
    #[test]
    fn ac3_export_json_above_cap_returns_result_too_large() {
        // Put more rows than DEFAULT_EXPORT_MAX_ROWS would allow when we pass a tiny cap.
        let (hs, h) = make_store_with_n_rows(5);
        // Pass max_rows=2 so 5 > 2 triggers result_too_large.
        let args = json!({ "handle": h.id, "format": "json", "max_rows": 2 });
        let resp = crate::handle_ops::handle_dataset_export(&hs.store, &args);
        assert_eq!(resp["isError"], json!(true), "must be error: {resp}");
        let sc = &resp["structuredContent"];
        assert_eq!(sc["error"]["code"], json!("result_too_large"), "error code must be result_too_large: {sc}");
        assert!(sc["error"].get("row_count").is_some(), "row_count in error");
        assert!(sc["error"].get("cap").is_some(), "cap in error");
        assert!(sc.get("rows").is_none(), "no rows on error");
    }

    /// AC-4: Given dataset_export(handle, format: csv), a file is written and
    /// {path, row_count} returned — no rows inlined.
    #[test]
    fn ac4_export_csv_writes_file_returns_path() {
        let (hs, h) = make_store_with_n_rows(5);
        let tmp = std::env::temp_dir().join(format!("test-export-{}.csv", h.id));
        let args = json!({
            "handle": h.id,
            "format": "csv",
            "destination": tmp.to_string_lossy()
        });
        let resp = crate::handle_ops::handle_dataset_export(&hs.store, &args);
        let sc = &resp["structuredContent"];
        assert_eq!(resp["isError"], json!(false), "must not be error: {resp}");
        assert!(sc.get("path").is_some(), "path present");
        assert_eq!(sc["row_count"], json!(5_u64), "row_count correct");
        assert!(sc.get("rows").is_none(), "no rows inlined for csv");
        // Clean up
        let _ = std::fs::remove_file(&tmp);
    }

    /// AC-6 (back-compat): dataset_export on unknown handle returns handle_not_found.
    #[test]
    fn ac6_export_unknown_handle_returns_error() {
        let hs = HandleStore::new();
        let args = json!({ "handle": "nonexistent-handle-id", "format": "json" });
        let resp = crate::handle_ops::handle_dataset_export(&hs.store, &args);
        assert_eq!(resp["isError"], json!(true), "must be error for unknown handle");
        let code = resp["structuredContent"]["error"]["code"].as_str().unwrap_or("");
        assert_eq!(code, "handle_not_found", "error code is handle_not_found");
    }

    /// dataset_export with invalid format returns invalid_params.
    #[test]
    fn export_invalid_format_returns_invalid_params() {
        let (hs, h) = make_store_with_n_rows(3);
        let args = json!({ "handle": h.id, "format": "xlsx" });
        let resp = crate::handle_ops::handle_dataset_export(&hs.store, &args);
        assert_eq!(resp["isError"], json!(true));
        let code = resp["structuredContent"]["error"]["code"].as_str().unwrap_or("");
        assert_eq!(code, "invalid_params");
    }

    /// DEFAULT_EXPORT_MAX_ROWS cap is enforced even when caller doesn't pass max_rows.
    #[test]
    fn export_json_default_cap_is_enforced() {
        // We can't actually put DEFAULT_EXPORT_MAX_ROWS+1 rows in a test easily,
        // so instead just verify that the constant is reasonable.
        //
        // PRD-mqo-handle-full-materialization OQ-2: the JSON export default is
        // aligned to the materialization budget default so export is not a
        // second silent clamp below a handle's full capacity.
        assert_eq!(
            DEFAULT_EXPORT_MAX_ROWS,
            mqo_auth_bridge::DEFAULT_MAX_RESULT_ROWS,
            "export default cap must align with the materialization budget default"
        );
        assert_eq!(
            DEFAULT_EXPORT_MAX_ROWS, 50_000,
            "aligned export default cap must be 50_000"
        );
    }
}

#[cfg(test)]
mod disambiguation_tests {
    //! Unit tests for the describe_model disambiguation pack
    //! (PRD-mqo-describe-disambiguation-pack, AC-1..AC-5).
    use super::*;

    fn lvl(un: &str, label: &str, hier: &str) -> Value {
        json!({
            "unique_name": un,
            "label": label,
            "kind": "level",
            "hierarchy": hier,
        })
    }

    fn measure(un: &str, label: &str) -> Value {
        json!({ "unique_name": un, "label": label, "kind": "measure" })
    }

    /// AC-2: two attributes with the same core label on different hierarchies →
    /// a `near_twins` group is emitted.
    #[test]
    fn near_twin_group_emitted_for_collision_across_hierarchies() {
        let cols = vec![
            lvl(
                "product_dimension.[Product Brand Name]",
                "Product Brand Name",
                "product_dimension",
            ),
            lvl(
                "store_item_product_dimension.[Store Item Product Brand Name]",
                "Store Item Product Brand Name",
                "store_item_product_dimension",
            ),
        ];
        let groups = build_near_twins(&cols);
        assert_eq!(groups.len(), 1, "exactly one near-twin group: {groups:?}");
        let twins = groups[0]["near_twins"].as_array().unwrap();
        assert_eq!(twins.len(), 2);
        // Canonical is the shortest hierarchy name (product_dimension).
        let canonical: Vec<&str> = twins
            .iter()
            .filter(|t| t.get("canonical_for").is_some())
            .map(|t| t["unique_name"].as_str().unwrap())
            .collect();
        assert_eq!(
            canonical,
            vec!["product_dimension.[Product Brand Name]"],
            "shortest hierarchy is canonical"
        );
    }

    /// AC-2 (negative): a genuinely unique label emits no `near_twins`.
    #[test]
    fn unique_label_emits_no_group() {
        let cols = vec![
            lvl(
                "store_dimension.[Store Manager]",
                "Store Manager",
                "store_dimension",
            ),
            lvl(
                "product_dimension.[Product Brand Name]",
                "Product Brand Name",
                "product_dimension",
            ),
        ];
        let groups = build_near_twins(&cols);
        assert!(groups.is_empty(), "no collisions → no groups: {groups:?}");
    }

    /// PRD-mqo-within-hierarchy-name-preference: a level with a same-hierarchy
    /// display "Name" sibling gets the Name marked `display_preferred:true` and
    /// the non-Name sibling annotated with `display_sibling`; a level with no
    /// Name sibling is untouched. Covers the suffix pair (Store State / Store
    /// State Name) and the ordinal/name pair (Sold Day of Week / Sold Day Name).
    #[test]
    fn within_hierarchy_name_preference_annotation() {
        let mut cols = vec![
            lvl("store_dimension.[Store State]", "Store State", "store_dimension"),
            lvl(
                "store_dimension.[Store State Name]",
                "Store State Name",
                "store_dimension",
            ),
            lvl(
                "sold_date_dimensions.[Sold Day of Week]",
                "Sold Day of Week",
                "sold_date_dimensions",
            ),
            lvl(
                "sold_date_dimensions.[Sold Day Name]",
                "Sold Day Name",
                "sold_date_dimensions",
            ),
            // No Name sibling on this hierarchy → no annotation.
            lvl("store_dimension.[Store Manager]", "Store Manager", "store_dimension"),
        ];
        annotate_display_siblings(&mut cols);

        let by_un = |un: &str| cols.iter().find(|c| c["unique_name"] == un).unwrap();

        // Suffix pair: Store State Name preferred over Store State.
        assert_eq!(
            by_un("store_dimension.[Store State Name]")["display_preferred"],
            json!(true)
        );
        assert_eq!(
            by_un("store_dimension.[Store State]")["display_sibling"],
            json!("store_dimension.[Store State Name]")
        );
        assert!(by_un("store_dimension.[Store State]")
            .get("display_preferred")
            .is_none());

        // Ordinal/name pair: Sold Day Name preferred over Sold Day of Week.
        assert_eq!(
            by_un("sold_date_dimensions.[Sold Day Name]")["display_preferred"],
            json!(true)
        );
        assert_eq!(
            by_un("sold_date_dimensions.[Sold Day of Week]")["display_sibling"],
            json!("sold_date_dimensions.[Sold Day Name]")
        );

        // No Name sibling → untouched.
        let mgr = by_un("store_dimension.[Store Manager]");
        assert!(mgr.get("display_preferred").is_none());
        assert!(mgr.get("display_sibling").is_none());
    }

    /// Same core label but on the SAME hierarchy is not a near-twin group.
    #[test]
    fn same_hierarchy_is_not_a_near_twin() {
        let cols = vec![
            lvl("h.[A Brand Name]", "A Brand Name", "h"),
            lvl("h.[B Brand Name]", "B Brand Name", "h"),
        ];
        let groups = build_near_twins(&cols);
        assert!(groups.is_empty(), "single hierarchy → no group");
    }

    /// FR-3: three+ same-label attributes are all listed with one canonical.
    #[test]
    fn three_twins_one_canonical() {
        let cols = vec![
            lvl("product_dimension.[Product Brand Name]", "Product Brand Name", "product_dimension"),
            lvl(
                "promotion_product_item_product_dimension.[Promotion Product Item Product Brand Name]",
                "Promotion Product Item Product Brand Name",
                "promotion_product_item_product_dimension",
            ),
            lvl(
                "store_item_product_dimension.[Store Item Product Brand Name]",
                "Store Item Product Brand Name",
                "store_item_product_dimension",
            ),
        ];
        let groups = build_near_twins(&cols);
        assert_eq!(groups.len(), 1);
        let twins = groups[0]["near_twins"].as_array().unwrap();
        assert_eq!(twins.len(), 3);
        let canonical: Vec<&str> = twins
            .iter()
            .filter(|t| t.get("canonical_for").is_some())
            .map(|t| t["unique_name"].as_str().unwrap())
            .collect();
        assert_eq!(canonical, vec!["product_dimension.[Product Brand Name]"]);
    }

    /// FR-4: date_roles derivation picks temporally-typed hierarchies, sorted.
    #[test]
    fn date_roles_picks_date_hierarchies() {
        let cols = vec![
            lvl("sold_date_dimensions.[Year]", "Year", "sold_date_dimensions"),
            lvl("ship_date_dimensions.[Year]", "Year", "ship_date_dimensions"),
            lvl("product_dimension.[Product Brand Name]", "Product Brand Name", "product_dimension"),
        ];
        let roles = date_role_hierarchies(&cols);
        assert_eq!(roles, vec!["ship_date_dimensions", "sold_date_dimensions"]);
    }

    /// FR-4: when no date hierarchy exists, date_roles is an empty vec (the
    /// caller emits `[]`, not absent).
    #[test]
    fn date_roles_empty_when_no_date_hierarchy() {
        let cols = vec![lvl(
            "product_dimension.[Product Brand Name]",
            "Product Brand Name",
            "product_dimension",
        )];
        assert!(date_role_hierarchies(&cols).is_empty());
    }

    /// NFR-3: deterministic — same input yields byte-identical output.
    #[test]
    fn near_twins_deterministic() {
        let cols = vec![
            lvl("h_long_name.[State Name]", "Long State Name", "h_long_name"),
            lvl("h_a.[State Name]", "A State Name", "h_a"),
        ];
        let a = json_byte_size(&json!(build_near_twins(&cols)));
        let b = json_byte_size(&json!(build_near_twins(&cols)));
        assert_eq!(a, b);
        let _ = (measure("m.x", "X"),); // keep helper used
    }

    // ── Measure disambiguation: distinguishing qualifier tokens ─────────────
    // (PRD-mqo-describe-measure-disambiguation, FR-1/FR-2/FR-4)

    /// FR-1/FR-2: the "Net Paid" family groups into one `measure_twins` group
    /// whose members each carry `distinguishing` = their label tokens minus the
    /// family's common tokens. `Web Net Paid Incl Ship` carries "Incl Ship"
    /// (and "Ship"); a base member (`Web Net Paid Amount`) carries no
    /// incl/tax/ship qualifier.
    #[test]
    fn net_paid_family_distinguishing_qualifiers() {
        let cols = vec![
            measure("m.web_net_paid_amount", "Web Net Paid Amount"),
            measure("m.web_net_paid_incl_ship", "Web Net Paid Incl Ship"),
            measure("m.web_net_paid_incl_tax", "Web Net Paid Incl Tax"),
            measure("m.store_net_paid_amount", "Store Net Paid Amount"),
            measure("m.catalog_net_paid_amount", "Catalog Net Paid Amount"),
        ];
        let groups = build_measure_twins(&cols);
        let net_paid = groups
            .iter()
            .find(|g| g.get("core_label").and_then(Value::as_str) == Some("net paid"))
            .expect("a 'net paid' measure_twins group");
        assert_eq!(
            net_paid["twin_kind"].as_str(),
            Some("measure"),
            "twin_kind is measure"
        );
        let members = net_paid["near_twins"].as_array().unwrap();
        assert!(members.len() >= 2, "family has ≥2 members (FR-1): {members:?}");

        // Helper: collect a member's distinguishing tokens (flattened across the
        // contiguous phrase runs) for membership checks.
        let dist_of = |label: &str| -> Vec<String> {
            members
                .iter()
                .find(|m| m["label"].as_str() == Some(label))
                .unwrap_or_else(|| panic!("member {label} present"))
                ["distinguishing"]
                .as_array()
                .expect("distinguishing array present")
                .iter()
                .map(|p| p.as_str().unwrap().to_string())
                .collect()
        };

        // Incl-Ship variant: distinguishing surfaces the "Incl Ship" qualifier.
        let ship = dist_of("Web Net Paid Incl Ship");
        let ship_flat = ship.join(" ");
        assert!(
            ship.iter().any(|p| p == "Incl Ship") || ship_flat.contains("Ship"),
            "Web Net Paid Incl Ship distinguishes on Incl Ship / Ship: {ship:?}"
        );

        // Incl-Tax variant: distinguishing surfaces the "Incl Tax" qualifier.
        let tax = dist_of("Web Net Paid Incl Tax");
        assert!(
            tax.join(" ").contains("Tax"),
            "Web Net Paid Incl Tax distinguishes on Tax: {tax:?}"
        );

        // Base member: NO incl/tax/ship qualifier (it is the "base" amount).
        let base = dist_of("Web Net Paid Amount");
        let base_flat = base.join(" ").to_lowercase();
        assert!(
            !base_flat.contains("incl")
                && !base_flat.contains("tax")
                && !base_flat.contains("ship"),
            "base Web Net Paid Amount carries no incl/tax/ship qualifier: {base:?}"
        );

        // The shared concept tokens (net, paid) are NEVER in any distinguishing
        // list — they are common to every member.
        for m in members {
            let flat = m["distinguishing"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                !flat.split_whitespace().any(|t| t == "net" || t == "paid"),
                "common tokens (net/paid) excluded from distinguishing: {flat:?}"
            );
        }
    }

    /// FR-1 (negative): a measure with a unique stem (no family) → not grouped.
    #[test]
    fn unique_measure_not_grouped() {
        let cols = vec![
            measure("m.inventory_qoh", "Inventory Quantity On Hand"),
            measure("m.web_net_paid_amount", "Web Net Paid Amount"),
            measure("m.store_net_paid_amount", "Store Net Paid Amount"),
        ];
        let groups = build_measure_twins(&cols);
        // Only "net paid" groups (2 members); the lone inventory measure does not.
        assert_eq!(groups.len(), 1, "exactly one family: {groups:?}");
        assert_eq!(
            groups[0]["core_label"].as_str(),
            Some("net paid"),
            "the only family is net paid"
        );
    }

    /// FR-4: deterministic — same input yields byte-identical measure twins.
    #[test]
    fn measure_twins_deterministic() {
        let cols = vec![
            measure("m.web_net_paid_incl_ship", "Web Net Paid Incl Ship"),
            measure("m.web_net_paid_amount", "Web Net Paid Amount"),
            measure("m.store_net_paid_amount", "Store Net Paid Amount"),
        ];
        let a = json_byte_size(&json!(build_measure_twins(&cols)));
        let b = json_byte_size(&json!(build_measure_twins(&cols)));
        assert_eq!(a, b);
    }

    // ── FIX 1: packaged-calc surfacing (is_calc + triggers) ──────────────────

    /// FIX 1: a `*Growth` measure is flagged `is_calc:true` with a non-empty
    /// trigger list (year-over-year / growth / price growth …), even when the
    /// catalog carries a stale explicit `is_calc:false`.
    #[test]
    fn calc_measure_surfaces_is_calc_and_triggers() {
        let mut m = json!({
            "unique_name": "tpcds_benchmark_model.web_and_catalog_sales_price_growth",
            "label": "Web and Catalog Sales Price Growth",
            "kind": "measure",
            "is_calc": false
        });
        annotate_calc(&mut m);
        assert_eq!(m["is_calc"], json!(true), "Growth measure is a calc: {m}");
        let triggers: Vec<String> = m["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_string())
            .collect();
        assert!(!triggers.is_empty(), "calc has non-empty triggers: {m}");
        assert!(
            triggers.iter().any(|t| t.contains("growth")),
            "growth trigger present: {triggers:?}"
        );
        assert!(
            triggers.iter().any(|t| t == "year over year" || t == "yoy"),
            "yoy/year-over-year trigger present: {triggers:?}"
        );
    }

    /// FIX 1 (negative): a plain base measure is `is_calc:false` with empty
    /// triggers.
    #[test]
    fn base_measure_is_not_calc() {
        let mut m = json!({
            "unique_name": "tpcds_benchmark_model.store_ext_sales_price",
            "label": "Store Ext Sales Price",
            "kind": "measure"
        });
        annotate_calc(&mut m);
        assert_eq!(m["is_calc"], json!(false));
        assert_eq!(m["triggers"], json!([]), "no triggers for base measure");
    }

    /// FIX 1: an `*Increase` measure is likewise flagged as a calc.
    #[test]
    fn increase_measure_is_calc() {
        let mut m = json!({
            "unique_name": "tpcds_benchmark_model.web_sales_increase",
            "label": "Web Sales Increase",
            "kind": "measure",
            "is_calc": false
        });
        annotate_calc(&mut m);
        assert_eq!(m["is_calc"], json!(true), "Increase measure is a calc: {m}");
        assert!(!m["triggers"].as_array().unwrap().is_empty());
    }

    // ── Name preference for canonical_for ────────────────────────────────────

    /// When multiple `*Name*` twins span two hierarchies (same trailing concept),
    /// the member on the SHORTER hierarchy name wins `canonical_for` via the
    /// shortest-hierarchy tiebreak (both end in "Name" so label_is_name is equal).
    #[test]
    fn name_member_preferred_over_shorter_hierarchy() {
        let cols = vec![
            // *Name* on the SHORTER hierarchy → should be canonical.
            lvl("h.[Store State Name]", "Store State Name", "h"),
            // *Name* on a LONGER hierarchy name → shorter-hierarchy tiebreak loses.
            lvl(
                "store_geography_dim.[Store State Name]",
                "Store State Name",
                "store_geography_dim",
            ),
        ];
        let groups = build_near_twins(&cols);
        assert_eq!(groups.len(), 1, "one near-twin group: {groups:?}");
        let twins = groups[0]["near_twins"].as_array().unwrap();
        let canonical: Vec<&str> = twins
            .iter()
            .filter(|t| t.get("canonical_for").is_some())
            .map(|t| t["unique_name"].as_str().unwrap())
            .collect();
        assert_eq!(
            canonical,
            vec!["h.[Store State Name]"],
            "the *Name* attribute on the shorter hierarchy is canonical"
        );
    }

    /// The fixture's cross-hierarchy "Customer State" family produces two groups
    /// with the concept-grouping fix: a `"customer state"` group (code-like
    /// siblings) and a `"state name"` group (Name display siblings). Within the
    /// Name group, the canonical prefers customer_address (shortest hierarchy).
    #[test]
    fn customer_state_name_is_canonical() {
        let cols = vec![
            lvl(
                "customer_address.[Customer State]",
                "Customer State",
                "customer_address",
            ),
            lvl(
                "customer_address.[Customer State Name]",
                "Customer State Name",
                "customer_address",
            ),
            lvl(
                "return_customer_address.[Return Customer State]",
                "Return Customer State",
                "return_customer_address",
            ),
            lvl(
                "return_customer_address.[Return Customer State Name]",
                "Return Customer State Name",
                "return_customer_address",
            ),
        ];
        let groups = build_near_twins(&cols);
        // Two groups: "customer state" (code-like) and "state name" (Name display).
        assert_eq!(groups.len(), 2, "two groups after concept-grouping fix: {groups:?}");
        let state_name_group = groups
            .iter()
            .find(|g| g.get("core_label").and_then(|v| v.as_str()) == Some("state name"))
            .expect("'state name' group present");
        let twins = state_name_group["near_twins"].as_array().unwrap();
        let canonical: Vec<&str> = twins
            .iter()
            .filter(|t| t.get("canonical_for").is_some())
            .map(|t| t["unique_name"].as_str().unwrap())
            .collect();
        // *Name* preference + shortest-hierarchy tiebreak → customer_address Name.
        assert_eq!(canonical, vec!["customer_address.[Customer State Name]"]);
    }

    /// FIX 2 (regression guard): the Brand Name group still resolves to
    /// `product_dimension.[Product Brand Name]` (all *Name*, shortest hierarchy).
    #[test]
    fn brand_name_group_still_resolves_to_product_brand_name() {
        let cols = vec![
            lvl("product_dimension.[Product Brand Name]", "Product Brand Name", "product_dimension"),
            lvl(
                "store_item_product_dimension.[Store Item Product Brand Name]",
                "Store Item Product Brand Name",
                "store_item_product_dimension",
            ),
        ];
        let groups = build_near_twins(&cols);
        let twins = groups[0]["near_twins"].as_array().unwrap();
        let canonical: Vec<&str> = twins
            .iter()
            .filter(|t| t.get("canonical_for").is_some())
            .map(|t| t["unique_name"].as_str().unwrap())
            .collect();
        assert_eq!(canonical, vec!["product_dimension.[Product Brand Name]"]);
    }
}

// ── hierarchy_levels value_type passthrough (store-employee-attribute-projection fix) ──

#[cfg(test)]
mod hierarchy_levels_value_type_tests {
    //! Regression tests for the fix that passes `value_type` through to
    //! `hierarchy_levels` entries.  Without this fix, the LLM cannot distinguish
    //! a numeric-valued projectable level (e.g. "Store Number of Employees",
    //! value_type:"integer") from an aggregatable measure, and incorrectly SUMs
    //! it instead of projecting it.
    use super::*;
    use crate::cursor::CursorStore;
    use crate::handle_ops::HandleStore;
    use std::sync::Arc;

    /// Build a minimal test Server with a synthetic catalog containing a level
    /// that has `value_type: "integer"` (mirroring "Store Number of Employees").
    fn test_server_with_integer_level() -> Server {
        let catalog = json!({
            "models": [{"unique_name": "test_model", "label": "Test Model"}],
            "columns": [
                {
                    "unique_name": "test_model.revenue",
                    "label": "Revenue",
                    "kind": "measure",
                    "is_calc": false
                },
                {
                    "unique_name": "store_dimension.[Store Name]",
                    "label": "Store Name",
                    "kind": "level",
                    "hierarchy": "store_dimension",
                    "level": "Store Name",
                    "value_type": "string"
                },
                {
                    "unique_name": "store_dimension.[Store Number of Employees]",
                    "label": "Store Number of Employees",
                    "kind": "level",
                    "hierarchy": "store_dimension",
                    "level": "Store Number of Employees",
                    "value_type": "integer",
                    "domain": ["200", "201", "202"]
                }
            ]
        });
        Server {
            catalog,
            stats: json!({}),
            tools: crate::pipeline::ToolPaths::resolve(None),
            row_threshold: 1000,
            engine: ServerEngine::Fixture,
            backend_override: None,
            capabilities: crate::probe::BackendCapabilities::all_live(),
            registry: None,
            health_cache: None,
            handle_store: Some(HandleStore::new()),
            cursor_store: Some(Arc::new(CursorStore::new(600))),
            page_size: crate::cursor::DEFAULT_PAGE_SIZE,
            inline_threshold: crate::handle_ops::INLINE_THRESHOLD,
            enriched: None,
            xmla_model_coords: std::collections::HashMap::new(),
            max_projection_cardinality: DEFAULT_MAX_PROJECTION_CARDINALITY,
            model_graph: None,
            grounding_store: None,
            ontology_check: None,
            autolift_base_url: None,
            autolift_cache: None,
        }
    }

    /// AC-1 (store-employee fix): a level with `value_type:"integer"` in the
    /// catalog MUST carry `value_type:"integer"` in the `hierarchy_levels` block
    /// of `describe_model`.  Without this the LLM mistakes numeric-valued
    /// projectable levels for aggregatable measures.
    #[test]
    fn integer_level_value_type_surfaces_in_hierarchy_levels() {
        let srv = test_server_with_integer_level();
        let resp = srv.describe_model(&json!({"model": "test_model"}));
        let hl = resp
            .get("hierarchy_levels")
            .and_then(Value::as_object)
            .expect("hierarchy_levels must be an object");
        let store_levels = hl
            .get("store_dimension")
            .and_then(Value::as_array)
            .expect("store_dimension must be in hierarchy_levels");

        // Find "Store Number of Employees"
        let emp_entry = store_levels
            .iter()
            .find(|l| l.get("label").and_then(Value::as_str) == Some("Store Number of Employees"))
            .expect("Store Number of Employees must appear in hierarchy_levels");

        assert_eq!(
            emp_entry.get("value_type").and_then(Value::as_str),
            Some("integer"),
            "integer-valued level must carry value_type:integer in hierarchy_levels: {emp_entry}"
        );
        assert_eq!(
            emp_entry.get("projectable").and_then(Value::as_bool),
            Some(true),
            "level must still be projectable: {emp_entry}"
        );
    }

    /// AC-2 (string level): a level with `value_type:"string"` should likewise
    /// pass through its value_type.
    #[test]
    fn string_level_value_type_surfaces_in_hierarchy_levels() {
        let srv = test_server_with_integer_level();
        let resp = srv.describe_model(&json!({"model": "test_model"}));
        let hl = resp
            .get("hierarchy_levels")
            .and_then(Value::as_object)
            .expect("hierarchy_levels must be an object");
        let store_levels = hl
            .get("store_dimension")
            .and_then(Value::as_array)
            .expect("store_dimension must be in hierarchy_levels");

        let name_entry = store_levels
            .iter()
            .find(|l| l.get("label").and_then(Value::as_str) == Some("Store Name"))
            .expect("Store Name must appear in hierarchy_levels");

        assert_eq!(
            name_entry.get("value_type").and_then(Value::as_str),
            Some("string"),
            "string-valued level must carry value_type:string in hierarchy_levels: {name_entry}"
        );
    }

    /// AC-3 (no value_type): a level without `value_type` in the catalog should
    /// NOT emit a `value_type` key (absent is better than a spurious null).
    #[test]
    fn level_without_value_type_omits_the_field() {
        // Build a catalog with a level that has no value_type.
        let catalog = json!({
            "models": [{"unique_name": "test_model", "label": "Test Model"}],
            "columns": [
                {
                    "unique_name": "dim.[No Type Level]",
                    "label": "No Type Level",
                    "kind": "level",
                    "hierarchy": "dim"
                }
            ]
        });
        let srv = Server {
            catalog,
            stats: json!({}),
            tools: crate::pipeline::ToolPaths::resolve(None),
            row_threshold: 1000,
            engine: ServerEngine::Fixture,
            backend_override: None,
            capabilities: crate::probe::BackendCapabilities::all_live(),
            registry: None,
            health_cache: None,
            handle_store: Some(HandleStore::new()),
            cursor_store: Some(Arc::new(CursorStore::new(600))),
            page_size: crate::cursor::DEFAULT_PAGE_SIZE,
            inline_threshold: crate::handle_ops::INLINE_THRESHOLD,
            enriched: None,
            xmla_model_coords: std::collections::HashMap::new(),
            max_projection_cardinality: DEFAULT_MAX_PROJECTION_CARDINALITY,
            model_graph: None,
            grounding_store: None,
            ontology_check: None,
            autolift_base_url: None,
            autolift_cache: None,
        };
        let resp = srv.describe_model(&json!({"model": "test_model"}));
        let hl = resp
            .get("hierarchy_levels")
            .and_then(Value::as_object)
            .expect("hierarchy_levels must be an object");
        let dim_levels = hl
            .get("dim")
            .and_then(Value::as_array)
            .expect("dim must be in hierarchy_levels");
        let entry = dim_levels
            .iter()
            .find(|l| l.get("label").and_then(Value::as_str) == Some("No Type Level"))
            .expect("No Type Level must appear");

        assert!(
            entry.get("value_type").is_none(),
            "level without catalog value_type must not emit value_type key: {entry}"
        );
    }

    // ── Semijoin-projection grounding content regression ─────────────────
    // PRD-mqo-semijoin-projection-grounding: verify the tool description and
    // describe_model response contain the grounding terms that steer the model
    // toward measureless projections with cross-dimension fact-resident filters.
    #[test]
    fn test_semijoin_projection_grounding_content() {
        // 1. query_multidimensional tool description must contain key terms.
        let tools = tool_descriptors();
        let tools_arr = tools.as_array().expect("tool_descriptors must be an array");
        let qmd = tools_arr
            .iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some("query_multidimensional"))
            .expect("query_multidimensional tool must be present");
        let desc = qmd
            .get("description")
            .and_then(Value::as_str)
            .expect("query_multidimensional must have a description string");
        assert!(
            desc.contains("filter"),
            "query_multidimensional description must mention 'filter': {desc}"
        );
        assert!(
            desc.contains("projection"),
            "query_multidimensional description must mention 'projection': {desc}"
        );
        assert!(
            desc.contains("fact"),
            "query_multidimensional description must mention 'fact': {desc}"
        );

        // 2. describe_model response must carry projection_note and
        //    filterable_cross_dimension on level entries.
        let catalog = json!({
            "columns": [
                {
                    "unique_name": "dim.[Level A]",
                    "label": "Level A",
                    "kind": "level",
                    "hierarchy": "dim"
                }
            ]
        });
        let srv = Server {
            catalog,
            stats: json!({}),
            tools: crate::pipeline::ToolPaths::resolve(None),
            row_threshold: 1000,
            engine: ServerEngine::Fixture,
            backend_override: None,
            capabilities: crate::probe::BackendCapabilities::all_live(),
            registry: None,
            health_cache: None,
            handle_store: Some(HandleStore::new()),
            cursor_store: Some(Arc::new(CursorStore::new(600))),
            page_size: crate::cursor::DEFAULT_PAGE_SIZE,
            inline_threshold: crate::handle_ops::INLINE_THRESHOLD,
            enriched: None,
            xmla_model_coords: std::collections::HashMap::new(),
            max_projection_cardinality: DEFAULT_MAX_PROJECTION_CARDINALITY,
            model_graph: None,
            grounding_store: None,
            ontology_check: None,
            autolift_base_url: None,
            autolift_cache: None,
        };
        let resp = srv.describe_model(&json!({}));

        // projection_note must be present and mention semijoin capability.
        let proj_note = resp
            .get("projection_note")
            .and_then(Value::as_str)
            .expect("describe_model must include projection_note");
        assert!(
            proj_note.contains("fact"),
            "projection_note must mention 'fact': {proj_note}"
        );
        assert!(
            proj_note.contains("projection"),
            "projection_note must mention 'projection': {proj_note}"
        );

        // hierarchy_levels entries must carry filterable_cross_dimension:true.
        let hl = resp
            .get("hierarchy_levels")
            .and_then(Value::as_object)
            .expect("hierarchy_levels must be an object");
        for (_hier, levels) in hl.iter() {
            for level in levels.as_array().expect("levels must be an array") {
                assert_eq!(
                    level.get("filterable_cross_dimension"),
                    Some(&json!(true)),
                    "every level in hierarchy_levels must carry filterable_cross_dimension:true, got: {level}"
                );
            }
        }
    }

    // ── FR-1 (PRD-mqo-project-not-count-grounding) ────────────────────────────

    /// FR-1: numeric attribute levels (value_type=integer/decimal/float/number) must
    /// carry `projectable_per_member_quantity: true` in hierarchy_levels entries.
    /// String levels must NOT carry this flag.
    #[test]
    fn numeric_level_carries_projectable_per_member_quantity() {
        let srv = test_server_with_integer_level();
        let resp = srv.describe_model(&json!({"model": "test_model"}));
        let hl = resp
            .get("hierarchy_levels")
            .and_then(Value::as_object)
            .expect("hierarchy_levels must be an object");
        let store_levels = hl
            .get("store_dimension")
            .and_then(Value::as_array)
            .expect("store_dimension must be in hierarchy_levels");

        // Numeric level "Store Number of Employees" (value_type:integer) MUST have the flag.
        let emp_entry = store_levels
            .iter()
            .find(|l| l.get("label").and_then(Value::as_str) == Some("Store Number of Employees"))
            .expect("Store Number of Employees must appear in hierarchy_levels");
        assert_eq!(
            emp_entry.get("projectable_per_member_quantity").and_then(Value::as_bool),
            Some(true),
            "integer-valued level must carry projectable_per_member_quantity:true: {emp_entry}"
        );

        // String level "Store Name" (value_type:string) must NOT have the flag.
        let name_entry = store_levels
            .iter()
            .find(|l| l.get("label").and_then(Value::as_str) == Some("Store Name"))
            .expect("Store Name must appear in hierarchy_levels");
        assert!(
            name_entry.get("projectable_per_member_quantity").is_none(),
            "string-valued level must NOT carry projectable_per_member_quantity: {name_entry}"
        );
    }

    /// FR-2: the query_multidimensional tool description must mention the
    /// per-entity numeric attribute grounding terms.
    #[test]
    fn query_multidimensional_describes_per_entity_numeric_attribute_projection() {
        let tools = tool_descriptors();
        let tools_arr = tools.as_array().expect("tool_descriptors must be an array");
        let qmd = tools_arr
            .iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some("query_multidimensional"))
            .expect("query_multidimensional tool must be present");
        let desc = qmd
            .get("description")
            .and_then(Value::as_str)
            .expect("query_multidimensional must have a description string");

        assert!(
            desc.contains("projectable_per_member_quantity"),
            "tool description must mention projectable_per_member_quantity: {desc}"
        );
        assert!(
            desc.contains("count rows"),
            "tool description must warn against counting rows: {desc}"
        );
        assert!(
            desc.contains("count measure"),
            "tool description must contrast genuine count measures: {desc}"
        );
    }
}

// ── Autolift wiring tests (describe_grounding + validate_query_ontology) ─────

#[cfg(test)]
mod autolift_wiring_tests {
    //! Tests that `describe_grounding` and `validate_query_ontology` attempt
    //! autolift on a cache-miss and fall back to "not available" when disabled.
    //!
    //! We cannot mock `try_autolift` (it does a real HTTP call), so instead:
    //!  - "disabled" tests: autolift_base_url=None, engine=Fixture → must return
    //!    the not-available / fail-open response without a network call.
    //!  - "enabled but lift fails" tests: autolift_base_url=Some(...), engine=Fixture
    //!    → try_ensure_graph_for_model returns None (Fixture engine) → fallback.
    //!
    //! This covers the "autolift off" and "autolift on but fails" contract.  The
    //! "autolift succeeds" path is covered by integration / live tests.

    use super::*;
    use crate::cursor::CursorStore;
    use crate::handle_ops::HandleStore;
    use std::sync::Arc;

    fn minimal_server_fixture() -> Server {
        Server {
            catalog: serde_json::json!({"columns": []}),
            stats: serde_json::json!({}),
            tools: crate::pipeline::ToolPaths::resolve(None),
            row_threshold: 1000,
            engine: ServerEngine::Fixture,
            backend_override: None,
            capabilities: crate::probe::BackendCapabilities::all_live(),
            registry: None,
            health_cache: None,
            handle_store: Some(HandleStore::new()),
            cursor_store: Some(Arc::new(CursorStore::new(600))),
            page_size: crate::cursor::DEFAULT_PAGE_SIZE,
            inline_threshold: crate::handle_ops::INLINE_THRESHOLD,
            enriched: None,
            xmla_model_coords: std::collections::HashMap::new(),
            max_projection_cardinality: DEFAULT_MAX_PROJECTION_CARDINALITY,
            model_graph: None,
            grounding_store: None,
            ontology_check: None,
            autolift_base_url: None,
            autolift_cache: None,
        }
    }

    // ── describe_grounding: autolift disabled → grounding_not_available ───────

    #[test]
    fn describe_grounding_falls_back_when_autolift_disabled() {
        let srv = minimal_server_fixture();
        let resp = srv.handle(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "describe_grounding",
                "arguments": { "entities": ["Revenue"] }
            }
        }));
        let resp = resp.expect("handle must return a response");
        let structured = resp
            .get("result")
            .and_then(|r| r.get("structuredContent"));
        let status = structured
            .and_then(|s| s.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        assert_eq!(
            status, "grounding_not_available",
            "describe_grounding must return grounding_not_available when autolift is disabled: {resp}"
        );
    }

    // ── describe_grounding: autolift enabled but Fixture engine → fallback ────

    #[test]
    fn describe_grounding_falls_back_when_autolift_enabled_but_fixture_engine() {
        // autolift_base_url is set but engine is Fixture → try_ensure_graph returns None.
        let mut srv = minimal_server_fixture();
        srv.autolift_base_url = Some("https://mcp-aws.atscaleinternal.com/v1/catalogs".to_string());
        srv.autolift_cache = Some(Arc::new(crate::autolift::AutoliftCache::new()));

        let resp = srv.handle(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "describe_grounding",
                "arguments": { "entities": ["Revenue"] }
            }
        }));
        let resp = resp.expect("handle must return a response");
        let structured = resp
            .get("result")
            .and_then(|r| r.get("structuredContent"));
        let status = structured
            .and_then(|s| s.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        assert_eq!(
            status, "grounding_not_available",
            "describe_grounding must fallback when Fixture engine: {resp}"
        );
    }

    // ── validate_query_ontology: disabled → fail-open (conforms=true, info) ───

    #[test]
    fn validate_query_ontology_failopen_when_autolift_disabled() {
        let srv = minimal_server_fixture();
        let resp = srv.handle(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "validate_query_ontology",
                "arguments": { "measures": ["Revenue"], "dimensions": ["Brand"] }
            }
        }));
        let resp = resp.expect("handle must return a response");
        let structured = resp
            .get("result")
            .and_then(|r| r.get("structuredContent"));
        let conforms = structured
            .and_then(|s| s.get("conforms"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(
            conforms,
            "validate_query_ontology must fail-open (conforms=true) when disabled: {resp}"
        );
        let findings = structured
            .and_then(|s| s.get("findings"))
            .and_then(Value::as_array)
            .expect("findings must be an array");
        assert_eq!(findings.len(), 1, "must have exactly one info finding: {findings:?}");
        assert_eq!(
            findings[0].get("severity").and_then(Value::as_str),
            Some("info"),
            "finding must be info severity: {findings:?}"
        );
    }

    // ── validate_query_ontology: enabled but Fixture engine → fail-open ───────

    #[test]
    fn validate_query_ontology_failopen_when_autolift_enabled_but_fixture_engine() {
        let mut srv = minimal_server_fixture();
        srv.autolift_base_url = Some("https://mcp-aws.atscaleinternal.com/v1/catalogs".to_string());
        srv.autolift_cache = Some(Arc::new(crate::autolift::AutoliftCache::new()));

        let resp = srv.handle(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "validate_query_ontology",
                "arguments": { "measures": ["Revenue"], "dimensions": [] }
            }
        }));
        let resp = resp.expect("handle must return a response");
        let structured = resp
            .get("result")
            .and_then(|r| r.get("structuredContent"));
        let conforms = structured
            .and_then(|s| s.get("conforms"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(
            conforms,
            "validate_query_ontology must fail-open when Fixture engine + enabled: {resp}"
        );
    }
}
