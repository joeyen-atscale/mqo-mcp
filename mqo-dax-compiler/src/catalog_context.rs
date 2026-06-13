//! Catalog context for engine-ready DAX name grounding.
//!
//! Parses a `CatalogSnapshot` JSON (from `mqo-catalog-binder`) into a lightweight
//! lookup table used by [`compile_grounded`] to emit `'TableName'[Display Label]`
//! column references and `[Display Label]` measure references instead of raw
//! binder `unique_name` strings.
//!
//! This module deliberately duplicates the minimal subset of the snapshot shape
//! (catalog/schema/columns) rather than taking a path-dependency on the binder
//! crate, keeping `mqo-dax-compiler` dep-minimal.

use std::collections::HashMap;

use serde::Deserialize;

// ── Local mirror of the relevant CatalogSnapshot subset ──────────────────────

/// Minimal mirror of `ColumnEntry` from `mqo-catalog-binder`.
#[derive(Debug, Deserialize)]
struct ColumnEntryMirror {
    unique_name: String,
    label: String,
    kind: String,
    /// For `kind == "level"`: optional enumerated member domain (from the
    /// level-domain capture). Drives domain-aware `Member`-filter grounding.
    #[serde(default)]
    domain: Option<Vec<String>>,
}

/// Minimal mirror of `CatalogSnapshot` from `mqo-catalog-binder`.
#[derive(Debug, Deserialize)]
struct CatalogSnapshotMirror {
    #[serde(default)]
    catalog: Option<String>,
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    columns: Vec<ColumnEntryMirror>,
    /// Explicit engine capability flag.  When present and `true` the engine
    /// supports "Mark as Date Table" and time-intel functions will be emitted.
    /// When absent defaults to `false` (conservative: `AtScale` XMLA target).
    #[serde(default)]
    has_date_table: bool,
    /// Explicit date-level reference.  When present, used verbatim in
    /// time-intel codegen instead of the auto-inferred or placeholder value.
    #[serde(default)]
    date_level_unique_name: Option<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// A compiled lookup context built from a `CatalogSnapshot` JSON file.
///
/// Used by [`crate::codegen::compile_grounded`] to resolve `unique_name` strings
/// into engine-ready `'TableName'[Display Label]` / `[Display Label]` forms.
#[derive(Debug, Clone)]
pub struct DaxCatalogContext {
    /// Table name used in `SUMMARIZECOLUMNS` column refs
    /// (e.g. `"tpcds_benchmark_model"`).
    pub table_name: String,

    /// `unique_name → display_label` for every column (measures and levels).
    pub labels: HashMap<String, String>,

    /// `unique_name` set for columns whose `kind` is `"measure"`.
    pub measure_names: std::collections::HashSet<String>,

    /// Reverse index: hierarchy name (bare, lowercase) → ordered list of level
    /// `unique_name` strings found in this catalog snapshot.
    ///
    /// Built at parse time so `Filter::Member` resolution has O(1) lookup.
    /// The hierarchy name key is the *last non-bracketed dot-segment* of each
    /// level's `unique_name` (i.e. the second-to-last segment), stored as-is
    /// (case-preserved) and also accessible via its bare last component.
    ///
    /// Ordering is insertion-order (as the snapshot lists levels), which is
    /// deterministic (NFR3).
    pub hierarchy_levels: HashMap<String, Vec<String>>,

    /// `level unique_name → enumerated member domain` for levels that carry one
    /// (from the level-domain capture). Used by [`Self::resolve_member_level`] to
    /// bind a `Member` filter value to the level whose domain contains it, instead
    /// of the hierarchy's first level. Levels without a domain are absent here and
    /// fall back to first-level grounding.
    pub level_domains: HashMap<String, Vec<String>>,

    /// Whether the target engine supports "Mark as Date Table" and thus the
    /// `SAMEPERIODLASTYEAR` / `DATESYTD` / `DATESQTD` / `DATESMTD` /
    /// `DATEADD` time-intelligence functions.
    ///
    /// Set `true` only when the engine or model declares a date-table
    /// designation and the time-intel functions are confirmed to work.
    ///
    /// **Default: `false`** — `AtScale` XMLA (`/v1/xmla`) does not expose a
    /// Mark-as-Date-Table designation, so these ops are unsupported by default.
    /// The [`DaxCatalogContext::from_json`] parser honours a `"has_date_table"`
    /// boolean field if present in the snapshot; otherwise defaults to `false`.
    pub has_date_table: bool,

