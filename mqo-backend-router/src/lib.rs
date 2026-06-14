//! # mqo-backend-router
//!
//! Routes a `BoundMqo` (from `mqo-spec`) plus level-cardinality stats to one of:
//! - `dax`  — default; low-cardinality aggregated queries
//! - `mdx`  — shape-triggered: asymmetric axes, drill-through, or cellset
//! - `sql`  — large-extract path when estimated rows exceed the threshold
//!
//! The routing decision is emitted as a JSON object:
//! ```json
//! {
//!   "backend": "dax" | "mdx" | "sql",
//!   "estimated_rows": 1234,
//!   "reason": "...",
//!   "sql_projection": "SELECT ..."   // present only for sql backend
//! }
//! ```

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Level-cardinality stats ────────────────────────────────────────────────

/// Per-level cardinality statistics consumed from `--stats`.
///
/// Keys are level unique names (e.g. `"time.calendar.[Year]"`);
/// values are the number of distinct members at that level.
pub type LevelStats = std::collections::HashMap<String, u64>;

// ── Shape flags ────────────────────────────────────────────────────────────

/// Optional per-query shape hints that force MDX routing.
///
/// These are passed via the JSON wrapper around `BoundMqo` or as separate
/// flags on the CLI (future). For now they are embedded in the stats JSON
/// alongside the level cardinalities so callers don't need a second file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShapeFlags {
    /// True when the query requests asymmetric axes (row/column cross-joins
    /// that cannot be flattened to a single tabular result).
    #[serde(default)]
    pub asymmetric_axes: bool,

    /// True when the query is a drill-through (detail rows, not aggregates).
    #[serde(default)]
    pub drill_through: bool,

    /// True when the caller explicitly requests a cellset (OLAP cube slice).
    #[serde(default)]
    pub cellset_requested: bool,
}

/// Full stats bundle read from `--stats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatBundle {
    /// Cardinality per level unique name.
    #[serde(default)]
    pub level_cardinalities: LevelStats,

    /// Optional shape flags; defaults to all-false if absent.
    #[serde(default)]
    pub shape_flags: ShapeFlags,
}

// ── Backend enum ──────────────────────────────────────────────────────────

/// The chosen backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Dax,
    Mdx,
    Sql,
}

// ── Routing decision ──────────────────────────────────────────────────────

/// The routing decision emitted to stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Which backend to use.
    pub backend: Backend,

    /// Estimated number of result rows (product of selected level cardinalities,
    /// after equality-filter reduction).
    pub estimated_rows: u64,

    /// Human-readable explanation for the decision.
    pub reason: String,

    /// Flat SQL SELECT projection. Present only when `backend == sql`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_projection: Option<String>,
}

// ── Routing error ─────────────────────────────────────────────────────────

/// Errors that can occur during routing.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// The `BoundMqo.measures` list is empty — nothing to project.
    #[error("BoundMqo has no measures — cannot route")]
    NoMeasures,
}

// ── Cardinality estimation ────────────────────────────────────────────────

/// Compute the estimated row count for a `BoundMqo` given level cardinalities.
///
/// Algorithm:
/// 1. Start with the product of cardinalities for all selected dimension levels.
/// 2. For each `Member` filter that restricts a hierarchy to *N* members,
///    replace that level's cardinality with `min(N, cardinality)`.
/// 3. Any level not found in `stats` is treated as cardinality 1
///    (conservative — avoids multiplying by a large unknown).
///
/// Returns `1` when there are no dimension levels selected (scalar aggregate).
#[must_use]
pub fn estimate_rows(bound: &mqo_spec::BoundMqo, stats: &LevelStats) -> u64 {
    use mqo_spec::Filter;

    // Build a map: hierarchy → min(member_filter_count, base_cardinality)
    // We key by hierarchy unique name as that's what BoundDimension carries.
    let mut cardinality: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

    for dim in &bound.dimensions {
        let base = stats.get(&dim.unique_name).copied().unwrap_or(1);
        cardinality.insert(dim.unique_name.clone(), base);
    }

    // Apply Member filters: if a filter restricts the hierarchy to N members,
    // cap that level's cardinality at N.
    for filter in &bound.mqo.filters {
        if let Filter::Member { hierarchy, members } = filter {
            // Find any bound dimension whose hierarchy matches.
            for dim in &bound.dimensions {
                if &dim.hierarchy == hierarchy {
                    let n = members.len() as u64;
                    cardinality
                        .entry(dim.unique_name.clone())
                        .and_modify(|c| *c = (*c).min(n));
                }
            }
        }
    }

    // Product of all per-level cardinalities.
    let product: u64 = cardinality.values().product();
    product.max(1) // scalar aggregate = 1 row
}

