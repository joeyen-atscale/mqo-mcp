//! mqo-param-validator
//!
//! Server-side validator that rejects unmapped MQO fields before execution.
//! Pure, deterministic — no LLM, no network, no unsafe.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use strsim::jaro_winkler;

// ---------------------------------------------------------------------------
// Catalog types
// ---------------------------------------------------------------------------

/// A snapshot of the catalog returned by `describe_model`.
/// Fields are represented as flat lists; hierarchies carry their member levels.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogSnapshot {
    #[serde(default)]
    pub measures: Vec<CatalogMeasure>,
    #[serde(default)]
    pub dimensions: Vec<CatalogDimension>,
    #[serde(default)]
    pub hierarchies: Vec<CatalogHierarchy>,
    #[serde(default)]
    pub date_roles: Vec<CatalogDateRole>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogMeasure {
    pub unique_name: String,
    /// Optional subject area / fact name for cross-fact detection
    #[serde(default)]
    pub subject_area: Option<String>,
    /// Optional human-readable label (catalog `label` field). When present this
    /// is the preferred surface for calc detection/triggers; falls back to
    /// `unique_name`.
    #[serde(default)]
    pub label: Option<String>,
    /// Optional explicit "this is a packaged calculated measure" flag from the
    /// catalog. When absent (or false) calc-detection falls back to name
    /// heuristics over the label/unique_name.
    #[serde(default)]
    pub is_calc: Option<bool>,
    /// Optional semi-additive flag from the enriched catalog: `true` when this
    /// measure is a balance/snapshot that does NOT add across the time axis
    /// (inventory-on-hand, account balance). When `None`/`false` the
    /// semi-additive guard does not fire (the recorded fixture nulls this, so
    /// the guard is dormant on the live fixture — see PRD OQ-1).
    #[serde(default)]
    pub semi_additive: Option<bool>,
    /// Optional declared semi-additive aggregation policy (e.g. `"last"`,
    /// `"first"`, `"average"`). When present the `SemiAdditiveSum` suggestion
    /// names it; else it suggests "average over period" with a note.
    #[serde(default)]
    pub semi_additive_agg: Option<String>,
    /// Optional explicit ratio/percentage/average classification for a calc
    /// measure. When present it overrides the name heuristic in the
    /// calc-aggregation guard. `None` falls back to the name signal.
    #[serde(default)]
    pub calc_kind: Option<CalcKind>,
}

/// Classification of an `is_calc` measure for the calc-aggregation guard.
/// `Ratio` (percentage / average / rate) cannot be summed or averaged across
/// groups; `Additive` (a `* Increase`/`* Growth`/`* Delta` calc) is safe to
/// aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CalcKind {
    /// Percentage, average, rate — non-additive across groups.
    Ratio,
    /// Delta/increase/growth — additive, safe to aggregate.
    Additive,
}

impl CatalogMeasure {
    /// The best display surface for this measure: `label` when set, else
    /// `unique_name`.
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.unique_name)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogDimension {
    pub unique_name: String,
    /// Subject areas this dimension is available in (conformed dims share multiple)
    #[serde(default)]
    pub subject_areas: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CatalogHierarchy {
    pub dimension_unique_name: String,
    pub hierarchy_unique_name: String,
    pub levels: Vec<String>,
    /// Optional per-level type/domain metadata used by the filter-level guard
    /// (Rule 4). Keyed by level label. When a level has no entry here the guard
    /// cannot decide value-fit and does NOT reject it (conservative; dormant
    /// without enrichment — see PRD OQ-1). Indexed by level label.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub level_meta: Vec<LevelDomainMeta>,
}

/// The data type a filter value must satisfy to bind to a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelValueType {
    /// A textual member key (e.g. a state abbreviation "CA", a brand name).
    String,
    /// A plain integer key (e.g. a sequential week number, a 4-digit year).
    Integer,
    /// A calendar date (e.g. "2001-01-15").
    Date,
}

/// Per-level domain/type metadata for the filter-level guard (Rule 4).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LevelDomainMeta {
    /// The level label this metadata applies to (matches a string in `levels`).
    pub level: String,
    /// The value type the level's member keys use. A range bound or member
    /// value whose type cannot match this is rejected.
    pub value_type: LevelValueType,
    /// Optional enumerated domain of member keys for a low-cardinality level
    /// (e.g. the 50 state codes). When present, a *member* filter value not in
    /// this set is rejected as out-of-domain. When absent the guard does NOT
    /// reject on membership (catalog-only; high-cardinality / unknown domain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<Vec<String>>,
    /// Optional human-readable description of the expected member-key shape
    /// (e.g. "sequential week number 1..N, not YYYYWW"), surfaced in the
    /// rejection suggestion. Mirrors filter-bind-report's `expected_key_shape`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_key_shape: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogDateRole {
    /// The role-played dimension unique name, e.g. "[Order Date]"
    pub role_name: String,
    /// The underlying date dimension it is built on, e.g. "Date"
    pub base_dimension: String,
}

// ---------------------------------------------------------------------------
// MQO input types (local deserialization struct — no dep on binder crate)
// ---------------------------------------------------------------------------

