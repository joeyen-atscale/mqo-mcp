use std::io::{self, BufRead};
use std::path::Path;

/// Configuration for budget limits. A `None` value means that dimension is unconstrained.
#[derive(Debug, Clone)]
pub struct BudgetLimits {
    /// Maximum number of queries allowed.
    pub max_queries: Option<u64>,
    /// Maximum estimated tokens allowed.
    pub max_est_tokens: Option<u64>,
    /// Maximum summed backend latency in milliseconds.
    pub max_latency_ms: Option<u64>,
    /// Maximum wall-clock time in milliseconds since ledger start.
    pub max_wall_ms: Option<u64>,
    /// Fraction of any limit at which to issue a CheckIn (e.g. 0.8 = 80%).
    pub checkin_fraction: f64,
}

/// Tracks accumulated spend against configured limits.
pub struct BudgetLedger {
    pub started_ms: u64,
    pub queries_run: u64,
    pub est_tokens: u64,
    pub total_latency_ms: u64,
    limits: BudgetLimits,
}

/// The verdict returned by `BudgetLedger::check`.
///
/// Precedence: Halt > CheckIn > Proceed.
#[derive(Debug)]
pub enum Verdict {
    /// Spend is within bounds; proceed with the next unit of work.
    Proceed,
    /// Soft limit reached — pause and surface to a human for approval.
    CheckIn { reason: String, fraction_used: f64 },
    /// Hard limit reached or exceeded — stop immediately.
    Halt { reason: String, limit: String },
}

impl BudgetLedger {
    /// Create a new ledger with the given limits and a wall-clock start timestamp.
    pub fn new(limits: BudgetLimits, started_ms: u64) -> Self {
        BudgetLedger {
            started_ms,
            queries_run: 0,
            est_tokens: 0,
            total_latency_ms: 0,
            limits,
        }
    }

    /// Record one completed unit of work.
    pub fn record_query(&mut self, est_tokens: u64, latency_ms: u64) {
        self.queries_run += 1;
        self.est_tokens += est_tokens;
        self.total_latency_ms += latency_ms;
    }

