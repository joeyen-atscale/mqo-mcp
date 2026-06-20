//! SQL-string validation layer (server-validator-migration, iter-1).
//!
//! These checks operate on a raw SQL string *before* it reaches the warehouse.
//! Each check is a decidable predicate on the SQL text alone — no catalog, no
//! network. Mirrors the MQO param-validator pattern: return `Vec<SqlRejection>`,
//! empty on pass, one entry per violation.
//!
//! **Stable error codes** are in [`SqlRule`]. Agents receive the code and a
//! one-line fix sentence so they can correct and retry.
//!
//! # Rules implemented
//!
//! | Code | Name | Reference |
//! |------|------|-----------|
//! | `multi_statement` | Multi-statement SQL rejection | ATSCALE-48466 |

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Error taxonomy
// ---------------------------------------------------------------------------

/// Stable, machine-readable rule codes for SQL-string validation rejections.
///
/// Each variant corresponds to exactly one decidable SQL-text check.  Add new
/// variants here (never reuse a variant name once released) so agents and
/// operators have a stable grep target in logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqlRule {
    /// The SQL string contains multiple statements separated by a semicolon.
    ///
    /// AtScale's query-execution tool accepts a single SELECT statement.
    /// Multi-statement input previously leaked raw internal/stack-trace text
    /// (ATSCALE-48466); this gate rejects it cleanly before execution.
    MultiStatement,
}

/// A single SQL-validation rejection.  The `rule` is machine-readable;
/// `message` is a user-actionable English sentence suitable for the agent to
/// act on.  No raw internal/stack-trace text is ever included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlRejection {
    /// Stable rule code — grep-friendly, never changes between releases.
    pub rule: SqlRule,
    /// Human-readable, agent-actionable message: what is wrong + how to fix.
    pub message: String,
}

