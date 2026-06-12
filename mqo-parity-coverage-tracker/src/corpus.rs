//! Corpus types: the `parity-corpus.v1` denominator consumed by `coverage` and `delta`.
//!
//! This mirrors the output contract of `mqo-parity-corpus-gen` and is
//! kept intentionally minimal — just what the coverage denominator needs.

use serde::Deserialize;

/// A single parity case in the corpus denominator.
#[derive(Debug, Clone, Deserialize)]
pub struct CorpusCase {
    /// Deterministic id derived from measure unique-name(s) + grain.
    pub case_id: String,
    /// The measure unique-name(s) covered by this case.
    pub measures: Vec<String>,
    /// The grain token (`total`, or a level slug like `year`/`region`).
    pub grain: String,
    /// True iff the measure is `is_calc` or `semi_additive`.
    pub has_grounding: bool,
}

/// The top-level `parity-corpus.v1` document.
#[derive(Debug, Clone, Deserialize)]
pub struct Corpus {
    /// Format version tag; consumers MUST key off this.
    pub version: String,
    /// The catalog the cases run against.
    pub catalog: String,
    /// The parity cases, sorted by `case_id`.
    pub cases: Vec<CorpusCase>,
}

impl Corpus {
    /// Parse a `parity-corpus.v1` JSON document.
    ///
    /// # Errors
    ///
    /// Returns a `serde_json::Error` if the input is not valid JSON or does not
    /// match the expected schema.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Collect all (measure, backend_pair) cells in the corpus using a fixed
    /// backend pair.  The corpus does not carry backend-pair information (it is
    /// backend-agnostic), so the caller passes the pair of interest.
    ///
    /// Returns a `Vec` of `(measure, backend_a, backend_b)` keys.
    #[must_use]
    pub fn cells_for_pair(&self, backend_a: &str, backend_b: &str) -> Vec<(String, String, String)> {
        let mut cells = Vec::new();
        for case in &self.cases {
            for measure in &case.measures {
                cells.push((measure.clone(), backend_a.to_string(), backend_b.to_string()));
            }
        }
        cells
    }

    /// Collect all unique measure names from the corpus.
    #[must_use]
    pub fn all_measures(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for case in &self.cases {
            for m in &case.measures {
                if seen.insert(m.clone()) {
                    out.push(m.clone());
                }
            }
        }
        out
    }
}
