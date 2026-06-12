use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Status of a finding in the investigation lifecycle.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum FindingStatus {
    Open,
    Confirmed,
    Refuted,
    Escalated,
    Suppressed,
}

/// A resolved investigation record. The store persists these as JSONL.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Finding {
    /// UUID-style identifier: "<query_id>-<timestamp_ms>"
    pub finding_id: String,
    pub query_id: String,
    /// Raw WatchEvent JSON from mcp-watch-daemon
    pub watch_event: serde_json::Value,
    /// Raw ResolvedHypothesisSet JSON from mcp-probe-executor
    pub resolved_hypotheses: serde_json::Value,
    pub status: FindingStatus,
    /// How many times this finding was superseded (0 on first sight)
    pub recurrence_count: u64,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
}

impl Finding {
    /// A finding is "active" (eligible to be superseded on recurrence) iff
    /// its status is Open, Confirmed, or Refuted — i.e. not terminally closed.
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            FindingStatus::Open | FindingStatus::Confirmed | FindingStatus::Refuted
        )
    }
}

// ---------------------------------------------------------------------------
// Internal JSONL record types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
enum JournalRecord {
    New(NewRecord),
    Update(UpdateRecord),
    Recur(RecurRecord),
}

/// A full finding create record.
#[derive(Serialize, Deserialize)]
struct NewRecord {
    finding_id: String,
    query_id: String,
    watch_event: serde_json::Value,
    resolved_hypotheses: serde_json::Value,
    status: FindingStatus,
    recurrence_count: u64,
    first_seen_ms: u64,
    last_seen_ms: u64,
}

/// A status-update patch record.
#[derive(Serialize, Deserialize)]
struct UpdateRecord {
    finding_id: String,
    status: FindingStatus,
    last_seen_ms: u64,
}

/// A supersede-on-recurrence patch record.
#[derive(Serialize, Deserialize)]
struct RecurRecord {
    finding_id: String,
    recurrence_count: u64,
    last_seen_ms: u64,
    watch_event: serde_json::Value,
    resolved_hypotheses: serde_json::Value,
}

// ---------------------------------------------------------------------------
// FindingStore
// ---------------------------------------------------------------------------

/// Append-only JSONL store for resolved investigation findings.
pub struct FindingStore {
    dir: PathBuf,
}

impl FindingStore {
    const FILENAME: &'static str = "findings.jsonl";

    fn journal_path(&self) -> PathBuf {
        self.dir.join(Self::FILENAME)
    }

    /// Open (or create) a finding store rooted at `dir`.
    pub fn open(dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(FindingStore {
            dir: dir.to_path_buf(),
        })
    }

    /// Append a single journal record as a JSONL line.
    fn append(&self, record: &JournalRecord) -> io::Result<()> {
        let line = serde_json::to_string(record)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let path = self.journal_path();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// Load and fold all records from the journal, returning latest state per
    /// finding_id.  A corrupt (unparseable) line causes an `Err` naming the
    /// line number — there is no silent skip.
    pub fn load_raw(&self) -> io::Result<Vec<Finding>> {
        let path = self.journal_path();
        if !path.exists() {
            return Ok(vec![]);
        }

        let file = std::fs::File::open(&path)?;
        let reader = io::BufReader::new(file);

        // Map: finding_id -> Finding (latest folded state)
        let mut map: std::collections::HashMap<String, Finding> =
            std::collections::HashMap::new();

        for (idx, line_result) in reader.lines().enumerate() {
            let line_no = idx + 1;
            let line = line_result?;
            if line.trim().is_empty() {
                continue;
            }

            let record: JournalRecord = serde_json::from_str(&line).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("corrupt line {}: {}", line_no, e),
                )
            })?;

