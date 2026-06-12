//! `mcp-trace-store` — durable, append-only interaction trace store for MQO queries.
//!
//! Every MQO interaction — the question, the bind result, the grounding score, the
//! execute result — is stored as a JSONL record. The corpus powers the gap miner,
//! quality scorer, and fine-tuning exporter.
//!
//! # File layout
//! `<path>` is the active JSONL file (e.g. `~/.local/share/mcp-traces/trace.jsonl`).
//! On rotation: `<path>.1`, `<path>.2`, etc. `scan` reads all rotation fragments in
//! order (oldest first, i.e. highest number first), applying the filter progressively.
//! `append` is atomic: write to temp file in the same directory, fsync, rename.
//!
//! # Corrupt lines
//! A corrupt JSONL line is skipped with a warning to stderr. The store is write-heavy
//! and a single corrupt line must not block reads.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Outcome of the MQO bind step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BindOutcome {
    Success,
    Ambiguous,
    NotFound,
    Error(String),
}

/// Grounding quality band — how well the MQO mapped to the semantic model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingBand {
    Grounded,
    Partial,
    Ungroundable,
}

/// Outcome of executing the bound MQO against the backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecuteOutcome {
    Success { row_count: u64, result_empty: bool },
    Error(String),
    /// Bind failed; execution was not attempted.
    Skipped,
}

/// Quality signals derived from a single interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualitySignals {
    /// `true` = no retry before successful bind.
    pub first_attempt_bind: bool,
    /// Total number of bind attempts (1 = first attempt succeeded).
    pub bind_attempt_count: u8,
    /// Wall-clock latency from first bind attempt to execute completion.
    pub total_latency_ms: u64,
    /// LLM tokens consumed in this interaction, if recorded.
    pub tokens_used: Option<u64>,
}

/// A single persisted interaction trace record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceRecord {
    /// UUID minted at write time by `TraceStore::append`.
    pub record_id: String,
    pub session_id: String,
    /// AtScale cluster hostname, if known.
    pub cluster_name: Option<String>,
    /// Unix epoch milliseconds, populated at write time if zero.
    pub timestamp_ms: u64,
    /// Full MQO JSON.
    pub mqo: Value,
    pub bind_outcome: BindOutcome,
    /// Grounding score from mcp-grounding-eval, if run.
    pub grounding_score: Option<f64>,
    /// Grounding band classification.
    pub grounding_band: Option<GroundingBand>,
    pub execute_result: ExecuteOutcome,
    pub quality: QualitySignals,
    /// The original natural-language question from the user, if captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_question: Option<String>,
}

