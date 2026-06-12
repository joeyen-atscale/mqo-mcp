//! `mcp-spike-evidence-bundle` — core types and verdict-assignment logic.
//!
//! This crate ingests the four sibling artifacts (session footprint, paramq
//! bench report, walkthrough transcript, handle-demo) plus a ticket-map JSON,
//! scores each ticket AC against its expected artifact, and emits a
//! `spike_evidence.json` with per-AC verdicts and per-ticket summary counts.
//!
//! # Design
//! - Pure aggregation: no measurement logic, no network access.
//! - An honest gap is `gap` or `skip-needs-live`, never a fabricated `produced`.
//! - Exit 0 on any honest gap; exit 2 on malformed input only.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Verdict ───────────────────────────────────────────────────────────────────

/// Verdict assigned to one acceptance-criterion entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Artifact answers this AC with a real value.
    Produced,
    /// Artifact exists but this AC is not covered (e.g. only one model
    /// measured when two were required).
    Gap,
    /// Requires a live port / LLM / SE-DEMO access this host cannot provide.
    SkipNeedsLive,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Produced => "produced",
            Self::Gap => "gap",
            Self::SkipNeedsLive => "skip-needs-live",
        };
        f.write_str(s)
    }
}

// ── Ticket-map input types ────────────────────────────────────────────────────

/// One AC entry inside the ticket-map.
#[derive(Debug, Clone, Deserialize)]
pub struct TicketAcSpec {
    /// AC identifier, e.g. "AC1".
    pub id: String,
    /// Human-readable description of the AC.
    pub text: String,
    /// Which artifact file answers this AC (matches a CLI argument key:
    /// "footprint", "paramq", "walkthrough", "`handle_demo`").
    pub expected_artifact: String,
    /// JSON pointer (RFC 6901) into the artifact for the value to inspect.
    /// E.g. `/classes/tool_result_rows`.
    #[serde(default)]
    pub artifact_pointer: Option<String>,
    /// Owner of this AC (team/person responsible).
    #[serde(default)]
    pub owner: Option<String>,
    /// If the artifact is absent or the pointer resolves to null/missing,
    /// use this verdict instead of `gap`. Must be "skip-needs-live" or "gap".
    #[serde(default)]
    pub verdict_if_absent: Option<String>,
    /// Required when `verdict_if_absent == "skip-needs-live"`: names the
    /// unmet dependency (e.g. "live LLM vendor", "DAX port").
    #[serde(default)]
    pub blocked_on: Option<String>,
}

/// One ticket entry in the ticket-map.
#[derive(Debug, Clone, Deserialize)]
pub struct TicketSpec {
    /// Jira ticket ID, e.g. "ATSCALE-49212".
    pub ticket: String,
    /// Acceptance criteria for this ticket.
    pub acs: Vec<TicketAcSpec>,
}

/// The full ticket-map input.
#[derive(Debug, Clone, Deserialize)]
pub struct TicketMap {
    /// All tickets and their ACs.
    pub tickets: Vec<TicketSpec>,
}

// ── Artifact map ──────────────────────────────────────────────────────────────

/// Parsed artifacts keyed by their CLI argument name.
pub type ArtifactMap = BTreeMap<String, serde_json::Value>;

// ── Output types ──────────────────────────────────────────────────────────────

/// Result for one AC after scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcResult {
    /// AC identifier.
    pub id: String,
    /// Verdict assigned.
    pub verdict: Verdict,
    /// Which artifact answered (or was expected to answer) this AC.
    pub artifact: String,
    /// One-line summary of the value found in the artifact, or an
    /// explanation when the verdict is gap/skip-needs-live.
    pub value_summary: String,
    /// Owner of this AC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Unmet dependency when verdict is skip-needs-live.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_on: Option<String>,
}

/// Per-ticket verdict summary counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TicketSummary {
    /// Number of ACs verdicted `produced`.
    pub produced: usize,
    /// Number of ACs verdicted `gap`.
    pub gap: usize,
    /// Number of ACs verdicted `skip-needs-live`.
    pub skip_needs_live: usize,
}

/// Result for one ticket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketResult {
    /// Ticket ID.
    pub ticket: String,
    /// Per-AC results.
    pub acs: Vec<AcResult>,
    /// Rollup counts.
    pub summary: TicketSummary,
}

/// The top-level output written to `spike_evidence.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpikeEvidence {
    /// Schema identifier.
    pub schema: String,
    /// Per-ticket results.
    pub tickets: Vec<TicketResult>,
}

// ── Scorer ────────────────────────────────────────────────────────────────────

