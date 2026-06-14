//! # mqo-dax-compiler
//!
//! Compiles a [`BoundMqoInput`] (the JSON produced by `mqo-bind` or compatible
//! binding step) into a syntactically-valid DAX `EVALUATE` statement.
//!
//! ## Design notes
//!
//! - This crate defines its own local deserialization types rather than
//!   depending on the binder crate, keeping the dep graph minimal.
//! - The emitter uses `SUMMARIZECOLUMNS` for grouped queries and bare
//!   `EVALUATE ROW(…)` for measure-only queries.
//! - Time-intelligence variants are each translated to the canonical DAX
//!   function/pattern per the PRD mapping.
//! - Calc-group filters are emitted as column-equality filters on the
//!   calc-group column (not invented MDX logic).

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]

pub mod catalog_context;
pub mod codegen;
pub mod input;
pub mod syntax_check;
pub mod validation;

pub use catalog_context::DaxCatalogContext;
pub use codegen::{compile, compile_grounded};
pub use input::{BoundMqoInput, BoundMeasureInput, BoundDimensionInput, CalcGroupMemberInput};
pub use validation::{validate_dax_output, DaxValidationError};

use thiserror::Error;

/// Errors that can occur during DAX compilation.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DaxCompileError {
    /// The bound MQO has no measures — cannot emit a valid EVALUATE.
    #[error("bound MQO must have at least one measure")]
    EmptyMeasures,

    /// A time-intelligence op references a measure that isn't in the query.
    #[error("TimeIntel references measure '{0}' which is not in the query")]
    UnknownTimeIntelMeasure(String),

    /// A Share time-intel op was requested but `of_level` is empty.
    #[error("Share time-intel requires a non-empty of_level")]
    EmptyShareLevel,

    /// JSON deserialization failed.
    #[error("failed to deserialize BoundMqo JSON: {0}")]
    DeserializeError(String),

    /// The emitted DAX failed the bundled syntax check.
    #[error("emitted DAX failed syntax check: {0}")]
    SyntaxCheckFailed(String),

    /// The emitted DAX failed the engine-validation gate — it contained an
    /// ungrounded reference or an unquoted space-bearing table identifier
    /// (PRD-mqo-dax-engine-validation-gate). The wrapped error names the
    /// specific offending token.
    #[error("emitted DAX failed the engine-validation gate: {0}")]
    ValidationFailed(#[from] DaxValidationError),

    /// A `Member` filter with an empty members list — an empty IN-set is
    /// not valid DAX.
    #[error(
        "Member filter on hierarchy '{hierarchy}' has an empty members list; \
         an empty IN-set is not valid DAX"
    )]
    EmptyMemberFilter {
        /// The hierarchy the filter targeted.
        hierarchy: String,
    },

    /// A `Member` filter could not be resolved to a real level-qualified
    /// column reference.
    ///
    /// This fires when either (a) no `DaxCatalogContext` was supplied, or
    /// (b) the context carries no level entries for the named hierarchy.
    ///
    /// To fix: supply a `DaxCatalogContext` that covers this hierarchy.
    #[error(
        "Member filter on hierarchy '{hierarchy}' could not be grounded to a level \
         (members: [{members}]): no level's domain contains the value, or the value is \
         ambiguous across levels. Specify the level explicitly, or query with the \
         intended level as a dimension to disambiguate."
    )]
    UngroundedMemberFilter {
        /// The hierarchy name from the filter spec.
        hierarchy: String,
        /// The member keys, joined for display.
        members: String,
    },

    /// A `Range` filter's `level` field could not be resolved to a real
    /// level-qualified column reference.
    ///
    /// This fires when a `DaxCatalogContext` is present but `level` is
    /// neither a known unique-name (key in `labels`) nor a display label
    /// with a reverse-lookup hit.  Emitting the heuristic `Level[Level]`
    /// would produce a column the engine rejects with an opaque 500.
    ///
    /// To fix: pass either the fully-qualified level unique-name
    /// (e.g. `"sold_date_dimensions.[Sold Calendar Year]"`) or the exact
    /// display label that appears in `describe_model` output
    /// (e.g. `"Sold Calendar Year"`).
    #[error(
        "Range filter on level '{level}' cannot be resolved to a real column \
         (neither a known unique-name nor a recognized display label); \
         use a fully-qualified unique-name or an exact display label from describe_model"
    )]
    UngroundedRangeFilter {
        /// The level identifier that could not be resolved.
        level: String,
    },

    /// A projection dimension level (or a `MemberLevel` filter level) could not
    /// be grounded to a physical `'Table'[Column]` reference.
    ///
    /// This fires when a `DaxCatalogContext` is present but the level is neither a
    /// known `unique_name` (key in `labels`/`tables`) nor a recognized display
    /// label, so no per-level physical table can be derived. Emitting a
    /// `/* ungrounded */` reference would produce DAX the XMLA engine rejects with
    /// an opaque 500; this typed decline names the unmappable level instead
    /// (PRD-mqo-projection-dax-grounding, FR-4).
    ///
    /// To fix: supply a catalog that covers this level, or pass the level's
    /// fully-qualified `unique_name` / exact display label from `describe_model`.
    #[error(
        "level '{unique_name}' cannot be grounded to a physical 'Table'[Column] \
         reference (not a known unique-name or display label in the catalog); \
         supply a catalog covering it or pass its fully-qualified unique-name"
    )]
    UngroundableLevel {
        /// The level identifier (unique-name or label) that could not be grounded.
        unique_name: String,
    },

    /// A time-intelligence op is not supported by the target engine.
    ///
    /// This error is raised **before** any DAX is dispatched, so callers can
    /// distinguish "the op is unsupported" (model/path error) from "the backend
    /// is down" (infra/transport error) without string-matching opaque messages.
    ///
    /// The `op` field names the operation (e.g. `"YoY"`, `"ToDate"`), and
    /// `reason` explains why it is unsupported (e.g. "requires Mark-as-Date-Table
    /// designation not present on this model").
    #[error("time-intelligence op '{op}' is not supported by the target engine: {reason}")]
    UnsupportedTimeIntelligence {
        /// The operation that is not supported (e.g. `"YoY"`, `"PriorPeriod"`, `"ToDate"`).
        op: String,
        /// Human-readable reason (e.g. "requires Mark-as-Date-Table designation not provided by `AtScale` XMLA").
        reason: String,
    },
}