// ── SQL projection builder ────────────────────────────────────────────────

// ── CatalogContext ────────────────────────────────────────────────────────

/// Catalog metadata used by the SQL projection builder.
///
/// Loaded from the `CatalogSnapshot` JSON when `--catalog` is supplied to
/// `mqo-route`. When absent, `build_sql_projection` falls back to the old
/// last-segment / unqualified-FROM behaviour (backwards-compatible).
#[derive(Debug, Clone, Default)]
pub struct CatalogContext {
    /// `AtScale` catalog name, e.g. `"atscale_catalogs"`.
    pub catalog: Option<String>,
    /// `AtScale` schema name, e.g. `"tpcds_Snowflake"`.
    pub schema: Option<String>,
    /// Maps every column `unique_name` → its human-readable display label.
    pub labels: HashMap<String, String>,
}

impl CatalogContext {
    /// Parse a `CatalogContext` from the raw catalog snapshot JSON value.
    #[must_use]
    pub fn from_json(v: &serde_json::Value) -> Self {
        let catalog = v
            .get("catalog")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let schema = v
            .get("schema")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let labels = v
            .get("columns")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|col| {
                        let un = col.get("unique_name")?.as_str()?.to_string();
                        let label = col.get("label")?.as_str()?.to_string();
                        Some((un, label))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            catalog,
            schema,
            labels,
        }
    }
}

/// Build a flat SQL SELECT projection for the large-extract (SQL) path.
///
/// Without `catalog`:
/// ```sql
/// SELECT "last_segment", ... FROM "model" LIMIT n
/// ```
///
/// With `catalog` (fully-qualified, display labels, aggregation):
/// ```sql
/// SELECT "Dim Label", SUM("Measure Label") AS "measure_slug"
/// FROM "atscale_catalogs"."tpcds_Snowflake"."model" LIMIT n
/// ```
///
/// For **projection** MQOs (`mqo.is_projection()` is true) the form is:
/// ```sql
/// SELECT DISTINCT "Dim Label", ... FROM "model" [LIMIT n]
/// ```
///
/// The alias on each measure uses the last `.`-segment of the `unique_name`
/// so callers can map response columns back to MQO unique names.
#[must_use]
pub fn build_sql_projection(
    bound: &mqo_spec::BoundMqo,
    catalog: Option<&CatalogContext>,
) -> String {
    let mut cols: Vec<String> = Vec::new();

    // Dimension columns — display label when available, last-segment fallback.
    for d in &bound.dimensions {
        let col = catalog
            .and_then(|c| c.labels.get(&d.unique_name))
            .map_or_else(|| quote_last_segment(&d.unique_name), |label| format!("\"{label}\""));
        cols.push(col);
    }

    // Measure columns — SUM("Display Label") AS "slug" when catalog present.
    // Skipped for projection MQOs (no measures).
    for m in &bound.measures {
        let col = if let Some(label) = catalog.and_then(|c| c.labels.get(&m.unique_name)) {
            let slug = m.unique_name.rsplit('.').next().unwrap_or(&m.unique_name);
            format!("SUM(\"{label}\") AS \"{slug}\"")
        } else {
            quote_last_segment(&m.unique_name)
        };
        cols.push(col);
    }

    let col_list = cols.join(", ");
    let from = build_from_clause(&bound.mqo.model, catalog);
    let limit_clause = bound
        .mqo
        .limit
        .map_or_else(String::new, |n| format!(" LIMIT {n}"));

    // Projection MQOs emit SELECT DISTINCT (no aggregation, distinct members).
    if bound.mqo.is_projection() {
        format!("SELECT DISTINCT {col_list} FROM {from}{limit_clause}")
    } else {
        format!("SELECT {col_list} FROM {from}{limit_clause}")
    }
}