/// Score one AC against the artifact map.
///
/// # Logic
/// 1. Look up the artifact by `spec.expected_artifact` key.
/// 2. If not present: use `spec.verdict_if_absent` (defaulting to `gap`).
/// 3. If present: follow `spec.artifact_pointer` (or use the root if absent).
///    - If the resolved value is non-null and non-empty-string → `produced`.
///    - Otherwise → `spec.verdict_if_absent` or `gap`.
///
/// # Errors
/// This function is infallible — a missing artifact is a gap, not an error.
#[must_use]
pub fn score_ac(spec: &TicketAcSpec, artifacts: &ArtifactMap) -> AcResult {
    let artifact_key = &spec.expected_artifact;

    // Determine the default absent-verdict.
    let absent_verdict = match spec.verdict_if_absent.as_deref() {
        Some("skip-needs-live") => Verdict::SkipNeedsLive,
        Some("gap" | _) | None => Verdict::Gap,
    };

    let absent_blocked_on = if absent_verdict == Verdict::SkipNeedsLive {
        spec.blocked_on.clone()
    } else {
        None
    };

    // Look up the artifact.
    let Some(artifact_val) = artifacts.get(artifact_key.as_str()) else {
        let summary = format!("artifact '{artifact_key}' not provided");
        return AcResult {
            id: spec.id.clone(),
            verdict: absent_verdict,
            artifact: artifact_key.clone(),
            value_summary: summary,
            owner: spec.owner.clone(),
            blocked_on: absent_blocked_on,
        };
    };

    // Resolve pointer if given.
    let resolved = spec
        .artifact_pointer
        .as_ref()
        .map_or(Some(artifact_val), |ptr| artifact_val.pointer(ptr.as_str()));

    // Decide verdict based on resolved value.
    match resolved {
        None => {
            let ptr_str = spec
                .artifact_pointer
                .as_deref()
                .unwrap_or("(root)");
            let summary = format!("pointer '{ptr_str}' not found in artifact '{artifact_key}'");
            AcResult {
                id: spec.id.clone(),
                verdict: absent_verdict,
                artifact: artifact_key.clone(),
                value_summary: summary,
                owner: spec.owner.clone(),
                blocked_on: absent_blocked_on,
            }
        }
        Some(v) if is_empty_value(v) => {
            let summary = format!("pointer resolved to empty/null in '{artifact_key}'");
            AcResult {
                id: spec.id.clone(),
                verdict: absent_verdict,
                artifact: artifact_key.clone(),
                value_summary: summary,
                owner: spec.owner.clone(),
                blocked_on: absent_blocked_on,
            }
        }
        Some(v) => {
            let value_summary = summarize_value(v);
            AcResult {
                id: spec.id.clone(),
                verdict: Verdict::Produced,
                artifact: artifact_key.clone(),
                value_summary,
                owner: spec.owner.clone(),
                blocked_on: None,
            }
        }
    }
}

/// Returns `true` if the JSON value is null or an empty string.
fn is_empty_value(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.is_empty(),
        _ => false,
    }
}

/// Produce a one-line summary of a JSON value (truncated at 120 chars).
fn summarize_value(v: &serde_json::Value) -> String {
    let raw = match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if raw.len() <= 120 {
        raw
    } else {
        format!("{}… (truncated)", &raw[..117])
    }
}

// ── Bundle runner ─────────────────────────────────────────────────────────────

/// Run the full evidence bundle over the ticket-map and artifact map.
///
/// Returns a `SpikeEvidence` with one entry per ticket and a per-AC verdict
/// for every AC in the map. Never panics; honest gaps surface as verdicts.
#[must_use]
pub fn run_bundle(ticket_map: &TicketMap, artifacts: &ArtifactMap) -> SpikeEvidence {
    let mut tickets: Vec<TicketResult> = Vec::new();

    for ticket_spec in &ticket_map.tickets {
        let mut ac_results: Vec<AcResult> = Vec::new();

        for ac_spec in &ticket_spec.acs {
            let result = score_ac(ac_spec, artifacts);
            ac_results.push(result);
        }

        let summary = rollup_summary(&ac_results);
        tickets.push(TicketResult {
            ticket: ticket_spec.ticket.clone(),
            acs: ac_results,
            summary,
        });
    }

    SpikeEvidence {
        schema: "spike-evidence.v1".to_owned(),
        tickets,
    }
}

/// Compute per-ticket summary counts from a list of AC results.
#[must_use]
pub fn rollup_summary(acs: &[AcResult]) -> TicketSummary {
    let mut summary = TicketSummary::default();
    for ac in acs {
        match ac.verdict {
            Verdict::Produced => summary.produced += 1,
            Verdict::Gap => summary.gap += 1,
            Verdict::SkipNeedsLive => summary.skip_needs_live += 1,
        }
    }
    summary
}

// ── Markdown renderer ─────────────────────────────────────────────────────────

/// Render a `SpikeEvidence` as a per-ticket Markdown brief.
///
/// Each ticket gets an H2 section with an AC→verdict→evidence table.
/// `skip-needs-live` rows include the `blocked_on` dependency.
#[must_use]
pub fn render_markdown(evidence: &SpikeEvidence) -> String {
    let mut out = String::new();
    out.push_str("# Spike Evidence Bundle\n\n");

    for ticket_result in &evidence.tickets {
        out.push_str(&format!("## {}\n\n", ticket_result.ticket));
        out.push_str("| AC | Verdict | Evidence | Blocked On |\n");
        out.push_str("|---|---|---|---|\n");

        for ac in &ticket_result.acs {
            let blocked = ac.blocked_on.as_deref().unwrap_or("");
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                ac.id, ac.verdict, ac.value_summary, blocked
            ));
        }

        let s = &ticket_result.summary;
        out.push_str(&format!(
            "\n**Summary:** produced={} gap={} skip-needs-live={} (total={})\n\n",
            s.produced,
            s.gap,
            s.skip_needs_live,
            s.produced + s.gap + s.skip_needs_live
        ));
    }

    out
}

