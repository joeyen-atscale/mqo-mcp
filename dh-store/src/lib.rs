//! # dh-store
//!
//! In-memory columnar dataset store with opaque handles, TTL + LRU eviction,
//! a total-size cap, and immutable derive-new-handle semantics.
//!
//! The LLM-visible surface is a [`DatasetHandle`] (from `dh-spec`); the actual
//! bytes live here, never in the context window.
//!
//! ## Immutability guarantee
//!
//! There is **no public mutation API**.  The only way to produce a changed
//! dataset is [`Store::derive`], which always allocates a fresh handle.
//! The original dataset is left untouched and remains retrievable under the
//! old handle until it expires or is evicted.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod dataset;
pub mod error;
pub mod store;

pub use dataset::{ColumnData, Dataset};
pub use dh_spec::{ColumnSchema, DatasetHandle, Lineage};
pub use error::LookupError;
pub use store::{Stats, Store};
