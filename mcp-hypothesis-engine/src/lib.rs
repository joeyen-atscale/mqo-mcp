// Re-export the engine API for integration tests.
pub mod engine;
pub use engine::{
    bfs_inbound, run_engine, CandidatePath, Corroboration, HypothesisSet, Hypothesis,
    extract_mean, compute_delta, confidence,
};
