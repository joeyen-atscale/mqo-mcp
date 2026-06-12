//! # mqo-mdx-compiler
//!
//! Compiles a [`BoundMqoInput`] (the JSON produced by `mqo-bind` or compatible
//! binding step) into a syntactically-valid MDX `SELECT` statement.
//!
//! ## Design notes
//!
//! - Cellset semantics: cross-product of selected members.
//! - Emits: `SELECT { measures } ON COLUMNS, NON EMPTY { dims } ON ROWS FROM [Cube]`
//! - Cube name is fully-qualified from `mqo.model` (three-part when a schema is
//!   embedded in the name, otherwise single-bracket form).
//! - `NON EMPTY` is always emitted on the row axis (R13).
//! - Calc-group member literals are inserted verbatim from bound metadata (R7).
//! - For any calculated measure, every hierarchy listed in
//!   `mdx_dependency_hierarchies` is added to the row axis (R6).
//! - For any semi-additive measure, a non-empty `trigger_hierarchies` list is
//!   required — if absent the compiler raises
//!   [`MdxCompileError::SemiAdditiveMissingTrigger`] (R11).

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]

pub mod codegen;
pub mod input;
pub mod syntax_check;

pub use codegen::compile;
pub use input::{BoundDimensionInput, BoundMeasureInput, BoundMqoInput, CalcGroupMemberInput};
pub use syntax_check::validate_mdx_syntax;

use thiserror::Error;

#[cfg(test)]
mod tests;

/// Errors that can occur during MDX compilation.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum MdxCompileError {
    /// The bound MQO has no measures — cannot emit a valid SELECT.
    #[error("bound MQO must have at least one measure")]
    EmptyMeasures,

    /// A semi-additive measure is present but has no trigger level (R11).
    #[error(
        "semi-additive measure '{0}' requires a trigger level but none was provided"
    )]
    SemiAdditiveMissingTrigger(String),

    /// JSON deserialization failed.
    #[error("failed to deserialize BoundMqo JSON: {0}")]
    DeserializeError(String),

    /// The compiled MDX failed the pre-flight structural syntax check.
    #[error("MDX syntax check failed: {0}")]
    SyntaxCheckFailed(String),
}