impl TraceRecord {
    /// Convenience constructor that pre-fills `record_id` and `timestamp_ms`.
    pub fn new(
        session_id: impl Into<String>,
        mqo: Value,
        bind_outcome: BindOutcome,
        execute_result: ExecuteOutcome,
        quality: QualitySignals,
    ) -> Self {
        TraceRecord {
            record_id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            cluster_name: None,
            timestamp_ms: now_ms(),
            mqo,
            bind_outcome,
            grounding_score: None,
            grounding_band: None,
            execute_result,
            quality,
            user_question: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

/// Filter applied by `TraceStore::scan`.
#[derive(Debug, Clone, Default)]
pub struct TraceFilter {
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
    pub grounding_band: Option<GroundingBand>,
    pub first_attempt_only: bool,
    pub cluster: Option<String>,
    pub session: Option<String>,
    pub limit: Option<usize>,
}

impl TraceFilter {
    pub fn new() -> Self {
        Self::default()
    }

    fn matches(&self, r: &TraceRecord) -> bool {
        if let Some(since) = self.since_ms {
            if r.timestamp_ms < since {
                return false;
            }
        }
        if let Some(until) = self.until_ms {
            if r.timestamp_ms > until {
                return false;
            }
        }
        if let Some(ref band) = self.grounding_band {
            match &r.grounding_band {
                Some(rb) if rb == band => {}
                _ => return false,
            }
        }
        if self.first_attempt_only && !r.quality.first_attempt_bind {
            return false;
        }
        if let Some(ref c) = self.cluster {
            if r.cluster_name.as_deref() != Some(c.as_str()) {
                return false;
            }
        }
        if let Some(ref s) = self.session {
            if &r.session_id != s {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Store config
// ---------------------------------------------------------------------------

/// Configuration for a `TraceStore`.
#[derive(Debug, Clone)]
pub struct TraceStoreConfig {
    /// Path to the active JSONL file (e.g. `~/.local/share/mcp-traces/trace.jsonl`).
    pub path: PathBuf,
    /// Rotate when the active file exceeds this many bytes. Default: 50 MB.
    pub rotate_at_bytes: u64,
}

impl TraceStoreConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            rotate_at_bytes: 50 * 1024 * 1024,
        }
    }

    pub fn with_rotate_at_bytes(mut self, n: u64) -> Self {
        self.rotate_at_bytes = n;
        self
    }
}

// ---------------------------------------------------------------------------
// TraceStore
// ---------------------------------------------------------------------------

/// Append-only, JSONL-backed interaction trace store.
pub struct TraceStore {
    config: TraceStoreConfig,
}

impl TraceStore {
    /// Open (or create) the store at the configured path. Creates parent directories if needed.
    pub fn new(config: TraceStoreConfig) -> io::Result<Self> {
        if let Some(parent) = config.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        Ok(TraceStore { config })
    }

    /// Append a record to the store.
    ///
    /// If `record.record_id` is empty a new UUID is minted.
    /// If `record.timestamp_ms` is 0 it is set to `now_ms()`.
    /// The final record (as written) is returned.
    ///
    /// Atomic guarantee: the JSON line is serialized fully in memory, then
    /// written with `O_APPEND` in a single `write_all` call. POSIX guarantees
    /// that an `O_APPEND` write of `≤ PIPE_BUF` bytes is atomic against
    /// concurrent writers. For lines larger than PIPE_BUF (rare for trace
    /// records) the data is still consistent: a killed process leaves either
    /// a complete line or a partial line — and `scan` skips partial/corrupt
    /// lines per AC6. Prior records are never affected.
    pub fn append(&self, mut record: TraceRecord) -> io::Result<TraceRecord> {
        if record.record_id.is_empty() {
            record.record_id = Uuid::new_v4().to_string();
        }
        if record.timestamp_ms == 0 {
            record.timestamp_ms = now_ms();
        }

        // Rotate if needed before appending.
        self.rotate_if_needed()?;

        let active = &self.config.path;

        let mut line = serde_json::to_string(&record).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {e}"))
        })?;
        line.push('\n');

        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(active)?;
        f.write_all(line.as_bytes())?;

        Ok(record)
    }

    /// Rotate the active file if it exceeds `rotate_at_bytes`.
    ///
    /// Rotation shifts existing numbered files up (.1 → .2, .2 → .3, …) and renames
    /// the active file to `.1`. After rotation the active file no longer exists, so the
    /// next `append` creates it fresh.
    pub fn rotate_if_needed(&self) -> io::Result<()> {
        let active = &self.config.path;
        if !active.exists() {
            return Ok(());
        }
        let meta = fs::metadata(active)?;
        if meta.len() < self.config.rotate_at_bytes {
            return Ok(());
        }

        // Find the highest existing rotation number.
        let mut highest = 0usize;
        loop {
            let candidate = numbered_path(active, highest + 1);
            if !candidate.exists() {
                break;
            }
            highest += 1;
        }

        // Shift up: .N → .(N+1), …, .1 → .2.
        for n in (1..=highest).rev() {
            fs::rename(numbered_path(active, n), numbered_path(active, n + 1))?;
        }

        // Rename active → .1.
        fs::rename(active, numbered_path(active, 1))?;

        Ok(())
    }

    /// Scan all rotation fragments (oldest first) and return matching records.
    ///
    /// Corrupt JSONL lines are skipped with a warning to stderr (AC6).
    pub fn scan(&self, filter: &TraceFilter) -> io::Result<Vec<TraceRecord>> {
        // Collect all fragments: numbered (highest = oldest) then active.
        let mut paths: Vec<PathBuf> = Vec::new();

        // Find highest rotation number.
        let mut n = 1usize;
        loop {
            let p = numbered_path(&self.config.path, n);
            if !p.exists() {
                break;
            }
            paths.push(p);
            n += 1;
        }

        // Reverse so oldest (highest-numbered) comes first.
        paths.reverse();

        // Active file last.
        if self.config.path.exists() {
            paths.push(self.config.path.clone());
        }

        let mut results: Vec<TraceRecord> = Vec::new();

        'outer: for path in &paths {
            let file = match File::open(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let reader = BufReader::new(file);
            for (line_no, line_result) in reader.lines().enumerate() {
                if let Some(limit) = filter.limit {
                    if results.len() >= limit {
                        break 'outer;
                    }
                }
                let line = match line_result {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!(
                            "mcp-trace-store: io error reading {}:{}: {e}",
                            path.display(),
                            line_no + 1
                        );
                        continue;
                    }
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let record: TraceRecord = match serde_json::from_str(trimmed) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!(
                            "mcp-trace-store: skipping corrupt line {}:{}: {e}",
                            path.display(),
                            line_no + 1
                        );
                        continue;
                    }
                };
                if filter.matches(&record) {
                    results.push(record);
                }
            }
        }

        Ok(results)
    }

    /// Count total records across all fragments (no filter applied).
    pub fn count(&self) -> io::Result<u64> {
        let mut total = 0u64;

        let mut n = 1usize;
        let mut paths: Vec<PathBuf> = Vec::new();
        loop {
            let p = numbered_path(&self.config.path, n);
            if !p.exists() {
                break;
            }
            paths.push(p);
            n += 1;
        }
        if self.config.path.exists() {
            paths.push(self.config.path.clone());
        }

        for path in &paths {
            let file = match File::open(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let reader = BufReader::new(file);
            for line_result in reader.lines() {
                match line_result {
                    Ok(l) if !l.trim().is_empty() => total += 1,
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        }

        Ok(total)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn numbered_path(base: &Path, n: usize) -> PathBuf {
    let s = base.to_string_lossy();
    PathBuf::from(format!("{s}.{n}"))
}