impl SqlRejection {
    fn new(rule: SqlRule, message: impl Into<String>) -> Self {
        SqlRejection {
            rule,
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validate entry point
// ---------------------------------------------------------------------------

/// Validate a raw SQL string against all SQL-level rules.
///
/// Returns an empty `Vec` when the SQL passes every check.
/// Returns one [`SqlRejection`] per violation otherwise.
/// Never panics; safe to call concurrently (stateless).
///
/// # Multi-statement detection (ATSCALE-48466)
///
/// AtScale's query-execution tool expects a single SELECT statement.
/// A semicolon that terminates one statement and begins another is rejected.
/// A trailing semicolon on an otherwise valid single statement is **not**
/// rejected — it is harmless and common.
pub fn validate_sql(sql: &str) -> Vec<SqlRejection> {
    let mut rejections = Vec::new();
    check_multi_statement(sql, &mut rejections);
    rejections
}

// ---------------------------------------------------------------------------
// SqlRule::MultiStatement
// ---------------------------------------------------------------------------

/// Detect multi-statement SQL: a semicolon followed by non-whitespace content
/// (after stripping SQL line-comments and string literals conservatively).
///
/// Strategy (conservative, no full parser required):
/// 1. Walk the SQL character by character, tracking:
///    - single-quoted string literals (`'...'`, with `''` escape),
///    - line comments (`-- ...`),
///    - block comments (`/* ... */`).
/// 2. When a `;` is found OUTSIDE any of those contexts, check whether any
///    non-whitespace token follows it (ignoring trailing whitespace / a final
///    bare `;`).
/// 3. If yes → multi-statement → reject.
///
/// This correctly passes:
///   * `"SELECT * FROM foo"` (no semicolon)
///   * `"SELECT * FROM foo;"` (trailing semicolon, nothing after)
///   * `"SELECT ';' FROM foo"` (semicolon inside a string literal)
///
/// And correctly rejects:
///   * `"SELECT 1; SELECT 2"` (two statements)
///   * `"SELECT 1; DROP TABLE foo"` (injection attempt)
fn check_multi_statement(sql: &str, rejections: &mut Vec<SqlRejection>) {
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Single-quoted string: skip to closing quote (handle '' escape).
        if chars[i] == '\'' {
            i += 1;
            while i < len {
                if chars[i] == '\'' {
                    i += 1;
                    // '' is an escaped quote, not the end.
                    if i < len && chars[i] == '\'' {
                        i += 1;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Line comment: -- to end of line.
        if i + 1 < len && chars[i] == '-' && chars[i + 1] == '-' {
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        // Block comment: /* ... */
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2; // skip closing */
            continue;
        }

        // Semicolon outside a literal/comment context.
        if chars[i] == ';' {
            // Check if anything non-whitespace follows.
            let rest = &chars[i + 1..];
            let has_follow = rest.iter().any(|c| !c.is_whitespace());
            if has_follow {
                rejections.push(SqlRejection::new(
                    SqlRule::MultiStatement,
                    "Multi-statement SQL is not supported: only a single SELECT statement may be \
                     submitted per query-execution call. Remove or split at the semicolon — submit \
                     each statement as a separate call.",
                ));
                return; // one rejection per query is enough
            }
        }

        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Conforming (admit) cases ────────────────────────────────────────────

    /// A plain SELECT with no semicolon passes.
    #[test]
    fn conforming_no_semicolon() {
        let sql = "SELECT * FROM [Sales].[Revenue]";
        let result = validate_sql(sql);
        assert!(
            result.is_empty(),
            "plain SELECT must pass; got: {:?}",
            result
        );
    }

    /// A trailing semicolon on a single statement is harmless and must pass.
    #[test]
    fn conforming_trailing_semicolon() {
        let sql = "SELECT store_name, net_sales FROM [Store Sales] WHERE year = 2023;";
        let result = validate_sql(sql);
        assert!(
            result.is_empty(),
            "trailing semicolon on single statement must pass; got: {:?}",
            result
        );
    }

    /// A trailing semicolon followed only by whitespace/newline must pass.
    #[test]
    fn conforming_trailing_semicolon_with_whitespace() {
        let sql = "SELECT 1;   \n  ";
        let result = validate_sql(sql);
        assert!(
            result.is_empty(),
            "trailing semicolon + whitespace must pass; got: {:?}",
            result
        );
    }

    /// A semicolon inside a string literal must NOT trigger the check.
    #[test]
    fn conforming_semicolon_inside_string_literal() {
        let sql = "SELECT ';' AS delim FROM [foo]";
        let result = validate_sql(sql);
        assert!(
            result.is_empty(),
            "semicolon in string literal must pass; got: {:?}",
            result
        );
    }

    /// A semicolon inside a line comment must NOT trigger the check.
    #[test]
    fn conforming_semicolon_in_line_comment() {
        let sql = "SELECT 1 -- statement ends here; no second statement\nFROM [foo]";
        let result = validate_sql(sql);
        assert!(
            result.is_empty(),
            "semicolon in line comment must pass; got: {:?}",
            result
        );
    }

    /// A semicolon inside a block comment must NOT trigger the check.
    #[test]
    fn conforming_semicolon_in_block_comment() {
        let sql = "SELECT 1 /* semicolon; here */ FROM [foo]";
        let result = validate_sql(sql);
        assert!(
            result.is_empty(),
            "semicolon in block comment must pass; got: {:?}",
            result
        );
    }

    /// Empty SQL passes (no statements → no multi-statement).
    #[test]
    fn conforming_empty_sql() {
        let result = validate_sql("");
        assert!(
            result.is_empty(),
            "empty string must pass; got: {:?}",
            result
        );
    }

    // ── Violating (reject) cases ────────────────────────────────────────────

    /// Canonical: two SELECT statements separated by a semicolon → rejected.
    #[test]
    fn violating_two_selects() {
        let sql = "SELECT 1; SELECT 2";
        let result = validate_sql(sql);
        assert_eq!(result.len(), 1, "two SELECTs must produce exactly one rejection; got: {:?}", result);
        assert_eq!(result[0].rule, SqlRule::MultiStatement);
        // Message must be actionable and contain no stack/internal text.
        assert!(
            result[0].message.contains("semicolon"),
            "message must mention semicolon; got: {:?}",
            result[0].message
        );
    }

    /// Injection attempt: SELECT + DROP → rejected.
    #[test]
    fn violating_select_drop() {
        let sql = "SELECT * FROM [foo]; DROP TABLE foo";
        let result = validate_sql(sql);
        assert_eq!(
            result.len(),
            1,
            "SELECT+DROP must be rejected; got: {:?}",
            result
        );
        assert_eq!(result[0].rule, SqlRule::MultiStatement);
    }

    /// Three statements → still exactly one rejection (don't spam).
    #[test]
    fn violating_three_statements_one_rejection() {
        let sql = "SELECT 1; SELECT 2; SELECT 3";
        let result = validate_sql(sql);
        assert_eq!(
            result.len(),
            1,
            "three stmts must produce exactly one rejection; got: {:?}",
            result
        );
        assert_eq!(result[0].rule, SqlRule::MultiStatement);
    }

    /// Whitespace between semicolon and second statement is still a violation.
    #[test]
    fn violating_whitespace_between_statements() {
        let sql = "SELECT 1;\n\nSELECT 2";
        let result = validate_sql(sql);
        assert_eq!(
            result.len(),
            1,
            "newline-separated statements must be rejected; got: {:?}",
            result
        );
        assert_eq!(result[0].rule, SqlRule::MultiStatement);
    }

    // ── Error-shape invariants ──────────────────────────────────────────────

    /// The rejection message must not contain any stack/internal trace markers.
    #[test]
    fn rejection_message_no_internal_text() {
        let sql = "SELECT 1; SELECT 2";
        let result = validate_sql(sql);
        assert!(!result.is_empty());
        let msg = &result[0].message;
        // These substrings would indicate a leaked runtime exception.
        for banned in &["panic", "unwrap", "stack", "thread", "at src/", ".rs:"] {
            assert!(
                !msg.to_lowercase().contains(banned),
                "message must not contain internal text {:?}; got: {:?}",
                banned,
                msg
            );
        }
    }

    /// SqlRejection is JSON-serializable (G2: machine-readable structured output).
    #[test]
    fn rejection_is_json_serializable() {
        let sql = "SELECT 1; SELECT 2";
        let result = validate_sql(sql);
        assert!(!result.is_empty());
        let json = serde_json::to_string(&result[0])
            .expect("SqlRejection must serialize to JSON");
        assert!(json.contains("multi_statement"), "JSON must contain rule code; got: {json}");
    }
}
