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

/// Normalize a string member value for comparison purposes.
///
/// Applied to **both** the filter value and captured domain entries before the
/// equality check on string-dtype levels (PRD-mqo-string-member-filter-completeness,
/// FR1–FR4, FR8).
///
/// The transformation is:
/// 1. Trim leading/trailing ASCII whitespace.
/// 2. Collapse internal whitespace runs to a single ASCII space.
/// 3. Fold to ASCII lowercase (invariant, non-locale — only ASCII is folded;
///    non-ASCII chars are preserved, satisfying NFR2).
/// 4. Fold common equivalent punctuation to a canonical form:
///    - Curly/typographic single quotes (`\u{2018}`, `\u{2019}`, `\u{02BC}`) → `'`
///    - Curly/typographic double quotes (`\u{201C}`, `\u{201D}`) → `"`
///    - En-dash (`\u{2013}`) and em-dash (`\u{2014}`) → `-`
///
/// Matching remains **equality** of the normalized forms — not substring, prefix,
/// or contains (FR3). This function is O(length) and allocation-bounded (NFR4).
///
/// An identical copy lives in `mqo-catalog-binder/src/binder.rs` so both
/// comparison sites use the same rule without a circular crate dependency (FR8, OQ3).
pub(crate) fn normalize_member_string(s: &str) -> String {
    // Step 1 & 2: trim + collapse whitespace, then step 3: ASCII lowercase.
    let collapsed: String = s
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_ascii_lowercase();

    // Step 4: fold punctuation equivalents.
    collapsed
        .chars()
        .map(|c| match c {
            // Curly/typographic single quotes → straight apostrophe
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
            // Curly/typographic double quotes → straight double quote
            '\u{201C}' | '\u{201D}' => '"',
            // En-dash and em-dash → hyphen-minus
            '\u{2013}' | '\u{2014}' => '-',
            other => other,
        })
        .collect()
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
    /// True LEVEL_CARDINALITY from MDSCHEMA (written by `catalog_ingest`).
    /// When present, used together with `domain.len()` to determine whether
    /// the captured domain is complete (all members captured) or partial
    /// (truncated at the capture cap). Absent in older snapshots or for
    /// levels with no cardinality metadata → treated conservatively as unknown.
    #[serde(default)]
    cardinality: Option<u64>,
    /// Explicit `domain_complete` flag. When `true`, the capturing agent
    /// confirmed that the domain was fully enumerated. When `false` or
    /// absent, the completeness is inferred from `cardinality` vs
    /// `domain.len()` (or conservatively `false` when neither is available).
    #[serde(default)]
    domain_complete: Option<bool>,
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
    /// `AtScale`'s XMLA tabular model creates one table per *hierarchy*, named by
    /// the hierarchy prefix of the level's `unique_name`
    /// (e.g. `ship_mode.[Carrier]` → `ship_mode`, `ship_mode.[Ship Mode Type]`
    /// → `ship_mode`). Used by [`crate::codegen::compile_grounded`] to emit
    /// `'<hierarchy>'[<label>]` column refs per level, instead of grounding
    /// every column to the single global `table_name` (which is the `PGWire`
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

    /// `level unique_name → domain_complete` flag for levels with a captured domain.
    ///
    /// A level is `true` iff the captured domain is **complete** — the ingestion
    /// enumerated every member of the level (cardinality ≤ cap and capture was
    /// not truncated). A level with `false` has only a partial sample: the domain
    /// can still locate the level for `Member` filter grounding, but a query
    /// driven by a filter on this level may not return all matching rows if the
    /// projection is scoped to the sampled domain.
    ///
    /// **Sources of truth** (first applicable wins):
    /// 1. An explicit `domain_complete: true` field in the snapshot JSON.
    /// 2. `cardinality` present in the snapshot AND `domain.len() >= cardinality`
    ///    (all members were captured before hitting the cap).
    /// 3. Conservative default: `false` (unknown = possibly-incomplete).
    ///
    /// Levels absent from this map have no captured domain and are treated
    /// as `false` by [`Self::is_domain_complete`].
    ///
    /// **NFR1 / AC4 invariant**: codegen changes keyed on this flag MUST produce
    /// byte-identical DAX for levels where `is_domain_complete` returns `true`.
    /// Only the `false` path adds new diagnostics / guards.
    pub level_domain_complete: HashMap<String, bool>,

    /// `level unique_name → captured domain size` for levels with a captured domain.
    ///
    /// Stored to enable the operator diagnostic (FR5 / AC6): when a filter on an
    /// incomplete-domain level drives a projection, the compiler can emit the
    /// sample size and cardinality so the operator can judge how much is missing.
    pub level_domain_sizes: HashMap<String, usize>,

    /// `level unique_name → true LEVEL_CARDINALITY` for levels where the ingestion
    /// recorded the MDSCHEMA cardinality.  Used in the FR5 diagnostic message
    /// (sample size vs true cardinality).  Absent when the snapshot was produced
    /// before this field was added or when MDSCHEMA returned no cardinality.
    pub level_cardinalities: HashMap<String, u64>,

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
        let mut level_domain_complete: HashMap<String, bool> = HashMap::new();
        let mut level_domain_sizes: HashMap<String, usize> = HashMap::new();
        let mut level_cardinalities: HashMap<String, u64> = HashMap::new();
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
                        let domain_len = d.len();
                        level_domains.insert(col.unique_name.clone(), d.clone());
                        level_domain_sizes.insert(col.unique_name.clone(), domain_len);

                        // Compute domain_complete (FR1/FR2/AC3):
                        // Priority 1: explicit flag in the snapshot JSON.
                        // Priority 2: cardinality from MDSCHEMA — complete iff
                        //             cardinality > 0 and domain captured all of them.
                        // Priority 3: conservative false (unknown = possibly-incomplete).
                        let complete = if let Some(explicit) = col.domain_complete {
                            // Explicit flag always wins (future-proof for agents that
                            // know completeness at capture time).
                            explicit
                        } else if let Some(card) = col.cardinality {
                            // card == 0: MDSCHEMA returned empty/unknown → conservative false.
                            // card > 0 and domain_len >= card: we captured everything.
                            card > 0 && domain_len as u64 >= card
                        } else {
                            // No cardinality metadata and no explicit flag:
                            // default false (AC3: absent flag → false).
                            false
                        };
                        level_domain_complete.insert(col.unique_name.clone(), complete);

                        // Store cardinality for operator diagnostics (FR5/AC6).
                        if let Some(card) = col.cardinality {
                            if card > 0 {
                                level_cardinalities.insert(col.unique_name.clone(), card);
                            }
                        }
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
            level_domain_complete,
            level_domain_sizes,
            level_cardinalities,
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
        let probe_raw = members.first()?;
        // Normalized probe used for string comparisons (trim + collapse whitespace
        // + ASCII lowercase + punctuation folding). This supersedes the old bare
        // `to_lowercase()` fast path (PRD-mqo-string-member-filter-completeness FR1-FR4).
        let probe = normalize_member_string(probe_raw);
        // Keep a plain lowercase copy for the numeric parse path (unchanged, FR5).
        let probe_lc = probe_raw.to_lowercase();
        let levels = self.hierarchy_levels.get(hierarchy).or_else(|| {
            let lower = hierarchy.to_lowercase();
            self.hierarchy_levels
                .iter()
                .find(|(k, _)| k.to_lowercase() == lower)
                .map(|(_, v)| v)
        })?;
        // Numeric probe: parse the probe as f64 once so we can do dtype-aware
        // comparison for integer/decimal levels without re-parsing per domain value.
        let probe_f64: Option<f64> = probe_lc.parse::<f64>().ok();

        // Candidates: levels of this hierarchy whose enumerated domain contains the
        // member. Dedup by unique_name — the reverse index can list a level under
        // several alias keys, so the same level may appear more than once.
        let mut candidates: Vec<&String> = levels
            .iter()
            .filter(|lvl| {
                let Some(domain) = self.level_domains.get(*lvl) else {
                    return false;
                };
                // Fast path: normalized string match — handles all string levels and
                // numeric levels where probe and domain value formats agree after
                // whitespace/case/punctuation normalization
                // (PRD-mqo-string-member-filter-completeness FR1-FR4).
                if domain.iter().any(|v| normalize_member_string(v) == probe) {
                    return true;
                }
                // Slow path: dtype-aware numeric comparison.  Only attempted when
                // (a) this level's dtype is "integer" or "decimal" AND
                // (b) the probe parsed as f64 successfully.
                let dtype = self.level_dtypes.get(*lvl).map_or("string", String::as_str);
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

    /// Return `true` when the captured domain of `level_unique_name` is known to be
    /// complete — every member of the level was enumerated during capture.
    ///
    /// Returns `false` conservatively when:
    /// - the level has no captured domain at all,
    /// - the snapshot was produced before domain completeness was tracked
    ///   (no `cardinality` and no `domain_complete` flag), or
    /// - the cardinality exceeded the domain size (truncated at the cap).
    ///
    /// Only returns `true` when there is positive evidence of completeness
    /// (explicit `domain_complete: true` or `cardinality ≤ domain.len()`).
    ///
    /// **Key invariant (NFR1/AC4):** when this returns `true`, the compiler
    /// MUST produce byte-identical DAX to the pre-domain-completeness build.
    /// Only the `false` path adds new diagnostics.
    #[must_use]
    pub fn is_domain_complete(&self, level_unique_name: &str) -> bool {
        self.level_domain_complete
            .get(level_unique_name)
            .copied()
            .unwrap_or(false)
    }

    /// Return a structured diagnostic string for an operator-visible partial-domain
    /// filter warning (FR5 / AC6).
    ///
    /// Returns `Some(diagnostic_str)` when:
    /// - `level_unique_name` has a captured domain (`level_domains` entry), AND
    /// - `is_domain_complete` returns `false` (the domain is a partial sample).
    ///
    /// Returns `None` when the domain is complete (no diagnostic needed) or
    /// when the level has no captured domain at all (no signal to report).
    ///
    /// The returned string is intended to be embedded as a DAX comment or logged
    /// by the operator control plane; it names the level, the captured sample
    /// size, and the true cardinality (when known).
    #[must_use]
    pub fn partial_domain_diagnostic(&self, level_unique_name: &str) -> Option<String> {
        // Only emit when there is a captured (but incomplete) domain.
        if !self.level_domains.contains_key(level_unique_name) {
            return None;
        }
        if self.is_domain_complete(level_unique_name) {
            return None;
        }
        let sample_size = self.level_domain_sizes.get(level_unique_name).copied().unwrap_or(0);
        let cardinality_note = self
            .level_cardinalities
            .get(level_unique_name)
            .map(|c| format!(", true_cardinality={c}"))
            .unwrap_or_default();
        Some(format!(
            "partial_domain_filter: level=\"{level_unique_name}\" sample_size={sample_size}{cardinality_note}"
        ))
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
    /// - `sold_date_dimensions` hierarchy with a Sold Calendar Year level (domain: years)
    ///   and a Sold Date Key level (domain: contains a 2002-prefixed token to test
    ///   that year-level preference wins over date-key level).
    /// - `store_dimension` hierarchy with a Store Name level (domain: store names).
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

    /// `is_four_digit_year` helper covers boundary cases.
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

// ── PRD-mqo-string-member-filter-completeness: normalize_member_string + string
//    member resolution robustness ───────────────────────────────────────────────
#[cfg(test)]
mod string_member_normalization_tests {
    use super::{normalize_member_string, DaxCatalogContext};

    // ── normalize_member_string unit tests ────────────────────────────────────

    #[test]
    fn normalize_trims_leading_trailing_whitespace() {
        assert_eq!(normalize_member_string("  able  "), "able");
        assert_eq!(normalize_member_string("able "), "able");
        assert_eq!(normalize_member_string(" able"), "able");
    }

    #[test]
    fn normalize_collapses_internal_whitespace() {
        assert_eq!(normalize_member_string("able  corp"), "able corp");
        assert_eq!(normalize_member_string("able\t corp"), "able corp");
        assert_eq!(normalize_member_string("a  b  c"), "a b c");
    }

    #[test]
    fn normalize_lowercases_ascii() {
        assert_eq!(normalize_member_string("ABLE"), "able");
        assert_eq!(normalize_member_string("Able Corp"), "able corp");
    }

    #[test]
    fn normalize_folds_curly_single_quotes() {
        // U+2018 LEFT SINGLE QUOTATION MARK, U+2019 RIGHT SINGLE QUOTATION MARK
        assert_eq!(normalize_member_string("\u{2018}able\u{2019}"), "'able'");
        // U+02BC MODIFIER LETTER APOSTROPHE
        assert_eq!(normalize_member_string("o\u{02BC}brien"), "o'brien");
    }

    #[test]
    fn normalize_folds_curly_double_quotes() {
        assert_eq!(normalize_member_string("\u{201C}able\u{201D}"), "\"able\"");
    }

    #[test]
    fn normalize_folds_en_and_em_dash() {
        // en-dash U+2013
        assert_eq!(normalize_member_string("able\u{2013}corp"), "able-corp");
        // em-dash U+2014
        assert_eq!(normalize_member_string("able\u{2014}corp"), "able-corp");
    }

    #[test]
    fn normalize_preserves_straight_apostrophe() {
        // Existing apostrophes must be preserved (not folded away).
        assert_eq!(normalize_member_string("o'brien"), "o'brien");
    }

    #[test]
    fn normalize_preserves_hyphen() {
        assert_eq!(normalize_member_string("able-corp"), "able-corp");
    }

    #[test]
    fn normalize_empty_string() {
        assert_eq!(normalize_member_string(""), "");
        assert_eq!(normalize_member_string("   "), "");
    }

    // ── AC-4: no over-match (equality preserved) ──────────────────────────────

    #[test]
    fn normalize_does_not_produce_substring_match() {
        // 'able' normalized is "able"; 'able corp', 'unable', 'abel' all normalize
        // differently — equality must NOT hold.
        let able = normalize_member_string("able");
        assert_ne!(normalize_member_string("able corp"), able, "able corp must not match able");
        assert_ne!(normalize_member_string("unable"), able, "unable must not match able");
        assert_ne!(normalize_member_string("abel"), able, "abel must not match able");
    }

    // ── resolve_member_level integration tests ────────────────────────────────

    fn mfg_ctx() -> DaxCatalogContext {
        // Simulates the able-manufacturer-brands scenario:
        // domain has "able" (exact), "able " (trailing space), "ABLE" (case variant),
        // "able  corp" (double internal space), and non-matching entries.
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level",
           "unique_name":"product_dimension.[Product Manufacturer Name]",
           "label":"Product Manufacturer Name",
           "hierarchy":"product_dimension",
           "level":"Product Manufacturer Name",
           "domain":["able","able ","ABLE","able  corp","unable","able corp","abel"]}
        ]}"#;
        DaxCatalogContext::from_json(json).unwrap()
    }

    /// AC-1: trailing whitespace in stored domain entry matches exact filter value.
    #[test]
    fn trailing_whitespace_domain_entry_matches_filter() {
        // The catalog has "able " (trailing space); filter probe is "able".
        // After normalization both become "able" → match.
        // We test via a catalog where ONLY the trailing-space variant is present.
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level",
           "unique_name":"product_dimension.[Mfg Name]",
           "label":"Mfg Name",
           "hierarchy":"product_dimension",
           "level":"Mfg Name",
           "domain":["able "]}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let got = c.resolve_member_level("product_dimension", &["able".into()], &[]);
        assert_eq!(got, Some("product_dimension.[Mfg Name]"),
            "trailing-space domain entry must match normalized filter value");
    }

    /// AC-2: internal double-space in domain entry matches filter with single space.
    #[test]
    fn internal_double_space_domain_entry_matches_filter() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level",
           "unique_name":"product_dimension.[Mfg Name]",
           "label":"Mfg Name",
           "hierarchy":"product_dimension",
           "level":"Mfg Name",
           "domain":["able  corp"]}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let got = c.resolve_member_level("product_dimension", &["able corp".into()], &[]);
        assert_eq!(got, Some("product_dimension.[Mfg Name]"),
            "double-space domain entry must match single-space filter value");
    }

    /// AC-5: case-folding is preserved (existing behaviour retained).
    #[test]
    fn case_folded_domain_entry_matches_filter() {
        let c = mfg_ctx();
        let got = c.resolve_member_level("product_dimension", &["ABLE".into()], &[]);
        assert_eq!(got, Some("product_dimension.[Product Manufacturer Name]"),
            "case-only difference must still match (FR4)");
    }

    /// AC-4 (no over-match): filter 'able' must NOT match 'able corp', 'unable', 'abel'.
    #[test]
    fn able_does_not_match_over_specific_or_prefix_entries() {
        // Build a catalog with ONLY the near-name entries (no exact 'able' or 'able ' entry).
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level",
           "unique_name":"product_dimension.[Mfg Name]",
           "label":"Mfg Name",
           "hierarchy":"product_dimension",
           "level":"Mfg Name",
           "domain":["able corp","unable","abel"]}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let got = c.resolve_member_level("product_dimension", &["able".into()], &[]);
        assert_eq!(got, None,
            "filter 'able' must not match 'able corp', 'unable', or 'abel' (FR3 — no over-match)");
    }

    /// AC-3: curly-quote variant in domain matches straight-quote filter.
    #[test]
    fn curly_quote_domain_entry_matches_straight_quote_filter() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level",
           "unique_name":"product_dimension.[Mfg Name]",
           "label":"Mfg Name",
           "hierarchy":"product_dimension",
           "level":"Mfg Name",
           "domain":["o’brien"]}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let got = c.resolve_member_level("product_dimension", &["o'brien".into()], &[]);
        assert_eq!(got, Some("product_dimension.[Mfg Name]"),
            "curly-quote in domain must match straight-quote filter (FR2)");
    }

    /// AC-6 (numeric branch unchanged): a decimal probe on an integer/decimal level
    /// still resolves via the numeric path — normalization must not break it.
    #[test]
    fn numeric_path_unchanged_after_string_normalization() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"customer_demographics.[Income Band]","label":"Income Band",
           "hierarchy":"customer_demographics","level":"Income Band",
           "domain":["9.00","10.00","20.00"],"value_type":"decimal"},
          {"kind":"level","unique_name":"customer_demographics.[Income Group]","label":"Income Group",
           "hierarchy":"customer_demographics","level":"Income Group",
           "domain":["Low","Medium","High"]}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        // Probe "9" must still match "9.00" via numeric path (FR5).
        let got = c.resolve_member_level("customer_demographics", &["9".into()], &[]);
        assert_eq!(got, Some("customer_demographics.[Income Band]"),
            "numeric path must be unchanged (FR5)");
    }
}

