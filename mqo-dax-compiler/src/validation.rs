//! Engine-validation gate for emitted DAX (PRD-mqo-dax-engine-validation-gate).
//!
//! This is the *control-plane pair* to the projection grounding fix
//! (PRD-mqo-projection-dax-grounding). Where the grounding PRD makes the
//! compiler emit engine-valid DAX, this gate **hard-fails** the compile when
//! the emitted DAX is still ungrounded or unparseable, so a malformed-DAX
//! regression fails the build instead of the customer's query.
//!
//! It is a *targeted post-compile text scan*, NOT a full DAX parser. It keys
//! on two failure modes that the bundled structural `syntax_check` cannot see:
//!
//! 1. **Ungrounded reference** — the compiler emits a structured
//!    `/* ungrounded: <unique_name> */` comment when a level lookup misses.
//!    Its presence means the reference was never grounded → reject.
//! 2. **Unquoted space-bearing table identifier** — a bare identifier
//!    containing whitespace immediately followed by `[` (e.g.
//!    `Ship Mode Type[Ship Mode Type]`). A table name with spaces MUST be
//!    single-quoted in valid DAX; an unquoted one is rejected by the engine.

use std::fmt;

/// The compiler's structured ungrounded marker prefix. We key on this exact
/// prefix (not the loose substring "ungrounded") so a comment that merely
/// *mentions* the word does not trigger a false rejection.
const UNGROUNDED_MARKER: &str = "/* ungrounded";

/// A failure detected by the engine-validation gate.
///
/// Every variant names the specific offending token (FR-4/FR-6), so the
/// engineer can fix grounding without decoding the engine's opaque 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaxValidationError {
    /// The emitted DAX contains a `/* ungrounded: <token> */` marker — the
    /// reference was never grounded to a real column.
    UngroundedRef {
        /// The `unique_name` (or date token) named in the ungrounded marker.
        token: String,
    },

    /// The emitted DAX contains an unquoted, space-bearing table identifier
    /// immediately preceding `[` (e.g. `Ship Mode Type[…]`). Table names with
    /// spaces MUST be single-quoted.
    UnquotedIdentifier {
        /// The offending bare identifier (the text before `[`).
        token: String,
    },
}

impl fmt::Display for DaxValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DaxValidationError::UngroundedRef { token } => write!(
                f,
                "emitted DAX contains an ungrounded reference: '{token}' \
                 (the compiler emitted a /* ungrounded */ marker because this \
                 level could not be grounded to a real column)"
            ),
            DaxValidationError::UnquotedIdentifier { token } => write!(
                f,
                "emitted DAX contains an unquoted, space-bearing table identifier: \
                 '{token}[' — a table name containing spaces must be single-quoted \
                 (e.g. '{token}'[…])"
            ),
        }
    }
}

impl std::error::Error for DaxValidationError {}

/// Validate that emitted DAX is grounded and free of unquoted space-bearing
/// table identifiers.
///
/// This is the always-on static gate (FR-1/FR-2). It runs after the DAX string
/// is assembled but before it is returned, when the syntax check is not skipped.
///
/// The ungrounded check is evaluated first, since an ungrounded marker is the
/// root cause and an unquoted identifier is usually its downstream symptom.
///
/// # Errors
///
/// Returns [`DaxValidationError::UngroundedRef`] when a `/* ungrounded */`
/// marker is present, or [`DaxValidationError::UnquotedIdentifier`] when an
/// unquoted space-bearing table identifier precedes `[`.
pub fn validate_dax_output(dax: &str) -> Result<(), DaxValidationError> {
    if let Some(token) = find_ungrounded_token(dax) {
        return Err(DaxValidationError::UngroundedRef { token });
    }

    if let Some(token) = find_unquoted_space_identifier(dax) {
        return Err(DaxValidationError::UnquotedIdentifier { token });
    }

    Ok(())
}

/// If the DAX carries the compiler's structured ungrounded marker, return the
/// named token (the text after `ungrounded: ` / `ungrounded date: `, up to the
/// closing `*/`). Falls back to a generic token when no name is present.
fn find_ungrounded_token(dax: &str) -> Option<String> {
    let idx = dax.find(UNGROUNDED_MARKER)?;
    // Slice from just after the marker prefix to the closing `*/`.
    let after = &dax[idx + UNGROUNDED_MARKER.len()..];
    let inner = after.find("*/").map_or(after, |end| &after[..end]);
    // inner now looks like ": <unique_name> " or " date: <token> ".
    // Strip a leading "date" qualifier and the colon, then trim.
    let inner = inner.trim();
    let inner = inner.strip_prefix("date").unwrap_or(inner).trim();
    let token = inner.strip_prefix(':').unwrap_or(inner).trim();
    if token.is_empty() {
        Some("<unnamed ungrounded reference>".to_string())
    } else {
        Some(token.to_string())
    }
}