// ── Input parsing ─────────────────────────────────────────────────────────────

/// Parse a JSON file, returning a diagnostic string on error.
///
/// # Errors
/// Returns an error string identifying the file path and parse failure.
pub fn parse_json_file(path: &str) -> Result<serde_json::Value, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("reading '{path}': {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("parsing '{path}': {e}"))
}

/// Parse the ticket-map JSON file.
///
/// # Errors
/// Returns an error string identifying the file path and parse failure.
pub fn parse_ticket_map(path: &str) -> Result<TicketMap, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("reading ticket-map '{path}': {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("parsing ticket-map '{path}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verdict_display() {
        assert_eq!(Verdict::Produced.to_string(), "produced");
        assert_eq!(Verdict::Gap.to_string(), "gap");
        assert_eq!(Verdict::SkipNeedsLive.to_string(), "skip-needs-live");
    }

    #[test]
    fn score_ac_produced_when_value_present() {
        let spec = TicketAcSpec {
            id: "AC1".to_owned(),
            text: "tool_result_rows present".to_owned(),
            expected_artifact: "footprint".to_owned(),
            artifact_pointer: Some("/classes/tool_result_rows".to_owned()),
            owner: None,
            verdict_if_absent: None,
            blocked_on: None,
        };
        let mut artifacts = ArtifactMap::new();
        artifacts.insert(
            "footprint".to_owned(),
            json!({"classes": {"tool_result_rows": 42}}),
        );
        let result = score_ac(&spec, &artifacts);
        assert_eq!(result.verdict, Verdict::Produced);
        assert_eq!(result.value_summary, "42");
    }

    #[test]
    fn score_ac_gap_when_artifact_missing() {
        let spec = TicketAcSpec {
            id: "AC1".to_owned(),
            text: "test".to_owned(),
            expected_artifact: "missing_artifact".to_owned(),
            artifact_pointer: None,
            owner: None,
            verdict_if_absent: None,
            blocked_on: None,
        };
        let artifacts = ArtifactMap::new();
        let result = score_ac(&spec, &artifacts);
        assert_eq!(result.verdict, Verdict::Gap);
    }

    #[test]
    fn score_ac_skip_needs_live_when_configured() {
        let spec = TicketAcSpec {
            id: "AC3".to_owned(),
            text: "live vendor loop".to_owned(),
            expected_artifact: "paramq".to_owned(),
            artifact_pointer: Some("/live_vendor_score".to_owned()),
            owner: None,
            verdict_if_absent: Some("skip-needs-live".to_owned()),
            blocked_on: Some("live LLM vendor".to_owned()),
        };
        let mut artifacts = ArtifactMap::new();
        artifacts.insert("paramq".to_owned(), json!({"overall": {}}));
        let result = score_ac(&spec, &artifacts);
        assert_eq!(result.verdict, Verdict::SkipNeedsLive);
        assert_eq!(result.blocked_on.as_deref(), Some("live LLM vendor"));
    }

    #[test]
    fn rollup_summary_counts_correctly() {
        let acs = vec![
            AcResult {
                id: "AC1".to_owned(),
                verdict: Verdict::Produced,
                artifact: "a".to_owned(),
                value_summary: "ok".to_owned(),
                owner: None,
                blocked_on: None,
            },
            AcResult {
                id: "AC2".to_owned(),
                verdict: Verdict::Gap,
                artifact: "b".to_owned(),
                value_summary: "missing".to_owned(),
                owner: None,
                blocked_on: None,
            },
            AcResult {
                id: "AC3".to_owned(),
                verdict: Verdict::SkipNeedsLive,
                artifact: "c".to_owned(),
                value_summary: "live needed".to_owned(),
                owner: None,
                blocked_on: Some("live LLM".to_owned()),
            },
        ];
        let summary = rollup_summary(&acs);
        assert_eq!(summary.produced, 1);
        assert_eq!(summary.gap, 1);
        assert_eq!(summary.skip_needs_live, 1);
    }

    #[test]
    fn is_empty_value_null_and_empty_string() {
        assert!(is_empty_value(&serde_json::Value::Null));
        assert!(is_empty_value(&json!("")));
        assert!(!is_empty_value(&json!(0)));
        assert!(!is_empty_value(&json!("hello")));
    }

    #[test]
    fn summarize_value_truncates_long_string() {
        let long = "x".repeat(200);
        let v = json!(long);
        let s = summarize_value(&v);
        // "… (truncated)" is 14 bytes (UTF-8: "…" = 3 bytes + " (truncated)" = 12 bytes)
        // so max = 117 + 15 = 132
        assert!(s.len() <= 135, "truncated string too long: {} bytes", s.len());
        assert!(s.contains("truncated"), "should mention truncation");
    }
}
