//! # mqo-mcp-server
//!
//! The capstone of the MQO fleet: an MCP server whose headline tool,
//! `query_multidimensional`, accepts a **Multidimensional Query Object** —
//! never raw SQL — and runs the full pipeline:
//!
//! ```text
//! MQO ──▶ mqo-bind ──▶ mqo-route ──▶ mqo-dax | mqo-mdx | sql-projection ──▶ engine ──▶ bounded rows
//! ```
//!
//! The fleet is a JSON pipeline of CLI subprocesses, not a library graph: this
//! server orchestrates the published `mqo-bind`, `mqo-route`, `mqo-dax`, and
//! `mqo-mdx` binaries, passing JSON on disk per each tool's documented CLI
//! contract.
//!
//! Engine selection: without `--endpoint`, a `FixtureEngine` (from
//! `mqo-auth-bridge`) is used for deterministic cluster-free CI. With
//! `--endpoint` + OIDC flags, a `LiveExecutor` is constructed and the compiled
//! query is sent to a live `AtScale` endpoint.
//!
//! Read-only by construction: the only query input is a selection-only object,
//! so the "can this tool write?" concern disappears. The three catalog tools are
//! advertised with `readOnlyHint: true`.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod chart_tools;
pub mod cursor;
pub mod handle_ops;
pub mod mcp;
pub mod pipeline;
pub mod probe;
pub mod routing;

// Re-export bridge types used by tests and callers.
pub use mqo_auth_bridge::{
    Backend, EndpointConfig, Engine, EngineError, EngineResult, FixtureEngine, LiveExecutor,
    OidcConfig, RowSource,
};

pub use handle_ops::{
    dataset_to_json_rows, json_rows_to_dataset, json_rows_to_dataset_with_bound, HandleStore,
    INLINE_THRESHOLD,
};
pub use mcp::{discover_xmla_coords, tool_descriptors, Server, ServerEnrichedData, ServerEngine, PROTOCOL_VERSION};
pub use pipeline::{error_class, error_class_values, PipelineError, PipelineOutput, ToolPaths};
pub use probe::{BackendCapabilities, PortStatus};
pub use routing::{run_health_check_sync, select_cluster, RoutingError};
