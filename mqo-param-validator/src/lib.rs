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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogMeasure {
    pub unique_name: String,
    /// Optional subject area / fact name for cross-fact detection
    #[serde(default)]
    pub subject_area: Option<String>,
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
            rejections.push(ParamRejection {
                field: mref.unique_name.clone(),
                class: FieldClass::Measure,
                reason: RejectReason::Unmapped,
                suggestions,
            });
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
            rejections.push(ParamRejection {
                field: dref.unique_name.clone(),
                class: FieldClass::Dimension,
                reason: RejectReason::Unmapped,
                suggestions,
            });
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
                        rejections.push(ParamRejection {
                            field: level.clone(),
                            class: FieldClass::HierarchyLevel,
                            reason: RejectReason::WrongHierarchyLevel,
                            suggestions,
                        });
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
            rejections.push(ParamRejection {
                field: fref.unique_name.clone(),
                class: FieldClass::Filter,
                reason: RejectReason::Unmapped,
                suggestions,
            });
        }
    }

    // AC5: cross-fact path detection
    check_cross_fact_paths(mqo, catalog, &mut rejections);

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
        rejections.push(ParamRejection {
            field: dref.unique_name.clone(),
            class: FieldClass::DateRole,
            reason: RejectReason::AmbiguousDateRole,
            suggestions,
        });
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
                    rejections.push(ParamRejection {
                        field: field_key,
                        class: FieldClass::Dimension,
                        reason: RejectReason::CrossFactPath,
                        suggestions: vec![Suggestion {
                            name: measure_sa.clone(),
                            similarity: 0.0,
                            note: Some(format!(
                                "measure [{}] belongs to subject area [{}]; \
                                 dimension [{}] is not available there",
                                mref.unique_name, measure_sa, dref.unique_name
                            )),
                        }],
                    });
                }
            }
        }
    }
}
