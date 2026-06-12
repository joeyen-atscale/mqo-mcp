//! Bundled DAX syntax validation.
//!
//! This is a structural validator — it checks that the emitted DAX has the
//! correct outer `EVALUATE` keyword, balanced parentheses, balanced brackets,
//! balanced double-quotes, and no obviously invalid tokens.
//!
//! No engine round-trip is required. This fulfils AC5 of the PRD.

/// Validate that a DAX string is structurally well-formed.
///
/// Checks performed:
/// 1. Must start with `EVALUATE` (case-insensitive, leading whitespace ignored).
/// 2. Parentheses must be balanced.
/// 3. Square brackets must be balanced.
/// 4. Double-quote strings must be balanced (no unclosed string literals).
/// 5. Must contain at least one recognised DAX table function or row construct.
///
/// # Errors
///
/// Returns `Err(String)` with a description when validation fails.
pub fn validate_dax_syntax(dax: &str) -> Result<(), String> {
    let trimmed = dax.trim();

    // Rule 1: Must start with EVALUATE (optionally ORDER BY follows).
    if !trimmed.to_uppercase().starts_with("EVALUATE") {
        return Err(format!(
            "DAX does not start with EVALUATE (got: {:?})",
            &trimmed[..trimmed.len().min(30)]
        ));
    }

    // Rule 2-4: balanced parens, brackets, quotes.
    let mut paren_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in trimmed.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return Err("unmatched closing parenthesis ')'".to_string());
                }
            }
            '[' => bracket_depth += 1,
            ']' => {
                bracket_depth -= 1;
                if bracket_depth < 0 {
                    return Err("unmatched closing bracket ']'".to_string());
                }
            }
            _ => {}
        }
    }

    if in_string {
        return Err("unclosed string literal (missing closing '\"')".to_string());
    }
    if paren_depth != 0 {
        return Err(format!(
            "unmatched parentheses: depth = {paren_depth} at end of input"
        ));
    }
    if bracket_depth != 0 {
        return Err(format!(
            "unmatched square brackets: depth = {bracket_depth} at end of input"
        ));
    }

    // Rule 5: Must contain at least one recognised DAX table construct.
    let upper = trimmed.to_uppercase();
    let has_construct = upper.contains("SUMMARIZECOLUMNS")
        || upper.contains("ROW(")
        || upper.contains("TOPN(")
        || upper.contains("CALCULATE(")
        || upper.contains("FILTER(")
        || upper.contains("ALL(")
        || upper.contains("VALUES(")
        || upper.contains("ADDCOLUMNS(");

    if !has_construct {
        return Err(
            "DAX does not contain any recognised table/row construct (SUMMARIZECOLUMNS, ROW, TOPN, CALCULATE, …)".to_string()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_evaluate_row() {
        let dax = r#"EVALUATE
ROW("Revenue", [Revenue])"#;
        assert!(validate_dax_syntax(dax).is_ok());
    }

    #[test]
    fn valid_summarize_columns() {
        let dax = r#"EVALUATE
SUMMARIZECOLUMNS(Calendar[Year], "Revenue", [Revenue])"#;
        assert!(validate_dax_syntax(dax).is_ok());
    }

    #[test]
    fn missing_evaluate() {
        let dax = r#"SUMMARIZECOLUMNS(Calendar[Year], "Revenue", [Revenue])"#;
        assert!(validate_dax_syntax(dax).is_err());
    }

    #[test]
    fn unmatched_paren() {
        let dax = "EVALUATE\nROW(\"Revenue\", [Revenue]";
        assert!(validate_dax_syntax(dax).is_err());
    }

    #[test]
    fn unmatched_bracket() {
        let dax = "EVALUATE\nROW(\"Revenue\", [Revenue)";
        // bracket is unmatched since ] never appears
        let res = validate_dax_syntax(dax);
        assert!(res.is_err());
    }

    /// Kill mutant: `replace < with >` on `paren_depth < 0` guard.
    /// An extra `)` after valid expression must be rejected.
    #[test]
    fn extra_closing_paren_is_rejected() {
        let dax = "EVALUATE\nROW(\"Revenue\", [Revenue]))";
        assert!(validate_dax_syntax(dax).is_err(), "extra ')' must fail");
    }

    /// Kill mutant: `delete match arm '"'` (string delimiter tracking).
    /// A quote inside what would be a string should not count as a real `(`.
    #[test]
    fn parens_inside_string_literal_do_not_count() {
        // The "((" inside the string is not real parens — must still pass.
        let dax = r#"EVALUATE
ROW("hello (( world", [Revenue])"#;
        assert!(
            validate_dax_syntax(dax).is_ok(),
            "parens inside string literal must not affect paren depth"
        );
    }

    /// Kill mutants: individual `|| → &&` substitutions on `has_construct` OR chain.
    /// Each test uses ONLY one recognised construct so that flipping any single `||`
    /// to `&&` would require ALL terms to be true — and the test would fail.
    #[test]
    fn has_construct_topn_only() {
        // TOPN alone — no SUMMARIZECOLUMNS/ROW/CALCULATE/etc.
        let dax = "EVALUATE\nTOPN(10, someTable, [Revenue], DESC)";
        assert!(validate_dax_syntax(dax).is_ok(), "TOPN alone must pass");
    }

    #[test]
    fn has_construct_calculate_only() {
        let dax = "EVALUATE\nCALCULATE([Revenue], SAMEPERIODLASTYEAR(DateTable[Date]))";
        assert!(
            validate_dax_syntax(dax).is_ok(),
            "CALCULATE alone must pass"
        );
    }

    #[test]
    fn has_construct_filter_only() {
        let dax = "EVALUATE\nFILTER(Sales, Sales[Year] = 2024)";
        assert!(validate_dax_syntax(dax).is_ok(), "FILTER alone must pass");
    }

    #[test]
    fn has_construct_all_only() {
        let dax = "EVALUATE\nALL(Calendar[Year])";
        assert!(validate_dax_syntax(dax).is_ok(), "ALL alone must pass");
    }

    #[test]
    fn has_construct_values_only() {
        let dax = "EVALUATE\nVALUES(Calendar[Year])";
        assert!(validate_dax_syntax(dax).is_ok(), "VALUES alone must pass");
    }

    #[test]
    fn has_construct_addcolumns_only() {
        let dax = "EVALUATE\nADDCOLUMNS(Sales, \"Extra\", 1)";
        assert!(
            validate_dax_syntax(dax).is_ok(),
            "ADDCOLUMNS alone must pass"
        );
    }

    /// No recognized construct → must fail.
    #[test]
    fn no_construct_fails() {
        let dax = "EVALUATE\n[Revenue]";
        assert!(
            validate_dax_syntax(dax).is_err(),
            "bare measure ref with no table construct must fail"
        );
    }
}