/// Build the fully-qualified FROM clause.
///
/// With catalog context: `"catalog"."schema"."model"`.
/// Without (or when catalog/schema absent): falls back to quoting each
/// `.`-separated component of the model string.
fn build_from_clause(model: &str, catalog: Option<&CatalogContext>) -> String {
    if let Some(ctx) = catalog {
        if ctx.catalog.is_some() || ctx.schema.is_some() {
            let mut parts: Vec<String> = Vec::new();
            if let Some(ref cat) = ctx.catalog {
                parts.push(format!("\"{cat}\""));
            }
            if let Some(ref sch) = ctx.schema {
                parts.push(format!("\"{sch}\""));
            }
            parts.push(format!("\"{model}\""));
            return parts.join(".");
        }
    }
    quote_model_path(model)
}

/// Quote the last `.`-separated segment of a `unique_name` for use as a SQL
/// column reference, e.g. `"store_sales.Total Store Sales"` → `"Total Store Sales"`.
fn quote_last_segment(unique_name: &str) -> String {
    let label = unique_name.rsplit('.').next().unwrap_or(unique_name);
    // Strip any bracketed notation from DAX-style names, e.g. `[Year]` → `Year`.
    let label = label.trim_matches(|c| c == '[' || c == ']');
    format!("\"{label}\"")
}

/// Quote each `.`-separated component of the model path:
/// `atscale_catalogs.tpcds_Snowflake.tpcds_model` → `"atscale_catalogs"."tpcds_Snowflake"."tpcds_model"`
fn quote_model_path(model: &str) -> String {
    model
        .split('.')
        .map(|part| format!("\"{part}\""))
        .collect::<Vec<_>>()
        .join(".")
}

// ── Main router ───────────────────────────────────────────────────────────