            match record {
                JournalRecord::New(n) => {
                    let finding = Finding {
                        finding_id: n.finding_id.clone(),
                        query_id: n.query_id,
                        watch_event: n.watch_event,
                        resolved_hypotheses: n.resolved_hypotheses,
                        status: n.status,
                        recurrence_count: n.recurrence_count,
                        first_seen_ms: n.first_seen_ms,
                        last_seen_ms: n.last_seen_ms,
                    };
                    map.insert(n.finding_id, finding);
                }
                JournalRecord::Update(u) => {
                    if let Some(f) = map.get_mut(&u.finding_id) {
                        f.status = u.status;
                        f.last_seen_ms = u.last_seen_ms;
                    }
                }
                JournalRecord::Recur(r) => {
                    if let Some(f) = map.get_mut(&r.finding_id) {
                        f.recurrence_count = r.recurrence_count;
                        f.last_seen_ms = r.last_seen_ms;
                        f.watch_event = r.watch_event;
                        f.resolved_hypotheses = r.resolved_hypotheses;
                    }
                }
            }
        }

        Ok(map.into_values().collect())
    }

    /// Find the most-recent active finding for a query_id (if any) by scanning
    /// all findings and picking the one with the latest last_seen_ms.
    fn find_active_for_query(
        findings: &[Finding],
        query_id: &str,
    ) -> Option<Finding> {
        findings
            .iter()
            .filter(|f| f.query_id == query_id && f.is_active())
            .max_by_key(|f| f.last_seen_ms)
            .cloned()
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Record a freshly resolved investigation.
    ///
    /// If an active finding already exists for `query_id`, supersede it:
    /// bump `recurrence_count`, update `last_seen_ms`, replace `watch_event`
    /// and `resolved_hypotheses` with the newer ones, keep `first_seen_ms`.
    ///
    /// Otherwise create a new finding with `recurrence_count = 0`.
    ///
    /// Returns the `finding_id` that was touched.
    pub fn record(
        &self,
        query_id: &str,
        watch_event: &serde_json::Value,
        resolved: &serde_json::Value,
        status: FindingStatus,
        now_ms: u64,
    ) -> io::Result<String> {
        let all = self.load_raw()?;

        if let Some(existing) = Self::find_active_for_query(&all, query_id) {
            let new_recurrence = existing.recurrence_count + 1;
            let record = JournalRecord::Recur(RecurRecord {
                finding_id: existing.finding_id.clone(),
                recurrence_count: new_recurrence,
                last_seen_ms: now_ms,
                watch_event: watch_event.clone(),
                resolved_hypotheses: resolved.clone(),
            });
            self.append(&record)?;
            Ok(existing.finding_id)
        } else {
            let finding_id = format!("{}-{}", query_id, Uuid::new_v4().as_simple());
            let record = JournalRecord::New(NewRecord {
                finding_id: finding_id.clone(),
                query_id: query_id.to_owned(),
                watch_event: watch_event.clone(),
                resolved_hypotheses: resolved.clone(),
                status,
                recurrence_count: 0,
                first_seen_ms: now_ms,
                last_seen_ms: now_ms,
            });
            self.append(&record)?;
            Ok(finding_id)
        }
    }

    /// Transition a finding's status. Appends an update record.
    /// Returns `true` if the finding was found, `false` otherwise.
    pub fn set_status(
        &self,
        finding_id: &str,
        status: FindingStatus,
        now_ms: u64,
    ) -> io::Result<bool> {
        let all = self.load_raw()?;
        if all.iter().any(|f| f.finding_id == finding_id) {
            let record = JournalRecord::Update(UpdateRecord {
                finding_id: finding_id.to_owned(),
                status,
                last_seen_ms: now_ms,
            });
            self.append(&record)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get a finding by its id (latest folded state).
    pub fn get(&self, finding_id: &str) -> io::Result<Option<Finding>> {
        let all = self.load_raw()?;
        Ok(all.into_iter().find(|f| f.finding_id == finding_id))
    }

    /// Return the active (Open/Confirmed/Refuted) finding for a query_id, if any.
    pub fn open_for_query(&self, query_id: &str) -> io::Result<Option<Finding>> {
        let all = self.load_raw()?;
        Ok(Self::find_active_for_query(&all, query_id))
    }

    /// Return all findings in their latest folded state.
    pub fn all(&self) -> io::Result<Vec<Finding>> {
        self.load_raw()
    }

    /// Return all findings for a given query_id.
    pub fn by_query(&self, query_id: &str) -> io::Result<Vec<Finding>> {
        let all = self.load_raw()?;
        Ok(all.into_iter().filter(|f| f.query_id == query_id).collect())
    }
}
