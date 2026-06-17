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

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Return `true` when `s` looks like a 4-digit calendar year (1000–9999).
///
/// Used by [`DaxCatalogContext::resolve_member_level`] to decide whether to
/// apply year-level preference (FR2 of PRD-mqo-date-member-cross-dimension-filter).
/// Only the numeric range is checked; no calendar validity check.
fn is_four_digit_year(s: &str) -> bool {
    if s.len() != 4 {
        return false;
    }
    s.bytes().all(|b| b.is_ascii_digit())
}

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
    /// Inferred value type for this level's domain members: `"integer"`,
    /// `"decimal"`, `"date"`, or `"string"` (default). Written by
    /// `catalog_ingest` when capturing live domain data. Used in
    /// `resolve_member_level` for dtype-aware numeric comparison.
    #[serde(default)]
    value_type: Option<String>,
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

    /// `level unique_name → physical table name` for every non-measure column.
    ///
    /// AtScale's XMLA tabular model creates one table per *hierarchy*, named by
    /// the hierarchy prefix of the level's `unique_name`
    /// (e.g. `ship_mode.[Carrier]` → `ship_mode`, `ship_mode.[Ship Mode Type]`
    /// → `ship_mode`). Used by [`crate::codegen::compile_grounded`] to emit
    /// `'<hierarchy>'[<label>]` column refs per level, instead of grounding
    /// every column to the single global `table_name` (which is the PGWire
    /// *database* name, `atscale_catalogs`, and invalid as a DAX table).
    ///
    /// Keyed by `unique_name`. A bare display label is NOT a key here — callers
    /// that may pass a bare label (e.g. a `MemberLevel` filter `level`) must
    /// reverse-resolve to the `unique_name` first (see
    /// [`Self::resolve_level_label`]).
    pub tables: HashMap<String, String>,

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

    /// `level unique_name → value_type` for levels that carry dtype metadata
    /// (e.g. `"integer"`, `"decimal"`, `"date"`, `"string"`). Written by
    /// `catalog_ingest` when capturing live domain data. Used by
    /// [`Self::resolve_member_level`] to perform dtype-aware numeric comparison
    /// when the probe string doesn't match the domain string exactly (e.g. probe
    /// `"9"` vs domain entry `"9.00"`). Absent entries default to `"string"`
    /// (exact lowercased match).
    pub level_dtypes: HashMap<String, String>,

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
        let mut tables: HashMap<String, String> = HashMap::new();
        let mut measure_names = std::collections::HashSet::new();
        let mut hierarchy_levels: HashMap<String, Vec<String>> = HashMap::new();
        let mut level_domains: HashMap<String, Vec<String>> = HashMap::new();
        let mut level_dtypes: HashMap<String, String> = HashMap::new();
        // Infer date_level_unique_name from the first column with kind "date_level"
        // or "date_dim" when not explicitly provided in the snapshot.
        let mut inferred_date_level: Option<String> = None;

        for col in &snapshot.columns {
            labels.insert(col.unique_name.clone(), col.label.clone());
            if col.kind == "measure" {
                measure_names.insert(col.unique_name.clone());
            } else {
                // Per-level physical table = the hierarchy prefix of the
                // unique_name (AtScale XMLA: one table per hierarchy).
                // ship_mode.[Carrier] → ship_mode.
                let table = col
                    .unique_name
                    .split('.')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("model")
                    .to_string();
                tables.insert(col.unique_name.clone(), table);

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
                // Record the level's value_type (if present) for dtype-aware
                // numeric comparison in resolve_member_level.
                if let Some(vt) = &col.value_type {
                    if !vt.is_empty() && vt != "string" {
                        level_dtypes.insert(col.unique_name.clone(), vt.clone());
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
            tables,
            measure_names,
            hierarchy_levels,
            level_domains,
            level_dtypes,
            has_date_table: snapshot.has_date_table,
            date_level_unique_name,
        })
    }

    /// Resolve a level key (a `unique_name`, OR a bare display label) to its
    /// canonical `unique_name` as known to this context.
    ///
    /// Returns the input verbatim when it is already a key in `labels`
    /// (a `unique_name`); otherwise reverse-resolves a bare display label to its
    /// `unique_name` via [`Self::resolve_level_label`]. Returns `None` when the
    /// key matches neither — the caller should then FR-4-decline rather than emit
    /// an ungrounded reference.
    #[must_use]
    pub fn canonical_level_key(&self, key: &str) -> Option<String> {
        if self.labels.contains_key(key) {
            return Some(key.to_string());
        }
        self.resolve_level_label(key).map(ToString::to_string)
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
    /// (0) for 4-digit year members on a date hierarchy, a level whose label
    ///     contains "year" (case-insensitive) is preferred over date-key/week-key
    ///     levels — deterministic year-level preference (FR2 of
    ///     PRD-mqo-date-member-cross-dimension-filter);
    /// (1) a candidate level (domain contains the member) that is also a query
    ///     dimension; else (2) the sole candidate if exactly one; else `None`
    ///     (ambiguous with no signal) → caller falls back to first-level.
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
        // Numeric probe: parse the probe as f64 once so we can do dtype-aware
        // comparison for integer/decimal levels without re-parsing per domain value.
        let probe_f64: Option<f64> = probe.parse::<f64>().ok();

        // Candidates: levels of this hierarchy whose enumerated domain contains the
        // member. Dedup by unique_name — the reverse index can list a level under
        // several alias keys, so the same level may appear more than once.
        let mut candidates: Vec<&String> = levels
            .iter()
            .filter(|lvl| {
                let Some(domain) = self.level_domains.get(*lvl) else {
                    return false;
                };
                // Fast path: exact lowercased string match (handles all string levels
                // and numeric levels where the probe and domain value formats agree).
                if domain.iter().any(|v| v.to_lowercase() == probe) {
                    return true;
                }
                // Slow path: dtype-aware numeric comparison.  Only attempted when
                // (a) this level's dtype is "integer" or "decimal" AND
                // (b) the probe parsed as f64 successfully.
                let dtype = self.level_dtypes.get(*lvl).map(String::as_str).unwrap_or("string");
                if matches!(dtype, "integer" | "decimal") {
                    if let Some(pf) = probe_f64 {
                        return domain.iter().any(|v| {
                            v.parse::<f64>()
                                .is_ok_and(|vf| (vf - pf).abs() < 1e-9)
                        });
                    }
                }
                false
            })
            .collect();
        candidates.sort();
        candidates.dedup();

        // (0) FR2 — year-level preference for 4-digit year members.
        // When a member looks like a 4-digit year (1000–9999) and there is at
        // least one candidate whose display label contains "year" (case-insensitive),
        // prefer those "year-label" candidates over any other date-key/week-key
        // candidates. This prevents "2002" from binding to a date-key level whose
        // domain happens to contain a 2002-prefixed token (e.g., "20020131").
        if is_four_digit_year(&probe) && candidates.len() > 1 {
            let year_candidates: Vec<&String> = candidates
                .iter()
                .copied()
                .filter(|lvl| self.level_label_contains_year(lvl))
                .collect();
            if !year_candidates.is_empty() {
                // Among the year-label candidates, apply the normal rules:
                // (1) prefer one the query also groups by; (2) sole candidate.
                if let Some(c) = year_candidates.iter().find(|c| dim_levels.iter().any(|d| d == **c)) {
                    return Some(c.as_str());
                }
                if year_candidates.len() == 1 {
                    return Some(year_candidates[0].as_str());
                }
                // Multiple year-label candidates, no dim signal — fall through to
                // the normal disambiguation below using the full candidate set.
            }
        }

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

    /// Return `true` when the display label of `level_unique_name` contains the
    /// word "year" (case-insensitive). Used by the year-level preference logic in
    /// [`Self::resolve_member_level`] (FR2).
    fn level_label_contains_year(&self, level_unique_name: &str) -> bool {
        self.labels
            .get(level_unique_name)
            .is_some_and(|label| label.to_lowercase().contains("year"))
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
    fn tables_map_uses_hierarchy_prefix_not_catalog() {
        // FR-1: per-level table = hierarchy prefix of unique_name, NOT the catalog
        // (database) name. The level's table must NOT be "tpcds_benchmark_model".
        let ctx = DaxCatalogContext::from_json(fixture_json()).unwrap();
        assert_eq!(
            ctx.tables
                .get("inventory_date_dimension.calendar.[Inventory Calendar Month]"),
            Some(&"inventory_date_dimension".to_string())
        );
        // Measures carry no per-level table entry.
        assert!(!ctx.tables.contains_key("tpcds.total_store_sales"));
    }

    #[test]
    fn canonical_level_key_accepts_unique_name_and_bare_label() {
        // FR-2: both the unique_name and the bare display label resolve to the
        // same canonical unique_name key.
        let ctx = DaxCatalogContext::from_json(fixture_json()).unwrap();
        let un = "inventory_date_dimension.calendar.[Inventory Calendar Month]";
        assert_eq!(ctx.canonical_level_key(un).as_deref(), Some(un));
        assert_eq!(
            ctx.canonical_level_key("Inventory Calendar Month").as_deref(),
            Some(un)
        );
        assert_eq!(ctx.canonical_level_key("no such level"), None);
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

    // ── Dtype-aware numeric member resolution (PRD-mqo-numeric-member-filter) ──

    fn numeric_ctx() -> DaxCatalogContext {
        // Income Band level: domain stored as "9.00", "10.00", "20.00" (decimal dtype).
        // Age Band level: domain stored as "18", "25", "35" (integer dtype).
        // String level: domain stored as "Low", "Medium", "High" (string dtype).
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"customer_demographics.[Income Band]","label":"Income Band",
           "hierarchy":"customer_demographics","level":"Income Band",
           "domain":["9.00","10.00","20.00"],"value_type":"decimal"},
          {"kind":"level","unique_name":"customer_demographics.[Age Band]","label":"Age Band",
           "hierarchy":"customer_demographics","level":"Age Band",
           "domain":["18","25","35"],"value_type":"integer"},
          {"kind":"level","unique_name":"customer_demographics.[Income Group]","label":"Income Group",
           "hierarchy":"customer_demographics","level":"Income Group",
           "domain":["Low","Medium","High"]}
        ]}"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    #[test]
    fn decimal_probe_without_trailing_zeros_matches_domain() {
        // Probe "9" should match domain entry "9.00" via numeric comparison.
        let c = numeric_ctx();
        let got = c.resolve_member_level("customer_demographics", &["9".into()], &[]);
        assert_eq!(got, Some("customer_demographics.[Income Band]"));
    }

    #[test]
    fn decimal_probe_with_trailing_zeros_matches_domain() {
        // Probe "10.00" matches domain entry "10.00" exactly (fast path) or numerically.
        let c = numeric_ctx();
        let got = c.resolve_member_level("customer_demographics", &["10.00".into()], &[]);
        assert_eq!(got, Some("customer_demographics.[Income Band]"));
    }

    #[test]
    fn integer_probe_matches_integer_domain() {
        // Probe "25" matches domain entry "25" in the integer-typed Age Band.
        let c = numeric_ctx();
        let got = c.resolve_member_level("customer_demographics", &["25".into()], &[]);
        assert_eq!(got, Some("customer_demographics.[Age Band]"));
    }

    #[test]
    fn numeric_probe_does_not_match_string_domain() {
        // "9" is not in the string domain ["Low","Medium","High"].
        let c = numeric_ctx();
        // Neither Age Band (18/25/35) nor Income Band (9.00/10.00/20.00) contain "5",
        // and Income Group (string) does not either.
        let got = c.resolve_member_level("customer_demographics", &["5".into()], &[]);
        assert_eq!(got, None);
    }

    #[test]
    fn value_type_decimal_stored_in_level_dtypes() {
        let c = numeric_ctx();
        assert_eq!(
            c.level_dtypes.get("customer_demographics.[Income Band]").map(String::as_str),
            Some("decimal")
        );
        assert_eq!(
            c.level_dtypes.get("customer_demographics.[Age Band]").map(String::as_str),
            Some("integer")
        );
        // String type is not stored (only non-string dtypes are recorded).
        assert!(!c.level_dtypes.contains_key("customer_demographics.[Income Group]"));
    }
}

// ── PRD-mqo-date-member-cross-dimension-filter: year-level preference (FR2) ─
#[cfg(test)]
mod date_cross_dimension_tests {
    use super::DaxCatalogContext;

    /// Build a catalog that has:
    /// - sold_date_dimensions hierarchy with a Sold Calendar Year level (domain: years)
    ///   and a Sold Date Key level (domain: contains a 2002-prefixed token to test
    ///   that year-level preference wins over date-key level).
    /// - store_dimension hierarchy with a Store Name level (domain: store names).
    fn date_and_store_ctx() -> DaxCatalogContext {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"sold_date_dimensions.[Sold Calendar Year]","label":"Sold Calendar Year","hierarchy":"sold_date_dimensions","level":"Year","domain":["1998","1999","2000","2001","2002","2003"]},
          {"kind":"level","unique_name":"sold_date_dimensions.[Sold Date Key]","label":"Sold Date Key","hierarchy":"sold_date_dimensions","level":"Date Key","domain":["20020101","20020102","20020103","20011231"]},
          {"kind":"level","unique_name":"store_dimension.[Store Name]","label":"Store Name","hierarchy":"store_dimension","level":"Store Name","domain":["ese","bar","baz"]},
          {"kind":"measure","unique_name":"tpcds.net_profit","label":"Net Profit"}
        ]}"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    /// FR2: A 4-digit year member "2002" binds to the Sold Calendar Year level,
    /// NOT to the Sold Date Key level whose domain contains "20020101" etc.
    /// (The domain match on the date key is not a 4-digit year exact match, but
    /// we verify year-label preference even if the exact probe only matches the
    /// year level here.)
    #[test]
    fn year_member_binds_to_year_level_not_date_key() {
        let c = date_and_store_ctx();
        let got = c.resolve_member_level("sold_date_dimensions", &["2002".into()], &[]);
        assert_eq!(
            got,
            Some("sold_date_dimensions.[Sold Calendar Year]"),
            "year member must bind to the year level, not a date-key level"
        );
    }

    /// FR2: Even without an exact domain match on the date-key level (since "2002"
    /// != "20020101"), this verifies the year-label preference when two levels both
    /// have "2002" in domain (simulating an integer year appearing in a date key domain).
    #[test]
    fn year_member_prefers_year_label_level_when_ambiguous() {
        // Both levels have "2002" in their domain.
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"sold_date_dimensions.[Sold Calendar Year]","label":"Sold Calendar Year","hierarchy":"sold_date_dimensions","level":"Year","domain":["2001","2002","2003"]},
          {"kind":"level","unique_name":"sold_date_dimensions.[Sold Week Key]","label":"Sold Week Key","hierarchy":"sold_date_dimensions","level":"Week Key","domain":["200147","2002","200201"]},
          {"kind":"measure","unique_name":"tpcds.m","label":"M"}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        // "2002" is in both domains — year-label preference must pick Sold Calendar Year.
        let got = c.resolve_member_level("sold_date_dimensions", &["2002".into()], &[]);
        assert_eq!(
            got,
            Some("sold_date_dimensions.[Sold Calendar Year]"),
            "year member must prefer the year-label level over week-key level"
        );
    }

    /// FR2: A non-year member (store name "ese") resolves via its own domain
    /// independently — year-level preference does NOT apply.
    #[test]
    fn non_year_member_resolves_via_domain_independently() {
        let c = date_and_store_ctx();
        let got = c.resolve_member_level("store_dimension", &["ese".into()], &[]);
        assert_eq!(
            got,
            Some("store_dimension.[Store Name]"),
            "non-year member must resolve via its level domain"
        );
    }

    /// FR2: A 3-digit or 5-digit string that is NOT a 4-digit year does not
    /// trigger year-level preference.
    #[test]
    fn non_four_digit_string_does_not_trigger_year_preference() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"dim.[Year]","label":"Sold Calendar Year","hierarchy":"dim","level":"Year","domain":["20020","30000"]},
          {"kind":"level","unique_name":"dim.[Key]","label":"Key","hierarchy":"dim","level":"Key","domain":["20020","99999"]},
          {"kind":"measure","unique_name":"tpcds.m","label":"M"}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        // "20020" is 5 digits — year-level preference does not apply;
        // normal disambiguation fires (ambiguous → None).
        let got = c.resolve_member_level("dim", &["20020".into()], &[]);
        // Ambiguous (both candidates, no dim signal) → None.
        assert_eq!(got, None);
    }

    /// is_four_digit_year helper covers boundary cases.
    #[test]
    fn is_four_digit_year_helper() {
        assert!(super::is_four_digit_year("2002"));
        assert!(super::is_four_digit_year("1998"));
        assert!(super::is_four_digit_year("9999"));
        assert!(super::is_four_digit_year("1000"));
        assert!(!super::is_four_digit_year("200"));    // 3 digits
        assert!(!super::is_four_digit_year("20020"));  // 5 digits
        assert!(!super::is_four_digit_year("200x"));   // non-numeric
        assert!(!super::is_four_digit_year(""));
    }
}