    /// Fold real spend from an audit-chain JSONL file.
    ///
    /// Each line must be a JSON object. The `latency_ms` field (if present and numeric)
    /// is summed into `total_latency_ms`; each parseable record also increments
    /// `queries_run`. Corrupt / non-JSON lines are skipped with a stderr warning.
    ///
    /// Returns the number of records successfully ingested.
    pub fn ingest_audit_log(&mut self, path: &Path) -> io::Result<u64> {
        let file = std::fs::File::open(path)?;
        let reader = io::BufReader::new(file);
        let mut count = 0u64;

        for (line_no, line_result) in reader.lines().enumerate() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    eprintln!(
                        "mcp-query-budget-governor: ingest_audit_log: I/O error on line {}: {}",
                        line_no + 1,
                        e
                    );
                    continue;
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "mcp-query-budget-governor: ingest_audit_log: corrupt JSON on line {}: {}",
                        line_no + 1,
                        e
                    );
                    continue;
                }
            };
            // Only count object records; skip non-objects.
            if let serde_json::Value::Object(ref obj) = value {
                if let Some(lat) = obj.get("latency_ms") {
                    if let Some(ms) = lat.as_u64() {
                        self.total_latency_ms += ms;
                    }
                }
                self.queries_run += 1;
                count += 1;
            } else {
                eprintln!(
                    "mcp-query-budget-governor: ingest_audit_log: non-object JSON on line {}, skipping",
                    line_no + 1
                );
            }
        }

        Ok(count)
    }

    /// Compute the current verdict. Pure: caller supplies `now_ms`.
    ///
    /// Halt dominates CheckIn dominates Proceed.
    /// For each configured (non-None) limit, compute the fraction used.
    /// If any fraction >= 1.0 → Halt (pick the worst).
    /// Else if any fraction >= checkin_fraction → CheckIn (pick the worst).
    /// Else Proceed.
    pub fn check(&self, now_ms: u64) -> Verdict {
        let wall_ms = now_ms.saturating_sub(self.started_ms);

        // Build a list of (fraction, dimension_name, limit_value_string) for each active limit.
        let mut fracs: Vec<(f64, &'static str, String)> = Vec::new();

        if let Some(max) = self.limits.max_queries {
            if max > 0 {
                fracs.push((
                    self.queries_run as f64 / max as f64,
                    "max_queries",
                    format!("{} queries", max),
                ));
            }
        }
        if let Some(max) = self.limits.max_est_tokens {
            if max > 0 {
                fracs.push((
                    self.est_tokens as f64 / max as f64,
                    "max_est_tokens",
                    format!("{} tokens", max),
                ));
            }
        }
        if let Some(max) = self.limits.max_latency_ms {
            if max > 0 {
                fracs.push((
                    self.total_latency_ms as f64 / max as f64,
                    "max_latency_ms",
                    format!("{} ms latency", max),
                ));
            }
        }
        if let Some(max) = self.limits.max_wall_ms {
            if max > 0 {
                fracs.push((
                    wall_ms as f64 / max as f64,
                    "max_wall_ms",
                    format!("{} ms wall", max),
                ));
            }
        }

        if fracs.is_empty() {
            return Verdict::Proceed;
        }

        // Find the most-exceeded dimension.
        let worst = fracs
            .iter()
            .cloned()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();

        if worst.0 >= 1.0 {
            return Verdict::Halt {
                reason: format!(
                    "hard limit reached: {} (fraction {:.3})",
                    worst.1, worst.0
                ),
                limit: worst.2,
            };
        }

        if worst.0 >= self.limits.checkin_fraction {
            return Verdict::CheckIn {
                reason: format!(
                    "soft limit reached: {} at {:.1}% of budget",
                    worst.1,
                    worst.0 * 100.0
                ),
                fraction_used: worst.0,
            };
        }

        Verdict::Proceed
    }

    /// Return the maximum fraction used across all configured limits.
    pub fn fraction_used(&self, now_ms: u64) -> f64 {
        let wall_ms = now_ms.saturating_sub(self.started_ms);

        let mut max_frac = 0.0f64;

        if let Some(max) = self.limits.max_queries {
            if max > 0 {
                max_frac = max_frac.max(self.queries_run as f64 / max as f64);
            }
        }
        if let Some(max) = self.limits.max_est_tokens {
            if max > 0 {
                max_frac = max_frac.max(self.est_tokens as f64 / max as f64);
            }
        }
        if let Some(max) = self.limits.max_latency_ms {
            if max > 0 {
                max_frac = max_frac.max(self.total_latency_ms as f64 / max as f64);
            }
        }
        if let Some(max) = self.limits.max_wall_ms {
            if max > 0 {
                max_frac = max_frac.max(wall_ms as f64 / max as f64);
            }
        }

        max_frac
    }
}

/// Optional ground-truth resource reader from the agentns kernel subsystem.
///
/// On non-Linux hosts or when `/proc/self/agent_counters` is absent, `read_self`
/// returns `CountersOutcome::Unsupported` and callers fall back to the userspace ledger.
pub mod agentns {
    /// Raw counters from the kernel agentns subsystem.
    pub struct AgentCounters {
        pub syscalls: u64,
        pub write_bytes: u64,
        pub connect: u64,
    }

    /// Result of attempting to read agent counters from the kernel.
    pub enum CountersOutcome {
        /// Successfully read kernel counters.
        Counters(AgentCounters),
        /// agentns not available on this host — caller should use userspace ledger.
        Unsupported,
    }

    /// Attempt to read `/proc/self/agent_counters`.
    ///
    /// Returns `Unsupported` whenever the file is absent (always on macOS).
    pub fn read_self() -> CountersOutcome {
        let path = std::path::Path::new("/proc/self/agent_counters");
        if !path.exists() {
            return CountersOutcome::Unsupported;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return CountersOutcome::Unsupported,
        };

        // Parse simple "key: value" format (one per line).
        let mut syscalls = 0u64;
        let mut write_bytes = 0u64;
        let mut connect = 0u64;

        for line in content.lines() {
            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() != 2 {
                continue;
            }
            let key = parts[0].trim();
            let val: u64 = match parts[1].trim().parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            match key {
                "syscalls" => syscalls = val,
                "write_bytes" => write_bytes = val,
                "connect" => connect = val,
                _ => {}
            }
        }

        CountersOutcome::Counters(AgentCounters {
            syscalls,
            write_bytes,
            connect,
        })
    }
}
