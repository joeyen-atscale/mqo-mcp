use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(Debug, Deserialize)]
struct EnrichedColumnRaw {
    unique_name: String,
    #[serde(default)]
    column_group: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
struct EnrichedCatalogRaw {
    columns: Vec<EnrichedColumnRaw>,
}

/// Lookup table: `unique_name` → sorted column-group set, loaded from `enriched-catalog.v1`.
///
/// Absent entry → treated as conformed (empty set, never flagged) per NG4 fail-open.
#[derive(Debug, Default)]
pub struct EnrichedColumnGroups(HashMap<String, BTreeSet<String>>);

impl EnrichedColumnGroups {
    /// Load from an `enriched-catalog.v1` JSON file.
    ///
    /// # Errors
    /// Returns `Err` with a diagnostic string if the file cannot be read or
    /// does not parse. Callers MUST fail loudly on `Err` (NFR4).
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read --enriched-catalog '{}': {e}", path.display()))?;
        let raw: EnrichedCatalogRaw = serde_json::from_str(&text).map_err(|e| {
            format!(
                "--enriched-catalog '{}' is not valid enriched-catalog.v1 JSON: {e}",
                path.display()
            )
        })?;
        let map = raw
            .columns
            .into_iter()
            .map(|c| (c.unique_name, c.column_group))
            .collect();
        Ok(Self(map))
    }

    /// Column-group set for `unique_name`. Empty set when absent (conformed, fail-open).
    #[must_use]
    pub fn groups_for(&self, unique_name: &str) -> &BTreeSet<String> {
        static EMPTY: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
        self.0
            .get(unique_name)
            .unwrap_or_else(|| EMPTY.get_or_init(BTreeSet::new))
    }

    /// True when a column-group set represents a conformed entity:
    /// empty set (no binding) or contains the wildcard `"*"`.
    #[must_use]
    pub fn is_conformed(groups: &BTreeSet<String>) -> bool {
        groups.is_empty() || groups.contains("*")
    }
}