// ── PRD-mqo-member-filter-recall-incomplete-domain: domain completeness flag ──
#[cfg(test)]
mod domain_completeness_tests {
    use super::DaxCatalogContext;

    // ── AC3: domain_complete flag round-trips correctly ───────────────────────

    /// AC3 (complete): cardinality == domain.len() → complete=true.
    #[test]
    fn level_with_cardinality_le_domain_is_complete() {
        // 3 members captured, cardinality=3 → all captured → complete=true.
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"product_dimension.[Brand]","label":"Product Brand Name",
           "hierarchy":"product_dimension","level":"Brand",
           "domain":["alpha","beta","gamma"],"cardinality":3}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        assert!(
            c.is_domain_complete("product_dimension.[Brand]"),
            "cardinality==domain.len() must yield domain_complete=true"
        );
    }

    /// AC3 (truncated): cardinality > domain.len() → complete=false (domain is a sample).
    #[test]
    fn level_with_cardinality_gt_domain_is_incomplete() {
        // 3 members captured, cardinality=246 (true level size) → truncated at cap.
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"product_dimension.[Brand]","label":"Product Brand Name",
           "hierarchy":"product_dimension","level":"Brand",
           "domain":["alpha","beta","gamma"],"cardinality":246}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        assert!(
            !c.is_domain_complete("product_dimension.[Brand]"),
            "cardinality > domain.len() must yield domain_complete=false"
        );
    }

    /// AC3 (absent flag / older snapshot): no cardinality, no domain_complete → false.
    #[test]
    fn level_without_cardinality_defaults_to_incomplete() {
        // Old snapshot: domain captured but no cardinality field at all.
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"product_dimension.[Brand]","label":"Product Brand Name",
           "hierarchy":"product_dimension","level":"Brand",
           "domain":["alpha","beta","gamma"]}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        // No cardinality, no explicit flag → conservatively false (AC3 / FR2 migration clause).
        assert!(
            !c.is_domain_complete("product_dimension.[Brand]"),
            "absent cardinality and no domain_complete flag must default to false (conservative)"
        );
    }

    /// AC3 (explicit flag=true): explicit domain_complete=true in the snapshot overrides cardinality.
    #[test]
    fn explicit_domain_complete_true_wins_over_cardinality() {
        // Explicit true even with no cardinality field — explicit wins.
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"product_dimension.[Brand]","label":"Product Brand Name",
           "hierarchy":"product_dimension","level":"Brand",
           "domain":["alpha","beta","gamma"],"domain_complete":true}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        assert!(
            c.is_domain_complete("product_dimension.[Brand]"),
            "explicit domain_complete=true must yield true regardless of cardinality"
        );
    }

    /// AC7 (no domain at all): is_domain_complete returns false for a level with no domain.
    #[test]
    fn level_without_domain_is_not_complete() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"store_dimension.[Store Name]","label":"Store Name",
           "hierarchy":"store_dimension","level":"Store Name"}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        assert!(
            !c.is_domain_complete("store_dimension.[Store Name]"),
            "level with no captured domain must return is_domain_complete=false"
        );
    }

    // ── AC6: partial_domain_diagnostic emits for incomplete, suppresses for complete ─

    /// AC6 (incomplete level): diagnostic is emitted with level, sample size, cardinality.
    #[test]
    fn partial_domain_diagnostic_emitted_for_incomplete_level() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"product_dimension.[Product Manufacturer Name]",
           "label":"Product Manufacturer Name",
           "hierarchy":"product_dimension","level":"Manufacturer Name",
           "domain":["able","b-corp","c-corp"],"cardinality":246}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let diag = c.partial_domain_diagnostic("product_dimension.[Product Manufacturer Name]");
        assert!(diag.is_some(), "partial-domain level must produce a diagnostic");
        let d = diag.unwrap();
        assert!(
            d.contains("partial_domain_filter"),
            "diagnostic must contain 'partial_domain_filter': {d}"
        );
        assert!(
            d.contains("sample_size=3"),
            "diagnostic must include the captured sample size: {d}"
        );
        assert!(
            d.contains("true_cardinality=246"),
            "diagnostic must include the true cardinality: {d}"
        );
    }

    /// AC6 (complete level): no diagnostic is emitted.
    #[test]
    fn partial_domain_diagnostic_suppressed_for_complete_level() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"product_dimension.[Brand]","label":"Brand",
           "hierarchy":"product_dimension","level":"Brand",
           "domain":["alpha","beta","gamma"],"cardinality":3}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let diag = c.partial_domain_diagnostic("product_dimension.[Brand]");
        assert!(
            diag.is_none(),
            "complete-domain level must NOT emit a partial-domain diagnostic"
        );
    }

    /// AC6 (no domain at all): no diagnostic emitted — there is nothing to report.
    #[test]
    fn partial_domain_diagnostic_suppressed_when_no_domain() {
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"store_dimension.[Store Name]","label":"Store Name",
           "hierarchy":"store_dimension","level":"Store Name"}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let diag = c.partial_domain_diagnostic("store_dimension.[Store Name]");
        assert!(
            diag.is_none(),
            "level with no captured domain must NOT emit a diagnostic (AC7)"
        );
    }

    /// AC6 / FR5: diagnostic omits true_cardinality when cardinality was not in snapshot.
    #[test]
    fn partial_domain_diagnostic_without_cardinality_note() {
        // Incomplete domain (explicit domain_complete=false, no cardinality).
        let json = r#"{"catalog":"atscale_catalogs","columns":[
          {"kind":"level","unique_name":"product_dimension.[Brand]","label":"Brand",
           "hierarchy":"product_dimension","level":"Brand",
           "domain":["alpha","beta"],"domain_complete":false}
        ]}"#;
        let c = DaxCatalogContext::from_json(json).unwrap();
        let diag = c.partial_domain_diagnostic("product_dimension.[Brand]");
        assert!(diag.is_some(), "explicit domain_complete=false must produce a diagnostic");
        let d = diag.unwrap();
        assert!(!d.contains("true_cardinality"), "no cardinality → no true_cardinality note: {d}");
        assert!(d.contains("sample_size=2"), "must include sample size: {d}");
    }
}
