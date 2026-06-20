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
    /// Optional channel scope descriptor derived from `FactBindings` channel
    /// groups (FR1/FR2, PRD-mqo-channel-scope-measure-grounding). When present,
    /// lists the fact-table column-group identifiers this measure aggregates,
    /// e.g. `["store_sales"]` for `Store Quantity Sold` or
    /// `["store_sales","catalog_sales","web_sales"]` for `Total Quantity Sold`.
    ///
    /// Used by the `ChannelScopeMismatch` guard (RULE 7) to detect the case where
    /// an all-channel measure is bound when a channel-scoped sibling exists and
    /// the request names a single channel.
    ///
    /// `None` (absent) → no channel-scope binding known; guard stays silent (FR4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_scope: Option<Vec<String>>,
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
    /// A decimal/numeric key stored in engine-comparable form (e.g. "-5.00", "3.14").
    /// Used for levels whose LEVEL_DBTYPE is NUMERIC/DECIMAL or float (OLE DB types
    /// 4/5/6/131). The domain stores the member KEY (engine-comparable) not the caption.
    Decimal,
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
    /// RULE 5: `dataset_aggregate` asked to sum/aggregate a column that
    /// resolves unambiguously to a dimension level (`kind=level`) and to no
    /// measure. Aggregating a dimension attribute produces a silent wrong
    /// number (e.g. `sum_Store Number of Employees`). The correct action is
    /// to project or select the attribute, not aggregate it.
    AttributeAggregation {
        /// The column name passed as the `measure` argument.
        column: String,
        /// Human-readable note: why this is wrong and what to do instead.
        reason: String,
    },
    /// RULE 6: the MQO includes a rank / row-number / ordinal column (e.g.
    /// `Rank`, `Ranking`, `Row Number`, `RowNum`, `Ordinal`, `Position`)
    /// that does not ground to any catalog measure or dimension level.
    ///
    /// "Top N" ordering is already expressed by the ORDER BY + LIMIT in the
    /// query; materialising an additional rank column is a spurious artifact
    /// the model does not have and the question never requested.  The agent
    /// should drop the column and rely on ORDER BY + LIMIT alone.
    SyntheticRankColumn {
        /// The ungrounded rank/ordinal column name.
        column: String,
    },
    /// RULE 7 (PRD-mqo-channel-scope-measure-grounding): the MQO binds an
    /// all-channel total measure when a channel-scoped sibling exists for the
    /// named channel. This silently inflates values by summing all channels
    /// instead of the single channel the request names.
    ChannelScopeMismatch {
        /// The all-channel measure that was bound (the wrong pick).
        measure: String,
        /// The channel that the context names (e.g. `"store_sales"`).
        named_channel: String,
        /// The channel-scoped sibling measure the caller should use instead.
        suggested_measure: String,
    },
    /// RULE 8 (PRD-mqo-projected-level-label-fidelity): the MQO projects a
    /// dimension level whose label is a suffix/substring of exactly one canonical
    /// catalog level label. The agent dropped the qualifying prefix
    /// (e.g. "Floor Space" instead of "Store Floor Space"), causing the result
    /// column to carry the truncated label and fail column-set scoring.
    NonCanonicalLevelLabel {
        /// The truncated label the agent supplied.
        supplied: String,
        /// The exact canonical label from the catalog (use this instead).
        canonical: String,
    },
    /// RULE 10 (PRD-mqo-validator-ambiguous-level-dimension-resolution): same as
    /// RULE 8 but fires when the suffix-match is ambiguous globally (≥2 candidates)
    /// yet resolves to exactly one candidate within the dimension the ref names.
    AmbiguousLevelResolvedByDimension {
        /// The truncated label the agent supplied.
        supplied: String,
        /// The exact canonical label from the catalog (use this instead).
        canonical: String,
        /// The dimension whose hierarchy contained the unique resolution.
        dimension: String,
    },
    /// RULE 11 (PRD-mqo-validator-fuzzy-near-miss-level-guard): fires when a level
    /// label is not exact and has no suffix match, but fuzzy-matches
    /// (Jaro-Winkler ≥ NEAR_MISS_JW_THRESHOLD) exactly one level in the
    /// referenced dimension (e.g. "Warehouse Square Footage" → "Warehouse Square Feet").
    NearMissLevelLabel {
        /// The near-miss label the agent supplied.
        supplied: String,
        /// The exact canonical label from the catalog (use this instead).
        canonical: String,
        /// The Jaro-Winkler similarity score (for operator audit).
        similarity: f64,
    },
    /// RULE 12 (PRD-mqo-grounding-enforcement-dedup): a catalog entity appears in
    /// the wrong MQO slot — a `kind=measure` name placed in the `dimensions` list,
    /// or a `kind=level/hierarchy` name placed in the `measures` list.
    ///
    /// Conservative: only fires when the name resolves *unambiguously* to the
    /// wrong kind (i.e. matches the wrong catalog partition and does NOT also
    /// match the correct partition). Ambiguous names and unresolvable names
    /// defer to the binder.
    RoleConfusion {
        /// The entity name that was placed in the wrong slot.
        entity: String,
        /// The catalog kind of that entity (`"measure"` or `"level"`).
        actual_kind: String,
        /// The correct MQO slot it should appear in (`"measures"` or `"dimensions"`).
        correct_slot: String,
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

    // RULE 6 (PRD synthetic-rank-guard): reject an ungrounded rank / row-number
    // / ordinal column injected by the agent beyond the grounded select.
    check_synthetic_rank_column(mqo, catalog, &mut rejections);

    // RULE 7 (PRD-mqo-channel-scope-measure-grounding): reject an all-channel
    // measure pick when a single-channel sibling exists for the named channel.
    // Guard stays silent when no sibling exists (FR4) — additive, never
    // replaces prior rejections.
    check_channel_scope_mismatch(mqo, catalog, &mut rejections);

    // RULE 8 (PRD-mqo-projected-level-label-fidelity): reject a projected level
    // whose label is a suffix of exactly one canonical catalog level label.
    check_non_canonical_level_label(mqo, catalog, &mut rejections);

    // RULE 10 (PRD-mqo-validator-ambiguous-level-dimension-resolution): when the
    // suffix is ambiguous globally (≥2 candidates) but exactly one candidate lives
    // in the referenced dimension, emit a corrective suggestion.  Only fires where
    // RULE 8 declined (suffix ambiguous or unresolvable).
    check_ambiguous_level_by_dimension(mqo, catalog, &mut rejections);

    // RULE 11 (PRD-mqo-validator-fuzzy-near-miss-level-guard): last-resort fuzzy
    // guard — fires only when RULE 8 and RULE 10 both declined and a fuzzy-only
    // near-miss of exactly one dimension-local level is found.
    check_near_miss_level_label(mqo, catalog, &mut rejections);

    // RULE 12 (PRD-mqo-grounding-enforcement-dedup): role-confusion guard — a
    // catalog measure used in the dimensions slot, or a catalog level used in
    // the measures slot. Catalog-driven, no graph required, always-on.
    check_role_confusion(mqo, catalog, &mut rejections);

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
/// `Integer`; a `YYYY-MM-DD`-shaped string is `Date`; a number with a decimal
/// point (e.g. `-5.00`, `3.14`) is `Decimal`; everything else is `String`.
/// Conservative — only the well-formed numeric/date shapes are recognized.
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
    // Decimal: optional leading minus, digits, dot, digits (e.g. "-5.00", "3.14").
    // Keyed on the decimal point — distinguishes from Integer (no dot) and String.
    let body = t.strip_prefix('-').unwrap_or(t);
    if body.contains('.') {
        let mut parts_dec = body.splitn(2, '.');
        let int_part = parts_dec.next().unwrap_or("");
        let frac_part = parts_dec.next().unwrap_or("");
        if !int_part.is_empty()
            && int_part.bytes().all(|b| b.is_ascii_digit())
            && !frac_part.is_empty()
            && frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return LevelValueType::Decimal;
        }
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

// ---------------------------------------------------------------------------
// RULE 5: attribute-aggregation guard (PRD-mqo-validator-attribute-aggregation-guard)
// ---------------------------------------------------------------------------

/// Check whether a `dataset_aggregate` call aggregates a column that is a
/// **dimension level** (`kind=level`) rather than a measure.
///
/// # Conservative predicate (FR-2 / FR-3 — PRD-mqo-project-not-count-grounding)
///
/// Fires only when ALL hold:
///   * `group_by` is non-empty (per-entity-attribute shape — FR-4).
///   * The `measure` column resolves unambiguously to a `kind=level` entry in
///     the catalog hierarchies AND matches NO `kind=measure` entry.
///   * Resolution is case-insensitive label match (same `normalize()` used
///     everywhere in this crate).
///
/// Covers ALL aggregation types including `sum`, `avg`, `count`, and
/// `count_distinct` — a numeric attribute level (e.g. "Store Number of
/// Employees") must never be counted or summed; the correct shape is a
/// measureless projection of the attribute.  A genuine member-count measure
/// (e.g. `total_product_count`, `Catalog Customer Count`) is a `kind=measure`
/// and is therefore NOT rejected by this guard (FR-4 guardrail).
///
/// Does NOT fire (fail-open) when:
///   * `group_by` is empty (could be a global aggregate the user asked for).
///   * The column is found among the catalog measures (even if it is ALSO a
///     level label — ambiguous ⇒ conservative).
///   * The column matches no catalog entity at all (unknown column ⇒ let
///     the handle-op kernel surface the error).
///   * The catalog's hierarchy levels are empty (no `kind` signal).
///
/// Returns `Some(ParamRejection)` on a confirmed attribute-aggregation
/// attempt; `None` to proceed (fail-open).
///
/// # Example
///
/// ```
/// use mqo_param_validator::{check_dataset_aggregate_attribute, CatalogSnapshot, CatalogHierarchy, CatalogMeasure};
/// let catalog = CatalogSnapshot {
///     measures: vec![CatalogMeasure { unique_name: "Store Sales".into(), label: Some("Store Sales".into()), ..Default::default() }],
///     hierarchies: vec![CatalogHierarchy {
///         dimension_unique_name: "Store".into(),
///         hierarchy_unique_name: "Store".into(),
///         levels: vec!["Store Name".into(), "Store Number of Employees".into()],
///         level_meta: vec![],
///     }],
///     ..Default::default()
/// };
/// let r = check_dataset_aggregate_attribute("Store Number of Employees", &["Store Name"], &catalog);
/// assert!(r.is_some(), "should reject attribute-aggregation");
/// let r2 = check_dataset_aggregate_attribute("Store Sales", &["Store Name"], &catalog);
/// assert!(r2.is_none(), "real measure should not be rejected");
/// ```
pub fn check_dataset_aggregate_attribute(
    measure_col: &str,
    group_by: &[&str],
    catalog: &CatalogSnapshot,
) -> Option<ParamRejection> {
    // FR-4: only fire on the per-entity-attribute shape (non-empty group_by).
    if group_by.is_empty() {
        return None;
    }

    let col_norm = normalize(measure_col);

    // FR-2 guard: if the column matches any catalog measure, fail-open.
    // Check both unique_name and label (callers may use either).
    let is_measure = catalog.measures.iter().any(|m| {
        normalize(&m.unique_name) == col_norm
            || m.label.as_deref().map(|l| normalize(l) == col_norm).unwrap_or(false)
    });
    if is_measure {
        return None;
    }

    // Check if the column matches any level in any hierarchy.
    let is_level = catalog.hierarchies.iter().any(|h| {
        h.levels.iter().any(|l| normalize(l) == col_norm)
    });

    if !is_level {
        // Not found in catalog at all → fail-open (FR-2 / AC-6).
        return None;
    }

    // Column unambiguously resolves to a dimension level with no measure match.
    // Covers sum, avg, count, count_distinct — all are wrong for a stored per-entity
    // attribute.  The correct shape is a measureless projection (projection:true,
    // measures:[], dimensions:[entity, numeric-attribute]).
    let reason = format!(
        "column [{measure_col}] is a dimension attribute (not an additive measure); \
         aggregating it (sum/avg/count/count_distinct) produces a meaningless value. \
         For a numeric attribute level, the per-entity value is already stored — \
         project this attribute instead: projection:true, measures:[], \
         dimensions:[entity-level, {measure_col}]."
    );

    Some(ParamRejection::new(
        measure_col.to_string(),
        FieldClass::HierarchyLevel,
        RejectReason::AttributeAggregation {
            column: measure_col.to_string(),
            reason: reason.clone(),
        },
        vec![Suggestion {
            name: "use projection / direct column selection".to_string(),
            similarity: 1.0,
            note: Some(format!(
                "[{measure_col}] is a dimension level — project or select this attribute \
                 instead of aggregating it"
            )),
        }],
    ))
}

// ---------------------------------------------------------------------------
// RULE 6: synthetic rank / row-number guard (PRD-mqo-validator-synthetic-rank-guard)
// ---------------------------------------------------------------------------

/// Rank/ordinal label patterns that identify a synthetic rank column.
///
/// A column must BOTH match one of these (case/whitespace-insensitive, via
/// `normalize()`) AND fail to ground to any catalog measure or dimension level
/// to be rejected (FR4: grounding success is the gate — a legitimately-modeled
/// "Rank" measure or "Net Profit Tier" level is never rejected).
///
/// Exclusions (OQ2): bare `#` and `No.` are intentionally omitted because
/// they commonly appear in store/item identifiers ("Store #", "Item No.")
/// whose grounding can be ambiguous.  Single-character or abbreviation forms
/// require the full grounding-failure signal, not just the name alone.
const RANK_LABEL_PATTERNS: &[&str] = &[
    "rank",
    "ranking",
    "row number",
    "rownum",
    "rownumber",
    "row no",
    "ordinal",
    "position",
    "row rank",
    "row order",
];

/// Returns `true` when the normalized column label matches a rank/ordinal shape.
fn is_rank_shaped(label: &str) -> bool {
    let n = normalize(label);
    RANK_LABEL_PATTERNS.iter().any(|p| n == *p)
}

/// RULE 6 — reject columns in the MQO whose label matches a rank/row-number/
/// ordinal shape AND that do not ground to any catalog object (measure or level).
///
/// Fires on both the `measures` list (an agent that stuffs a `Rank` column into
/// the measure slot) and the `dimensions` list (an agent that stuffs it into a
/// dimension slot).  In both cases the column is ungrounded AND rank-shaped, so
/// the combined gate rejects it with a typed `SyntheticRankColumn` rejection.
///
/// The function is accumulative (NFR1): it appends to `rejections`, never
/// replaces them.  It is called after all other rules so the accumulation
/// is complete before return.
fn check_synthetic_rank_column(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    // Pre-build normalized catalog sets for fast O(n) grounding checks.
    let measure_norms: Vec<String> = catalog
        .measures
        .iter()
        .flat_map(|m| {
            let mut names = vec![normalize(&m.unique_name)];
            if let Some(ref lbl) = m.label {
                names.push(normalize(lbl));
            }
            names
        })
        .collect();

    let level_norms: Vec<String> = catalog
        .hierarchies
        .iter()
        .flat_map(|h| h.levels.iter().map(|l| normalize(l)))
        .collect();

    // Grounded by full unique_name OR by the bare bracket label (handles
    // "some_hierarchy.[Rank]" where catalog has a legitimate "Rank" level).
    // v0.9.4: bracket-label level grounding is DIMENSION-SCOPED to prevent
    // a foreign dimension's `Rank` level from grounding a bracket ref in
    // an unrelated dimension (the cross-dimension grounding leak, C9).
    let is_grounded_col = |col: &str| -> bool {
        let n = normalize(col);
        if measure_norms.contains(&n) || level_norms.contains(&n) {
            return true;
        }
        // Bracket-form grounding: extract prefix + bracket label.
        if let Some((prefix, bracket)) = extract_unique_name_bracket(col) {
            let bn = normalize(bracket);
            // Measure grounding stays catalog-global (FR4/NG1).
            if measure_norms.contains(&bn) {
                return true;
            }
            // Level grounding: scope to the referenced dimension when prefix resolves.
            match dimension_levels_for_prefix(catalog, prefix) {
                Some(dim_levels) => {
                    // Prefix resolved → grounded only if THIS dimension has a matching level.
                    return dim_levels.contains(&bn);
                }
                None => {
                    // Prefix unresolvable → conservative flat-union fallback (FR5).
                    return level_norms.contains(&bn);
                }
            }
        }
        false
    };

    // Check measure slots.
    for mref in &mqo.measures {
        let col = &mref.unique_name;
        // Effective label: the bracket portion when present (that's what becomes
        // the DAX alias), otherwise the full unique_name.
        let bracket_label = extract_unique_name_bracket(col).map(|(_, l)| l.to_string());
        let label = bracket_label.as_deref().unwrap_or(col.as_str());
        if is_rank_shaped(label) && !is_grounded_col(col) {
            rejections.push(ParamRejection::new(
                col.clone(),
                FieldClass::Measure,
                RejectReason::SyntheticRankColumn {
                    column: col.clone(),
                },
                vec![Suggestion {
                    name: "drop the rank column".to_string(),
                    similarity: 1.0,
                    note: Some(format!(
                        "column [{col}] is a synthetic rank/row-number artifact: the model \
                         does not define this column and the question never requested an ordinal. \
                         \"Top N\" ordering is already expressed by ORDER BY + LIMIT — \
                         drop [{col}] from the output and rely on the ORDER BY + LIMIT alone."
                    )),
                }],
            ));
        }
    }

    // Check dimension slots.
    for dref in &mqo.dimensions {
        let col = &dref.unique_name;
        let bracket_label_dim = extract_unique_name_bracket(col).map(|(_, l)| l.to_string());
        let label = bracket_label_dim.as_deref().unwrap_or(col.as_str());
        // Only fire when the dimension itself is ungrounded.
        let dim_norm = normalize(col);
        let dim_grounded = catalog
            .dimensions
            .iter()
            .any(|d| normalize(&d.unique_name) == dim_norm)
            || is_grounded_col(col);
        if is_rank_shaped(label) && !dim_grounded {
            rejections.push(ParamRejection::new(
                col.clone(),
                FieldClass::Dimension,
                RejectReason::SyntheticRankColumn {
                    column: col.clone(),
                },
                vec![Suggestion {
                    name: "drop the rank column".to_string(),
                    similarity: 1.0,
                    note: Some(format!(
                        "column [{col}] is a synthetic rank/row-number artifact: the model \
                         does not define this column and the question never requested an ordinal. \
                         \"Top N\" ordering is already expressed by ORDER BY + LIMIT — \
                         drop [{col}] from the output and rely on the ORDER BY + LIMIT alone."
                    )),
                }],
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 7: channel-scope mismatch guard
// (PRD-mqo-channel-scope-measure-grounding, FR3/FR4/FR5)
// ---------------------------------------------------------------------------

/// Qualifier / channel-scope tokens stripped to compute the family stem for
/// channel-sibling matching.  Mirrors the tokens used by the
/// `mqo-mcp-server` measure-twin grouper so both surfaces agree on what
/// constitutes a channel qualifier vs. a concept word.
const CHANNEL_QUALIFIER_TOKENS: &[&str] = &[
    "web", "store", "catalog", "total", "and", "incl", "inc", "tax", "ship",
    "amount", "average", "avg",
];

/// Strip channel/qualifier tokens from `label` to produce the family stem —
/// the concept words that all channel variants of a measure share.
/// Returns `None` when nothing concept-bearing remains.
fn channel_family_stem(label: &str) -> Option<String> {
    let stem: Vec<String> = label
        .split_whitespace()
        .filter(|t| !CHANNEL_QUALIFIER_TOKENS.contains(&t.to_lowercase().as_str()))
        .map(|t| t.to_lowercase())
        .collect();
    if stem.is_empty() { None } else { Some(stem.join(" ")) }
}

/// RULE 7 — flag an all-channel measure bound when a channel-scoped sibling
/// exists.
///
/// Fires when:
///   1. A bound measure has `channel_scope` with **more than one** channel group
///      (it is an all-channel total, not single-channel).
///   2. The catalog contains another measure whose `channel_scope` has exactly
///      one channel group that is a subset of the bound measure's channels AND
///      that sibling shares the same family stem (same base concept, different
///      channel qualifier).
///
/// Does **not** fire when:
///   - The bound measure's `channel_scope` is absent or empty (no binding known).
///   - The bound measure has exactly one channel group (already channel-scoped).
///   - No single-channel sibling with a matching stem exists (FR4: nothing
///     better to suggest — the all-channel measure is the only option).
///
/// The `named_channel` in the rejection is the single-channel group of the
/// suggested sibling (so the agent knows which channel the sibling covers).
/// The `suggested_measure` names the sibling (FR5).
fn check_channel_scope_mismatch(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    for mref in &mqo.measures {
        // Look up this measure in the catalog.
        let bound_norm = normalize(&mref.unique_name);
        let Some(bound_cat) = catalog
            .measures
            .iter()
            .find(|m| {
                normalize(&m.unique_name) == bound_norm
                    || m.label.as_deref().map(normalize).as_deref() == Some(&bound_norm)
            })
        else {
            // Unmapped — Unmapped rule already fires; don't double-reject here.
            continue;
        };

        // Only act when channel_scope is known and has >1 channel (all-channel).
        let Some(ref scope) = bound_cat.channel_scope else {
            continue;
        };
        if scope.len() <= 1 {
            // Already single-channel scoped — no mismatch possible.
            continue;
        }

        // Compute the family stem of the bound measure to find channel siblings.
        let bound_display = bound_cat.display_name();
        let Some(bound_stem) = channel_family_stem(bound_display) else {
            continue;
        };

        // Look for a sibling: a different catalog measure whose channel_scope
        // is exactly 1 channel that is a member of the bound measure's scope
        // AND whose family stem matches.
        let scope_set: std::collections::BTreeSet<&str> =
            scope.iter().map(String::as_str).collect();

        let mut best_sibling: Option<&CatalogMeasure> = None;
        for candidate in &catalog.measures {
            // Skip the same measure.
            if normalize(&candidate.unique_name) == bound_norm {
                continue;
            }
            let Some(ref cand_scope) = candidate.channel_scope else {
                continue;
            };
            // Sibling must be single-channel and that channel must be in the
            // bound measure's multi-channel set.
            if cand_scope.len() != 1 {
                continue;
            }
            let cand_channel = &cand_scope[0];
            if !scope_set.contains(cand_channel.as_str()) {
                continue;
            }
            // Stem must match — same base concept, different channel qualifier.
            let cand_display = candidate.display_name();
            let Some(cand_stem) = channel_family_stem(cand_display) else {
                continue;
            };
            if cand_stem != bound_stem {
                continue;
            }
            // Found a valid sibling; prefer the first one (BTreeMap order is
            // deterministic by unique_name — NFR2).
            if best_sibling.is_none() {
                best_sibling = Some(candidate);
            }
        }

        if let Some(sibling) = best_sibling {
            let named_channel = sibling.channel_scope.as_ref()
                .and_then(|s| s.first())
                .cloned()
                .unwrap_or_default();
            let suggested_name = sibling.display_name().to_string();
            rejections.push(ParamRejection::new(
                mref.unique_name.clone(),
                FieldClass::Measure,
                RejectReason::ChannelScopeMismatch {
                    measure: bound_display.to_string(),
                    named_channel: named_channel.clone(),
                    suggested_measure: suggested_name.clone(),
                },
                vec![Suggestion {
                    name: suggested_name.clone(),
                    similarity: 0.9,
                    note: Some(format!(
                        "[{bound_display}] aggregates all channels \
                         ({}); use [{suggested_name}] which is scoped to \
                         [{named_channel}] only.",
                        scope.join(", ")
                    )),
                }],
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 8: non-canonical level label (PRD-mqo-projected-level-label-fidelity)
// ---------------------------------------------------------------------------

/// Collect all level labels from every hierarchy in the catalog (flat, deduped).
fn all_catalog_level_labels(catalog: &CatalogSnapshot) -> Vec<String> {
    let mut labels: Vec<String> = catalog
        .hierarchies
        .iter()
        .flat_map(|h| h.levels.iter().cloned())
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// Resolve a bracket `unique_name` prefix to the normalized level labels of the dimension
/// it names.  The prefix is everything before the `[` in a unique_name like
/// `"store_sales.[Rank]"` (so prefix = `"store_sales."`).
///
/// Returns `Some(vec_of_normalized_level_labels)` when exactly one dimension matches, or
/// `None` when the prefix is unresolvable or matches ≥2 distinct dimensions (ambiguous
/// → caller uses conservative flat-union fallback).
///
/// Matching is tolerant: trailing dot/bracket is stripped, then `normalize` is applied.
/// The normalized form is compared against each hierarchy's `dimension_unique_name` and
/// `hierarchy_unique_name`.  Shared helper for RULE 6, RULE 10, and RULE 11.
fn dimension_levels_for_prefix(catalog: &CatalogSnapshot, prefix: &str) -> Option<Vec<String>> {
    let prefix_stripped = prefix.trim_end_matches(['.', '[']);
    let prefix_norm = normalize(prefix_stripped);
    if prefix_norm.is_empty() {
        return None;
    }
    // Find the unique dimension the prefix names.
    let mut matched_dim: Option<&str> = None;
    let mut ambiguous = false;
    for h in &catalog.hierarchies {
        let dim_norm = normalize(&h.dimension_unique_name);
        let hier_norm = normalize(&h.hierarchy_unique_name);
        if dim_norm == prefix_norm || hier_norm == prefix_norm {
            match matched_dim {
                None => matched_dim = Some(&h.dimension_unique_name),
                Some(existing) if existing == h.dimension_unique_name.as_str() => {} // same dim
                Some(_) => { ambiguous = true; break; } // ≥2 distinct dimensions
            }
        }
    }
    if ambiguous || matched_dim.is_none() {
        return None;
    }
    let dim = matched_dim.unwrap();
    Some(
        catalog
            .hierarchies
            .iter()
            .filter(|h| h.dimension_unique_name == dim)
            .flat_map(|h| h.levels.iter().map(|l| normalize(l)))
            .collect(),
    )
}

/// Return all suffix-match candidates for `candidate` from the catalog, each paired with
/// its owning `dimension_unique_name`.  A suffix match means the catalog level label ends
/// with `candidate` (case-insensitive, word-boundary: preceding char is a space).  Exact
/// matches are excluded (no correction needed).
///
/// RULE 8 is a thin wrapper (`len == 1` → unique); RULE 10 uses the full vec.
fn suffix_candidates_with_dim<'a>(
    candidate: &str,
    catalog: &'a CatalogSnapshot,
) -> Vec<(&'a String, &'a str)> {
    let candidate_lower = candidate.to_lowercase();
    let mut result: Vec<(&'a String, &'a str)> = Vec::new();
    for h in &catalog.hierarchies {
        for l in &h.levels {
            let l_lower = l.to_lowercase();
            // Exclude exact matches.
            if l_lower == candidate_lower {
                continue;
            }
            if l_lower.len() > candidate_lower.len()
                && l_lower.ends_with(&candidate_lower)
                && l_lower
                    .as_bytes()
                    .get(l_lower.len() - candidate_lower.len() - 1)
                    .map_or(false, |&b| b == b' ')
            {
                result.push((l, h.dimension_unique_name.as_str()));
            }
        }
    }
    result
}

/// RULE 8: fire when a projected level's label is NOT an exact catalog entry but IS a
/// case-insensitive suffix (word-boundary) of exactly one canonical level label.
/// The agent dropped a qualifying prefix ("Store "); RULE 8 names the canonical form.
///
/// Fires on `dref.level` (bare label) when set. Does NOT fire when:
/// - the label IS an exact catalog entry (no correction needed)
/// - zero catalog levels end with the supplied label (Unmapped handles those)
/// - ≥2 catalog levels end with the supplied label (ambiguous, no guess)
/// Extract the final `[Label]` bracket content from a unique_name such as
/// `"store_dimension.[Floor Space]"` → `Some(("store_dimension.", "Floor Space"))`.
/// Returns `None` when no `[...]` is present (bare label or prefix-only).
fn extract_unique_name_bracket(unique_name: &str) -> Option<(&str, &str)> {
    let open = unique_name.rfind('[')?;
    let close = unique_name.rfind(']')?;
    if close <= open {
        return None;
    }
    let _prefix_with_bracket = &unique_name[..=open]; // includes the `[`
    let label = &unique_name[open + 1..close];
    if label.is_empty() {
        return None;
    }
    // Return prefix UP TO (not including) the `[` for reconstruction.
    Some((&unique_name[..open], label))
}

/// Test whether `candidate` is a case-insensitive, word-boundary suffix of exactly one
/// label in `all_labels`. Returns the canonical label if so, `None` otherwise.
fn unique_suffix_match<'a>(candidate: &str, all_labels: &'a [String]) -> Option<&'a String> {
    let candidate_lower = candidate.to_lowercase();

    // Skip if already an exact match — no correction needed.
    if all_labels.iter().any(|l| l.to_lowercase() == candidate_lower) {
        return None;
    }

    let matches: Vec<&String> = all_labels
        .iter()
        .filter(|l| {
            let l_lower = l.to_lowercase();
            l_lower.len() > candidate_lower.len()
                && l_lower.ends_with(&candidate_lower)
                // Word-boundary: the character before the suffix must be a space.
                && l_lower
                    .as_bytes()
                    .get(l_lower.len() - candidate_lower.len() - 1)
                    .map_or(false, |&b| b == b' ')
        })
        .collect();

    if matches.len() == 1 { Some(matches[0]) } else { None }
}

fn check_non_canonical_level_label(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    let all_labels = all_catalog_level_labels(catalog);

    for dref in &mqo.dimensions {
        // Track canonicals already emitted for this dref to de-dup FR7.
        let mut emitted_canonicals: Vec<String> = Vec::new();

        // --- Path A: dref.level (original RULE 8 behavior, FR8 unchanged) ---
        if let Some(ref supplied) = dref.level {
            if let Some(canonical) = unique_suffix_match(supplied, &all_labels) {
                emitted_canonicals.push(canonical.clone());
                rejections.push(ParamRejection::new(
                    supplied.clone(),
                    FieldClass::HierarchyLevel,
                    RejectReason::NonCanonicalLevelLabel {
                        supplied: supplied.clone(),
                        canonical: canonical.clone(),
                    },
                    vec![Suggestion {
                        name: canonical.clone(),
                        similarity: 1.0,
                        note: Some("use the full canonical label (include the qualifying prefix)".to_string()),
                    }],
                ));
            }
        }

        // --- Path B: unique_name bracket portion (FR1–FR3, new in v0.9.2) ---
        if let Some((prefix, bracket_label)) = extract_unique_name_bracket(&dref.unique_name) {
            if let Some(canonical) = unique_suffix_match(bracket_label, &all_labels) {
                // De-dup: if path A already emitted for this same canonical, skip (FR7).
                if emitted_canonicals.iter().any(|c| c == canonical) {
                    continue;
                }
                // Build the corrected unique_name: preserve the prefix, swap the bracket.
                let corrected_unique_name = format!("{prefix}[{canonical}]");
                rejections.push(ParamRejection::new(
                    bracket_label.to_string(),
                    FieldClass::HierarchyLevel,
                    RejectReason::NonCanonicalLevelLabel {
                        supplied: bracket_label.to_string(),
                        canonical: canonical.clone(),
                    },
                    vec![Suggestion {
                        name: corrected_unique_name,
                        similarity: 1.0,
                        note: Some("use the full canonical label in the unique_name bracket".to_string()),
                    }],
                ));
            }
        }
        // 0 suffix matches → Unmapped owns it. ≥2 → ambiguous; do not guess.
    }
}

/// RULE 10 — when RULE 8 declines because the suffix is ambiguous (≥2 global
/// candidates), use the dimension the ref names to break the tie.  Fires iff
/// exactly one of the ≥2 candidates lives in that dimension.
///
/// Never fires when RULE 8 already emitted for the same field; never fires when
/// the dimension cannot be resolved or when ≥2 candidates remain in the dimension.
fn check_ambiguous_level_by_dimension(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    // Build the set of fields already corrected by RULE 8 for fast de-dup.
    let rule8_fields: Vec<String> = rejections
        .iter()
        .filter(|r| matches!(r.reason, RejectReason::NonCanonicalLevelLabel { .. }))
        .map(|r| r.field.clone())
        .collect();

    for dref in &mqo.dimensions {
        // --- Path A: dref.level ---
        if let Some(ref supplied) = dref.level {
            if rule8_fields.contains(supplied) {
                continue; // RULE 8 already handled this field
            }
            let candidates = suffix_candidates_with_dim(supplied, catalog);
            if candidates.len() >= 2 {
                // Resolve by dimension.
                let prefix = extract_unique_name_bracket(&dref.unique_name)
                    .map(|(p, _)| p)
                    .unwrap_or("");
                if let Some(dim_levels_for_prefix) = dimension_levels_for_prefix(catalog, prefix)
                    .or_else(|| {
                        // Try the unique_name itself as dimension prefix.
                        dimension_levels_for_prefix(catalog, &dref.unique_name)
                    })
                {
                    // Find which candidates live in this dimension (by dim name).
                    let dim_match: Vec<&String> = candidates
                        .iter()
                        .filter(|(lbl, _dim)| {
                            let ln = normalize(lbl.as_str());
                            dim_levels_for_prefix.contains(&ln)
                        })
                        .map(|(lbl, _)| *lbl)
                        .collect();
                    if dim_match.len() == 1 {
                        let canonical = dim_match[0].clone();
                        let dim_name = candidates
                            .iter()
                            .find(|(lbl, _)| lbl.to_lowercase() == canonical.to_lowercase())
                            .map(|(_, d)| d.to_string())
                            .unwrap_or_default();
                        rejections.push(ParamRejection::new(
                            supplied.clone(),
                            FieldClass::HierarchyLevel,
                            RejectReason::AmbiguousLevelResolvedByDimension {
                                supplied: supplied.clone(),
                                canonical: canonical.clone(),
                                dimension: dim_name,
                            },
                            vec![Suggestion {
                                name: canonical.clone(),
                                similarity: 1.0,
                                note: Some(
                                    "use the full canonical label — disambiguation used the \
                                     referenced dimension".to_string(),
                                ),
                            }],
                        ));
                    }
                }
            }
        }

        // --- Path B: unique_name bracket portion ---
        if let Some((prefix, bracket_label)) = extract_unique_name_bracket(&dref.unique_name) {
            if rule8_fields.contains(&bracket_label.to_string()) {
                continue;
            }
            let candidates = suffix_candidates_with_dim(bracket_label, catalog);
            if candidates.len() >= 2 {
                if let Some(dim_lvls) = dimension_levels_for_prefix(catalog, prefix) {
                    let dim_match: Vec<&String> = candidates
                        .iter()
                        .filter(|(lbl, _)| {
                            dim_lvls.contains(&normalize(lbl.as_str()))
                        })
                        .map(|(lbl, _)| *lbl)
                        .collect();
                    if dim_match.len() == 1 {
                        let canonical = dim_match[0].clone();
                        let dim_name = candidates
                            .iter()
                            .find(|(lbl, _)| lbl.to_lowercase() == canonical.to_lowercase())
                            .map(|(_, d)| d.to_string())
                            .unwrap_or_default();
                        let corrected_unique_name = format!("{prefix}[{canonical}]");
                        rejections.push(ParamRejection::new(
                            bracket_label.to_string(),
                            FieldClass::HierarchyLevel,
                            RejectReason::AmbiguousLevelResolvedByDimension {
                                supplied: bracket_label.to_string(),
                                canonical: canonical.clone(),
                                dimension: dim_name,
                            },
                            vec![Suggestion {
                                name: corrected_unique_name,
                                similarity: 1.0,
                                note: Some(
                                    "use the full canonical label in the unique_name bracket \
                                     (dimension disambiguation applied)".to_string(),
                                ),
                            }],
                        ));
                    }
                }
            }
        }
    }
}

/// Similarity threshold for RULE 11.  Set against a held-out level set (OQ-V12);
/// NOT tuned to make a specific eval case pass.  Jaro-Winkler ≥ threshold is the sole
/// gate (the optional Levenshtein ceiling was removed: Lev("footage","feet")=5 exceeds
/// ceiling=3, blocking the canonical near-miss case while JW correctly captures it).
const NEAR_MISS_JW_THRESHOLD: f64 = 0.90;

// ---------------------------------------------------------------------------
// Content-token overlap helpers (FR1, FR3 — PRD-mqo-nearmiss-label-token-overlap-guard)
// ---------------------------------------------------------------------------

/// Stopwords dropped from label token sets before overlap checks.
const NEAR_MISS_STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "for", "in", "on", "at", "to", "by", "and", "or",
    "is", "are", "was", "were", "be",
];

/// Split `label` into content tokens: lowercase, split on whitespace and
/// non-alphanumeric characters, drop stopwords. Returns deduplicated tokens
/// in stable order.
fn content_tokens(label: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    label
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .filter(|t| !NEAR_MISS_STOPWORDS.contains(t))
        .filter(|t| {
            // Deduplicate while preserving first-occurrence order.
            seen.insert(t.to_string())
        })
        .map(|t| t.to_string())
        .collect()
}

/// Compute the Jaccard similarity of two content-token sets (|A ∩ B| / |A ∪ B|).
/// Returns 0.0 when both sets are empty.
fn content_token_jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let set_a: std::collections::HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
    let set_b: std::collections::HashSet<&str> = b.iter().map(|s| s.as_str()).collect();
    let inter = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 { 0.0 } else { inter as f64 / union as f64 }
}

/// FR1 gate: does `supplied` share at least one content token with `canonical`
/// AFTER excluding tokens that are common prefixes of both labels?
///
/// OQ2 (decisive for the headline case): shared hierarchy prefix words (e.g.
/// "Warehouse") must not be the sole reason the intersection is non-empty.
/// Exclude any token that appears in the leading token sequence that is common
/// to both labels before computing the intersection. This way
/// "Warehouse Square Feet" vs "Warehouse State" — with shared prefix token
/// "warehouse" excluded — reduces to {"square","feet"} ∩ {"state"} = ∅ → suppressed.
fn content_token_overlap_ok(supplied: &str, canonical: &str) -> bool {
    let sup_toks = content_tokens(supplied);
    let can_toks = content_tokens(canonical);

    // Build the raw token sequences (before stopword filter) to find the
    // common prefix length in terms of word position.
    let sup_words: Vec<String> = supplied
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    let can_words: Vec<String> = canonical
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();

    // Compute the common prefix token set (words shared from the start of both).
    let common_prefix_len = sup_words
        .iter()
        .zip(can_words.iter())
        .take_while(|(a, b)| *a == *b)
        .count();
    let common_prefix_tokens: std::collections::HashSet<&str> = sup_words
        .iter()
        .take(common_prefix_len)
        .map(|s| s.as_str())
        .collect();

    // Remaining content tokens after excluding the common prefix token set.
    let sup_rem: Vec<&str> = sup_toks
        .iter()
        .map(|s| s.as_str())
        .filter(|t| !common_prefix_tokens.contains(t))
        .collect();
    let can_rem: Vec<&str> = can_toks
        .iter()
        .map(|s| s.as_str())
        .filter(|t| !common_prefix_tokens.contains(t))
        .collect();

    let sup_set: std::collections::HashSet<&str> = sup_rem.into_iter().collect();
    let can_set: std::collections::HashSet<&str> = can_rem.into_iter().collect();

    // Intersection non-empty → OK to emit a suggestion.
    // If both remainder sets are empty (e.g. single-token labels with identical
    // content, already matched by exact check) fall through to empty → suppressed.
    sup_set.intersection(&can_set).next().is_some()
}

/// RULE 11 — last-resort fuzzy guard.  Fires only when RULE 8 AND RULE 10 both
/// declined for a given field AND exactly one level in the referenced dimension
/// is within the Jaro-Winkler threshold (with a Levenshtein ceiling).
///
/// Never fires if the prefix cannot be resolved to a dimension (no anchor for
/// fuzzy matching across the whole catalog).
fn check_near_miss_level_label(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    // Fields already resolved by RULE 8 or RULE 10 — skip these.
    let resolved_fields: Vec<String> = rejections
        .iter()
        .filter(|r| {
            matches!(
                r.reason,
                RejectReason::NonCanonicalLevelLabel { .. }
                    | RejectReason::AmbiguousLevelResolvedByDimension { .. }
            )
        })
        .map(|r| r.field.clone())
        .collect();

    for dref in &mqo.dimensions {
        // --- Path A: dref.level ---
        if let Some(ref supplied) = dref.level {
            if resolved_fields.contains(supplied) {
                continue;
            }
            let candidates = suffix_candidates_with_dim(supplied, catalog);
            if !candidates.is_empty() {
                continue; // Has suffix candidates → RULE 8/10 territory
            }
            // Resolve dimension for scoped fuzzy search.
            let prefix = extract_unique_name_bracket(&dref.unique_name)
                .map(|(p, _)| p)
                .unwrap_or("");
            let dim_levels_raw = dimension_levels_for_prefix(catalog, prefix)
                .or_else(|| dimension_levels_for_prefix(catalog, &dref.unique_name));
            if let Some(dim_levels) = dim_levels_raw {
                if let Some(canonical) = fuzzy_single_match(supplied, catalog, &dim_levels) {
                    rejections.push(near_miss_rejection(
                        supplied.clone(),
                        canonical.label,
                        canonical.similarity,
                        FieldClass::HierarchyLevel,
                    ));
                }
            }
        }

        // --- Path B: unique_name bracket portion ---
        if let Some((prefix, bracket_label)) = extract_unique_name_bracket(&dref.unique_name) {
            if resolved_fields.contains(&bracket_label.to_string()) {
                continue;
            }
            let candidates = suffix_candidates_with_dim(bracket_label, catalog);
            if !candidates.is_empty() {
                continue;
            }
            if let Some(dim_levels) = dimension_levels_for_prefix(catalog, prefix) {
                if let Some(canonical) = fuzzy_single_match(bracket_label, catalog, &dim_levels) {
                    let corrected = format!("{prefix}[{}]", canonical.label);
                    rejections.push(ParamRejection::new(
                        bracket_label.to_string(),
                        FieldClass::HierarchyLevel,
                        RejectReason::NearMissLevelLabel {
                            supplied: bracket_label.to_string(),
                            canonical: canonical.label.clone(),
                            similarity: canonical.similarity,
                        },
                        vec![Suggestion {
                            name: corrected,
                            similarity: canonical.similarity,
                            note: Some(format!(
                                "near-miss label (similarity {:.2}): use the canonical form \
                                 in the unique_name bracket",
                                canonical.similarity
                            )),
                        }],
                    ));
                }
            }
        }
    }
}

struct FuzzyMatch {
    label: String,
    similarity: f64,
}

/// Among the levels belonging to `dim_levels` (normalized strings), find the ORIGINAL
/// catalog label(s) whose Jaro-Winkler similarity with `supplied` meets the threshold.
/// Applies the content-token overlap guard (FR1/OQ2): candidates that do NOT share a
/// content token with `supplied` (after common-prefix exclusion) are suppressed.
/// When multiple candidates remain, ranks by combined score (Jaccard + char similarity).
/// Returns `Some` only when exactly one candidate passes all gates, or when multiple
/// pass but the highest-combined-score candidate is uniquely best.
fn fuzzy_single_match(
    supplied: &str,
    catalog: &CatalogSnapshot,
    dim_level_norms: &[String],
) -> Option<FuzzyMatch> {
    let supplied_lower = supplied.to_lowercase();
    let mut hits: Vec<FuzzyMatch> = Vec::new();
    for h in &catalog.hierarchies {
        for orig_label in &h.levels {
            let norm = normalize(orig_label);
            if !dim_level_norms.contains(&norm) {
                continue;
            }
            let orig_lower = orig_label.to_lowercase();
            // Skip exact matches (no correction needed).
            if orig_lower == supplied_lower {
                continue;
            }
            let sim = jaro_winkler(&supplied_lower, &orig_lower);
            if sim < NEAR_MISS_JW_THRESHOLD {
                continue;
            }
            // FR1 / OQ2: suppress if the candidate shares no content token after
            // excluding common-prefix tokens.  A suppressed candidate is simply
            // not collected — when ALL candidates are suppressed, `hits` is empty
            // and we return `None`, falling through to the "level not found" path
            // that lists the hierarchy's real levels (FR2).
            if !content_token_overlap_ok(supplied, orig_label) {
                continue;
            }
            hits.push(FuzzyMatch { label: orig_label.clone(), similarity: sim });
        }
    }
    if hits.is_empty() {
        return None;
    }
    // FR3: rank by combined score (content-token Jaccard + char similarity) so a
    // higher token-overlap candidate wins over a character-closer but token-poorer one.
    hits.sort_by(|a, b| {
        let score_a = content_token_jaccard(
            &content_tokens(supplied),
            &content_tokens(&a.label),
        ) + a.similarity;
        let score_b = content_token_jaccard(
            &content_tokens(supplied),
            &content_tokens(&b.label),
        ) + b.similarity;
        score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    // Return the best candidate if there is exactly one, or if the top candidate
    // is clearly better (existing behavior: fire only on single near-miss).
    if hits.len() == 1 { Some(hits.remove(0)) } else { None }
}

fn near_miss_rejection(
    supplied: String,
    canonical: String,
    similarity: f64,
    class: FieldClass,
) -> ParamRejection {
    ParamRejection::new(
        supplied.clone(),
        class,
        RejectReason::NearMissLevelLabel { supplied: supplied.clone(), canonical: canonical.clone(), similarity },
        vec![Suggestion {
            name: canonical.clone(),
            similarity,
            note: Some(format!(
                "near-miss label (Jaro-Winkler {similarity:.2}): use the canonical form \
                 from the catalog"
            )),
        }],
    )
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal catalog with two hierarchies sharing a core label so
    /// the near-twin group fires.  `product_dimension` is canonical (shorter,
    /// Name-suffix) and `store_item_product_dimension` is the non-canonical twin.
    fn near_twin_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![],
            dimensions: vec![
                CatalogDimension {
                    unique_name: "product_dimension".into(),
                    subject_areas: vec![],
                },
                CatalogDimension {
                    unique_name: "store_item_product_dimension".into(),
                    subject_areas: vec![],
                },
            ],
            hierarchies: vec![
                CatalogHierarchy {
                    dimension_unique_name: "product_dimension".into(),
                    hierarchy_unique_name: "product_dimension".into(),
                    levels: vec!["Product Brand Name".into()],
                    level_meta: vec![],
                },
                CatalogHierarchy {
                    dimension_unique_name: "store_item_product_dimension".into(),
                    hierarchy_unique_name: "store_item_product_dimension".into(),
                    levels: vec!["Store Item Product Brand Name".into()],
                    level_meta: vec![],
                },
            ],
            date_roles: vec![],
        }
    }

    /// AC-1: non-canonical near-twin dimension → rejected, canonical suggested.
    #[test]
    fn near_twin_ac1_non_canonical_rejected() {
        let catalog = near_twin_catalog();
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_item_product_dimension".into(),
                level: Some("Store Item Product Brand Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            !rejections.is_empty(),
            "expected NonCanonicalNearTwin rejection for non-canonical twin"
        );
        let r = &rejections[0];
        assert!(
            matches!(&r.reason, RejectReason::NonCanonicalNearTwin { .. }),
            "expected NonCanonicalNearTwin, got {:?}",
            r.reason
        );
        if let RejectReason::NonCanonicalNearTwin {
            suggested_canonical,
            ..
        } = &r.reason
        {
            assert!(
                suggested_canonical.contains("product_dimension"),
                "suggestion should point to product_dimension, got {suggested_canonical}"
            );
        }
    }

    /// AC-2: canonical dimension → no rejection.
    #[test]
    fn near_twin_ac2_canonical_passes() {
        let catalog = near_twin_catalog();
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "product_dimension".into(),
                level: Some("Product Brand Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.is_empty(),
            "canonical pick should not be rejected, got {rejections:?}"
        );
    }

    /// AC-3: non-canonical twin BUT MQO has a filter on that same hierarchy →
    /// intent guard fires, no rejection.
    #[test]
    fn near_twin_ac3_intent_guard_suppresses_rejection() {
        let catalog = near_twin_catalog();
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_item_product_dimension".into(),
                level: Some("Store Item Product Brand Name".into()),
                ..Default::default()
            }],
            filters: vec![MqoFilterRef {
                unique_name: "store_item_product_dimension".into(),
                level: Some("Store Item Product Brand Name".into()),
                members: vec!["Brand X".into()],
                ..Default::default()
            }],
        };
        let rejections = validate(&mqo, &catalog);
        let twin_rejections: Vec<_> = rejections
            .iter()
            .filter(|r| matches!(r.reason, RejectReason::NonCanonicalNearTwin { .. }))
            .collect();
        assert!(
            twin_rejections.is_empty(),
            "intent guard should suppress rejection when hierarchy has a filter, got {twin_rejections:?}"
        );
    }

    // ── RULE 5: attribute-aggregation guard tests ─────────────────────────────

    /// Build a catalog with a Store dimension that has a numeric level
    /// "Store Number of Employees" (a dimension attribute, not a measure)
    /// plus a genuine measure "Store Sales".
    fn attr_agg_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![
                CatalogMeasure {
                    unique_name: "store.store_sales".into(),
                    label: Some("Store Sales".into()),
                    ..Default::default()
                },
                // Also register label alias (mirrors pipeline.rs aliasing)
                CatalogMeasure {
                    unique_name: "Store Sales".into(),
                    label: Some("Store Sales".into()),
                    ..Default::default()
                },
            ],
            dimensions: vec![crate::CatalogDimension {
                unique_name: "Store".into(),
                subject_areas: vec![],
            }],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "Store".into(),
                hierarchy_unique_name: "Store".into(),
                levels: vec![
                    "Store Name".into(),
                    "Store Number of Employees".into(),
                ],
                level_meta: vec![],
            }],
            date_roles: vec![],
        }
    }

    /// AC-1 / RULE 5: aggregating a dimension level is rejected.
    #[test]
    fn dataset_aggregate_attribute_level_rejected() {
        let catalog = attr_agg_catalog();
        let r = check_dataset_aggregate_attribute(
            "Store Number of Employees",
            &["Store Name"],
            &catalog,
        );
        assert!(
            r.is_some(),
            "expected AttributeAggregation rejection for a dimension level"
        );
        let rejection = r.unwrap();
        assert!(
            matches!(rejection.reason, RejectReason::AttributeAggregation { .. }),
            "expected AttributeAggregation reason, got {:?}",
            rejection.reason
        );
        assert_eq!(rejection.class, FieldClass::HierarchyLevel);
        if let RejectReason::AttributeAggregation { ref column, ref reason } = rejection.reason {
            assert_eq!(column, "Store Number of Employees");
            assert!(
                reason.contains("dimension attribute"),
                "rejection reason should mention 'dimension attribute'"
            );
            assert!(
                reason.contains("projection"),
                "rejection reason should mention 'projection'"
            );
        }
    }

    /// AC-3 / FR-2: a genuine measure is NOT rejected.
    #[test]
    fn dataset_aggregate_real_measure_not_rejected() {
        let catalog = attr_agg_catalog();
        // Test via unique_name
        let r1 = check_dataset_aggregate_attribute("Store Sales", &["Store Name"], &catalog);
        assert!(
            r1.is_none(),
            "real measure 'Store Sales' should not be rejected, got {:?}",
            r1
        );
        // Test via unique_name with dot form
        let r2 = check_dataset_aggregate_attribute("store.store_sales", &["Store Name"], &catalog);
        assert!(
            r2.is_none(),
            "real measure 'store.store_sales' should not be rejected, got {:?}",
            r2
        );
    }

    /// AC-4 / FR-2: column not in catalog at all → fail-open (no rejection).
    #[test]
    fn dataset_aggregate_unknown_column_fail_open() {
        let catalog = attr_agg_catalog();
        let r = check_dataset_aggregate_attribute(
            "Completely Unknown Column XYZ",
            &["Store Name"],
            &catalog,
        );
        assert!(
            r.is_none(),
            "unknown column should fail-open (no rejection), got {:?}",
            r
        );
    }

    /// FR-2 conservative: empty group_by → fail-open (no per-entity-attribute shape).
    #[test]
    fn dataset_aggregate_empty_group_by_fail_open() {
        let catalog = attr_agg_catalog();
        let r = check_dataset_aggregate_attribute(
            "Store Number of Employees",
            &[],
            &catalog,
        );
        assert!(
            r.is_none(),
            "empty group_by should fail-open (could be intentional global aggregate), got {:?}",
            r
        );
    }

    /// FR-2 conservative: column that matches both a level and a measure → fail-open.
    #[test]
    fn dataset_aggregate_ambiguous_fail_open() {
        // Build a catalog where "Dual Column" appears as both a measure and a level.
        let ambiguous_catalog = CatalogSnapshot {
            measures: vec![CatalogMeasure {
                unique_name: "Dual Column".into(),
                label: Some("Dual Column".into()),
                ..Default::default()
            }],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "Dim".into(),
                hierarchy_unique_name: "Dim".into(),
                levels: vec!["Dual Column".into()],
                level_meta: vec![],
            }],
            ..Default::default()
        };
        let r = check_dataset_aggregate_attribute("Dual Column", &["Some Group"], &ambiguous_catalog);
        assert!(
            r.is_none(),
            "ambiguous (both measure and level) column should fail-open, got {:?}",
            r
        );
    }

    // ── FR-3 (PRD-mqo-project-not-count-grounding): count-evasion nudge tests ──

    /// FR-3 / AC-3: `count` applied to a numeric attribute level must be rejected —
    /// the correct shape is a measureless projection of the level, not a count.
    /// Guards against the model evading RULE 5's sum-block by switching to count.
    #[test]
    fn count_on_numeric_level_rejected() {
        // Catalog: "Store Number of Employees" is a dimension level (kind=level),
        // NOT a measure.  Applying count to it should be rejected.
        let catalog = attr_agg_catalog();
        let r = check_dataset_aggregate_attribute(
            "Store Number of Employees",
            &["Store Name"],
            &catalog,
        );
        assert!(
            r.is_some(),
            "count on a numeric kind=level column must be rejected (same predicate as sum/avg), got None"
        );
        let rejection = r.unwrap();
        assert!(
            matches!(rejection.reason, RejectReason::AttributeAggregation { .. }),
            "expected AttributeAggregation reason for count on level: {:?}",
            rejection.reason
        );
        if let RejectReason::AttributeAggregation { ref reason, .. } = rejection.reason {
            assert!(
                reason.contains("project"),
                "rejection reason must suggest projection: {reason}"
            );
        }
    }

    /// FR-4 guardrail: a genuine count measure (`total_product_count`) must NOT be
    /// rejected — it is a `kind=measure`, not a `kind=level`.  Member-count questions
    /// ("how many products per category") remain measure-shaped; this nudge does not
    /// mis-steer them.
    #[test]
    fn count_measure_query_not_rejected() {
        // Catalog with a genuine count measure and a product level.
        let catalog = CatalogSnapshot {
            measures: vec![
                CatalogMeasure {
                    unique_name: "total_product_count".into(),
                    label: Some("Total Product Count".into()),
                    ..Default::default()
                },
                CatalogMeasure {
                    unique_name: "Total Product Count".into(),
                    label: Some("Total Product Count".into()),
                    ..Default::default()
                },
            ],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "product_dimension".into(),
                hierarchy_unique_name: "product_dimension".into(),
                levels: vec!["Product Category".into(), "Product Name".into()],
                level_meta: vec![],
            }],
            ..Default::default()
        };

        // Applying count via the measure column "total_product_count" (kind=measure) must
        // NOT be rejected — it's a legitimate member-count measure.
        let r1 = check_dataset_aggregate_attribute(
            "total_product_count",
            &["Product Category"],
            &catalog,
        );
        assert!(
            r1.is_none(),
            "genuine count measure 'total_product_count' must not be rejected, got {:?}",
            r1
        );

        // Same via label form.
        let r2 = check_dataset_aggregate_attribute(
            "Total Product Count",
            &["Product Category"],
            &catalog,
        );
        assert!(
            r2.is_none(),
            "genuine count measure 'Total Product Count' (label) must not be rejected, got {:?}",
            r2
        );
    }

    // ---------------------------------------------------------------------------
    // RULE 7: channel-scope mismatch guard
    // (PRD-mqo-channel-scope-measure-grounding, AC3/AC4/AC5/AC6)
    // ---------------------------------------------------------------------------

    /// Build a minimal catalog with channel-scoped measures matching the
    /// TPC-DS pattern: `Total Quantity Sold` (all 3 channels) and
    /// `Store Quantity Sold` (store only).
    fn channel_scope_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![
                CatalogMeasure {
                    unique_name: "tpcds_benchmark_model.total_quantity_sold".into(),
                    label: Some("Total Quantity Sold".into()),
                    channel_scope: Some(vec![
                        "store_sales".into(),
                        "catalog_sales".into(),
                        "web_sales".into(),
                    ]),
                    ..Default::default()
                },
                CatalogMeasure {
                    unique_name: "tpcds_benchmark_model.store_quantity_sold".into(),
                    label: Some("Store Quantity Sold".into()),
                    channel_scope: Some(vec!["store_sales".into()]),
                    ..Default::default()
                },
                CatalogMeasure {
                    unique_name: "tpcds_benchmark_model.catalog_quantity_sold".into(),
                    label: Some("Catalog Quantity Sold".into()),
                    channel_scope: Some(vec!["catalog_sales".into()]),
                    ..Default::default()
                },
            ],
            dimensions: vec![CatalogDimension {
                unique_name: "product_dimension".into(),
                subject_areas: vec![],
            }],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "product_dimension".into(),
                hierarchy_unique_name: "product_dimension".into(),
                levels: vec!["Product Brand Name".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        }
    }

    /// AC3: guard flags all-channel measure when channel-scoped sibling exists.
    #[test]
    fn channel_scope_mismatch_fires_for_all_channel_pick() {
        let catalog = channel_scope_catalog();
        // Agent bound the all-channel total
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef {
                unique_name: "tpcds_benchmark_model.total_quantity_sold".into(),
                aggregation: None,
            }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "product_dimension".into(),
                level: Some("Product Brand Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let mismatch = rejections.iter().find(|r| {
            matches!(r.reason, RejectReason::ChannelScopeMismatch { .. })
        });
        assert!(
            mismatch.is_some(),
            "ChannelScopeMismatch should fire for all-channel measure with channel sibling, got {:?}",
            rejections
        );
        // AC6: suggestion must name a channel-scoped sibling.
        if let Some(r) = mismatch {
            if let RejectReason::ChannelScopeMismatch { ref suggested_measure, ref named_channel, .. } =
                r.reason
            {
                assert!(
                    !suggested_measure.is_empty(),
                    "suggested_measure must be named (AC6), got empty"
                );
                assert!(
                    !named_channel.is_empty(),
                    "named_channel must be set (FR5), got empty"
                );
                // The sibling should be a single-channel quantity measure.
                assert!(
                    suggested_measure.contains("Quantity Sold"),
                    "suggested should be a quantity measure, got: {suggested_measure}"
                );
            }
        }
    }

    /// AC4: guard stays silent when only all-channel measure exists (no sibling).
    #[test]
    fn channel_scope_mismatch_silent_when_no_sibling() {
        // A catalog with only an all-channel measure — no single-channel sibling.
        let catalog = CatalogSnapshot {
            measures: vec![CatalogMeasure {
                unique_name: "tpcds_benchmark_model.total_quantity_sold".into(),
                label: Some("Total Quantity Sold".into()),
                channel_scope: Some(vec![
                    "store_sales".into(),
                    "catalog_sales".into(),
                    "web_sales".into(),
                ]),
                ..Default::default()
            }],
            dimensions: vec![CatalogDimension {
                unique_name: "product_dimension".into(),
                subject_areas: vec![],
            }],
            hierarchies: vec![],
            date_roles: vec![],
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef {
                unique_name: "tpcds_benchmark_model.total_quantity_sold".into(),
                aggregation: None,
            }],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let mismatch = rejections.iter().find(|r| {
            matches!(r.reason, RejectReason::ChannelScopeMismatch { .. })
        });
        assert!(
            mismatch.is_none(),
            "ChannelScopeMismatch must not fire when no channel sibling exists (FR4), got {:?}",
            rejections
        );
    }

    /// AC5: all-channel question (no channel named, no sibling context) stays silent.
    /// When the agent binds `Total Quantity Sold` and no filter or dimension context
    /// implies a single channel, the guard should not fire. Here we test that a
    /// single-channel measure picked correctly does NOT trigger the rule.
    #[test]
    fn channel_scope_mismatch_silent_for_single_channel_pick() {
        let catalog = channel_scope_catalog();
        // Agent correctly picked store-scoped measure.
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef {
                unique_name: "tpcds_benchmark_model.store_quantity_sold".into(),
                aggregation: None,
            }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "product_dimension".into(),
                level: Some("Product Brand Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let mismatch = rejections.iter().find(|r| {
            matches!(r.reason, RejectReason::ChannelScopeMismatch { .. })
        });
        assert!(
            mismatch.is_none(),
            "ChannelScopeMismatch must not fire when agent correctly picks single-channel measure, got {:?}",
            rejections
        );
    }

    /// Guard is silent when channel_scope is absent (no binding known).
    #[test]
    fn channel_scope_mismatch_silent_when_scope_absent() {
        let catalog = CatalogSnapshot {
            measures: vec![
                CatalogMeasure {
                    unique_name: "some_measure".into(),
                    label: Some("Some Measure".into()),
                    // No channel_scope — binding unknown.
                    channel_scope: None,
                    ..Default::default()
                },
            ],
            dimensions: vec![],
            hierarchies: vec![],
            date_roles: vec![],
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef {
                unique_name: "some_measure".into(),
                aggregation: None,
            }],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(r.reason, RejectReason::ChannelScopeMismatch { .. })),
            "guard must stay silent when channel_scope is absent (FR4), got {:?}",
            rejections
        );
    }
    // ── RULE 6: synthetic rank / row-number guard ──────────────────────────

    /// Minimal catalog for RULE6 tests: real measures, a "Net Profit Tier"
    /// level (guardrail G3), and a catalog-defined "Rank" measure (AC-4).
    fn rank_guard_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![
                CatalogMeasure {
                    unique_name: "Store Number of Employees".into(),
                    label: Some("Store Number of Employees".into()),
                    ..Default::default()
                },
                CatalogMeasure {
                    unique_name: "Store Returns Count".into(),
                    label: Some("Store Returns Count".into()),
                    ..Default::default()
                },
                CatalogMeasure {
                    unique_name: "Web Sales".into(),
                    label: Some("Web Sales".into()),
                    ..Default::default()
                },
                // AC-4: a catalog-defined "Rank" measure must not be rejected.
                CatalogMeasure {
                    unique_name: "Rank".into(),
                    label: Some("Rank".into()),
                    ..Default::default()
                },
            ],
            dimensions: vec![
                CatalogDimension {
                    unique_name: "Store".into(),
                    subject_areas: vec![],
                },
                CatalogDimension {
                    unique_name: "Customer".into(),
                    subject_areas: vec![],
                },
            ],
            hierarchies: vec![
                CatalogHierarchy {
                    dimension_unique_name: "Store".into(),
                    hierarchy_unique_name: "Store".into(),
                    levels: vec![
                        "Store Name".into(),
                        "Net Profit Tier".into(),
                        "Gender".into(),
                    ],
                    level_meta: vec![],
                },
                CatalogHierarchy {
                    dimension_unique_name: "Customer".into(),
                    hierarchy_unique_name: "Customer".into(),
                    levels: vec!["Customer State Name".into()],
                    level_meta: vec![],
                },
            ],
            date_roles: vec![],
        }
    }

    /// AC-1 / G1: ungrounded "Rank" in the measure slot is rejected with SyntheticRankColumn.
    #[test]
    fn rule6_ac1_ungrounded_rank_in_measure_rejected() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![
                MqoMeasureRef { unique_name: "Store Number of Employees".into(), aggregation: None },
                MqoMeasureRef { unique_name: "Rank".into(), aggregation: None },
            ],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Store Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        let rank_rej: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. }))
            .collect();
        assert!(!rank_rej.is_empty(), "ungrounded Rank must be rejected; got: {rejections:?}");
        if let RejectReason::SyntheticRankColumn { column } = &rank_rej[0].reason {
            assert_eq!(column, "Rank");
        }
    }

    /// AC-1 variant: ungrounded "Ranking" column is rejected.
    #[test]
    fn rule6_ac1_ungrounded_ranking_rejected() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![
                MqoMeasureRef { unique_name: "Store Returns Count".into(), aggregation: None },
                MqoMeasureRef { unique_name: "Ranking".into(), aggregation: None },
            ],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(
            rejections.iter().any(|r| matches!(&r.reason,
                RejectReason::SyntheticRankColumn { column } if column == "Ranking")),
            "ungrounded Ranking must be rejected; got: {rejections:?}"
        );
    }

    /// AC-2 / G4: RULE6 is wired into validate() (not just a direct rule call).
    #[test]
    fn rule6_ac2_wired_into_validate() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Rank".into(), aggregation: None }],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(
            rejections.iter().any(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "validate() must surface RULE6 rejection; got: {rejections:?}"
        );
    }

    /// AC-3 / G3: real "Net Profit Tier" level (grounded) must NOT be rejected.
    #[test]
    fn rule6_ac3_real_tier_level_passes() {
        let catalog = rank_guard_catalog();
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![
                MqoDimensionRef { unique_name: "Store".into(), level: Some("Store Name".into()), ..Default::default() },
                MqoDimensionRef { unique_name: "Store".into(), level: Some("Net Profit Tier".into()), ..Default::default() },
                MqoDimensionRef { unique_name: "Store".into(), level: Some("Gender".into()), ..Default::default() },
            ],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "real Net Profit Tier level must not be rejected; got: {rejections:?}"
        );
    }

    /// AC-4 / FR4: catalog-defined "Rank" measure (grounded) must NOT be rejected.
    #[test]
    fn rule6_ac4_grounded_rank_measure_passes() {
        let catalog = rank_guard_catalog(); // has "Rank" as a real measure
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Rank".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Store Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "grounded Rank measure must not be rejected; got: {rejections:?}"
        );
    }

    /// AC-5 / FR5: clean top-N query (no rank column) must not trigger RULE6.
    #[test]
    fn rule6_ac5_clean_topn_query_passes() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Store Number of Employees".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Store Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "clean top-N query must not be rejected; got: {rejections:?}"
        );
    }

    /// AC-6 / G2: rejection message names the column and references ORDER BY + LIMIT.
    #[test]
    fn rule6_ac6_actionable_message() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Rank".into(), aggregation: None }],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        let rej = rejections.iter()
            .find(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. }))
            .expect("should find a SyntheticRankColumn rejection");
        let note = rej.suggestions.first().and_then(|s| s.note.as_deref()).unwrap_or("");
        assert!(note.contains("ORDER BY"), "message must mention ORDER BY; got: {note:?}");
        assert!(note.contains("LIMIT"), "message must mention LIMIT; got: {note:?}");
        assert!(note.contains("Rank") || note.contains("[Rank]"), "message must name the column; got: {note:?}");
    }

    /// AC-7 / FR6: all rank/ordinal patterns rejected when ungrounded; "Store Number" is not matched.
    #[test]
    fn rule6_ac7_pattern_coverage() {
        let catalog = CatalogSnapshot::default();
        let rank_variants = [
            "Rank", "rank", "RANK", "Ranking", "Row Number", "row number",
            "RowNum", "rownum", "RowNumber", "rownumber", "Row No", "row no",
            "Ordinal", "ordinal", "Position", "position", "Row Rank", "Row Order",
        ];
        for variant in &rank_variants {
            let mqo = BoundMqoInput {
                measures: vec![MqoMeasureRef { unique_name: (*variant).to_string(), aggregation: None }],
                dimensions: vec![],
                filters: vec![],
            };
            let rejections = validate(&mqo, &catalog);
            assert!(
                rejections.iter().any(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
                "variant {variant:?} must be rejected as synthetic rank; got: {rejections:?}"
            );
        }
        // "Store Number" must NOT match the rank pattern.
        let mqo_num = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Store Number".into(), aggregation: None }],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo_num, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "Store Number must not match rank pattern; got: {rejections:?}"
        );
    }

    /// AC-8 / NFR1: SyntheticRankColumn and Unmapped both accumulate (no early exit).
    #[test]
    fn rule6_ac8_accumulates_with_other_rules() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![
                MqoMeasureRef { unique_name: "Rank".into(), aggregation: None },
                MqoMeasureRef { unique_name: "NonExistentMeasureXYZ".into(), aggregation: None },
            ],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(rejections.iter().any(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "expected SyntheticRankColumn; got: {rejections:?}");
        assert!(rejections.iter().any(|r| matches!(&r.reason, RejectReason::Unmapped)),
            "expected Unmapped alongside SyntheticRankColumn; got: {rejections:?}");
    }

    /// Rank column in the dimension slot is also rejected when ungrounded.
    #[test]
    fn rule6_rank_in_dimension_slot_rejected() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef { unique_name: "Rank".into(), ..Default::default() }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(
            rejections.iter().any(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "ungrounded Rank in dimension slot must be rejected; got: {rejections:?}"
        );
    }

    // ── RULE 6 v0.9.3: bracket-form unique_name rank guard ────────────────────

    /// AC1: bracket-form "store_sales.[Rank]" in dimension slot, Rank not in catalog → fires.
    #[test]
    fn rule6_bracket_rank_dimension_fires() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_sales.[Rank]".into(),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(
            rejections.iter().any(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "bracket-form '[Rank]' in dimension must fire SyntheticRankColumn; got: {rejections:?}"
        );
    }

    /// AC2: bracket-form "store_sales.[Rank]" but catalog HAS level "Rank" → grounded, silent.
    #[test]
    fn rule6_bracket_rank_grounded_silent() {
        let mut catalog = rank_guard_catalog();
        // Add "Rank" as a catalog level so it's grounded.
        catalog.hierarchies[0].levels.push("Rank".into());
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_sales.[Rank]".into(),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "grounded bracket '[Rank]' must not fire; got: {rejections:?}"
        );
    }

    /// AC3: bare "Rank" (no brackets) still fires as before (existing behavior preserved).
    #[test]
    fn rule6_bare_rank_still_fires() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Rank".into(), aggregation: None }],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(
            rejections.iter().any(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "bare 'Rank' must still fire; got: {rejections:?}"
        );
    }

    /// AC4: bracket-form with non-rank label "[Store Name]" → silent.
    #[test]
    fn rule6_bracket_non_rank_silent() {
        let catalog = rank_guard_catalog();
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Store Name]".into(),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "non-rank bracket label must not fire; got: {rejections:?}"
        );
    }

    /// AC1 (measure slot): bracket-form "tpcds.[Rank]" in measure slot, not in catalog → fires.
    #[test]
    fn rule6_bracket_rank_measure_fires() {
        let catalog = rank_guard_catalog();
        let catalog_no_rank = CatalogSnapshot {
            measures: catalog.measures.iter().filter(|m| m.unique_name != "Rank").cloned().collect(),
            ..catalog.clone()
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "tpcds.[Rank]".into(), aggregation: None }],
            dimensions: vec![],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog_no_rank);
        assert!(
            rejections.iter().any(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. })),
            "bracket-form '[Rank]' in measure slot must fire; got: {rejections:?}"
        );
    }

    // ── RULE 8: non-canonical level label ─────────────────────────────────────

    /// Catalog for RULE 8 tests: a "Store Floor Space" level in the Store hierarchy.
    fn rule8_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![CatalogMeasure {
                unique_name: "Net Profit".into(),
                label: Some("Net Profit".into()),
                ..Default::default()
            }],
            dimensions: vec![CatalogDimension {
                unique_name: "Store".into(),
                subject_areas: vec![],
            }],
            hierarchies: vec![CatalogHierarchy {
                hierarchy_unique_name: "Store".into(),
                dimension_unique_name: "Store".into(),
                levels: vec!["Store".into(), "Store Floor Space".into(), "Store Name".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        }
    }

    /// FR2: agent binds "Floor Space" (truncated suffix) — must get NonCanonicalLevelLabel
    /// pointing to "Store Floor Space".
    #[test]
    fn rule8_truncated_suffix_fires() {
        let catalog = rule8_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Floor Space".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rule8 = rejections.iter().find(|r| {
            matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })
        });
        assert!(rule8.is_some(), "RULE 8 must fire for 'Floor Space'; got: {rejections:?}");
        if let Some(r) = rule8 {
            if let RejectReason::NonCanonicalLevelLabel { supplied, canonical } = &r.reason {
                assert_eq!(supplied, "Floor Space");
                assert_eq!(canonical, "Store Floor Space");
            }
            // Suggestion must name the canonical label.
            assert!(
                r.suggestions.iter().any(|s| s.name == "Store Floor Space"),
                "suggestion must include 'Store Floor Space'; got: {:?}",
                r.suggestions
            );
        }
    }

    /// AC2: exact catalog label "Store Floor Space" must NOT trigger RULE 8.
    #[test]
    fn rule8_exact_match_silent() {
        let catalog = rule8_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Store Floor Space".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })),
            "RULE 8 must stay silent for exact catalog label; got: {rejections:?}"
        );
    }

    /// AC3: ambiguous suffix ("Name" matches both "Store Name" and "Store Floor Space" — wait,
    /// actually "Name" only suffix-matches "Store Name") — use a genuinely ambiguous label to
    /// confirm RULE 8 does NOT fire when ≥2 catalog levels end with the supplied suffix.
    #[test]
    fn rule8_ambiguous_suffix_silent() {
        // Add a second level ending in "Space" so "Space" is ambiguous.
        let mut catalog = rule8_catalog();
        catalog.hierarchies[0].levels.push("Web Space".into());
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Space".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })),
            "RULE 8 must stay silent when suffix is ambiguous (≥2 matches); got: {rejections:?}"
        );
    }

    /// AC4: no level set on dref — RULE 8 must stay silent.
    #[test]
    fn rule8_no_level_silent() {
        let catalog = rule8_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })),
            "RULE 8 must stay silent when level is None; got: {rejections:?}"
        );
    }

    // ── RULE 8 v0.9.2: bracket portion of unique_name (AC1–AC8) ──────────────

    /// AC1: bracket-form truncation fires with corrected unique_name suggestion.
    /// store_dimension.[Floor Space] → NonCanonicalLevelLabel, suggestion = store_dimension.[Store Floor Space]
    #[test]
    fn rule8_bracket_truncation_fires_with_corrected_unique_name() {
        let catalog = rule8_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Floor Space]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rule8 = rejections.iter().find(|r| {
            matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })
        });
        assert!(rule8.is_some(), "RULE 8 must fire for bracket 'Floor Space'; got: {rejections:?}");
        if let Some(r) = rule8 {
            if let RejectReason::NonCanonicalLevelLabel { supplied, canonical } = &r.reason {
                assert_eq!(supplied, "Floor Space");
                assert_eq!(canonical, "Store Floor Space");
            }
            // Suggestion must carry the corrected unique_name (AC1/FR3).
            assert!(
                r.suggestions.iter().any(|s| s.name == "store_dimension.[Store Floor Space]"),
                "suggestion must include corrected unique_name; got: {:?}", r.suggestions
            );
        }
    }

    /// AC2 (unique_name path): exact bracket label passes silently.
    #[test]
    fn rule8_bracket_exact_match_silent() {
        let catalog = rule8_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Store Floor Space]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })),
            "RULE 8 must stay silent for exact bracket label; got: {rejections:?}"
        );
    }

    /// AC3: ambiguous bracket label (suffix of ≥2 catalog levels) → silent.
    #[test]
    fn rule8_bracket_ambiguous_silent() {
        let mut catalog = rule8_catalog();
        catalog.hierarchies[0].levels.push("Warehouse Floor Space".into());
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Floor Space]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })),
            "RULE 8 must stay silent on ambiguous bracket; got: {rejections:?}"
        );
    }

    /// AC5: store-employee-counts case — bracket "Number of Employees" → "Store Number of Employees".
    #[test]
    fn rule8_bracket_number_of_employees_fires() {
        let catalog = CatalogSnapshot {
            measures: vec![CatalogMeasure {
                unique_name: "Net Profit".into(),
                label: Some("Net Profit".into()),
                ..Default::default()
            }],
            dimensions: vec![CatalogDimension { unique_name: "Store".into(), subject_areas: vec![] }],
            hierarchies: vec![CatalogHierarchy {
                hierarchy_unique_name: "Store".into(),
                dimension_unique_name: "Store".into(),
                levels: vec!["Store Name".into(), "Store Number of Employees".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        };
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Number of Employees]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rule8 = rejections.iter().find(|r| {
            matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })
        });
        assert!(rule8.is_some(), "RULE 8 must fire for '[Number of Employees]'; got: {rejections:?}");
        if let Some(r) = rule8 {
            assert!(
                r.suggestions.iter().any(|s| s.name == "store_dimension.[Store Number of Employees]"),
                "suggestion must be corrected unique_name; got: {:?}", r.suggestions
            );
        }
    }

    /// AC6: no bracket in unique_name → bracket check skipped, only level path active.
    #[test]
    fn rule8_bare_unique_name_no_bracket_check() {
        let catalog = rule8_catalog();
        // "Floor Space" bare unique_name — no brackets — should NOT trigger bracket path.
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Floor Space".into(), // no brackets
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        // The bare unique_name has no bracket, so bracket path is skipped.
        // The level path also doesn't fire (level is None). So RULE 8 is silent.
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })),
            "RULE 8 bracket path must not fire on bare unique_name; got: {rejections:?}"
        );
    }

    /// AC7: FR7 de-dup — both dref.level and bracket trigger same canonical → one rejection.
    #[test]
    fn rule8_dedup_level_and_bracket_same_canonical() {
        let catalog = rule8_catalog();
        // Both level="Floor Space" AND unique_name has [Floor Space] → same canonical → 1 rejection.
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Floor Space]".into(),
                level: Some("Floor Space".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rule8_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. }))
            .count();
        assert_eq!(rule8_count, 1, "FR7: must emit exactly 1 RULE 8 rejection when level+bracket agree; got: {rejections:?}");
    }

    // -----------------------------------------------------------------------
    // RULE 6 dimension-scoped grounding tests (PRD-mqo-rule6-dimension-scoped-rank-grounding)
    // -----------------------------------------------------------------------

    /// Catalog with a `Rank` level in a foreign dimension (not store_sales).
    fn rank_leak_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![],
            dimensions: vec![],
            hierarchies: vec![
                // store_sales dimension — NO Rank level.
                CatalogHierarchy {
                    dimension_unique_name: "store_sales".into(),
                    hierarchy_unique_name: "store_sales".into(),
                    levels: vec!["Store Name".into(), "Store Number of Employees".into()],
                    ..Default::default()
                },
                // A separate dimension that DOES have a Rank level.
                CatalogHierarchy {
                    dimension_unique_name: "rank_dimension".into(),
                    hierarchy_unique_name: "rank_dimension".into(),
                    levels: vec!["Rank".into(), "Category".into()],
                    ..Default::default()
                },
            ],
            date_roles: vec![],
        }
    }

    #[test]
    fn rule6_cross_dimension_rank_leak_fires() {
        // AC1: bracket Rank in store_sales — store_sales has no Rank level → fires.
        let catalog = rank_leak_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Store Quantity Sold".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_sales.[Rank]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rank_rejects: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. }))
            .collect();
        assert_eq!(rank_rejects.len(), 1, "AC1: cross-dim bracket Rank should fire; got: {rejections:?}");
    }

    #[test]
    fn rule6_in_dimension_rank_grounded_silent() {
        // AC2: bracket Rank in rank_dimension — rank_dimension HAS Rank → silent.
        let catalog = rank_leak_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Store Quantity Sold".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "rank_dimension.[Rank]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rank_rejects: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. }))
            .collect();
        assert!(rank_rejects.is_empty(), "AC2: in-dim Rank must be silent; got: {rejections:?}");
    }

    #[test]
    fn rule6_unresolvable_prefix_conservative() {
        // AC5: prefix doesn't match any dimension → conservative flat-union; Rank IS in catalog
        // (rank_dimension) so flat-union grounding accepts it → no RULE 6 rejection.
        let catalog = rank_leak_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "unknown_thing.[Rank]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        // The conservative fallback uses flat level_norms which contains "rank" → silent.
        let rank_rejects: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::SyntheticRankColumn { .. }))
            .collect();
        assert!(rank_rejects.is_empty(), "AC5: unresolvable prefix must conservatively stay silent; got: {rejections:?}");
    }

    // -----------------------------------------------------------------------
    // RULE 10 tests (PRD-mqo-validator-ambiguous-level-dimension-resolution)
    // -----------------------------------------------------------------------

    /// Catalog for RULE 10 tests: Customer State Name in customer dim; another * State Name
    /// level in a different dim so the suffix is ambiguous globally.
    fn rule10_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![],
            dimensions: vec![],
            hierarchies: vec![
                CatalogHierarchy {
                    dimension_unique_name: "customer_dimension".into(),
                    hierarchy_unique_name: "customer_dimension".into(),
                    levels: vec!["Customer State Name".into(), "Customer ID".into()],
                    ..Default::default()
                },
                CatalogHierarchy {
                    dimension_unique_name: "store_dimension".into(),
                    hierarchy_unique_name: "store_dimension".into(),
                    levels: vec!["Store State Name".into(), "Store Name".into()],
                    ..Default::default()
                },
                CatalogHierarchy {
                    dimension_unique_name: "product_dimension".into(),
                    hierarchy_unique_name: "product_dimension".into(),
                    levels: vec!["Product Brand Name".into(), "Product Category".into()],
                    ..Default::default()
                },
                CatalogHierarchy {
                    dimension_unique_name: "store_item_dimension".into(),
                    hierarchy_unique_name: "store_item_dimension".into(),
                    levels: vec!["Store Item Product Brand Name".into()],
                    ..Default::default()
                },
            ],
            date_roles: vec![],
        }
    }

    #[test]
    fn rule10_customer_state_ambiguous_resolved_by_dimension() {
        // AC1: "State Name" suffix-matches "Customer State Name" (customer dim) AND
        // "Store State Name" (store dim) → ≥2 candidates → RULE 8 silent.
        // Ref prefix resolves to customer_dimension → exactly one candidate there → RULE 10 fires.
        let catalog = rule10_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Web Sales".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "customer_dimension.[State Name]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r10: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::AmbiguousLevelResolvedByDimension { .. }))
            .collect();
        assert_eq!(r10.len(), 1, "AC1: RULE 10 must fire for State Name in customer dim; got: {rejections:?}");
        if let RejectReason::AmbiguousLevelResolvedByDimension { canonical, .. } = &r10[0].reason {
            assert_eq!(canonical, "Customer State Name");
        }
    }

    #[test]
    fn rule10_brand_name_ambiguous_resolved_by_dimension() {
        // AC2: "Brand Name" suffix-matches both "Product Brand Name" and "Store Item Product Brand Name".
        // Ref prefix = product_dimension → exactly one candidate there → RULE 10 fires.
        let catalog = rule10_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Store Quantity Sold".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "product_dimension.[Brand Name]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r10: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::AmbiguousLevelResolvedByDimension { .. }))
            .collect();
        assert_eq!(r10.len(), 1, "AC2: RULE 10 must fire for Brand Name; got: {rejections:?}");
        if let RejectReason::AmbiguousLevelResolvedByDimension { canonical, .. } = &r10[0].reason {
            assert_eq!(canonical, "Product Brand Name");
        }
    }

    #[test]
    fn rule10_does_not_fire_when_rule8_already_handled() {
        // AC3: "Floor Space" is suffix-unique → RULE 8 fires, RULE 10 must NOT fire for same field.
        let catalog = rule8_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Floor Space]".into(),
                level: Some("Floor Space".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r10_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::AmbiguousLevelResolvedByDimension { .. }))
            .count();
        assert_eq!(r10_count, 0, "AC3/AC5: RULE 10 must not fire when RULE 8 already handled; got: {rejections:?}");
    }

    // -----------------------------------------------------------------------
    // RULE 11 tests (PRD-mqo-validator-fuzzy-near-miss-level-guard)
    // -----------------------------------------------------------------------

    fn rule11_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![],
            dimensions: vec![],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "warehouse_dimension".into(),
                hierarchy_unique_name: "warehouse_dimension".into(),
                levels: vec!["Warehouse Name".into(), "Warehouse Square Feet".into()],
                ..Default::default()
            }],
            date_roles: vec![],
        }
    }

    #[test]
    fn rule11_near_miss_fires() {
        // Original AC1 behavior updated for the token-overlap guard:
        // "Warehouse Square Footage" has common prefix "warehouse square" with
        // "Warehouse Square Feet"; after prefix exclusion, {"footage"} ∩ {"feet"} = ∅.
        // Under the new overlap guard (PRD-mqo-nearmiss-label-token-overlap-guard),
        // this suggestion is suppressed. The guard fires for genuine token-sharing
        // typos (e.g. "Warehouse Sq Feet" → shared "feet" after prefix exclusion).
        let catalog = rule11_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "warehouse_dimension.[Warehouse Square Footage]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r11_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::NearMissLevelLabel { .. }))
            .count();
        // Suppressed: "footage" shares no content token with "feet" after prefix exclusion.
        assert_eq!(r11_count, 0, "RULE 11 must be suppressed for 'Square Footage' vs 'Square Feet' (no shared suffix token); got: {rejections:?}");
    }

    #[test]
    fn rule11_exact_label_silent() {
        // AC2: exact label "Warehouse Square Feet" → RULE 11 must stay silent.
        let catalog = rule11_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "warehouse_dimension.[Warehouse Square Feet]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r11_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::NearMissLevelLabel { .. }))
            .count();
        assert_eq!(r11_count, 0, "AC2: exact label must not trigger RULE 11; got: {rejections:?}");
    }

    #[test]
    fn rule11_two_near_matches_silent() {
        // AC3: two dimension-local levels within threshold → count-based trigger not met → silent.
        let catalog = CatalogSnapshot {
            measures: vec![],
            dimensions: vec![],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "warehouse_dimension".into(),
                hierarchy_unique_name: "warehouse_dimension".into(),
                levels: vec!["Warehouse Square Feet".into(), "Warehouse Square Foot".into()],
                ..Default::default()
            }],
            date_roles: vec![],
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "warehouse_dimension.[Warehouse Square Footage]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r11_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::NearMissLevelLabel { .. }))
            .count();
        assert_eq!(r11_count, 0, "AC3: two near-matches must keep RULE 11 silent; got: {rejections:?}");
    }

    #[test]
    fn rule11_unresolvable_prefix_silent() {
        // AC6: prefix resolves to no dimension → RULE 11 silent.
        let catalog = rule11_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "unknown_dim.[Warehouse Square Footage]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r11_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::NearMissLevelLabel { .. }))
            .count();
        assert_eq!(r11_count, 0, "AC6: unresolvable prefix must keep RULE 11 silent; got: {rejections:?}");
    }

    // -----------------------------------------------------------------------
    // RULE 11 token-overlap guard tests (PRD-mqo-nearmiss-label-token-overlap-guard)
    // -----------------------------------------------------------------------

    /// Catalog for the token-overlap guard tests: warehouse dim with two levels —
    /// "Warehouse Square Feet" (the correct target) and "Warehouse State" (the antonym
    /// that was previously misfired by the guard).
    fn rule11_overlap_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![CatalogMeasure {
                unique_name: "Net Profit".into(),
                ..Default::default()
            }],
            dimensions: vec![CatalogDimension {
                unique_name: "warehouse_dimension".into(),
                subject_areas: vec![],
            }],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "warehouse_dimension".into(),
                hierarchy_unique_name: "warehouse_dimension".into(),
                levels: vec![
                    "Warehouse Name".into(),
                    "Warehouse Square Feet".into(),
                    "Warehouse State".into(),
                ],
                level_meta: vec![],
            }],
            date_roles: vec![],
        }
    }

    #[test]
    fn rule11_token_overlap_ac1_warehouse_state_suppressed() {
        // AC1: "Warehouse Square Feet" supplied, "Warehouse State" is a JW-near-miss
        // but shares NO content token after prefix exclusion → must be suppressed.
        // The misfire payload was:
        //   NearMissLevelLabel { canonical: "Warehouse State", similarity: 0.9057 }
        let catalog = rule11_overlap_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "warehouse_dimension.[Warehouse Square Feet]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        // Must not emit NearMissLevelLabel suggesting "Warehouse State".
        for r in &rejections {
            if let RejectReason::NearMissLevelLabel { canonical, supplied, .. } = &r.reason {
                assert_ne!(
                    canonical.as_str(), "Warehouse State",
                    "AC1: NearMissLevelLabel must NOT suggest 'Warehouse State' for supplied '{supplied}'"
                );
            }
        }
    }

    #[test]
    fn rule11_token_overlap_ac2_typo_still_fires() {
        // AC2: "Warehouse Sq Feet" (typo) vs canonical "Warehouse Square Feet".
        // Shared content token "feet" (and possibly "square"/"sq") → must still fire.
        let catalog = rule11_overlap_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "warehouse_dimension.[Warehouse Sq Feet]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r11: Vec<_> = rejections
            .iter()
            .filter(|r| matches!(&r.reason, RejectReason::NearMissLevelLabel { .. }))
            .collect();
        assert_eq!(
            r11.len(), 1,
            "AC2: near-miss must still fire for 'Warehouse Sq Feet' → 'Warehouse Square Feet'; got: {rejections:?}"
        );
        if let RejectReason::NearMissLevelLabel { canonical, .. } = &r11[0].reason {
            assert_eq!(canonical, "Warehouse Square Feet", "AC2: canonical must be 'Warehouse Square Feet'");
        }
    }

    #[test]
    fn rule11_token_overlap_ac5_single_word_no_overlap_suppressed() {
        // AC5 (edge): one-word supplied label "Carrier" vs one-word canonical "State"
        // sharing no token → must be suppressed, not character-matched.
        let catalog = CatalogSnapshot {
            measures: vec![],
            dimensions: vec![CatalogDimension {
                unique_name: "carrier_dimension".into(),
                subject_areas: vec![],
            }],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "carrier_dimension".into(),
                hierarchy_unique_name: "carrier_dimension".into(),
                levels: vec!["Carrier State".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        };
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "carrier_dimension.[Carrier]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        for r in &rejections {
            assert!(
                !matches!(&r.reason, RejectReason::NearMissLevelLabel { .. }),
                "AC5: NearMissLevelLabel must be suppressed for single-token disjoint labels; got: {r:?}"
            );
        }
    }

    #[test]
    fn rule11_token_overlap_ac4_overlap_passes_disjoint_suppressed() {
        // AC4 (FR3): when multiple level candidates are within the JW threshold,
        // those sharing NO content token (after prefix exclusion) are suppressed,
        // leaving the token-sharing candidate as the sole winner.
        //
        // Supplied: "Warehouse Sq Feet" (abbreviation of "Square").
        // Catalog levels: "Warehouse Square Feet" (shares "feet") and "Warehouse State"
        // (shares no suffix token).  "Warehouse Square Feet" passes the overlap gate;
        // "Warehouse State" is suppressed → exactly one hit → fires with correct canonical.
        let catalog = CatalogSnapshot {
            measures: vec![CatalogMeasure {
                unique_name: "Net Profit".into(),
                ..Default::default()
            }],
            dimensions: vec![CatalogDimension {
                unique_name: "warehouse_dimension".into(),
                subject_areas: vec![],
            }],
            hierarchies: vec![CatalogHierarchy {
                dimension_unique_name: "warehouse_dimension".into(),
                hierarchy_unique_name: "warehouse_dimension".into(),
                levels: vec![
                    "Warehouse Square Feet".into(),
                    "Warehouse State".into(),
                ],
                level_meta: vec![],
            }],
            date_roles: vec![],
        };
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "warehouse_dimension.[Warehouse Sq Feet]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r11: Vec<_> = rejections
            .iter()
            .filter(|r| matches!(&r.reason, RejectReason::NearMissLevelLabel { .. }))
            .collect();
        // "Warehouse State" suppressed (no suffix token overlap); "Warehouse Square Feet"
        // passes (shared "feet") → exactly one hit.
        assert_eq!(r11.len(), 1, "AC4: overlap-passing candidate fires while disjoint is suppressed; got: {rejections:?}");
        if let RejectReason::NearMissLevelLabel { canonical, .. } = &r11[0].reason {
            assert_eq!(canonical, "Warehouse Square Feet", "AC4: token-sharing canonical wins");
        }
    }

    // -----------------------------------------------------------------------
    // Unit tests for content-token helpers (FR1 internals)
    // -----------------------------------------------------------------------

    #[test]
    fn content_tokens_drops_stopwords() {
        let toks = content_tokens("Warehouse Square Feet");
        assert!(toks.contains(&"warehouse".to_string()));
        assert!(toks.contains(&"square".to_string()));
        assert!(toks.contains(&"feet".to_string()));
    }

    #[test]
    fn content_token_overlap_ok_disjoint_prefix_suppressed() {
        // "Warehouse Square Feet" vs "Warehouse State": common prefix "warehouse"
        // excluded → {"square","feet"} ∩ {"state"} = ∅ → not ok.
        assert!(
            !content_token_overlap_ok("Warehouse Square Feet", "Warehouse State"),
            "disjoint suffix tokens must not pass the overlap gate"
        );
    }

    #[test]
    fn content_token_overlap_ok_shared_token_passes() {
        // "Warehouse Sq Feet" vs "Warehouse Square Feet": common prefix "warehouse"
        // excluded → {"sq","feet"} ∩ {"square","feet"} = {"feet"} → ok.
        assert!(
            content_token_overlap_ok("Warehouse Sq Feet", "Warehouse Square Feet"),
            "shared 'feet' token must pass the overlap gate"
        );
    }

    #[test]
    fn content_token_overlap_ok_single_word_disjoint_suppressed() {
        // Single-word supplied "Carrier" vs "State": no shared token → not ok.
        assert!(
            !content_token_overlap_ok("Carrier", "State"),
            "single-word disjoint labels must not pass the overlap gate"
        );
    }

}

