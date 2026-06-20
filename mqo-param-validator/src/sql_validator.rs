//! SQL-string validation layer (server-validator-migration, iter-1 + iter-2).
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
//! | `non_select_statement` | Non-SELECT SQL rejection | iter-2 |
//! | `window_function_in_select` | Window function in SELECT rejection | iter-2 |

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

    /// The SQL string does not begin with SELECT or WITH (after stripping
    /// leading whitespace and comments).
    ///
    /// The query-execution tool only accepts a single SELECT statement.
    /// DML or DDL (UPDATE, DELETE, DROP, INSERT, CREATE, ALTER, …) submitted
    /// as a single statement would bypass the multi-statement check but is
    /// equally invalid. This gate rejects any query whose first keyword is not
    /// SELECT or WITH (which introduces a common-table-expression SELECT).
    NonSelectStatement,

    /// The SQL string contains a window function (`OVER (` or `OVER(`).
    ///
    /// AtScale's semantic layer compiles the MQO into SQL with aggregate
    /// functions only. Window functions (RANK, ROW_NUMBER, DENSE_RANK, NTILE,
    /// LAG, LEAD, FIRST_VALUE, LAST_VALUE, …) are not part of the compiled
    /// output and indicate the agent injected a synthetic ranking or offset
    /// column. The correct way to rank results is ORDER BY + LIMIT on the MQO.
    WindowFunctionInSelect,
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
///
/// # Non-SELECT detection
///
/// The query-execution tool only accepts SELECT (or WITH … SELECT).
/// A single DML/DDL statement (UPDATE, DELETE, DROP, etc.) is rejected
/// before it can reach the warehouse.
///
/// # Window-function detection
///
/// Window functions (RANK() OVER, ROW_NUMBER() OVER, etc.) are not produced
/// by the MQO compiler and indicate the agent injected a synthetic column.
/// Use ORDER BY + LIMIT on the MQO instead.
pub fn validate_sql(sql: &str) -> Vec<SqlRejection> {
    let mut rejections = Vec::new();
    // Run non-select check first — catches DML/DDL before the other checks.
    check_non_select_statement(sql, &mut rejections);
    // Multi-statement check (ATSCALE-48466).
    check_multi_statement(sql, &mut rejections);
    // Window-function check — must run after non-select so we don't double-fire
    // on a DML statement that happens to contain OVER.
    if rejections.is_empty() {
        check_window_function(sql, &mut rejections);
    }
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
// SqlRule::NonSelectStatement
// ---------------------------------------------------------------------------

/// Reject SQL that does not begin with SELECT or WITH (after stripping leading
/// whitespace and line/block comments).
///
/// Strategy:
/// 1. Strip leading whitespace.
/// 2. Strip leading block comments (`/* … */`) and line comments (`-- …\n`),
///    repeating until the first non-comment token is reached.
/// 3. Extract the first keyword (alphabetic token, case-insensitive).
/// 4. If the keyword is not `SELECT` or `WITH`, reject.
///
/// This correctly passes:
///   * `"SELECT * FROM foo"` → first keyword is SELECT
///   * `"WITH cte AS (SELECT 1) SELECT * FROM cte"` → first keyword is WITH
///   * `"  \n  SELECT 1"` → first keyword is SELECT (leading whitespace)
///   * `"/* comment */ SELECT 1"` → first keyword is SELECT (comment stripped)
///
/// And correctly rejects:
///   * `"UPDATE foo SET x=1"` → first keyword is UPDATE
///   * `"DROP TABLE foo"` → first keyword is DROP
///   * `"INSERT INTO foo VALUES (1)"` → first keyword is INSERT
///   * `""` → no keyword (empty → reject; callers should never send empty SQL)
fn check_non_select_statement(sql: &str, rejections: &mut Vec<SqlRejection>) {
    let first_kw = first_sql_keyword(sql);
    match first_kw.as_deref() {
        Some("select") | Some("with") => {}
        Some(kw) => {
            rejections.push(SqlRejection::new(
                SqlRule::NonSelectStatement,
                format!(
                    "Only SELECT statements are accepted by the query-execution tool; \
                     received a statement starting with '{}'. Submit a SELECT query \
                     (or a WITH … SELECT common-table-expression).",
                    kw.to_uppercase()
                ),
            ));
        }
        None => {
            // Empty or comment-only SQL — reject (no valid statement).
            rejections.push(SqlRejection::new(
                SqlRule::NonSelectStatement,
                "The SQL string is empty or contains only comments. \
                 Submit a SELECT statement.",
            ));
        }
    }
}

/// Extract the first SQL keyword from `sql`, skipping leading whitespace and
/// comments. Returns the lowercase keyword, or `None` when `sql` contains no
/// alphabetic token outside of comments.
fn first_sql_keyword(sql: &str) -> Option<String> {
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;

    loop {
        // Skip leading whitespace.
        while i < len && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= len {
            return None;
        }

        // Skip line comment: -- … \n
        if i + 1 < len && chars[i] == '-' && chars[i + 1] == '-' {
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue; // restart the outer loop to skip whitespace again
        }

        // Skip block comment: /* … */
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2; // skip closing */
            continue; // restart the outer loop
        }

        // First non-whitespace, non-comment character found.
        // Extract consecutive alphabetic chars as the keyword.
        if chars[i].is_alphabetic() {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let kw: String = chars[start..i].iter().collect::<String>().to_lowercase();
            return Some(kw);
        }

        // Non-alphabetic, non-comment, non-whitespace first character (e.g. `(`).
        // Not a keyword — treat as unknown/invalid.
        return None;
    }
}

