//! Error types for dataset lookup.

use std::fmt;

/// Result of looking up a [`DatasetHandle`] in the store.
///
/// This is a tri-state so callers can distinguish *never existed* from *existed
/// but expired* — important for user-facing error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    /// The handle was never inserted (or was already evicted for size reasons).
    NotFound,
    /// The handle existed but its TTL has elapsed.
    ///
    /// This is only returned after [`Store::evict_expired`] has been called;
    /// before eviction the entry may still appear as [`NotFound`] on some
    /// implementations.  After eviction the tombstone is present and
    /// `Expired` is returned.
    Expired,
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "dataset not found"),
            Self::Expired => write!(f, "dataset handle has expired"),
        }
    }
}

impl std::error::Error for LookupError {}