    /// The `unique_name` of the date dimension/level to use in time-intelligence
    /// function calls (e.g. `"sold_date_dimension.calendar.[Sold Calendar Year]"`).
    ///
    /// When `Some`, replaces the hardcoded `DateTable[Date]` placeholder in the
    /// time-intel codegen path. When `None` with a context present, the fallback
    /// placeholder is used (annotated as ungrounded for diagnostics).
    ///
    /// The [`DaxCatalogContext::from_json`] parser infers this from the first
    /// column entry whose `kind` is `"date_level"` or `"date_dim"`.  Callers
    /// that build `DaxCatalogContext` programmatically should set this field
    /// directly.
    pub date_level_unique_name: Option<String>,
}

impl DaxCatalogContext {
    /// Parse a `CatalogSnapshot` JSON string and build a `DaxCatalogContext`.
    ///
    /// **Table name resolution** (first match wins):
    /// 1. `snapshot.catalog`
    /// 2. `snapshot.schema`
    /// 3. First dotted component of the first `unique_name`
    /// 4. Fallback literal `"model"`
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` when the JSON is malformed.
    pub fn from_json(json_str: &str) -> Result<Self, String> {
        let snapshot: CatalogSnapshotMirror =
            serde_json::from_str(json_str).map_err(|e| format!("catalog JSON parse error: {e}"))?;

        // Resolve table_name.
        let table_name = snapshot
            .catalog
            .clone()
            .or_else(|| snapshot.schema.clone())
            .or_else(|| {
                snapshot
                    .columns
                    .first()
                    .map(|c| c.unique_name.split('.').next().unwrap_or("model").to_string())
            })
            .unwrap_or_else(|| "model".to_string());

        let mut labels: HashMap<String, String> = HashMap::new();
        let mut measure_names = std::collections::HashSet::new();
        let mut hierarchy_levels: HashMap<String, Vec<String>> = HashMap::new();
        let mut level_domains: HashMap<String, Vec<String>> = HashMap::new();
        // Infer date_level_unique_name from the first column with kind "date_level"
        // or "date_dim" when not explicitly provided in the snapshot.
        let mut inferred_date_level: Option<String> = None;

        for col in &snapshot.columns {
            labels.insert(col.unique_name.clone(), col.label.clone());
            if col.kind == "measure" {
                measure_names.insert(col.unique_name.clone());
            } else {
                // Track date dimension levels for time-intel grounding.
                if inferred_date_level.is_none()
                    && (col.kind == "date_level" || col.kind == "date_dim")
                {
                    inferred_date_level = Some(col.unique_name.clone());
                }

                // Record the level's enumerated domain (if captured) for
                // domain-aware Member-filter grounding.
                if let Some(d) = &col.domain {
                    if !d.is_empty() {
                        level_domains.insert(col.unique_name.clone(), d.clone());
                    }
                }

                // For level columns, build the hierarchy_levels reverse index.
                // unique_name form: "hierarchy_part1.hierarchy_part2.[Level Label]"
                // or               "hierarchy_part1.hierarchy_part2.Level Label"
                // The hierarchy key is everything before the last dot-segment.
                // We also register the bare last-component of the hierarchy prefix
                // (the dimension name) as an alias, so filters using just the
                // dimension name (e.g. "sold_date_dimensions") can resolve.
                let parts: Vec<&str> = col.unique_name.split('.').collect();
                if parts.len() >= 2 {
                    // Full hierarchy prefix (everything but the last segment).
                    let hierarchy_prefix = parts[..parts.len() - 1].join(".");
                    hierarchy_levels
                        .entry(hierarchy_prefix)
                        .or_default()
                        .push(col.unique_name.clone());

                    // Bare last component of the hierarchy prefix (e.g. "calendar"
                    // from "inventory_date_dimension.calendar").
                    if parts.len() >= 3 {
                        let bare_last = parts[parts.len() - 2];
                        let key = bare_last.to_string();
                        if key != parts[parts.len() - 1].trim_matches(|c| c == '[' || c == ']') {
                            hierarchy_levels
                                .entry(key)
                                .or_default()
                                .push(col.unique_name.clone());
                        }
                    }

                    // First component of the unique_name as another alias
                    // (e.g. "inventory_date_dimension").
                    let first_component = parts[0].to_string();
                    if parts.len() >= 2 {
                        hierarchy_levels
                            .entry(first_component)
                            .or_default()
                            .push(col.unique_name.clone());
                    }
                }
            }
        }

        let date_level_unique_name = snapshot
            .date_level_unique_name
            .or(inferred_date_level);

        Ok(Self {
            table_name,
            labels,
            measure_names,
            hierarchy_levels,
            level_domains,
            has_date_table: snapshot.has_date_table,
            date_level_unique_name,
        })
    }

    /// Resolve a `Member` filter to the level whose enumerated domain CONTAINS the
    /// member value(s) — the fix for the silent first-level mis-binding
    /// (PRD-mqo-member-filter-domain-grounding). Returns the `unique_name` of the
    /// first level (in catalog order) of `hierarchy` whose captured domain contains
    /// the first member (case-insensitive). Returns `None` when the hierarchy has no
    /// domain metadata or no level's domain contains the member — callers then fall
    /// back to [`Self::resolve_hierarchy_first_level`] (no regression).
    /// `dim_levels` are the level `unique_name`s the query groups by; when a
    /// member is ambiguous (its value appears in more than one level's domain,
    /// e.g. "M" in both Gender and Marital Status), a level that the query also
    /// groups by wins (you filter the attribute you're breaking out). Resolution:
    /// (1) a candidate level (domain contains the member) that is also a query
    /// dimension; else (2) the sole candidate if exactly one; else `None`
    /// (ambiguous with no signal) → caller falls back to first-level.
    #[must_use]
    pub fn resolve_member_level(
        &self,
        hierarchy: &str,
        members: &[String],
        dim_levels: &[String],
    ) -> Option<&str> {
        let probe = members.first()?.to_lowercase();
        let levels = self.hierarchy_levels.get(hierarchy).or_else(|| {
            let lower = hierarchy.to_lowercase();
            self.hierarchy_levels
                .iter()
                .find(|(k, _)| k.to_lowercase() == lower)
                .map(|(_, v)| v)
        })?;
        // Candidates: levels of this hierarchy whose enumerated domain contains the
        // member. Dedup by unique_name — the reverse index can list a level under
        // several alias keys, so the same level may appear more than once.
        let mut candidates: Vec<&String> = levels
            .iter()
            .filter(|lvl| {
                self.level_domains
                    .get(*lvl)
                    .is_some_and(|d| d.iter().any(|v| v.to_lowercase() == probe))
            })
            .collect();
        candidates.sort();
        candidates.dedup();
        // (1) prefer a candidate the query also groups by (disambiguates "M").
        if let Some(c) = candidates.iter().find(|c| dim_levels.iter().any(|d| d == **c)) {
            return Some(c.as_str());
        }
        // (2) sole unambiguous candidate.
        if candidates.len() == 1 {
            return Some(candidates[0].as_str());
        }
        // (≥2 candidates, none a dimension) ambiguous → no guess; caller falls back.
        None
    }

    /// Resolve a bare display label back to its level unique-name.
    ///
    /// Searches `self.labels` (which maps `unique_name → display_label`)
    /// for a value equal to `label`.  Returns the first matching unique-name,
    /// or `None` when no entry matches.  Case-sensitive; match is exact.
    ///
    /// Used by the `Filter::Range` codegen arm to accept a natural bare label
    /// (e.g. `"Sold Calendar Year"`) and resolve it to the fully-qualified
    /// unique-name (e.g. `"sold_date_dimensions.[Sold Calendar Year]"`) needed
    /// to produce a grounded `'TableName'[Label]` column reference.
    #[must_use]
    pub fn resolve_level_label(&self, label: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|(_, v)| v.as_str() == label)
            .map(|(k, _)| k.as_str())
    }

    /// Resolve a hierarchy name to the `unique_name` of the first level in
    /// the catalog for that hierarchy.
    ///
    /// The hierarchy name is matched against the reverse index using several
    /// heuristics (full prefix, bare component, first component). Returns
    /// `Some(unique_name)` when exactly one candidate level is found, or when
    /// there are multiple candidates and the first one is selected
    /// deterministically (insertion order, NFR3). Returns `None` when no
    /// level entries match the hierarchy name.
    #[must_use]
    pub fn resolve_hierarchy_first_level(&self, hierarchy: &str) -> Option<&str> {
        // Direct match on the hierarchy_levels map (handles full prefix and aliases).
        if let Some(levels) = self.hierarchy_levels.get(hierarchy) {
            return levels.first().map(String::as_str);
        }
        // Case-insensitive fallback: scan for a key matching hierarchy lowercased.
        let lower = hierarchy.to_lowercase();
        for (key, levels) in &self.hierarchy_levels {
            if key.to_lowercase() == lower {
                return levels.first().map(String::as_str);
            }
        }
        None
    }

    /// True when at least one level of `hierarchy` carries a captured domain.
    ///
    /// Drives the compiler's decline-vs-fallback decision
    /// (PRD-mqo-member-grounding-decline-not-fallback): when a member filter
    /// finds no domain match, the compiler should **decline** (honest typed
    /// error) if the hierarchy has domains to decide on, but may **fall back**
    /// to first-level grounding only when the hierarchy has *no* captured
    /// domains at all (legacy behavior on un-ingested deployments — the OQ-1
    /// safety valve that prevents a mass-decline regression).
    #[must_use]
    pub fn hierarchy_has_any_domain(&self, hierarchy: &str) -> bool {
        let levels = self.hierarchy_levels.get(hierarchy).or_else(|| {
            let lower = hierarchy.to_lowercase();
            self.hierarchy_levels
                .iter()
                .find(|(k, _)| k.to_lowercase() == lower)
                .map(|(_, v)| v)
        });
        levels.is_some_and(|lvls| lvls.iter().any(|l| self.level_domains.contains_key(l)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_json() -> &'static str {
        r#"{
            "catalog": "tpcds_benchmark_model",
            "schema": "tpcds_Snowflake",
            "columns": [
                {
                    "unique_name": "inventory_date_dimension.calendar.[Inventory Calendar Month]",
                    "label": "Inventory Calendar Month",
                    "kind": "level",
                    "hierarchy": "inventory_date_dimension.calendar",
                    "level": "Month"
                },
                {
                    "unique_name": "tpcds.total_store_sales",
                    "label": "Total Store Sales",
                    "kind": "measure"
                }
            ]
        }"#
    }

    #[test]
    fn table_name_from_catalog() {
        let ctx = DaxCatalogContext::from_json(fixture_json()).unwrap();
        assert_eq!(ctx.table_name, "tpcds_benchmark_model");
    }

    #[test]
    fn labels_populated() {
        let ctx = DaxCatalogContext::from_json(fixture_json()).unwrap();
        assert_eq!(
            ctx.labels
                .get("inventory_date_dimension.calendar.[Inventory Calendar Month]"),
            Some(&"Inventory Calendar Month".to_string())
        );
        assert_eq!(
            ctx.labels.get("tpcds.total_store_sales"),
            Some(&"Total Store Sales".to_string())
        );
    }

    #[test]
    fn measure_names_populated() {
        let ctx = DaxCatalogContext::from_json(fixture_json()).unwrap();
        assert!(ctx.measure_names.contains("tpcds.total_store_sales"));
        assert!(!ctx
            .measure_names
            .contains("inventory_date_dimension.calendar.[Inventory Calendar Month]"));
    }

    #[test]
    fn table_name_falls_back_to_schema() {
        let json = r#"{"schema":"my_schema","columns":[]}"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        assert_eq!(ctx.table_name, "my_schema");
    }

    #[test]
    fn table_name_falls_back_to_first_unique_name_prefix() {
        let json = r#"{"columns":[{"unique_name":"sales.revenue","label":"Revenue","kind":"measure"}]}"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        assert_eq!(ctx.table_name, "sales");
    }

    #[test]
    fn table_name_falls_back_to_model_when_empty() {
        let json = r#"{"columns":[]}"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        assert_eq!(ctx.table_name, "model");
    }

    #[test]
    fn invalid_json_returns_error() {
        assert!(DaxCatalogContext::from_json("not json").is_err());
    }

    #[test]
    fn has_date_table_defaults_to_false() {
        let ctx = DaxCatalogContext::from_json(fixture_json()).unwrap();
        assert!(!ctx.has_date_table);
    }

    #[test]
    fn has_date_table_parses_true() {
        let json = r#"{"catalog":"m","has_date_table":true,"columns":[]}"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        assert!(ctx.has_date_table);
    }

    #[test]
    fn date_level_unique_name_defaults_to_none() {
        let ctx = DaxCatalogContext::from_json(fixture_json()).unwrap();
        assert!(ctx.date_level_unique_name.is_none());
    }

    #[test]
    fn date_level_unique_name_parsed_explicitly() {
        let json = r#"{"catalog":"m","has_date_table":true,"date_level_unique_name":"d.cal.[Year]","columns":[]}"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        assert_eq!(ctx.date_level_unique_name.as_deref(), Some("d.cal.[Year]"));
    }

    #[test]
    fn date_level_inferred_from_kind_date_level() {
        let json = r#"{"catalog":"m","has_date_table":true,"columns":[
            {"unique_name":"date_dim.cal.[Year]","label":"Year","kind":"date_level"},
            {"unique_name":"sales.revenue","label":"Revenue","kind":"measure"}
        ]}"#;
        let ctx = DaxCatalogContext::from_json(json).unwrap();
        assert_eq!(
            ctx.date_level_unique_name.as_deref(),
            Some("date_dim.cal.[Year]")
        );
    }
}

#[cfg(test)]
mod member_grounding_tests {
    use super::DaxCatalogContext;

    fn ctx() -> DaxCatalogContext {
        // Gender {F,M} and Marital Status {D,M,S,U,W} both contain "M" (ambiguous);
        // Product Category {Electronics,...} contains "Electronics" (unambiguous).
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"customer_demographics.[Gender]","label":"Gender","hierarchy":"customer_demographics","level":"Gender","domain":["F","M"]},
          {"kind":"level","unique_name":"customer_demographics.[Marital Status]","label":"Marital Status","hierarchy":"customer_demographics","level":"Marital Status","domain":["D","M","S","U","W"]},
          {"kind":"level","unique_name":"product_dimension.[Product Category]","label":"Product Category","hierarchy":"product_dimension","level":"Product Category","domain":["Books","Electronics","Home"]}
        ]}"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    #[test]
    fn unambiguous_member_binds_to_its_level() {
        let c = ctx();
        let got = c.resolve_member_level("product_dimension", &["Electronics".into()], &[]);
        assert_eq!(got, Some("product_dimension.[Product Category]"));
    }

    #[test]
    fn ambiguous_member_prefers_dimension_level() {
        let c = ctx();
        // "M" is in both Gender and Marital Status; the query groups by Marital Status.
        let got = c.resolve_member_level(
            "customer_demographics",
            &["M".into()],
            &["customer_demographics.[Marital Status]".into()],
        );
        assert_eq!(got, Some("customer_demographics.[Marital Status]"));
    }

    #[test]
    fn ambiguous_member_no_dim_signal_declines() {
        let c = ctx();
        // No dimension on the hierarchy -> ambiguous "M" -> None (caller falls back).
        assert_eq!(c.resolve_member_level("customer_demographics", &["M".into()], &[]), None);
    }

    #[test]
    fn no_domain_metadata_declines() {
        let c = ctx();
        // hierarchy without domain'd levels -> None -> caller uses first-level.
        assert_eq!(c.resolve_member_level("nonexistent_dim", &["x".into()], &[]), None);
    }

    // PRD-mqo-member-grounding-decline-not-fallback: the compiler falls back to
    // first-level grounding ONLY when the hierarchy has no captured domains; when
    // domains exist but none match, it declines. hierarchy_has_any_domain is the gate.
    #[test]
    fn hierarchy_with_captured_domains_reports_true() {
        let c = ctx();
        assert!(c.hierarchy_has_any_domain("customer_demographics"));
        assert!(c.hierarchy_has_any_domain("product_dimension"));
    }

    #[test]
    fn hierarchy_without_domains_reports_false() {
        // store_dimension has a level but NO domain -> false (safety valve keeps
        // first-level fallback for un-ingested hierarchies).
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"store_dimension.[Store Name]","label":"Store Name","hierarchy":"store_dimension","level":"Store Name"}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        assert!(!c.hierarchy_has_any_domain("store_dimension"));
        assert!(!c.hierarchy_has_any_domain("nonexistent_dim"));
    }
}