/// Route a `BoundMqo` to a backend.
///
/// Decision tree (in priority order):
/// 1. Shape flags set (`asymmetric_axes`, `drill_through`, `cellset_requested`)
///    → **MDX**.
/// 2. `estimated_rows > row_threshold` → **SQL** with flat projection.
/// 3. Projection MQO (`mqo.is_projection()`) → **DAX** (SUMMARIZECOLUMNS path).
/// 4. Otherwise → **DAX**.
///
/// # Errors
///
/// Returns [`RouterError::NoMeasures`] when `bound.measures` is empty and the
/// MQO is not a valid projection.
pub fn route(
    bound: &mqo_spec::BoundMqo,
    stats: &StatBundle,
    row_threshold: u64,
    catalog: Option<&CatalogContext>,
) -> Result<RoutingDecision, RouterError> {
    // Allow projection MQOs through (no measures, explicit opt-in, ≥1 dim).
    // Non-projection measureless MQOs are still an error.
    if bound.measures.is_empty() && !bound.mqo.is_projection() {
        return Err(RouterError::NoMeasures);
    }

    let flags = &stats.shape_flags;

    // 1. Shape-triggered MDX
    if flags.asymmetric_axes || flags.drill_through || flags.cellset_requested {
        let reason = if flags.asymmetric_axes {
            "asymmetric axes requested".to_string()
        } else if flags.drill_through {
            "drill-through requested".to_string()
        } else {
            "cellset requested".to_string()
        };

        return Ok(RoutingDecision {
            backend: Backend::Mdx,
            estimated_rows: estimate_rows(bound, &stats.level_cardinalities),
            reason,
            sql_projection: None,
        });
    }

    let est = estimate_rows(bound, &stats.level_cardinalities);

    // When a limit is present, the engine will return at most `limit` rows.
    // Compare the capped value against the threshold so bounded queries are
    // never pushed to SQL solely by a large cardinality cross-product.
    let effective_est = bound
        .mqo
        .limit
        .map_or(est, |l| est.min(l));

    // 2. Large-extract → SQL
    if effective_est > row_threshold {
        let projection = build_sql_projection(bound, catalog);
        return Ok(RoutingDecision {
            backend: Backend::Sql,
            estimated_rows: est,
            reason: format!(
                "estimated_rows ({est}) exceeds row_threshold ({row_threshold})"
            ),
            sql_projection: Some(projection),
        });
    }

    // 3. Default → DAX
    // If the limit was what brought effective_est ≤ threshold, say so explicitly
    // so operators can audit why a high-cardinality query landed on the engine path.
    let reason = if est > row_threshold {
        format!(
            "query limit ({}) capped raw estimated_rows ({est}) to within threshold ({row_threshold})",
            bound.mqo.limit.unwrap_or(0)
        )
    } else {
        format!("estimated_rows ({est}) is within threshold ({row_threshold})")
    };

    Ok(RoutingDecision {
        backend: Backend::Dax,
        estimated_rows: est,
        reason,
        sql_projection: None,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mqo_spec::{BoundDimension, BoundMeasure, BoundMqo, Filter, LevelSelection, MeasureRef, Mqo};

    // ── Fixture helpers ────────────────────────────────────────────────────

    fn make_mqo(model: &str) -> Mqo {
        Mqo {
            model: model.to_string(),
            measures: vec![MeasureRef {
                unique_name: "sales.revenue".to_string(),
            }],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
            projection: false,
        }
    }

    fn make_bound(mqo: Mqo, dim_names: &[(&str, &str)]) -> BoundMqo {
        let measures = mqo
            .measures
            .iter()
            .map(|m| BoundMeasure {
                unique_name: m.unique_name.clone(),
                is_calc: false,
                semi_additive: false,
                required_dimension: None,
            })
            .collect();

        let dimensions = dim_names
            .iter()
            .map(|(unique_name, hierarchy)| BoundDimension {
                unique_name: unique_name.to_string(),
                hierarchy: hierarchy.to_string(),
            })
            .collect();

        BoundMqo {
            mqo,
            measures,
            dimensions,
        }
    }

    fn low_card_stats(levels: &[(&str, u64)]) -> StatBundle {
        StatBundle {
            level_cardinalities: levels.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            shape_flags: ShapeFlags::default(),
        }
    }

    // ── AC1: low-cardinality aggregated MQO → DAX ─────────────────────────

    #[test]
    fn ac1_low_cardinality_routes_to_dax() {
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[
                ("time.calendar.[Year]", "time.calendar"),
                ("geo.country.[Country]", "geo.country"),
            ],
        );
        let stats = low_card_stats(&[
            ("time.calendar.[Year]", 5),
            ("geo.country.[Country]", 10),
        ]);
        // estimated_rows = 5 * 10 = 50; threshold = 50_000
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Dax);
        assert_eq!(decision.estimated_rows, 50);
        assert!(decision.sql_projection.is_none());
    }

    // ── AC2: MDX-flagged MQO routes to MDX ────────────────────────────────

    #[test]
    fn ac2_drill_through_routes_to_mdx() {
        let mqo = make_mqo("sales");
        let bound = make_bound(mqo, &[("time.calendar.[Year]", "time.calendar")]);
        let stats = StatBundle {
            level_cardinalities: [("time.calendar.[Year]".to_string(), 5)].into(),
            shape_flags: ShapeFlags {
                drill_through: true,
                ..Default::default()
            },
        };
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Mdx);
        assert!(decision.sql_projection.is_none());
        assert!(decision.reason.contains("drill-through"));
    }

    #[test]
    fn ac2_asymmetric_axes_routes_to_mdx() {
        let mqo = make_mqo("sales");
        let bound = make_bound(mqo, &[]);
        let stats = StatBundle {
            level_cardinalities: Default::default(),
            shape_flags: ShapeFlags {
                asymmetric_axes: true,
                ..Default::default()
            },
        };
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Mdx);
        assert!(decision.reason.contains("asymmetric"));
    }

    #[test]
    fn ac2_cellset_requested_routes_to_mdx() {
        let mqo = make_mqo("sales");
        let bound = make_bound(mqo, &[]);
        let stats = StatBundle {
            level_cardinalities: Default::default(),
            shape_flags: ShapeFlags {
                cellset_requested: true,
                ..Default::default()
            },
        };
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Mdx);
        assert!(decision.reason.contains("cellset"));
    }

    // ── AC3: high-cardinality MQO routes to SQL with non-empty projection ─

    #[test]
    fn ac3_high_cardinality_routes_to_sql_with_projection() {
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[
                ("time.calendar.[Date]", "time.calendar"),
                ("product.category.[Product]", "product.category"),
            ],
        );
        let stats = low_card_stats(&[
            ("time.calendar.[Date]", 1000),
            ("product.category.[Product]", 200),
        ]);
        // estimated_rows = 1000 * 200 = 200_000; threshold = 50_000
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Sql);
        assert_eq!(decision.estimated_rows, 200_000);
        let proj = decision.sql_projection.expect("sql_projection must be present");
        assert!(!proj.is_empty());
        assert!(proj.starts_with("SELECT "));
    }

    // ── AC4: estimated_rows formula assertion ─────────────────────────────

    #[test]
    fn ac4_estimated_rows_is_product_of_level_cardinalities() {
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[
                ("time.calendar.[Year]", "time.calendar"),
                ("geo.region.[Region]", "geo.region"),
                ("product.line.[Line]", "product.line"),
            ],
        );
        let stats = low_card_stats(&[
            ("time.calendar.[Year]", 4),
            ("geo.region.[Region]", 7),
            ("product.line.[Line]", 3),
        ]);
        // 4 * 7 * 3 = 84
        assert_eq!(estimate_rows(&bound, &stats.level_cardinalities), 84);
    }

    #[test]
    fn ac4_member_filter_reduces_cardinality() {
        // A Member filter on a hierarchy caps that level at the member-count.
        let mut mqo = make_mqo("sales");
        // Filter: only 2 specific years
        mqo.dimensions.push(LevelSelection {
            hierarchy: "time.calendar".to_string(),
            level: "Year".to_string(),
        });
        mqo.filters.push(Filter::Member {
            hierarchy: "time.calendar".to_string(),
            members: vec!["2023".to_string(), "2024".to_string()],
        });

        let mut bound = make_bound(
            mqo,
            &[("time.calendar.[Year]", "time.calendar")],
        );
        // Sync mqo filters back (make_bound clones the mqo already)
        // We need the filters in bound.mqo — rebuild properly:
        bound.mqo.filters = vec![Filter::Member {
            hierarchy: "time.calendar".to_string(),
            members: vec!["2023".to_string(), "2024".to_string()],
        }];

        let stats = low_card_stats(&[("time.calendar.[Year]", 10)]);
        // base = 10, filter reduces to min(10, 2) = 2
        assert_eq!(estimate_rows(&bound, &stats.level_cardinalities), 2);
    }

    #[test]
    fn ac4_unknown_level_defaults_to_cardinality_one() {
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[("some.unknown.[Level]", "some.unknown")],
        );
        let stats = low_card_stats(&[]); // no entries
        // unknown level → cardinality 1 → product = 1
        assert_eq!(estimate_rows(&bound, &stats.level_cardinalities), 1);
    }

    #[test]
    fn ac4_no_dimensions_is_scalar_aggregate_one_row() {
        let mqo = make_mqo("sales");
        let bound = make_bound(mqo, &[]);
        let stats = low_card_stats(&[]);
        assert_eq!(estimate_rows(&bound, &stats.level_cardinalities), 1);
    }

    // ── AC5: --row-threshold overrides default ────────────────────────────

    #[test]
    fn ac5_row_threshold_override_changes_routing_boundary() {
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[("time.calendar.[Month]", "time.calendar")],
        );
        let stats = low_card_stats(&[("time.calendar.[Month]", 60)]);
        // estimated_rows = 60

        // Default threshold 50_000 → DAX
        let decision_dax = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision_dax.backend, Backend::Dax);

        // Custom threshold 50 → SQL (60 > 50)
        let decision_sql = route(&bound, &stats, 50, None).unwrap();
        assert_eq!(decision_sql.backend, Backend::Sql);
        assert!(decision_sql.sql_projection.is_some());
    }

    #[test]
    fn ac5_threshold_boundary_exactly_at_threshold_is_dax() {
        // estimated_rows == threshold → DAX (not strictly greater)
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[("time.calendar.[Month]", "time.calendar")],
        );
        let stats = low_card_stats(&[("time.calendar.[Month]", 100)]);
        let decision = route(&bound, &stats, 100, None).unwrap();
        assert_eq!(decision.backend, Backend::Dax);
    }

    // ── Additional: error on empty measures ───────────────────────────────

    #[test]
    fn error_on_no_measures() {
        let bound = BoundMqo {
            mqo: Mqo {
                model: "sales".to_string(),
                measures: vec![],
                dimensions: vec![],
                filters: vec![],
                time_intelligence: vec![],
                order: None,
                limit: None,
                non_empty: false,
                projection: false,
            },
            measures: vec![],
            dimensions: vec![],
        };
        let stats = low_card_stats(&[]);
        let result = route(&bound, &stats, 50_000, None);
        assert!(matches!(result, Err(RouterError::NoMeasures)));
    }

    // ── SQL projection content ─────────────────────────────────────────────

    #[test]
    fn sql_projection_includes_dims_and_measures() {
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[
                ("time.calendar.[Year]", "time.calendar"),
                ("geo.country.[Country]", "geo.country"),
            ],
        );
        let proj = build_sql_projection(&bound, None);
        // Columns are the last-segment, double-quoted.
        assert!(proj.contains("\"Year\""), "proj = {proj}");
        assert!(proj.contains("\"Country\""), "proj = {proj}");
        assert!(proj.contains("\"revenue\""), "proj = {proj}");
        // Model path is double-quoted per component.
        assert!(proj.contains("\"sales\""), "proj = {proj}");
        assert!(proj.starts_with("SELECT "), "proj = {proj}");
    }

    // ── AC6: CatalogContext produces fully-qualified, display-label SQL ────

    #[test]
    fn ac6_catalog_context_produces_qualified_display_label_sql() {
        let mut mqo = make_mqo("tpcds_benchmark_model");
        mqo.measures = vec![MeasureRef {
            unique_name: "tpcds_benchmark_model.total_store_sales".to_string(),
        }];
        mqo.dimensions.push(LevelSelection {
            hierarchy: "ship_mode".to_string(),
            level: "Carrier".to_string(),
        });
        mqo.limit = Some(5);

        let bound = make_bound(
            mqo,
            &[("ship_mode.[Carrier]", "ship_mode")],
        );

        let ctx = CatalogContext {
            catalog: Some("atscale_catalogs".to_string()),
            schema: Some("tpcds_Snowflake".to_string()),
            labels: [
                (
                    "tpcds_benchmark_model.total_store_sales".to_string(),
                    "Total Store Sales".to_string(),
                ),
                (
                    "ship_mode.[Carrier]".to_string(),
                    "Carrier".to_string(),
                ),
            ]
            .into(),
        };

        let proj = build_sql_projection(&bound, Some(&ctx));

        // Fully-qualified FROM
        assert!(
            proj.contains(r#""atscale_catalogs"."tpcds_Snowflake"."tpcds_benchmark_model""#),
            "proj = {proj}"
        );
        // Display label for dimension
        assert!(proj.contains(r#""Carrier""#), "proj = {proj}");
        // SUM + display label for measure + alias
        assert!(
            proj.contains(r#"SUM("Total Store Sales") AS "total_store_sales""#),
            "proj = {proj}"
        );
        assert!(proj.ends_with("LIMIT 5"), "proj = {proj}");
    }

    // ── MDX takes priority over high cardinality ───────────────────────────

    #[test]
    fn mdx_flag_takes_priority_over_high_cardinality() {
        let mqo = make_mqo("sales");
        let bound = make_bound(
            mqo,
            &[("time.calendar.[Date]", "time.calendar")],
        );
        let stats = StatBundle {
            level_cardinalities: [("time.calendar.[Date]".to_string(), 1_000_000)].into(),
            shape_flags: ShapeFlags {
                asymmetric_axes: true,
                ..Default::default()
            },
        };
        // Even though estimated_rows = 1_000_000 >> threshold, MDX wins
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Mdx);
    }

    // ── Limit-aware routing (PRD-mqo-limit-aware-routing) ─────────────────

    fn high_card_bound_with_limit(limit: Option<u64>) -> (BoundMqo, StatBundle) {
        let mut mqo = make_mqo("sales");
        mqo.limit = limit;
        let bound = make_bound(
            mqo,
            &[
                ("product.category.[Product]", "product.category"),
                ("time.calendar.[Week]", "time.calendar"),
            ],
        );
        // product 1000 × 200 = 200_000 — well above the 50_000 default threshold
        let stats = low_card_stats(&[
            ("product.category.[Product]", 1000),
            ("time.calendar.[Week]", 200),
        ]);
        (bound, stats)
    }

    // AC1: bounded high-cardinality → DAX (limit ≤ threshold caps the estimate)
    #[test]
    fn prd_ac1_bounded_high_cardinality_routes_to_dax() {
        let (bound, stats) = high_card_bound_with_limit(Some(50));
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Dax);
        assert_eq!(decision.estimated_rows, 200_000); // raw estimate preserved
        assert!(decision.sql_projection.is_none());
    }

    // AC2: same MQO unbounded still routes to SQL
    #[test]
    fn prd_ac2_unbounded_high_cardinality_still_routes_to_sql() {
        let (bound, stats) = high_card_bound_with_limit(None);
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Sql);
        assert!(decision.sql_projection.is_some());
        assert_eq!(decision.estimated_rows, 200_000);
    }

    // AC3: limit present but > threshold AND product > threshold → SQL
    #[test]
    fn prd_ac3_limit_larger_than_threshold_routes_to_sql() {
        // min(200_000, 100_000) = 100_000 > 50_000 → SQL
        let (bound, stats) = high_card_bound_with_limit(Some(100_000));
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Sql);
    }

    // AC5: limit == threshold edge → DAX (effective == threshold, not strictly greater)
    #[test]
    fn prd_ac5_limit_exactly_at_threshold_routes_to_dax() {
        // min(200_000, 50_000) = 50_000 == threshold → DAX (≤ not >)
        let (bound, stats) = high_card_bound_with_limit(Some(50_000));
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Dax);
    }

    // AC6: shape flag beats the bounded row test
    #[test]
    fn prd_ac6_shape_flag_beats_bounded_row_test() {
        let mut mqo = make_mqo("sales");
        mqo.limit = Some(50);
        let bound = make_bound(mqo, &[("product.category.[Product]", "product.category")]);
        let stats = StatBundle {
            level_cardinalities: [("product.category.[Product]".to_string(), 200_000)].into(),
            shape_flags: ShapeFlags {
                asymmetric_axes: true,
                ..Default::default()
            },
        };
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Mdx);
    }

    // AC7: reason string names both the raw estimate and the limit when limit caps
    #[test]
    fn prd_ac7_reason_string_names_limit_cap() {
        let (bound, stats) = high_card_bound_with_limit(Some(50));
        let decision = route(&bound, &stats, 50_000, None).unwrap();
        assert_eq!(decision.backend, Backend::Dax);
        // must contain raw estimate
        assert!(
            decision.reason.contains("200000"),
            "reason should contain raw estimate: {}",
            decision.reason
        );
        // must contain the limit value
        assert!(
            decision.reason.contains("50"),
            "reason should contain limit value: {}",
            decision.reason
        );
        // must be textually distinct from the regular within-threshold reason
        assert!(
            !decision.reason.starts_with("estimated_rows"),
            "limit-capped reason must differ from regular DAX reason: {}",
            decision.reason
        );
    }
}