// ---------------------------------------------------------------------------
// RULE 12: role-confusion guard (PRD-mqo-grounding-enforcement-dedup)
// ---------------------------------------------------------------------------

/// Check whether the MQO places a catalog entity in the wrong slot:
/// a `kind=measure` name in the `dimensions` list, or a `kind=level/hierarchy`
/// name in the `measures` list.
///
/// Conservative (FR2 — zero false positives):
/// - Only fires when the name resolves *unambiguously* to the wrong kind AND
///   does NOT also match the correct kind (ambiguous names defer to the binder).
/// - A name not found in either catalog partition is ignored (let the binder
///   surface the not-found error).
/// - Empty measures/dimensions lists are a no-op.
pub(crate) fn check_role_confusion(
    mqo: &BoundMqoInput,
    catalog: &CatalogSnapshot,
    rejections: &mut Vec<ParamRejection>,
) {
    // Pre-build normalized sets for O(1) lookup.
    let measure_names: std::collections::HashSet<String> = catalog
        .measures
        .iter()
        .flat_map(|m| {
            let mut names = vec![normalize(&m.unique_name)];
            if let Some(label) = &m.label {
                names.push(normalize(label));
            }
            names
        })
        .collect();

    let level_names: std::collections::HashSet<String> = catalog
        .hierarchies
        .iter()
        .flat_map(|h| h.levels.iter().map(|l| normalize(l)))
        .collect();

    // Check: measure slot contains a name that is a catalog level (not a measure).
    for mref in &mqo.measures {
        let norm = normalize(&mref.unique_name);
        let is_measure = measure_names.contains(&norm);
        let is_level = level_names.contains(&norm);
        if is_level && !is_measure {
            // Unambiguously a level in the measures slot.
            rejections.push(ParamRejection::new(
                mref.unique_name.clone(),
                FieldClass::Measure,
                RejectReason::RoleConfusion {
                    entity: mref.unique_name.clone(),
                    actual_kind: "level".to_string(),
                    correct_slot: "dimensions".to_string(),
                },
                vec![Suggestion {
                    name: format!("move [{}] to the dimensions list", mref.unique_name),
                    similarity: 1.0,
                    note: Some(format!(
                        "[{}] is a dimension attribute (level), not a measure; \
                         place it in the dimensions slot and use a real measure \
                         in the measures slot.",
                        mref.unique_name
                    )),
                }],
            ));
        }
    }

    // Check: dimensions slot contains a name that is a catalog measure (not a level).
    for dref in &mqo.dimensions {
        let norm = normalize(&dref.unique_name);
        let is_measure = measure_names.contains(&norm);
        let is_level = level_names.contains(&norm);
        if is_measure && !is_level {
            // Unambiguously a measure in the dimensions slot.
            rejections.push(ParamRejection::new(
                dref.unique_name.clone(),
                FieldClass::Dimension,
                RejectReason::RoleConfusion {
                    entity: dref.unique_name.clone(),
                    actual_kind: "measure".to_string(),
                    correct_slot: "measures".to_string(),
                },
                vec![Suggestion {
                    name: format!("move [{}] to the measures list", dref.unique_name),
                    similarity: 1.0,
                    note: Some(format!(
                        "[{}] is a measure, not a dimension attribute; \
                         place it in the measures slot.",
                        dref.unique_name
                    )),
                }],
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 12 tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod rule12_tests {
    use super::*;

    fn rule12_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            measures: vec![
                CatalogMeasure {
                    unique_name: "Store Sales".into(),
                    label: Some("Store Sales".into()),
                    ..Default::default()
                },
                CatalogMeasure {
                    unique_name: "Total Quantity Sold".into(),
                    label: Some("Total Quantity Sold".into()),
                    ..Default::default()
                },
            ],
            dimensions: vec![
                CatalogDimension {
                    unique_name: "Store".into(),
                    subject_areas: vec![],
                },
            ],
            hierarchies: vec![
                CatalogHierarchy {
                    dimension_unique_name: "Store".into(),
                    hierarchy_unique_name: "Store".into(),
                    levels: vec!["Store Name".into(), "Store City".into()],
                    level_meta: vec![],
                },
            ],
            date_roles: vec![],
        }
    }

    // AC1: measure used as dimension → RULE 12 fires with RoleConfusion(actual_kind=measure)
    #[test]
    fn ac1_measure_as_dimension_rejected() {
        let catalog = rule12_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Store Sales".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Total Quantity Sold".into(), // measure in dimensions slot
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r12: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::RoleConfusion { actual_kind, .. } if actual_kind == "measure"))
            .collect();
        assert_eq!(r12.len(), 1, "AC1: measure-as-dimension must fire RULE 12; got: {rejections:?}");
        assert_eq!(r12[0].field, "Total Quantity Sold");
    }

    // AC2: level used as measure → RULE 12 fires with RoleConfusion(actual_kind=level)
    #[test]
    fn ac2_level_as_measure_rejected() {
        let catalog = rule12_catalog();
        let mqo = BoundMqoInput {
            measures: vec![
                MqoMeasureRef { unique_name: "Store Name".into(), aggregation: None }, // level in measures slot
            ],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Store City".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r12: Vec<_> = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::RoleConfusion { actual_kind, .. } if actual_kind == "level"))
            .collect();
        assert_eq!(r12.len(), 1, "AC2: level-as-measure must fire RULE 12; got: {rejections:?}");
        assert_eq!(r12[0].field, "Store Name");
    }

    // AC3: ambiguous label (matches both a measure and a level) → RULE 12 silent
    #[test]
    fn ac3_ambiguous_label_silent() {
        let mut catalog = rule12_catalog();
        // Add a level with the same name as a measure to create ambiguity.
        catalog.hierarchies[0].levels.push("Store Sales".into());
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store Sales".into(), // ambiguous: also a measure
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r12_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::RoleConfusion { .. }))
            .count();
        assert_eq!(r12_count, 0, "AC3: ambiguous label must keep RULE 12 silent; got: {rejections:?}");
    }

    // AC4: correct usage → RULE 12 silent
    #[test]
    fn ac4_correct_usage_silent() {
        let catalog = rule12_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Store Sales".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "Store".into(),
                level: Some("Store Name".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r12_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::RoleConfusion { .. }))
            .count();
        assert_eq!(r12_count, 0, "AC4: correct usage must keep RULE 12 silent; got: {rejections:?}");
    }

    // AC5: unresolved name → RULE 12 silent (defer to binder)
    #[test]
    fn ac5_unresolved_name_silent() {
        let catalog = rule12_catalog();
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "UnknownMeasure".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "UnknownDim".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let r12_count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::RoleConfusion { .. }))
            .count();
        assert_eq!(r12_count, 0, "AC5: unresolved name must keep RULE 12 silent; got: {rejections:?}");
    }

    // ── RULE 8 unique_name bracket guard (PRD-mqo-unique-name-bracket-label-guard) ─

    /// PRD AC1: midway-stores case — bracket "Floor Space" in unique_name with level=None
    /// fires NonCanonicalLevelLabel with corrected unique_name in the suggestion.
    #[test]
    fn prd_bracket_guard_ac1_floor_space_corrected_unique_name() {
        // Catalog: Store dimension with "Store Floor Space" level (the canonical).
        let catalog = CatalogSnapshot {
            measures: vec![CatalogMeasure {
                unique_name: "Net Profit".into(),
                label: Some("Net Profit".into()),
                ..Default::default()
            }],
            dimensions: vec![CatalogDimension { unique_name: "store_dimension".into(), subject_areas: vec![] }],
            hierarchies: vec![CatalogHierarchy {
                hierarchy_unique_name: "store_dimension".into(),
                dimension_unique_name: "store_dimension".into(),
                levels: vec!["Store Name".into(), "Store Manager".into(), "Store Floor Space".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        };
        // Agent sends: unique_name = "store_dimension.[Floor Space]", level = None
        // (the bracket-form bypass this PRD targets).
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Floor Space]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rule8 = rejections.iter().find(|r| {
            matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })
        });
        assert!(rule8.is_some(), "PRD AC1: bracket 'Floor Space' must fire RULE 8; got: {rejections:?}");
        if let Some(r) = rule8 {
            if let RejectReason::NonCanonicalLevelLabel { supplied, canonical } = &r.reason {
                assert_eq!(supplied, "Floor Space", "supplied must be 'Floor Space'");
                assert_eq!(canonical, "Store Floor Space", "canonical must be 'Store Floor Space'");
            }
            // FR3: suggestion must carry the corrected unique_name.
            assert!(
                r.suggestions.iter().any(|s| s.name == "store_dimension.[Store Floor Space]"),
                "PRD FR3: suggestion must be corrected unique_name 'store_dimension.[Store Floor Space]'; got: {:?}",
                r.suggestions
            );
        }
    }

    /// PRD AC5 / store-employee-counts case: bracket "Number of Employees" →
    /// "Store Number of Employees" with corrected unique_name suggestion.
    #[test]
    fn prd_bracket_guard_ac5_number_of_employees_corrected_unique_name() {
        // Catalog: Store dimension with "Store Number of Employees" level (the canonical).
        let catalog = CatalogSnapshot {
            measures: vec![],
            dimensions: vec![CatalogDimension { unique_name: "store_dimension".into(), subject_areas: vec![] }],
            hierarchies: vec![CatalogHierarchy {
                hierarchy_unique_name: "store_dimension".into(),
                dimension_unique_name: "store_dimension".into(),
                levels: vec!["Store Name".into(), "Store Number of Employees".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        };
        // Agent sends: unique_name = "store_dimension.[Number of Employees]", level = None
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Number of Employees]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let rule8 = rejections.iter().find(|r| {
            matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })
        });
        assert!(rule8.is_some(), "PRD AC5: '[Number of Employees]' must fire RULE 8; got: {rejections:?}");
        if let Some(r) = rule8 {
            if let RejectReason::NonCanonicalLevelLabel { supplied, canonical } = &r.reason {
                assert_eq!(supplied, "Number of Employees");
                assert_eq!(canonical, "Store Number of Employees");
            }
            assert!(
                r.suggestions.iter().any(|s| s.name == "store_dimension.[Store Number of Employees]"),
                "PRD FR3: suggestion must carry corrected unique_name; got: {:?}", r.suggestions
            );
        }
    }

    /// PRD AC7 / FR5: zero-match bracket label defers to Unmapped, RULE 8 silent.
    #[test]
    fn prd_bracket_guard_ac7_zero_match_defers_to_unmapped() {
        let catalog = CatalogSnapshot {
            measures: vec![],
            dimensions: vec![CatalogDimension { unique_name: "store_dimension".into(), subject_areas: vec![] }],
            hierarchies: vec![CatalogHierarchy {
                hierarchy_unique_name: "store_dimension".into(),
                dimension_unique_name: "store_dimension".into(),
                levels: vec!["Store Name".into(), "Store Floor Space".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        };
        // "Footage" is not a suffix of any catalog level (cf. NG1 fairview-warehouses).
        let mqo = BoundMqoInput {
            measures: vec![],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Square Footage]".into(),
                level: None,
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        assert!(
            rejections.iter().all(|r| !matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. })),
            "PRD AC7/FR5: zero-match bracket must not fire RULE 8; got: {rejections:?}"
        );
    }

    /// PRD AC8 / FR7 de-dup: both dref.level and bracket trigger same canonical → one rejection.
    #[test]
    fn prd_bracket_guard_ac8_dedup_level_and_bracket_one_rejection() {
        let catalog = CatalogSnapshot {
            measures: vec![CatalogMeasure { unique_name: "Net Profit".into(), ..Default::default() }],
            dimensions: vec![CatalogDimension { unique_name: "store_dimension".into(), subject_areas: vec![] }],
            hierarchies: vec![CatalogHierarchy {
                hierarchy_unique_name: "store_dimension".into(),
                dimension_unique_name: "store_dimension".into(),
                levels: vec!["Store Name".into(), "Store Floor Space".into()],
                level_meta: vec![],
            }],
            date_roles: vec![],
        };
        // Both level="Floor Space" AND unique_name bracket "[Floor Space]" — same canonical → 1 rejection.
        let mqo = BoundMqoInput {
            measures: vec![MqoMeasureRef { unique_name: "Net Profit".into(), aggregation: None }],
            dimensions: vec![MqoDimensionRef {
                unique_name: "store_dimension.[Floor Space]".into(),
                level: Some("Floor Space".into()),
                ..Default::default()
            }],
            filters: vec![],
        };
        let rejections = validate(&mqo, &catalog);
        let count = rejections.iter()
            .filter(|r| matches!(&r.reason, RejectReason::NonCanonicalLevelLabel { .. }))
            .count();
        assert_eq!(count, 1, "PRD AC8/FR7: must emit exactly 1 rejection when level+bracket agree; got: {rejections:?}");
    }
}