// ---------------------------------------------------------------------------
// SqlRule::WindowFunctionInSelect
// ---------------------------------------------------------------------------

/// Detect window functions: an `OVER` keyword followed by `(` outside string
/// literals and comments.
///
/// Strategy (conservative, no full parser):
/// 1. Walk the SQL character by character, tracking string literals, line
///    comments, and block comments (same context-tracking as multi-statement).
/// 2. When the sequence `OVER` (case-insensitive, whole-word: preceded by a
///    non-alphanumeric-underscore char or the start of the string, followed by
///    optional whitespace then `(`) is found outside any context, reject.
///
/// This correctly passes:
///   * `"SELECT store_name, net_sales FROM [Store Sales]"` (no OVER)
///   * `"SELECT 'OVER (1+2)' FROM foo"` (OVER in string literal)
///   * `"SELECT -- OVER (rank)\n1 FROM foo"` (OVER in line comment)
///
/// And correctly rejects:
///   * `"SELECT RANK() OVER (ORDER BY sales) FROM foo"` (window function)
///   * `"SELECT row_number() over(partition by x) FROM foo"` (window function)
fn check_window_function(sql: &str, rejections: &mut Vec<SqlRejection>) {
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

        // Check for OVER keyword (case-insensitive, 4 chars).
        if i + 4 <= len {
            let candidate: String = chars[i..i + 4].iter().collect::<String>().to_lowercase();
            if candidate == "over" {
                // Verify word boundary BEFORE: the preceding char (if any) must
                // not be alphanumeric or underscore.
                let preceded_by_word_char = i > 0
                    && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
                if !preceded_by_word_char {
                    // Check that OVER is followed (with optional whitespace) by `(`.
                    let mut j = i + 4;
                    while j < len && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if j < len && chars[j] == '(' {
                        // Also verify OVER is not part of a longer word (e.g. OVERVIEW).
                        let followed_by_word_char = i + 4 < len
                            && (chars[i + 4].is_alphanumeric() || chars[i + 4] == '_');
                        if !followed_by_word_char {
                            rejections.push(SqlRejection::new(
                                SqlRule::WindowFunctionInSelect,
                                "Window functions (RANK OVER, ROW_NUMBER OVER, etc.) are not \
                                 supported by the query-execution tool. Use ORDER BY + LIMIT on the \
                                 MQO to rank results instead of adding a synthetic rank column.",
                            ));
                            return; // one rejection per query is enough
                        }
                    }
                }
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

    /// Empty SQL is rejected by NonSelectStatement (no SELECT keyword).
    /// The multi-statement check alone would pass, but the non-select check fires.
    #[test]
    fn conforming_empty_sql() {
        let result = validate_sql("");
        // NOTE: empty SQL now triggers NonSelectStatement (no SELECT keyword found).
        // This is the correct behavior — the query-execution tool requires a SELECT.
        // The old "empty passes" invariant no longer holds after iter-2.
        let ns: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::NonSelectStatement).collect();
        assert!(
            !ns.is_empty(),
            "empty SQL must be rejected by NonSelectStatement; got: {:?}",
            result
        );
        // Specifically, MultiStatement must NOT fire for empty SQL.
        let ms: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::MultiStatement).collect();
        assert!(ms.is_empty(), "empty SQL must not fire MultiStatement; got: {:?}", result);
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

    // ── NonSelectStatement: conforming (admit) cases ────────────────────────

    /// A plain SELECT passes.
    #[test]
    fn non_select_conforming_plain_select() {
        let result = validate_sql("SELECT store_name FROM [Store Sales]");
        assert!(result.is_empty(), "plain SELECT must pass; got: {:?}", result);
    }

    /// A WITH … SELECT common-table-expression passes.
    #[test]
    fn non_select_conforming_with_select() {
        let sql = "WITH cte AS (SELECT 1 AS n) SELECT n FROM cte";
        let result = validate_sql(sql);
        assert!(result.is_empty(), "WITH…SELECT must pass; got: {:?}", result);
    }

    /// Leading whitespace before SELECT passes.
    #[test]
    fn non_select_conforming_leading_whitespace() {
        let result = validate_sql("   \n  SELECT 1");
        assert!(result.is_empty(), "leading whitespace SELECT must pass; got: {:?}", result);
    }

    /// A block comment before SELECT passes.
    #[test]
    fn non_select_conforming_block_comment_then_select() {
        let result = validate_sql("/* get revenue */ SELECT revenue FROM [Sales]");
        assert!(result.is_empty(), "comment+SELECT must pass; got: {:?}", result);
    }

    /// A line comment before SELECT passes.
    #[test]
    fn non_select_conforming_line_comment_then_select() {
        let result = validate_sql("-- get revenue\nSELECT revenue FROM [Sales]");
        assert!(result.is_empty(), "line comment+SELECT must pass; got: {:?}", result);
    }

    // ── NonSelectStatement: violating (reject) cases ────────────────────────

    /// UPDATE statement → rejected.
    #[test]
    fn non_select_violating_update() {
        let result = validate_sql("UPDATE foo SET x = 1");
        assert_eq!(result.len(), 1, "UPDATE must be rejected; got: {:?}", result);
        assert_eq!(result[0].rule, SqlRule::NonSelectStatement);
        assert!(result[0].message.contains("UPDATE"), "message must name the offending keyword");
    }

    /// DROP TABLE → rejected.
    #[test]
    fn non_select_violating_drop() {
        let result = validate_sql("DROP TABLE foo");
        assert_eq!(result.len(), 1, "DROP must be rejected; got: {:?}", result);
        assert_eq!(result[0].rule, SqlRule::NonSelectStatement);
    }

    /// INSERT INTO → rejected.
    #[test]
    fn non_select_violating_insert() {
        let result = validate_sql("INSERT INTO foo VALUES (1)");
        assert_eq!(result.len(), 1, "INSERT must be rejected; got: {:?}", result);
        assert_eq!(result[0].rule, SqlRule::NonSelectStatement);
    }

    /// DELETE FROM → rejected.
    #[test]
    fn non_select_violating_delete() {
        let result = validate_sql("DELETE FROM foo WHERE id = 1");
        assert_eq!(result.len(), 1, "DELETE must be rejected; got: {:?}", result);
        assert_eq!(result[0].rule, SqlRule::NonSelectStatement);
    }

    /// CREATE TABLE → rejected.
    #[test]
    fn non_select_violating_create() {
        let result = validate_sql("CREATE TABLE foo (id INT)");
        assert_eq!(result.len(), 1, "CREATE must be rejected; got: {:?}", result);
        assert_eq!(result[0].rule, SqlRule::NonSelectStatement);
    }

    /// Empty string → rejected (no statement).
    #[test]
    fn non_select_violating_empty() {
        let result = validate_sql("");
        // Empty SQL is rejected by NonSelectStatement (no keyword found).
        let ns_rejections: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::NonSelectStatement).collect();
        assert!(!ns_rejections.is_empty(), "empty SQL must produce NonSelectStatement rejection; got: {:?}", result);
    }

    /// NonSelectStatement rejection is JSON-serializable with stable code.
    #[test]
    fn non_select_rejection_json_serializable() {
        let result = validate_sql("UPDATE foo SET x = 1");
        assert!(!result.is_empty());
        let json = serde_json::to_string(&result[0]).expect("must serialize");
        assert!(json.contains("non_select_statement"), "JSON must contain rule code; got: {json}");
    }

    // ── WindowFunctionInSelect: conforming (admit) cases ───────────────────

    /// A plain SELECT with no window function passes.
    #[test]
    fn window_fn_conforming_no_window() {
        let sql = "SELECT store_name, SUM(net_sales) FROM [Store Sales] GROUP BY store_name";
        let result = validate_sql(sql);
        assert!(result.is_empty(), "no window function must pass; got: {:?}", result);
    }

    /// OVER in a string literal must NOT trigger.
    #[test]
    fn window_fn_conforming_over_in_string_literal() {
        let sql = "SELECT 'OVER (partition)' AS note FROM [foo]";
        let result = validate_sql(sql);
        assert!(result.is_empty(), "OVER in string literal must pass; got: {:?}", result);
    }

    /// OVER in a line comment must NOT trigger.
    #[test]
    fn window_fn_conforming_over_in_line_comment() {
        let sql = "SELECT 1 -- OVER (rank) is a window function\nFROM [foo]";
        let result = validate_sql(sql);
        assert!(result.is_empty(), "OVER in line comment must pass; got: {:?}", result);
    }

    /// OVER in a block comment must NOT trigger.
    #[test]
    fn window_fn_conforming_over_in_block_comment() {
        let sql = "SELECT 1 /* OVER (rank) */ FROM [foo]";
        let result = validate_sql(sql);
        assert!(result.is_empty(), "OVER in block comment must pass; got: {:?}", result);
    }

    /// A column named `overview` (OVER embedded in a longer word) must NOT trigger.
    #[test]
    fn window_fn_conforming_over_in_identifier() {
        let sql = "SELECT overview FROM [foo]";
        let result = validate_sql(sql);
        assert!(result.is_empty(), "OVER inside 'overview' must not trigger; got: {:?}", result);
    }

    // ── WindowFunctionInSelect: violating (reject) cases ───────────────────

    /// RANK() OVER (ORDER BY ...) → rejected.
    #[test]
    fn window_fn_violating_rank_over() {
        let sql = "SELECT brand, sales, RANK() OVER (ORDER BY sales DESC) AS rank FROM [foo]";
        let result = validate_sql(sql);
        let wf: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::WindowFunctionInSelect).collect();
        assert!(!wf.is_empty(), "RANK() OVER must be rejected; got: {:?}", result);
        assert!(wf[0].message.contains("ORDER BY") || wf[0].message.contains("LIMIT"),
            "message must guide to ORDER BY+LIMIT; got: {:?}", wf[0].message);
    }

    /// ROW_NUMBER() OVER (PARTITION BY ...) → rejected.
    #[test]
    fn window_fn_violating_row_number_over() {
        let sql = "SELECT x, row_number() over(partition by dept) AS rn FROM [foo]";
        let result = validate_sql(sql);
        let wf: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::WindowFunctionInSelect).collect();
        assert!(!wf.is_empty(), "row_number() over must be rejected; got: {:?}", result);
    }

    /// DENSE_RANK() OVER → rejected.
    #[test]
    fn window_fn_violating_dense_rank_over() {
        let sql = "SELECT x, DENSE_RANK() OVER (ORDER BY x) FROM [foo]";
        let result = validate_sql(sql);
        let wf: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::WindowFunctionInSelect).collect();
        assert!(!wf.is_empty(), "DENSE_RANK() OVER must be rejected; got: {:?}", result);
    }

    /// LAG() OVER → rejected (offset window function).
    #[test]
    fn window_fn_violating_lag_over() {
        let sql = "SELECT x, LAG(x, 1) OVER (ORDER BY t) FROM [foo]";
        let result = validate_sql(sql);
        let wf: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::WindowFunctionInSelect).collect();
        assert!(!wf.is_empty(), "LAG() OVER must be rejected; got: {:?}", result);
    }

    /// WindowFunctionInSelect rejection is JSON-serializable.
    #[test]
    fn window_fn_rejection_json_serializable() {
        let sql = "SELECT RANK() OVER (ORDER BY x) FROM [foo]";
        let result = validate_sql(sql);
        let wf: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::WindowFunctionInSelect).collect();
        assert!(!wf.is_empty(), "must produce WindowFunctionInSelect rejection");
        let json = serde_json::to_string(wf[0]).expect("must serialize");
        assert!(json.contains("window_function_in_select"), "JSON must contain rule code; got: {json}");
    }

    // ── Interaction tests: multiple rules ──────────────────────────────────

    /// A SELECT with both a trailing semicolon and a window function gets the
    /// window-function rejection (multi-statement check passes: trailing semi only).
    #[test]
    fn interaction_trailing_semi_and_window_fn() {
        let sql = "SELECT RANK() OVER (ORDER BY x) FROM [foo];";
        let result = validate_sql(sql);
        // MultiStatement should NOT fire (trailing semi only); WindowFunction should fire.
        let ms: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::MultiStatement).collect();
        let wf: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::WindowFunctionInSelect).collect();
        assert!(ms.is_empty(), "trailing semi must not fire MultiStatement; got: {:?}", result);
        assert!(!wf.is_empty(), "window function must still be rejected; got: {:?}", result);
    }

    /// A non-SELECT DML statement does NOT additionally get a window-function
    /// rejection (the window check skips when NonSelect already fired).
    #[test]
    fn interaction_dml_no_window_double_rejection() {
        // UPDATE with OVER in it — only NonSelectStatement should fire,
        // not WindowFunctionInSelect (window check is gated on no prior rejections).
        let sql = "UPDATE foo SET rank = RANK() OVER (ORDER BY x)";
        let result = validate_sql(sql);
        let ns: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::NonSelectStatement).collect();
        let wf: Vec<_> = result.iter().filter(|r| r.rule == SqlRule::WindowFunctionInSelect).collect();
        assert!(!ns.is_empty(), "DML must fire NonSelectStatement; got: {:?}", result);
        assert!(wf.is_empty(), "window check must be suppressed after NonSelectStatement; got: {:?}", result);
    }
}