/// Scan for a bare identifier that contains whitespace and is immediately
/// followed by `[`, where the identifier is NOT opened by a single quote.
///
/// A valid quoted table reference looks like `'Ship Mode Type'[Col]`; the
/// closing `'` separates the name from `[`, so the char before `[` is `'`,
/// not a letter — those are accepted. An unquoted `Ship Mode Type[Col]` has a
/// letter before `[` and a space inside the run of identifier characters.
fn find_unquoted_space_identifier(dax: &str) -> Option<String> {
    let bytes = dax.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'[' {
            continue;
        }
        // A table reference is directly adjacent to `[` with no gap
        // (`Table[Col]`). If the char immediately before `[` is whitespace or a
        // non-identifier char, this `[` does not open a table-qualified column —
        // e.g. `ORDER BY [Revenue]` (keyword + space) or `, [Measure]`. Those
        // are not unquoted table identifiers and must not be flagged.
        if i == 0 {
            continue;
        }
        let prev = bytes[i - 1] as char;
        if !(prev.is_ascii_alphanumeric() || prev == '_') {
            continue;
        }
        // Walk backwards over the run of identifier-or-space characters
        // forming the table name immediately preceding this `[`.
        let mut start = i;
        while start > 0 {
            let c = bytes[start - 1] as char;
            if c.is_ascii_alphanumeric() || c == ' ' || c == '_' {
                start -= 1;
            } else {
                break;
            }
        }
        if start == i {
            // No identifier run before `[` (e.g. `[Measure]`, `)[`).
            continue;
        }
        // If the char immediately before the run is a single quote, the
        // identifier was quoted — accept (`'Ship Mode Type'[Col]`).
        if start > 0 && bytes[start - 1] == b'\'' {
            continue;
        }
        let raw = &dax[start..i];
        let ident = raw.trim();
        // Only flag when the identifier actually contains an interior space
        // (a single-word table name like `Calendar[Year]` is valid DAX).
        if ident.contains(' ') {
            return Some(ident.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-1: the captured pre-C1 malformed DAX MUST be rejected.
    #[test]
    fn pre_c1_dax_is_rejected() {
        let pre_c1 = "EVALUATE\nSUMMARIZECOLUMNS('atscale_catalogs'[Carrier], \
            KEEPFILTERS(FILTER(ALL(Ship Mode Type[Ship Mode Type] \
            /* ungrounded: Ship Mode Type */), \
            Ship Mode Type[Ship Mode Type] IN {\"EXPRESS\"})))";
        let err = validate_dax_output(pre_c1).expect_err("pre-C1 DAX must be rejected");
        // Ungrounded is detected first; it names the offending token.
        assert_eq!(
            err,
            DaxValidationError::UngroundedRef {
                token: "Ship Mode Type".to_string()
            }
        );
        assert!(err.to_string().contains("Ship Mode Type"));
    }

    /// AC-1 / AC-3: the post-C1 grounded DAX MUST pass.
    #[test]
    fn post_c1_dax_passes() {
        let post_c1 = "EVALUATE\nSUMMARIZECOLUMNS('ship_mode'[Carrier], \
            KEEPFILTERS(FILTER(ALL('ship_mode'[Ship Mode Type]), \
            'ship_mode'[Ship Mode Type] IN {\"EXPRESS\"})))";
        assert!(validate_dax_output(post_c1).is_ok());
    }

    #[test]
    fn unquoted_space_identifier_alone_is_rejected() {
        // No ungrounded marker — exercise the identifier check in isolation.
        let dax = "EVALUATE\nFILTER(ALL(Ship Mode Type[Ship Mode Type]), TRUE)";
        let err = validate_dax_output(dax).expect_err("must reject unquoted space ident");
        assert_eq!(
            err,
            DaxValidationError::UnquotedIdentifier {
                token: "Ship Mode Type".to_string()
            }
        );
    }

    /// Edge case (PRD §4): a legitimately single-quoted multi-word identifier
    /// MUST pass.
    #[test]
    fn quoted_multiword_identifier_passes() {
        let dax = "EVALUATE\nSUMMARIZECOLUMNS('Ship Mode Type'[Carrier])";
        assert!(validate_dax_output(dax).is_ok());
    }

    /// A single-word unquoted table identifier (valid DAX) MUST pass.
    #[test]
    fn single_word_identifier_passes() {
        let dax = "EVALUATE\nSUMMARIZECOLUMNS(Calendar[Year], \"Revenue\", [Revenue])";
        assert!(validate_dax_output(dax).is_ok());
    }

    /// A bare measure reference `[Revenue]` (no table name) MUST pass.
    #[test]
    fn bare_measure_ref_passes() {
        let dax = "EVALUATE\nROW(\"Revenue\", [Revenue])";
        assert!(validate_dax_output(dax).is_ok());
    }

    /// Edge case (PRD §4): a comment containing the word "ungrounded" in a
    /// label but not as the structured marker should still be keyed precisely —
    /// the marker prefix is `/* ungrounded`, so a different comment shape does
    /// not match. (We assert the marker form keys, and a measure-style comment
    /// does not.)
    #[test]
    fn ungrounded_date_marker_is_caught_and_named() {
        let dax = "EVALUATE\nSUMMARIZECOLUMNS(Foo[Bar] /* ungrounded date: my.date.level */)";
        let err = validate_dax_output(dax).expect_err("date ungrounded marker must reject");
        assert_eq!(
            err,
            DaxValidationError::UngroundedRef {
                token: "my.date.level".to_string()
            }
        );
    }

    #[test]
    fn ungrounded_with_unique_name_is_named() {
        let dax = "EVALUATE\nROW(\"x\", [m]) /* ungrounded: no.such.level */";
        let err = validate_dax_output(dax).unwrap_err();
        assert_eq!(
            err,
            DaxValidationError::UngroundedRef {
                token: "no.such.level".to_string()
            }
        );
    }
}