/// The bound MQO submitted by the caller.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct BoundMqoInput {
    #[serde(default)]
    pub measures: Vec<MqoMeasureRef>,
    #[serde(default)]
    pub dimensions: Vec<MqoDimensionRef>,
    #[serde(default)]
    pub filters: Vec<MqoFilterRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MqoMeasureRef {
    pub unique_name: String,
    /// Optional aggregation the MQO applies to this measure. `None` is treated
    /// as the measure's default aggregation (additive for a base measure), which
    /// the semi-additive / calc guards treat as additive. Recognized additive
    /// tokens: `sum`, `count`. Recognized non-additive: `last`, `first`,
    /// `min`, `max`, `avg`/`average`, `distinct_count`.
    #[serde(default)]
    pub aggregation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MqoDimensionRef {
    pub unique_name: String,
    /// Optional specific level within a hierarchy
    #[serde(default)]
    pub level: Option<String>,
    /// Optional hierarchy to use (if multiple exist for the dimension)
    #[serde(default)]
    pub hierarchy: Option<String>,
    /// Optional date role qualifier (e.g. "[Order Date]")
    #[serde(default)]
    pub role_qualifier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MqoFilterRef {
    pub unique_name: String,
    #[serde(default)]
    pub level: Option<String>,
    /// For a member filter: the member key values being filtered on (e.g.
    /// `["CA"]`). Empty/absent for a pure range filter. Used by the
    /// filter-level guard (Rule 4) to check value type/domain against the level.
    #[serde(default)]
    pub members: Vec<String>,
    /// For a range filter: the lower bound, as a raw string (e.g. `"200147"`,
    /// `"2001-01-01"`). `None` when no lower bound.
    #[serde(default)]
    pub range_lo: Option<String>,
    /// For a range filter: the upper bound, as a raw string. `None` when no
    /// upper bound.
    #[serde(default)]
    pub range_hi: Option<String>,
}

// ---------------------------------------------------------------------------
// Rejection types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldClass {
    Measure,
    Dimension,
    HierarchyLevel,
    DateRole,
    Filter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RejectReason {
    /// The field name is not present in the catalog at all.
    Unmapped,
    /// The level exists in the catalog but not in the hierarchy implied by the dimension.
    WrongHierarchyLevel,
    /// A date concept resolves to ≥2 role-played date dims and no role qualifier was given.
    AmbiguousDateRole,
    /// The measure and dimension come from clearly disjoint fact subject areas.
    CrossFactPath,
    /// The MQO hand-derives a measure that already exists as a packaged calc
    /// (e.g. lag/period-over-period arithmetic over a base measure that a
    /// packaged `* Increase`/`* Growth` calc already provides).
    ManualCalcRederivation,
    /// RULE 1: the MQO picks a non-canonical near-twin *dimension* (a same-core-
    /// label attribute on the wrong hierarchy, e.g. `Store Item Product Brand
    /// Name` instead of canonical `Product Brand Name`).
    NonCanonicalNearTwin {
        /// The non-canonical member the MQO picked (hierarchy unique_name).
        picked: String,
        /// The canonical member's unique_name the caller should use instead.
        suggested_canonical: String,
        /// The shared core label of the near-twin group (e.g. "brand name").
        group_core_label: String,
    },
    /// RULE 2: an additive aggregation (sum/default) of a `semi_additive`
    /// measure over a time dimension — a silent double-count of a balance.
    SemiAdditiveSum {
        /// The semi-additive measure being summed.
        measure: String,
        /// The time-typed dimension in the grouping that breaks additivity.
        time_dimension: String,
        /// The aggregation the caller should use instead (last/first/avg).
        suggested_agg: String,
    },
    /// RULE 3: sum/avg of a ratio/percentage/average `is_calc` measure — a
    /// silent statistical error (summing a percentage / averaging an average).
    CalcMisaggregation {
        /// The calc measure being mis-aggregated.
        measure: String,
        /// The offending aggregation (sum/avg).
        aggregation: String,
        /// Why this is wrong + the corrective guidance.
        reason: String,
    },
    /// RULE 4: a filter whose value type/domain cannot match the target level
    /// (a DATE/YYYYWW bound on a sequential-week numeric level; a member not in
    /// the level's domain; or a named level that does not exist).
    FilterLevelMismatch {
        /// The filter field (hierarchy / level reference).
        filter: String,
        /// The level the filter targets (or the missing level name).
        target_level: String,
        /// Why the value/level cannot match.
        reason: String,
        /// The correct level or value format to use.
        suggested: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub name: String,
    pub similarity: f64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamRejection {
    pub field: String,
    pub class: FieldClass,
    pub reason: RejectReason,
    pub suggestions: Vec<Suggestion>,
    /// For `ManualCalcRederivation`: the unique_name (or label) of the packaged
    /// calc the caller should use instead. `None` for every other reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_calc: Option<String>,
}

impl ParamRejection {
    /// Convenience constructor for the common case where there is no
    /// `suggested_calc` (every reason except `ManualCalcRederivation`).
    fn new(
        field: String,
        class: FieldClass,
        reason: RejectReason,
        suggestions: Vec<Suggestion>,
    ) -> Self {
        ParamRejection {
            field,
            class,
            reason,
            suggestions,
            suggested_calc: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Normalization helpers
// ---------------------------------------------------------------------------

fn normalize(s: &str) -> String {
    let lower: String = s
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '[' || *c == ']')
        .collect::<String>()
        .to_lowercase();
    lower
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Top-N nearest matches from `candidates`, ranked by descending Jaro-Winkler.
fn nearest_matches(query: &str, candidates: &[&str], top_n: usize) -> Vec<Suggestion> {
    let qn = normalize(query);
    let mut scored: Vec<(f64, &str)> = candidates
        .iter()
        .map(|c| (jaro_winkler(&qn, &normalize(c)), *c))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(top_n)
        .filter(|(score, _)| *score > 0.0)
        .map(|(score, name)| Suggestion {
            name: name.to_string(),
            similarity: score,
            note: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Primary validator
// ---------------------------------------------------------------------------

/// Validate a bound MQO against a catalog snapshot.
///
/// Returns an empty `Vec` if every field resolves cleanly.
/// Returns one `ParamRejection` per offending field otherwise.
/// Never panics.
pub fn validate(mqo: &BoundMqoInput, catalog: &CatalogSnapshot) -> Vec<ParamRejection> {
    let mut rejections: Vec<ParamRejection> = Vec::new();

    // Pre-build normalized look-up sets
    let measure_names: Vec<&str> = catalog
        .measures
        .iter()
        .map(|m| m.unique_name.as_str())
        .collect();
    let dimension_names: Vec<&str> = catalog
        .dimensions
        .iter()
        .map(|d| d.unique_name.as_str())
        .collect();

    // --- AC1 / AC2: measure resolution ---
    for mref in &mqo.measures {
        let norm = normalize(&mref.unique_name);
        let found = catalog
            .measures
            .iter()
            .any(|m| normalize(&m.unique_name) == norm);
        if !found {
            let all_names: Vec<&str> = measure_names
                .iter()
                .copied()
                .chain(dimension_names.iter().copied())
                .collect();
            let suggestions = nearest_matches(&mref.unique_name, &all_names, 5);
            rejections.push(ParamRejection::new(
                mref.unique_name.clone(),
                FieldClass::Measure,
                RejectReason::Unmapped,
                suggestions,
            ));
        }
    }

    // --- Dimension resolution: AC1 (happy path), AC2 (unmapped), AC3, AC4 ---
    for dref in &mqo.dimensions {
        let norm = normalize(&dref.unique_name);
        let dim_found = catalog
            .dimensions
            .iter()
            .any(|d| normalize(&d.unique_name) == norm);

        if !dim_found {
            let all_names: Vec<&str> = dimension_names
                .iter()
                .copied()
                .chain(measure_names.iter().copied())
                .collect();
            let suggestions = nearest_matches(&dref.unique_name, &all_names, 5);
            rejections.push(ParamRejection::new(
                dref.unique_name.clone(),
                FieldClass::Dimension,
                RejectReason::Unmapped,
                suggestions,
            ));
            continue; // can't meaningfully check level/date-role if dim itself is unknown
        }

        // AC3: wrong hierarchy level
        if let Some(ref level) = dref.level {
            let level_norm = normalize(level);

            let dim_hierarchies: Vec<&CatalogHierarchy> = catalog
                .hierarchies
                .iter()
                .filter(|h| normalize(&h.dimension_unique_name) == norm)
                .collect();

            if !dim_hierarchies.is_empty() {
                let chosen_hier: Option<&CatalogHierarchy> =
                    if let Some(ref hier_name) = dref.hierarchy {
                        let hname_norm = normalize(hier_name);
                        dim_hierarchies
                            .iter()
                            .copied()
                            .find(|h| normalize(&h.hierarchy_unique_name) == hname_norm)
                    } else {
                        dim_hierarchies.first().copied()
                    };

                if let Some(hier) = chosen_hier {
                    let level_in_hier =
                        hier.levels.iter().any(|l| normalize(l) == level_norm);
                    if !level_in_hier {
                        let suggestions: Vec<Suggestion> = hier
                            .levels
                            .iter()
                            .map(|l| Suggestion {
                                name: l.clone(),
                                similarity: jaro_winkler(&level_norm, &normalize(l)),
                                note: Some(format!(
                                    "valid level in [{}]",
                                    hier.hierarchy_unique_name
                                )),
                            })
                            .collect();
                        rejections.push(ParamRejection::new(
                            level.clone(),
                            FieldClass::HierarchyLevel,
                            RejectReason::WrongHierarchyLevel,
                            suggestions,
                        ));
                    }
                }
            }
        }

        // AC4: ambiguous date role
        check_date_role_ambiguity(dref, catalog, &mut rejections);
    }

    // Filter resolution
    for fref in &mqo.filters {
        let fnorm = normalize(&fref.unique_name);
        let found = catalog
            .dimensions
            .iter()
            .any(|d| normalize(&d.unique_name) == fnorm)
            || catalog
                .date_roles
                .iter()
                .any(|r| normalize(&r.role_name) == fnorm || normalize(&r.base_dimension) == fnorm);
        if !found {
            let all_names: Vec<&str> = dimension_names
                .iter()
                .copied()
                .chain(measure_names.iter().copied())
                .collect();
            let suggestions = nearest_matches(&fref.unique_name, &all_names, 5);
            rejections.push(ParamRejection::new(
                fref.unique_name.clone(),
                FieldClass::Filter,
                RejectReason::Unmapped,
                suggestions,
            ));
        }
    }

    // AC5: cross-fact path detection
    check_cross_fact_paths(mqo, catalog, &mut rejections);

    // FR-2/FR-3: reject hand-rederivation of a packaged calc (pre-execution).
    check_manual_calc_rederivation(mqo, catalog, &mut rejections);

    // RULE 1 (PRD near-twin): reject non-canonical near-twin dimension picks.
    check_near_twin_dimension(mqo, catalog, &mut rejections);

    // RULE 2 (PRD semi-additive guard): reject additive sum of a semi-additive
    // measure over a time dimension. Dormant unless the catalog carries
    // `semi_additive == true` (the recorded fixture nulls it).
    check_semi_additive_sum(mqo, catalog, &mut rejections);

    // RULE 3 (PRD calc-aggregation guard): reject sum/avg of a ratio calc.
    check_calc_misaggregation(mqo, catalog, &mut rejections);

    // RULE 4 (PRD filter-level check): reject a filter whose value type/domain
    // cannot match the target level. Dormant unless `level_meta` is enriched.
    check_filter_level(mqo, catalog, &mut rejections);

    rejections
}

// ---------------------------------------------------------------------------
// Aggregation classification (shared by RULE 2 + RULE 3)
// ---------------------------------------------------------------------------

/// Is the MQO aggregation additive? `None` (default) is treated as additive
/// (a base measure defaults to SUM/COUNT). Recognized additive tokens: `sum`,
/// `count`. Everything else (`last`, `first`, `min`, `max`, `avg`, `average`,
/// `distinct_count`, …) is non-additive.
fn agg_is_additive(agg: Option<&str>) -> bool {
    match agg {
        None => true,
        Some(a) => {
            let a = a.trim().to_lowercase();
            matches!(a.as_str(), "sum" | "count" | "default" | "")
        }
    }
}

/// Is this an EXPLICIT additive aggregation override (`sum`/`count`/`total`)?
/// Unlike [`agg_is_additive`], a `None` (default) is NOT explicit: for a
/// semi-additive measure the default resolves to the model's semi-additive
/// function (last-non-empty) at the engine, which is correct. Only an explicit
/// additive override double-counts the balance, so the semi-additive guard
/// keys off this — rejecting the default would false-positive every legitimate
/// "balance by period" query.
fn agg_is_explicit_additive(agg: Option<&str>) -> bool {
    matches!(
        agg.map(|a| a.trim().to_lowercase()).as_deref(),
        Some("sum") | Some("count") | Some("total")
    )
}

/// Is this aggregation an averaging aggregation (`avg`/`average`/`mean`)?
fn agg_is_average(agg: Option<&str>) -> bool {
    matches!(
        agg.map(|a| a.trim().to_lowercase()).as_deref(),
        Some("avg") | Some("average") | Some("mean")
    )
}

/// AC4 helper: detect ambiguous date role references.
fn check_date_role_ambiguity(
    dref: &MqoDimensionRef,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    if catalog.date_roles.is_empty() {
        return;
    }

    let norm = normalize(&dref.unique_name);

    // Collect all roles whose base_dimension normalizes to the same string
    let matching_roles: Vec<&CatalogDateRole> = catalog
        .date_roles
        .iter()
        .filter(|r| normalize(&r.base_dimension) == norm)
        .collect();

    if matching_roles.len() >= 2 && dref.role_qualifier.is_none() {
        let suggestions: Vec<Suggestion> = matching_roles
            .iter()
            .map(|r| Suggestion {
                name: r.role_name.clone(),
                similarity: 1.0,
                note: Some(format!(
                    "role-played date dimension based on [{}]",
                    r.base_dimension
                )),
            })
            .collect();
        rejections.push(ParamRejection::new(
            dref.unique_name.clone(),
            FieldClass::DateRole,
            RejectReason::AmbiguousDateRole,
            suggestions,
        ));
    }
}

/// AC5 helper: conservative cross-fact path detection.
///
/// Only flags when a measure's `subject_area` is set AND none of the
/// referenced dimensions cover that subject area. Conformed dimensions
/// (subject_areas == []) are never flagged (no false positives).
fn check_cross_fact_paths(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    for mref in &mqo.measures {
        let mnorm = normalize(&mref.unique_name);
        let measure = match catalog
            .measures
            .iter()
            .find(|m| normalize(&m.unique_name) == mnorm)
        {
            Some(m) => m,
            None => continue, // already Unmapped
        };

        let measure_sa = match &measure.subject_area {
            Some(sa) => sa.clone(),
            None => continue, // no subject area — conservative, skip
        };

        for dref in &mqo.dimensions {
            let dnorm = normalize(&dref.unique_name);
            let dim = match catalog
                .dimensions
                .iter()
                .find(|d| normalize(&d.unique_name) == dnorm)
            {
                Some(d) => d,
                None => continue, // already Unmapped
            };

            // Conformed dims (empty subject_areas list) never cause cross-fact
            if dim.subject_areas.is_empty() {
                continue;
            }

            if !dim.subject_areas.contains(&measure_sa) {
                let field_key = format!("{} + {}", mref.unique_name, dref.unique_name);
                let already = rejections
                    .iter()
                    .any(|r| r.field == field_key && r.reason == RejectReason::CrossFactPath);
                if !already {
                    rejections.push(ParamRejection::new(
                        field_key,
                        FieldClass::Dimension,
                        RejectReason::CrossFactPath,
                        vec![Suggestion {
                            name: measure_sa.clone(),
                            similarity: 0.0,
                            note: Some(format!(
                                "measure [{}] belongs to subject area [{}]; \
                                 dimension [{}] is not available there",
                                mref.unique_name, measure_sa, dref.unique_name
                            )),
                        }],
                    ));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FR-1: packaged-calc surfacing (is_calc + triggers)
// ---------------------------------------------------------------------------

/// The calc "kind" suffixes we recognize in a packaged-calc measure name.
/// Order matters: longer / more specific phrases first so they win the strip.
/// Each entry is `(suffix_phrase, extra_trigger_phrases)`.
const CALC_SUFFIXES: &[(&str, &[&str])] = &[
    (
        "price growth",
        &[
            "price growth",
            "growth",
            "trending",
            "vs prior period",
            "period over period",
            "yoy",
            "year over year",
        ],
    ),
    (
        "increase",
        &[
            "increase",
            "trending",
            "vs prior period",
            "period over period",
            "growth",
            "change",
            "prior period change",
        ],
    ),
    (
        "growth",
        &[
            "growth",
            "trending",
            "vs prior period",
            "period over period",
            "yoy",
            "year over year",
            "increase",
        ],
    ),
    (
        "change",
        &[
            "change",
            "vs prior period",
            "period over period",
            "prior period change",
            "trending",
        ],
    ),
    ("yoy", &["yoy", "year over year", "vs prior year", "growth"]),
    (
        "vs prior",
        &["vs prior", "vs prior period", "period over period", "prior"],
    ),
    ("prior", &["prior", "vs prior period", "period over period"]),
];

/// Surfacing output for one measure: whether it is a packaged calc and, if so,
/// the natural-language phrases that should steer the model toward it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalcSurfacing {
    pub unique_name: String,
    pub label: String,
    pub is_calc: bool,
    pub triggers: Vec<String>,
}

/// Return `Some((calc_base, extra_triggers))` if `name` matches a packaged-calc
/// name pattern (e.g. "Store Sales Increase"), where `calc_base` is the name
/// with the calc suffix stripped ("Store Sales"). `None` for an ordinary
/// measure. Pure string heuristic — no catalog state.
fn detect_calc_pattern(name: &str) -> Option<(String, &'static [&'static str])> {
    let norm = normalize(name);
    for (suffix, triggers) in CALC_SUFFIXES {
        // Match "<base> <suffix>" — the suffix as the tail of the name.
        if let Some(stripped) = norm.strip_suffix(suffix) {
            let base = stripped.trim();
            // Require a non-empty base so we don't treat the bare word
            // "Increase" as a calc.
            if !base.is_empty() {
                return Some((base.to_string(), triggers));
            }
        }
    }
    None
}

/// Is this catalog measure a packaged calc? Honors an explicit `is_calc: true`
/// flag first; otherwise falls back to the name heuristic.
pub fn is_packaged_calc(measure: &CatalogMeasure) -> bool {
    if measure.is_calc == Some(true) {
        return true;
    }
    detect_calc_pattern(measure.display_name()).is_some()
}

/// Derive the trigger-phrase list for a calc measure from its name.
/// Always includes the lower-cased full display name plus the calc-kind
/// synonym phrases. Deduplicated, order-stable.
pub fn calc_triggers(measure: &CatalogMeasure) -> Vec<String> {
    let display = measure.display_name();
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    push(normalize(display));
    if let Some((base, triggers)) = detect_calc_pattern(display) {
        for t in triggers {
            push((*t).to_string());
        }
        // base phrase too, e.g. "store sales"
        push(base);
    }
    out
}

/// FR-1: inspect a catalog and surface packaged calcs with `is_calc` + triggers.
/// Non-calc measures are returned with `is_calc:false` and an empty trigger list,
/// so the caller has a complete map.
pub fn inspect_calcs(catalog: &CatalogSnapshot) -> Vec<CalcSurfacing> {
    catalog
        .measures
        .iter()
        .map(|m| {
            let is_calc = is_packaged_calc(m);
            CalcSurfacing {
                unique_name: m.unique_name.clone(),
                label: m.display_name().to_string(),
                is_calc,
                triggers: if is_calc { calc_triggers(m) } else { Vec::new() },
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// FR-2 / FR-3: reject manual re-derivation of a packaged calc
// ---------------------------------------------------------------------------

/// Date-concept tokens used to detect that a date-level dimension is present.
const DATE_TOKENS: &[&str] = &[
    "date", "year", "quarter", "month", "week", "day", "calendar", "fiscal",
];

/// Does this dimension/filter name reference a date concept?
fn references_date(name: &str) -> bool {
    let norm = normalize(name);
    norm.split_whitespace().any(|w| DATE_TOKENS.contains(&w))
}

/// Lag / period-offset markers that, when present in a measure name, are a
/// direct signal the caller is hand-rolling a period-over-period delta.
const LAG_MARKERS: &[&str] = &[
    "lag",
    "lagged",
    "prior",
    "previous",
    "prev",
    "parallelperiod",
    "prevmember",
    "preceding",
];

fn has_lag_marker(name: &str) -> bool {
    let norm = normalize(name);
    LAG_MARKERS.iter().any(|m| norm.contains(m))
}

/// True if every word of `calc_base` appears in `measure_norm` — i.e. the MQO
/// measure is plausibly the base series the calc is built on.
/// Example: calc_base "store sales" ⊂ "total store sales".
fn measure_is_base_for(measure_norm: &str, calc_base: &str) -> bool {
    let base_words: Vec<&str> = calc_base.split_whitespace().collect();
    if base_words.is_empty() {
        return false;
    }
    base_words
        .iter()
        .all(|bw| measure_norm.split_whitespace().any(|mw| mw == *bw))
}

/// FR-2/FR-3: detect an MQO that hand-derives a packaged calc.
///
/// Conservative — fires only when ALL hold:
///
/// 1. The catalog contains a packaged `* Increase`/`* Growth`/… calc.
/// 2. The MQO re-derives the calc's base series over a period axis. The MQO
///    grammar carries no DAX, so we require a *positive* re-derivation signal
///    in the measure shape (otherwise a plain "Total Store Sales by Quarter"
///    would be falsely rejected). The signal is either (a) the calc's base
///    series measure appears 2+ times (the hand-rolled "current vs lagged
///    base" pattern), or (b) a measure references the base series AND carries
///    a lag/offset marker in its name ("Prior", "lagged", "ParallelPeriod",
///    …). The matched base measure must NOT itself be a calc.
/// 3. A date-level dimension (or filter) is present — the period axis the calc
///    derives over.
/// 4. The packaged calc is NOT already among the MQO's measures.
///
/// If any condition is unmet, it does NOT reject (FR-5: zero false positives).
fn check_manual_calc_rederivation(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    // A date axis must be present for a period-over-period re-derivation.
    let has_date_axis = mqo.dimensions.iter().any(|d| {
        references_date(&d.unique_name)
            || d.level.as_deref().map(references_date).unwrap_or(false)
            || d.role_qualifier
                .as_deref()
                .map(references_date)
                .unwrap_or(false)
    }) || mqo.filters.iter().any(|f| {
        references_date(&f.unique_name) || f.level.as_deref().map(references_date).unwrap_or(false)
    });
    if !has_date_axis {
        return;
    }

    // Normalized names of the measures the caller actually requested.
    let requested_norms: Vec<String> = mqo
        .measures
        .iter()
        .map(|m| normalize(&m.unique_name))
        .collect();

    for calc in &catalog.measures {
        if !is_packaged_calc(calc) {
            continue;
        }
        // Only the period-over-period kinds (Increase/Growth/Change/…) re-derive
        // over a date axis. Require a detectable suffix to extract a base series.
        let calc_base = match detect_calc_pattern(calc.display_name()) {
            Some((base, _)) => base,
            None => continue,
        };
        let calc_norm = normalize(calc.display_name());

        // Condition 4: already using the calc → nothing to reject.
        if requested_norms.contains(&calc_norm) {
            continue;
        }

        // Condition 2: MQO measures that are the calc's base series and not
        // themselves a calc.
        let base_refs: Vec<&MqoMeasureRef> = mqo
            .measures
            .iter()
            .filter(|mref| {
                let mnorm = normalize(&mref.unique_name);
                if mnorm == calc_norm {
                    return false; // the calc itself
                }
                if !measure_is_base_for(&mnorm, &calc_base) {
                    return false;
                }
                // Don't treat another packaged calc as a base series.
                detect_calc_pattern(&mref.unique_name).is_none()
            })
            .collect();

        if base_refs.is_empty() {
            continue;
        }

        // Positive signal: duplicate base series (current + lagged), OR an
        // explicit lag/offset marker on a base-series measure.
        let duplicate_base = base_refs.len() >= 2;
        let lagged_base = base_refs.iter().any(|r| has_lag_marker(&r.unique_name));
        if !duplicate_base && !lagged_base {
            continue;
        }

        let base_ref = base_refs[0];
        let field_key = format!(
            "{} (re-derives {})",
            base_ref.unique_name,
            calc.display_name()
        );
        let already = rejections.iter().any(|r| {
            r.reason == RejectReason::ManualCalcRederivation
                && r.suggested_calc.as_deref() == Some(calc.display_name())
        });
        if already {
            continue;
        }

        rejections.push(ParamRejection {
            field: field_key,
            class: FieldClass::Measure,
            reason: RejectReason::ManualCalcRederivation,
            suggestions: vec![Suggestion {
                name: calc.unique_name.clone(),
                similarity: 1.0,
                note: Some(format!(
                    "use packaged calc [{}] instead of re-deriving [{}] over a date axis",
                    calc.display_name(),
                    base_ref.unique_name
                )),
            }],
            suggested_calc: Some(calc.display_name().to_string()),
        });
    }
}

// ---------------------------------------------------------------------------
// RULE 1: near-twin dimension rejection (PRD near-twin-rejection)
// ---------------------------------------------------------------------------

/// Maximum number of trailing tokens used as the near-twin "core label" key.
/// Mirrors `mqo-mcp-server`'s `NEAR_TWIN_CORE_TOKENS` so the validator's
/// canonical agrees with `describe_model`'s `near_twins` hint.
const NEAR_TWIN_CORE_TOKENS: usize = 2;

/// Lowercase + collapse-whitespace label normalization, matching the server's
/// `normalize_label` (no bracket/punctuation stripping — labels are plain).
fn nt_normalize_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The "core label" of an attribute (trailing concept words). Replicates
/// `mqo-mcp-server::core_label`: drops a trailing "name" token so a display
/// attribute shares a bucket with its code-like sibling, then takes the last
/// `NEAR_TWIN_CORE_TOKENS` tokens. `None` for an empty label.
fn nt_core_label(label: &str) -> Option<String> {
    let norm = nt_normalize_label(label);
    let mut toks: Vec<&str> = norm.split(' ').filter(|t| !t.is_empty()).collect();
    if toks.len() > 1 && toks.last() == Some(&"name") {
        toks.pop();
    }
    if toks.len() < NEAR_TWIN_CORE_TOKENS {
        if toks.is_empty() {
            return None;
        }
        return Some(toks.join(" "));
    }
    Some(toks[toks.len() - NEAR_TWIN_CORE_TOKENS..].join(" "))
}

/// Does this label name a human-readable display attribute (trailing word
/// "name")? Replicates `mqo-mcp-server::label_is_name`.
fn nt_label_is_name(label: &str) -> bool {
    nt_normalize_label(label)
        .split(' ')
        .next_back()
        .is_some_and(|w| w == "name")
}

/// One near-twin dimension member: a (hierarchy, level-label) pair. In the
/// validator's catalog a "dimension member" the MQO can pick is a level within
/// a hierarchy; the MQO refers to it by `unique_name == hierarchy` + `level`.
struct TwinMember {
    /// The hierarchy unique_name (the MQO dimension key).
    hierarchy: String,
    /// The level label.
    label: String,
}

/// True for date-role hierarchies (sold/ship/return/inventory date dimensions,
/// week hierarchies, etc.). These are DISTINCT date roles, NOT near-twins to
/// canonicalize — excluding them prevents wrongly rejecting `Sold Calendar Year`
/// in favor of a path-incompatible `Ship Calendar Year`. Mirrors the server's
/// `is_date_role_hierarchy`; date-role correctness is owned by cross-fact binding.
fn nt_is_date_role_hierarchy(hier: &str) -> bool {
    let h = hier.to_lowercase();
    h.contains("date") || h.contains("calendar") || h.contains("time")
}

/// True when `other`'s `_`-tokens END WITH `canon`'s `_`-tokens — i.e. `other`
/// is a "decorated path" to the same base hierarchy as `canon`
/// (`store_item_product_dimension` ends with `product_dimension`). Used to
/// gate near-twin canonicalization to clean one-base-many-paths groups only.
fn nt_hier_is_suffix(canon: &str, other: &str) -> bool {
    let c: Vec<&str> = canon.split('_').filter(|t| !t.is_empty()).collect();
    let o: Vec<&str> = other.split('_').filter(|t| !t.is_empty()).collect();
    o.len() >= c.len() && o[o.len() - c.len()..] == c[..]
}

/// A near-twin dimension group: a core label shared across ≥2 hierarchies, with
/// a clear canonical member (Name-preferring, shortest-hierarchy tiebreak).
struct TwinGroup {
    core: String,
    members: Vec<TwinMember>,
    /// Index into `members` of the canonical member, when one is clear.
    canonical: Option<usize>,
}

/// Build near-twin dimension groups from the catalog hierarchies/levels.
///
/// Replicates `mqo-mcp-server::build_near_twins`: bucket dimension *levels* by
/// their core label, keep buckets spanning ≥2 distinct hierarchies, and pick a
/// canonical member preferring a "*Name*" display attribute, then the
/// lexicographically shortest hierarchy name. Deterministic.
fn build_twin_groups(catalog: &CatalogSnapshot) -> Vec<TwinGroup> {
    use std::collections::BTreeMap;
    // core -> Vec<(hierarchy, label)>
    let mut buckets: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for hier in &catalog.hierarchies {
        // Date-role hierarchies are distinct semantic roles, not near-twins —
        // never canonicalize across them (would reject Sold→Ship Calendar Year).
        if nt_is_date_role_hierarchy(&hier.hierarchy_unique_name) {
            continue;
        }
        for level in &hier.levels {
            if let Some(core) = nt_core_label(level) {
                buckets
                    .entry(core)
                    .or_default()
                    .push((hier.hierarchy_unique_name.clone(), level.clone()));
            }
        }
    }

    let mut groups = Vec::new();
    for (core, mut members) in buckets {
        let distinct_hiers: std::collections::BTreeSet<&str> =
            members.iter().map(|(h, _)| h.as_str()).collect();
        if distinct_hiers.len() < 2 {
            continue; // not a near-twin group
        }
        // Stable order for determinism.
        members.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

        // Canonical selection: Name-preferring, then shortest hierarchy name,
        // then lexicographic. Identical heuristic to the server.
        let canonical = members
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (!nt_label_is_name(&a.1))
                    .cmp(&(!nt_label_is_name(&b.1)))
                    .then_with(|| a.0.len().cmp(&b.0.len()))
                    .then_with(|| a.0.cmp(&b.0))
                    .then_with(|| a.1.cmp(&b.1))
            })
            .map(|(i, _)| i);

        // FALSE-POSITIVE TIGHTENING: only enforce a canonical when the group is a
        // clean "one base + decorated paths" structure — the canonical hierarchy
        // is a token-suffix of every other member's hierarchy (e.g.
        // `store_item_product_dimension` ⊃ `product_dimension`). When members are
        // distinct subjects/scopes (`store_dimension` vs `sold_customer_address`
        // for GMT offset; catalog-vs-generic) the non-canonical may be intended —
        // do NOT reject (the validator can't see the NL question).
        let canonical = canonical.filter(|&ci| {
            let canon_h = members[ci].0.as_str();
            members
                .iter()
                .enumerate()
                .all(|(j, (h, _))| j == ci || nt_hier_is_suffix(canon_h, h))
        });

        groups.push(TwinGroup {
            core,
            members: members
                .into_iter()
                .map(|(hierarchy, label)| TwinMember { hierarchy, label })
                .collect(),
            canonical,
        });
    }
    groups
}

/// Fact-compatibility of a single (twin) hierarchy against the MQO's measures.
///
/// Reuses the same subject-area conformance signal as [`check_cross_fact_paths`]:
/// a hierarchy maps to a catalog dimension (via `dimension_unique_name`) carrying
/// the `subject_areas` it is available in; a measure carries its `subject_area`.
/// The pair is incompatible when a measure's subject area is set and the
/// hierarchy's dimension lists subject areas that do NOT include it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compat {
    /// At least one of the MQO's measures cannot reach this hierarchy.
    Incompatible,
    /// Every measure can reach this hierarchy (no cross-fact boundary).
    Compatible,
    /// Compatibility cannot be determined (no subject-area signal, conformed
    /// dim, or the dimension/measure isn't in the catalog) — be conservative.
    Unknown,
}

/// Classify a twin hierarchy's fact-compatibility with the MQO's measures
/// (PRD-mqo-path-incompatible-decline-guard FR-1). Returns [`Compat::Unknown`]
/// whenever the subject-area signal is absent so the caller falls back to the
/// existing reroute behavior (CONSERVATIVE — never break the Brand Name reroute).
fn twin_hierarchy_compat(
    hier_unique_name: &str,
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
) -> Compat {
    let hier_norm = normalize(hier_unique_name);
    // Hierarchy → owning dimension unique_name (default to the hierarchy name,
    // matching the common case where hierarchy_unique_name == dimension name).
    let dim_unique_name = catalog
        .hierarchies
        .iter()
        .find(|h| normalize(&h.hierarchy_unique_name) == hier_norm)
        .map_or(hier_unique_name.to_string(), |h| {
            h.dimension_unique_name.clone()
        });
    let dim = catalog
        .dimensions
        .iter()
        .find(|d| normalize(&d.unique_name) == normalize(&dim_unique_name));
    let dim = match dim {
        Some(d) => d,
        None => return Compat::Unknown, // no catalog dimension → can't tell
    };
    // Conformed dimension (no subject-area restriction) → reaches every fact.
    if dim.subject_areas.is_empty() {
        return Compat::Unknown;
    }

    let mut saw_signal = false;
    for mref in &mqo.measures {
        let mnorm = normalize(&mref.unique_name);
        let measure = match catalog
            .measures
            .iter()
            .find(|m| normalize(&m.unique_name) == mnorm)
        {
            Some(m) => m,
            None => continue,
        };
        let measure_sa = match &measure.subject_area {
            Some(sa) => sa,
            None => continue, // no subject area on this measure → no signal
        };
        saw_signal = true;
        if !dim.subject_areas.contains(measure_sa) {
            return Compat::Incompatible;
        }
    }
    if saw_signal {
        Compat::Compatible
    } else {
        Compat::Unknown
    }
}

/// RULE 1: reject a non-canonical near-twin *dimension* pick.
///
/// Conservative — fires only when ALL hold (PRD FR-3/FR-4/FR-5):
///   * the picked dimension is a non-canonical member of a group with a clear
///     canonical and ≥2 hierarchies (`build_twin_groups`),
///   * dimensions only — never measures (FR-5),
///   * INTENT GUARD (FR-4): the MQO has NO other filter/dimension referencing
///     the picked member's own hierarchy. A deliberate scoping on that
///     hierarchy means the pick is intentional → no rejection.
///
/// PATH-INCOMPATIBLE DECLINE GUARD (PRD-mqo-path-incompatible-decline-guard):
/// before suggesting the canonical, check fact-compatibility of BOTH the picked
/// twin and the proposed canonical against the MQO's measures. If the picked twin
/// is INCOMPATIBLE while the canonical would be COMPATIBLE, WITHHOLD the reroute
/// (do not suggest the canonical) — the query then reaches the binder, which
/// surfaces the genuine cross-fact incompatibility (the correct decline). Any
/// other combination (both compatible, both incompatible, or undeterminable)
/// keeps the existing behavior.
fn check_near_twin_dimension(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    let groups = build_twin_groups(catalog);
    if groups.is_empty() {
        return;
    }

    for dref in &mqo.dimensions {
        // The MQO dimension key is the hierarchy; the level is the member label.
        let picked_hier = normalize(&dref.unique_name);
        let picked_level = match &dref.level {
            Some(l) => normalize(l),
            None => continue, // no level → can't identify a twin member
        };

        for group in &groups {
            let canonical_idx = match group.canonical {
                Some(i) => i,
                None => continue, // no clear canonical → don't guess (edge case)
            };
            // Find the picked member within this group.
            let picked_member_idx = group.members.iter().position(|m| {
                normalize(&m.hierarchy) == picked_hier && normalize(&m.label) == picked_level
            });
            let picked_idx = match picked_member_idx {
                Some(i) => i,
                None => continue, // this dim isn't a member of this group
            };
            if picked_idx == canonical_idx {
                continue; // already canonical → pass (AC-2)
            }

            let picked = &group.members[picked_idx];
            let canonical = &group.members[canonical_idx];

            // INTENT GUARD (FR-4 / AC-3): a deliberate filter or dimension on
            // the picked member's OWN hierarchy means the scoping is
            // intentional → never reject. The triggering `dref` itself is on
            // that hierarchy, so we look for ANOTHER reference (a filter, or a
            // second dimension) to the same hierarchy.
            let own_hier = normalize(&picked.hierarchy);
            let intent_on_own_hierarchy = mqo
                .filters
                .iter()
                .any(|f| normalize(&f.unique_name) == own_hier)
                || mqo
                    .dimensions
                    .iter()
                    .filter(|d| !std::ptr::eq(*d, dref))
                    .any(|d| normalize(&d.unique_name) == own_hier);
            if intent_on_own_hierarchy {
                continue;
            }

            // PATH-INCOMPATIBLE DECLINE GUARD: withhold the reroute when the
            // picked twin is path-incompatible with the MQO's measures but the
            // canonical would be compatible — rerouting there would fabricate a
            // compatible answer for a query that should decline (fm3-010). Any
            // other combination falls through to the normal suggestion (and an
            // Unknown on either side stays conservative — Brand Name unchanged).
            let picked_compat = twin_hierarchy_compat(&picked.hierarchy, mqo, catalog);
            let canon_compat = twin_hierarchy_compat(&canonical.hierarchy, mqo, catalog);
            if picked_compat == Compat::Incompatible && canon_compat == Compat::Compatible {
                continue;
            }

            let field_key = format!("{}.[{}]", picked.hierarchy, picked.label);
            let suggested = format!("{}.[{}]", canonical.hierarchy, canonical.label);
            rejections.push(ParamRejection {
                field: field_key,
                class: FieldClass::Dimension,
                reason: RejectReason::NonCanonicalNearTwin {
                    picked: format!("{}.[{}]", picked.hierarchy, picked.label),
                    suggested_canonical: suggested.clone(),
                    group_core_label: group.core.clone(),
                },
                suggestions: vec![Suggestion {
                    name: suggested,
                    similarity: 1.0,
                    note: Some(format!(
                        "non-canonical near-twin of core label [{}]; use the canonical \
                         [{}.{}] on hierarchy [{}]",
                        group.core, canonical.hierarchy, canonical.label, canonical.hierarchy
                    )),
                }],
                suggested_calc: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2: semi-additive guard (PRD semi-additive-guard)
// ---------------------------------------------------------------------------

/// Is this dimension/level a time/date-typed axis? Reuses the crate's
/// `references_date` date-concept token detection over the dimension name,
/// level, and role qualifier.
fn dimension_is_time(dref: &MqoDimensionRef) -> bool {
    references_date(&dref.unique_name)
        || dref.level.as_deref().map(references_date).unwrap_or(false)
        || dref
            .role_qualifier
            .as_deref()
            .map(references_date)
            .unwrap_or(false)
}

/// RULE 2: reject an EXPLICIT additive aggregation of a `semi_additive` measure
/// over a time dimension. Fires only when ALL hold (PRD FR-2/FR-4):
///   * the catalog measure has `semi_additive == Some(true)`,
///   * a time-typed dimension is in the grouping,
///   * the aggregation is an EXPLICIT additive override (sum/count/total —
///     `agg_is_explicit_additive`). A None/default agg is NOT a misuse: the
///     engine applies the measure's semi-additive function under the default,
///     so flagging it would false-positive every "balance by period" query.
///
/// No longer dormant once the served catalog carries `semi_additive` (see
/// `mqo-mcp-server` pipeline snapshot build); inert on measures lacking it.
fn check_semi_additive_sum(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    // The first time-typed dimension in the grouping (if any).
    let time_dim = mqo.dimensions.iter().find(|d| dimension_is_time(d));
    let time_dim = match time_dim {
        Some(d) => d,
        None => return, // AC-2: no time dim → no rejection
    };
    let time_label = time_dim
        .level
        .clone()
        .unwrap_or_else(|| time_dim.unique_name.clone());

    for mref in &mqo.measures {
        let mnorm = normalize(&mref.unique_name);
        let measure = match catalog
            .measures
            .iter()
            .find(|m| normalize(&m.unique_name) == mnorm)
        {
            Some(m) => m,
            None => continue,
        };
        // AC-3 / AC-4: only when explicitly semi_additive==true.
        if measure.semi_additive != Some(true) {
            continue;
        }
        // FR-4: fire ONLY on an EXPLICIT additive override (sum/count/total).
        // A None/default aggregation on a semi-additive measure resolves to the
        // model's semi-additive function (last-non-empty) at the engine, which
        // is correct — rejecting it would false-positive every "balance by
        // period" query (e.g. inventory-on-hand by month). Only an explicit
        // additive override double-counts the balance.
        if !agg_is_explicit_additive(mref.aggregation.as_deref()) {
            continue;
        }
        let suggested = measure
            .semi_additive_agg
            .clone()
            .unwrap_or_else(|| "average over period".to_string());
        let note = if measure.semi_additive_agg.is_some() {
            format!(
                "[{}] is semi-additive; summing it over [{}] double-counts a balance — use [{}]",
                mref.unique_name, time_label, suggested
            )
        } else {
            format!(
                "[{}] is semi-additive; summing it over [{}] double-counts a balance — \
                 use a period-end (last) or average-over-period aggregation",
                mref.unique_name, time_label
            )
        };
        rejections.push(ParamRejection {
            field: mref.unique_name.clone(),
            class: FieldClass::Measure,
            reason: RejectReason::SemiAdditiveSum {
                measure: mref.unique_name.clone(),
                time_dimension: time_label.clone(),
                suggested_agg: suggested.clone(),
            },
            suggestions: vec![Suggestion {
                name: suggested,
                similarity: 1.0,
                note: Some(note),
            }],
            suggested_calc: None,
        });
    }
}

// ---------------------------------------------------------------------------
// RULE 3: calc-aggregation guard (PRD calc-aggregation-guard)
// ---------------------------------------------------------------------------

/// Ratio/percentage/average name signals: a calc whose name carries one of
/// these is non-additive across groups.
const RATIO_SIGNALS: &[&str] = &["%", "pct", "rate", "average", "avg"];

/// Additive-calc name signals (`* Increase`/`* Growth`/`* Delta`): these calcs
/// ARE safe to aggregate and must never be rejected by RULE 3.
const ADDITIVE_CALC_SIGNALS: &[&str] = &["increase", "growth", "delta"];

/// Classify an `is_calc` measure as ratio (non-additive) or additive.
/// Prefers an explicit `calc_kind` catalog flag; else a name heuristic. Returns
/// `None` when there is no signal either way (conservative — do not reject).
fn classify_calc(measure: &CatalogMeasure) -> Option<CalcKind> {
    if let Some(k) = measure.calc_kind {
        return Some(k);
    }
    let display = measure.display_name();
    let norm = normalize(display);
    // Additive markers win first: a "* Increase"/"* Growth"/"* Delta" calc is
    // additive even if its name also contains an "avg"-like token.
    if ADDITIVE_CALC_SIGNALS
        .iter()
        .any(|s| norm.split_whitespace().any(|w| w == *s))
    {
        return Some(CalcKind::Additive);
    }
    // Ratio markers: `%` (raw), or whole-word pct/rate/average/avg.
    let raw_lower = display.to_lowercase();
    if raw_lower.contains('%')
        || RATIO_SIGNALS
            .iter()
            .any(|s| *s != "%" && norm.split_whitespace().any(|w| w == *s))
    {
        return Some(CalcKind::Ratio);
    }
    None
}

/// RULE 3: reject sum/avg of a ratio/percentage/average `is_calc` measure.
///
/// Fires only when ALL hold (PRD FR-2/FR-3/FR-5):
///   * the measure is a calc (`is_packaged_calc`),
///   * it classifies as `Ratio` (`classify_calc`),
///   * the aggregation is sum (additive) OR avg.
///
/// Never fires on additive calcs (`* Increase`/`* Growth`/`* Delta`),
/// non-calc measures, or calcs with no ratio signal (conservative).
fn check_calc_misaggregation(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    for mref in &mqo.measures {
        let mnorm = normalize(&mref.unique_name);
        let measure = match catalog
            .measures
            .iter()
            .find(|m| normalize(&m.unique_name) == mnorm)
        {
            Some(m) => m,
            None => continue,
        };
        if !is_packaged_calc(measure) {
            continue; // FR-5: non-calc → never rejected
        }
        match classify_calc(measure) {
            Some(CalcKind::Ratio) => {}
            // FR-3: additive calc → never rejected. No signal → conservative.
            Some(CalcKind::Additive) | None => continue,
        }
        // Reject only sum (additive/default) or avg — the mis-aggregations.
        let agg = mref.aggregation.as_deref();
        let is_sum = agg_is_additive(agg);
        let is_avg = agg_is_average(agg);
        if !is_sum && !is_avg {
            continue; // e.g. last/min/max at own grain → not a mis-aggregation
        }
        let agg_label = agg.unwrap_or("sum (default)").to_string();
        let reason = if is_avg {
            format!(
                "averaging the ratio/average calc [{}] across groups double-aggregates \
                 (average-of-averages is not the weighted average)",
                mref.unique_name
            )
        } else {
            format!(
                "summing the ratio/percentage calc [{}] across groups is meaningless \
                 (a percentage does not add)",
                mref.unique_name
            )
        };
        rejections.push(ParamRejection {
            field: mref.unique_name.clone(),
            class: FieldClass::Measure,
            reason: RejectReason::CalcMisaggregation {
                measure: mref.unique_name.clone(),
                aggregation: agg_label,
                reason: reason.clone(),
            },
            suggestions: vec![Suggestion {
                name: mref.unique_name.clone(),
                similarity: 1.0,
                note: Some(format!(
                    "{reason}; query the ratio at the requested grain directly (the semantic \
                     layer computes it correctly per group) or aggregate the additive base \
                     numerator/denominator measures"
                )),
            }],
            suggested_calc: None,
        });
    }
}

// ---------------------------------------------------------------------------
// RULE 4: filter-level check (PRD filter-level-check)
// ---------------------------------------------------------------------------

/// Infer the value type of a raw filter value string. A pure-integer string is
/// `Integer`; a `YYYY-MM-DD`-shaped string is `Date`; everything else is
/// `String`. Conservative — only the two well-formed numeric/date shapes are
/// recognized, all else stays `String`.
fn infer_value_type(v: &str) -> LevelValueType {
    let t = v.trim();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return LevelValueType::Integer;
    }
    // YYYY-MM-DD (allow YYYY/MM/DD too).
    let parts: Vec<&str> = t.split(['-', '/']).collect();
    if parts.len() == 3
        && parts[0].len() == 4
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return LevelValueType::Date;
    }
    LevelValueType::String
}

/// Look up the level-domain metadata for a (hierarchy, level) pair, if enriched.
fn find_level_meta<'a>(
    catalog: &'a CatalogSnapshot,
    hier_norm: &str,
    level_norm: &str,
) -> Option<&'a LevelDomainMeta> {
    catalog
        .hierarchies
        .iter()
        .filter(|h| normalize(&h.hierarchy_unique_name) == hier_norm)
        .flat_map(|h| h.level_meta.iter())
        .find(|m| normalize(&m.level) == level_norm)
}

/// Does the named level exist in the named hierarchy?
fn level_exists(catalog: &CatalogSnapshot, hier_norm: &str, level_norm: &str) -> bool {
    catalog
        .hierarchies
        .iter()
        .filter(|h| normalize(&h.hierarchy_unique_name) == hier_norm)
        .any(|h| h.levels.iter().any(|l| normalize(l) == level_norm))
}

/// Conservative member-domain check for a `Member` filter that names no level
/// (`mqo_spec::Filter::Member` is a hierarchy + member keys, no level). Rejects a
/// member ONLY when it is safe: there is ≥1 enumerated same-type level domain in
/// the hierarchy, the member is in NONE of them, AND there is NO same-type level
/// lacking an enumerated domain (a high-cardinality level — e.g. a store name —
/// the member could legitimately be a key of). This catches a wrong code/value on
/// a fully-enumerated dimension without false-positiving high-card member filters.
/// The broad "member silently bound to the wrong level" case (e.g. `Store
/// State="CA"` grounding to `Store City`) is the binder's responsibility (it must
/// not silently ground), not this catalog-only guard.
fn check_member_domain(
    fref: &MqoFilterRef,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    if fref.members.is_empty() {
        return;
    }
    let hier_norm = normalize(&fref.unique_name);
    let hiers: Vec<&CatalogHierarchy> = catalog
        .hierarchies
        .iter()
        .filter(|h| normalize(&h.hierarchy_unique_name) == hier_norm)
        .collect();
    if hiers.is_empty() {
        return;
    }
    let metas: Vec<&LevelDomainMeta> = hiers.iter().flat_map(|h| h.level_meta.iter()).collect();
    if metas.iter().all(|m| m.domain.is_none()) {
        return; // nothing enumerated → cannot decide
    }
    let all_levels: Vec<String> = hiers.iter().flat_map(|h| h.levels.clone()).collect();

    for member in &fref.members {
        let mt = infer_value_type(member);
        let same_type_doms: Vec<&Vec<String>> = metas
            .iter()
            .filter(|m| m.value_type == mt)
            .filter_map(|m| m.domain.as_ref())
            .collect();
        if same_type_doms.is_empty() {
            continue; // no comparable enumerated level
        }
        let in_domain = same_type_doms
            .iter()
            .any(|d| d.iter().any(|v| normalize(v) == normalize(member)));
        if in_domain {
            continue;
        }
        // SAFE GUARD: skip if any same-type level lacks an enumerated domain (a
        // high-card level the member could be a key of). A level with no meta
        // entry is treated as possibly-same-type (conservative → skip).
        let has_unenumerated_same_type = all_levels.iter().any(|lvl| {
            let lnorm = normalize(lvl);
            match metas.iter().find(|m| normalize(&m.level) == lnorm) {
                Some(m) => m.domain.is_none() && m.value_type == mt,
                None => true,
            }
        });
        if has_unenumerated_same_type {
            continue; // unsafe to reject
        }
        let suggested: Vec<String> = same_type_doms
            .iter()
            .flat_map(|d| d.iter().take(12).cloned())
            .collect();
        rejections.push(ParamRejection {
            field: format!("{} member [{}]", fref.unique_name, member),
            class: FieldClass::Filter,
            reason: RejectReason::FilterLevelMismatch {
                filter: fref.unique_name.clone(),
                target_level: String::new(),
                reason: format!(
                    "member [{}] is not in the domain of any level of hierarchy [{}]",
                    member, fref.unique_name
                ),
                suggested: suggested.join(", "),
            },
            suggestions: suggested
                .iter()
                .take(8)
                .map(|v| Suggestion {
                    name: v.clone(),
                    similarity: jaro_winkler(&normalize(member), &normalize(v)),
                    note: Some(format!("valid member of [{}]", fref.unique_name)),
                })
                .collect(),
            suggested_calc: None,
        });
    }
}

/// RULE 4: reject a filter whose value type/domain cannot match the target
/// level, or whose named level does not exist.
///
/// Conservative guardrails (PRD FR-4):
///   * Only acts on a filter that names a `level` — a bare hierarchy filter is
///     left to the binder/`Unmapped` path.
///   * When the level exists but has NO `level_meta`, the guard cannot decide
///     value-fit and does NOT reject (catalog-only emptiness ≠ filter error).
///   * A member value not in an explicit `domain` is rejected (AC-1); a value
///     with no live rows is NEVER rejected (we only check the catalog domain).
///   * A range/member bound whose type ≠ the level's `value_type` is rejected
///     (AC-2 YYYYWW/DATE on a sequential-week Integer level).
fn check_filter_level(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    for fref in &mqo.filters {
        let level = match &fref.level {
            Some(l) => l,
            None => {
                // A Member filter names no level — run the conservative
                // member-domain check against the hierarchy's enumerated levels.
                check_member_domain(fref, catalog, rejections);
                continue;
            }
        };
        let hier_norm = normalize(&fref.unique_name);
        let level_norm = normalize(level);

        // Whether the hierarchy is even known. If the hierarchy isn't in the
        // catalog the Unmapped pass already handled it; skip (don't duplicate).
        let hier_known = catalog
            .hierarchies
            .iter()
            .any(|h| normalize(&h.hierarchy_unique_name) == hier_norm);
        if !hier_known {
            continue;
        }

        // FR-3 / edge case: the named level must exist. If the hierarchy has
        // declared levels but not this one, reject (don't silently ground).
        if !level_exists(catalog, &hier_norm, &level_norm) {
            // Suggest the hierarchy's known levels.
            let known: Vec<String> = catalog
                .hierarchies
                .iter()
                .filter(|h| normalize(&h.hierarchy_unique_name) == hier_norm)
                .flat_map(|h| h.levels.clone())
                .collect();
            if known.is_empty() {
                continue; // no level info at all → can't decide, skip
            }
            rejections.push(ParamRejection {
                field: format!("{}.[{}]", fref.unique_name, level),
                class: FieldClass::Filter,
                reason: RejectReason::FilterLevelMismatch {
                    filter: fref.unique_name.clone(),
                    target_level: level.clone(),
                    reason: format!(
                        "level [{}] does not exist in hierarchy [{}]",
                        level, fref.unique_name
                    ),
                    suggested: known.join(", "),
                },
                suggestions: known
                    .iter()
                    .map(|l| Suggestion {
                        name: l.clone(),
                        similarity: jaro_winkler(&level_norm, &normalize(l)),
                        note: Some(format!("valid level in [{}]", fref.unique_name)),
                    })
                    .collect(),
                suggested_calc: None,
            });
            continue;
        }

        // The level exists. Decide value-fit ONLY if enriched metadata exists.
        let meta = match find_level_meta(catalog, &hier_norm, &level_norm) {
            Some(m) => m,
            None => continue, // FR-4: no domain/type info → never reject
        };

        // Collect the values to check (member values + range bounds).
        let mut bad_value: Option<(String, String)> = None; // (value, why)

        // Range bound type check (AC-2 / AC-4).
        for bound in [fref.range_lo.as_deref(), fref.range_hi.as_deref()]
            .into_iter()
            .flatten()
        {
            let vt = infer_value_type(bound);
            if vt != meta.value_type {
                bad_value = Some((
                    bound.to_string(),
                    format!(
                        "range bound [{}] is {:?} but level [{}] expects {:?}",
                        bound, vt, level, meta.value_type
                    ),
                ));
                break;
            }
        }

        // Member value checks: type, then explicit-domain membership (AC-1/AC-3).
        if bad_value.is_none() {
            for member in &fref.members {
                let vt = infer_value_type(member);
                if vt != meta.value_type {
                    bad_value = Some((
                        member.clone(),
                        format!(
                            "member value [{}] is {:?} but level [{}] expects {:?}",
                            member, vt, level, meta.value_type
                        ),
                    ));
                    break;
                }
                if let Some(domain) = &meta.domain {
                    // AC-3 GUARD: only reject when the domain is an explicit
                    // closed enumeration AND the value is outside it. An
                    // in-domain value with no live rows is never rejected
                    // (we never consult live data).
                    let in_domain = domain
                        .iter()
                        .any(|d| normalize(d) == normalize(member));
                    if !in_domain {
                        bad_value = Some((
                            member.clone(),
                            format!(
                                "member value [{member}] is not in the domain of level [{level}]",
                            ),
                        ));
                        break;
                    }
                }
            }
        }

        if let Some((value, why)) = bad_value {
            let suggested = meta
                .expected_key_shape
                .clone()
                .unwrap_or_else(|| format!("a {:?} value valid for level [{}]", meta.value_type, level));
            rejections.push(ParamRejection {
                field: format!("{}.[{}] = {}", fref.unique_name, level, value),
                class: FieldClass::Filter,
                reason: RejectReason::FilterLevelMismatch {
                    filter: fref.unique_name.clone(),
                    target_level: level.clone(),
                    reason: why.clone(),
                    suggested: suggested.clone(),
                },
                suggestions: vec![Suggestion {
                    name: suggested,
                    similarity: 0.0,
                    note: Some(why),
                }],
                suggested_calc: None,
            });
        }
    }
}
