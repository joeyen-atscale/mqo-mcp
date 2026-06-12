//! Core diff logic: compare two DescribeModel catalogs and produce a DiffReport.

use crate::catalog::{DescribeModel, Dimension, Measure};
use crate::report::{
    ClusterInfo, DiffReport, EntityDiff, FieldDiff, OnlyEntry, OverallVerdict, Summary, Verdict,
};
use std::collections::HashMap;

/// Configuration for the diff run.
pub struct DiffConfig {
    pub cluster_a: String,
    pub cluster_b: String,
    /// Fraction tolerance for numeric field comparison (0.0 = exact).
    /// Not currently used for string fields; reserved for future numeric stats.
    pub numeric_tolerance: f64,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Compare two describe_model catalogs and return a structured [`DiffReport`].
pub fn diff_catalogs(
    a: &DescribeModel,
    b: &DescribeModel,
    config: &DiffConfig,
) -> DiffReport {
    let mut differences: Vec<EntityDiff> = Vec::new();
    let mut only_in_a: Vec<OnlyEntry> = Vec::new();
    let mut only_in_b: Vec<OnlyEntry> = Vec::new();

    // Collect all measures and dimensions from all models, keyed by unique_name.
    let measures_a = collect_measures(a);
    let measures_b = collect_measures(b);
    let dims_a = collect_dimensions(a);
    let dims_b = collect_dimensions(b);

    // --- diff measures ---
    diff_measures(
        &measures_a,
        &measures_b,
        config,
        &mut differences,
        &mut only_in_a,
        &mut only_in_b,
    );

    // --- diff dimensions ---
    diff_dimensions(
        &dims_a,
        &dims_b,
        &mut differences,
        &mut only_in_a,
        &mut only_in_b,
    );

    // --- build summary ---
    let mut summary = Summary {
        only_in_a: only_in_a.len(),
        only_in_b: only_in_b.len(),
        ..Summary::default()
    };

    for d in &differences {
        match d.verdict {
            Verdict::Agree => summary.agree += 1,
            Verdict::Diverge => summary.diverge += 1,
            Verdict::CriticalDiverge => summary.critical_diverge += 1,
        }
    }

    // --- overall verdict ---
    let overall_verdict = if summary.critical_diverge > 0 {
        OverallVerdict::CriticalDiverge
    } else if summary.diverge > 0 || !only_in_a.is_empty() || !only_in_b.is_empty() {
        OverallVerdict::Diverge
    } else {
        OverallVerdict::Agree
    };

    DiffReport {
        clusters: ClusterInfo {
            a: config.cluster_a.clone(),
            b: config.cluster_b.clone(),
        },
        summary,
        differences,
        only_in_a,
        only_in_b,
        overall_verdict,
    }
}

// ---------------------------------------------------------------------------
// Collect entities
// ---------------------------------------------------------------------------

fn collect_measures(catalog: &DescribeModel) -> HashMap<String, Measure> {
    let mut map = HashMap::new();
    for model in &catalog.models {
        for m in &model.measures {
            map.entry(m.unique_name.clone()).or_insert_with(|| m.clone());
        }
    }
    map
}

fn collect_dimensions(catalog: &DescribeModel) -> HashMap<String, Dimension> {
    let mut map = HashMap::new();
    for model in &catalog.models {
        for d in &model.dimensions {
            map.entry(d.unique_name.clone()).or_insert_with(|| d.clone());
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Diff measures
// ---------------------------------------------------------------------------

fn diff_measures(
    a_map: &HashMap<String, Measure>,
    b_map: &HashMap<String, Measure>,
    config: &DiffConfig,
    differences: &mut Vec<EntityDiff>,
    only_in_a: &mut Vec<OnlyEntry>,
    only_in_b: &mut Vec<OnlyEntry>,
) {
    // Entities only in A
    for k in a_map.keys() {
        if !b_map.contains_key(k) {
            only_in_a.push(OnlyEntry {
                entity_type: "measure".to_string(),
                unique_name: k.clone(),
            });
        }
    }

    // Entities only in B
    for k in b_map.keys() {
        if !a_map.contains_key(k) {
            only_in_b.push(OnlyEntry {
                entity_type: "measure".to_string(),
                unique_name: k.clone(),
            });
        }
    }

    // Entities in both — compare fields
    for (k, ma) in a_map {
        if let Some(mb) = b_map.get(k) {
            let ed = compare_measures(k, ma, mb, config);
            differences.push(ed);
        }
    }
}

fn compare_measures(
    unique_name: &str,
    a: &Measure,
    b: &Measure,
    _config: &DiffConfig,
) -> EntityDiff {
    let mut field_diffs: Vec<FieldDiff> = Vec::new();

    compare_opt_str_field(
        "name",
        a.name.as_deref(),
        b.name.as_deref(),
        false,
        &mut field_diffs,
    );
    compare_opt_str_field(
        "expression",
        a.expression.as_deref(),
        b.expression.as_deref(),
        true,
        &mut field_diffs,
    );
    compare_opt_str_field(
        "folder",
        a.folder.as_deref(),
        b.folder.as_deref(),
        false,
        &mut field_diffs,
    );
    compare_opt_str_field(
        "format_string",
        a.format_string.as_deref(),
        b.format_string.as_deref(),
        false,
        &mut field_diffs,
    );
    compare_opt_str_field(
        "aggregation_type",
        a.aggregation_type.as_deref(),
        b.aggregation_type.as_deref(),
        true,
        &mut field_diffs,
    );

    let verdict = verdict_from_field_diffs(&field_diffs);

    EntityDiff {
        entity_type: "measure".to_string(),
        unique_name: unique_name.to_string(),
        verdict,
        field_diffs,
    }
}

// ---------------------------------------------------------------------------
// Diff dimensions
// ---------------------------------------------------------------------------

fn diff_dimensions(
    a_map: &HashMap<String, Dimension>,
    b_map: &HashMap<String, Dimension>,
    differences: &mut Vec<EntityDiff>,
    only_in_a: &mut Vec<OnlyEntry>,
    only_in_b: &mut Vec<OnlyEntry>,
) {
    for k in a_map.keys() {
        if !b_map.contains_key(k) {
            only_in_a.push(OnlyEntry {
                entity_type: "dimension".to_string(),
                unique_name: k.clone(),
            });
        }
    }

    for k in b_map.keys() {
        if !a_map.contains_key(k) {
            only_in_b.push(OnlyEntry {
                entity_type: "dimension".to_string(),
                unique_name: k.clone(),
            });
        }
    }

    for (k, da) in a_map {
        if let Some(db) = b_map.get(k) {
            let ed = compare_dimensions(k, da, db);
            differences.push(ed);
        }
    }
}

fn compare_dimensions(unique_name: &str, a: &Dimension, b: &Dimension) -> EntityDiff {
    let mut field_diffs: Vec<FieldDiff> = Vec::new();

    compare_opt_str_field(
        "name",
        a.name.as_deref(),
        b.name.as_deref(),
        false,
        &mut field_diffs,
    );
    compare_opt_str_field(
        "expression",
        a.expression.as_deref(),
        b.expression.as_deref(),
        true,
        &mut field_diffs,
    );
    compare_opt_str_field(
        "folder",
        a.folder.as_deref(),
        b.folder.as_deref(),
        false,
        &mut field_diffs,
    );
    compare_opt_str_field(
        "format_string",
        a.format_string.as_deref(),
        b.format_string.as_deref(),
        false,
        &mut field_diffs,
    );

    let verdict = verdict_from_field_diffs(&field_diffs);

    EntityDiff {
        entity_type: "dimension".to_string(),
        unique_name: unique_name.to_string(),
        verdict,
        field_diffs,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compare_opt_str_field(
    field: &str,
    a_val: Option<&str>,
    b_val: Option<&str>,
    critical: bool,
    diffs: &mut Vec<FieldDiff>,
) {
    if a_val != b_val {
        diffs.push(FieldDiff {
            field: field.to_string(),
            cluster_a: a_val.map(str::to_string),
            cluster_b: b_val.map(str::to_string),
            critical,
        });
    }
}

fn verdict_from_field_diffs(diffs: &[FieldDiff]) -> Verdict {
    if diffs.is_empty() {
        Verdict::Agree
    } else if diffs.iter().any(|d| d.critical) {
        Verdict::CriticalDiverge
    } else {
        Verdict::Diverge
    }
}

// ---------------------------------------------------------------------------
// Exit code helper
// ---------------------------------------------------------------------------

/// Map an [`OverallVerdict`] to a process exit code.
/// 0 = agree, 1 = diverge, 2 = critical_diverge.
pub fn exit_code(verdict: &OverallVerdict) -> i32 {
    match verdict {
        OverallVerdict::Agree => 0,
        OverallVerdict::Diverge => 1,
        OverallVerdict::CriticalDiverge => 2,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{DescribeModel, Measure, Model};

    fn make_catalog_with_measure(
        model_name: &str,
        unique_name: &str,
        expression: Option<&str>,
        aggregation_type: Option<&str>,
        folder: Option<&str>,
    ) -> DescribeModel {
        DescribeModel {
            models: vec![Model {
                unique_name: model_name.to_string(),
                name: None,
                provenance: None,
                measures: vec![Measure {
                    unique_name: unique_name.to_string(),
                    name: Some(unique_name.to_string()),
                    expression: expression.map(str::to_string),
                    folder: folder.map(str::to_string),
                    format_string: None,
                    aggregation_type: aggregation_type.map(str::to_string),
                    extra: Default::default(),
                }],
                dimensions: vec![],
                extra: Default::default(),
            }],
            extra: Default::default(),
        }
    }

    #[test]
    fn identical_measures_agree() {
        let a = make_catalog_with_measure("m", "Total Sales", Some("[Sales]"), Some("SUM"), None);
        let b = make_catalog_with_measure("m", "Total Sales", Some("[Sales]"), Some("SUM"), None);
        let config = DiffConfig {
            cluster_a: "prod".into(),
            cluster_b: "staging".into(),
            numeric_tolerance: 0.001,
        };
        let report = diff_catalogs(&a, &b, &config);
        assert_eq!(report.overall_verdict, OverallVerdict::Agree);
        assert_eq!(report.summary.agree, 1);
        assert_eq!(report.summary.diverge, 0);
        assert_eq!(report.summary.critical_diverge, 0);
    }

    #[test]
    fn different_expression_is_critical_diverge() {
        let a = make_catalog_with_measure("m", "Total Sales", Some("[Sales]"), Some("SUM"), None);
        let b = make_catalog_with_measure("m", "Total Sales", Some("[Revenue]"), Some("SUM"), None);
        let config = DiffConfig {
            cluster_a: "prod".into(),
            cluster_b: "staging".into(),
            numeric_tolerance: 0.001,
        };
        let report = diff_catalogs(&a, &b, &config);
        assert_eq!(report.overall_verdict, OverallVerdict::CriticalDiverge);
        assert_eq!(report.summary.critical_diverge, 1);
    }

    #[test]
    fn different_folder_is_diverge_not_critical() {
        let a = make_catalog_with_measure(
            "m",
            "Total Sales",
            Some("[Sales]"),
            Some("SUM"),
            Some("Sales Folder"),
        );
        let b = make_catalog_with_measure(
            "m",
            "Total Sales",
            Some("[Sales]"),
            Some("SUM"),
            Some("Other Folder"),
        );
        let config = DiffConfig {
            cluster_a: "prod".into(),
            cluster_b: "staging".into(),
            numeric_tolerance: 0.001,
        };
        let report = diff_catalogs(&a, &b, &config);
        assert_eq!(report.overall_verdict, OverallVerdict::Diverge);
        assert_eq!(report.summary.diverge, 1);
        assert_eq!(report.summary.critical_diverge, 0);
    }
}
