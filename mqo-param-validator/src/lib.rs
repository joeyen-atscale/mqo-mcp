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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogHierarchy {
    pub dimension_unique_name: String,
    pub hierarchy_unique_name: String,
    pub levels: Vec<String>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MqoMeasureRef {
    pub unique_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MqoFilterRef {
    pub unique_name: String,
    #[serde(default)]
    pub level: Option<String>,
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

    rejections
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
