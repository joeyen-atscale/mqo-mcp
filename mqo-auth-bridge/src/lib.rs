//! # mqo-auth-bridge
//!
//! OIDC client-credentials token flow + live query executor for the MQO MCP
//! server.
//!
//! The MQO MCP server runs a full **bind → route → compile** pipeline and then
//! calls an engine. `mqo-auth-bridge` provides:
//!
//! - [`Engine`] — the trait the server programs against.
//! - [`FixtureEngine`] — the deterministic synth engine (cluster-free CI).
//! - [`LiveExecutor`] — authenticates via OIDC client-credentials, then sends
//!   the compiled query to a live `AtScale` endpoint (DAX/SQL over `PGWire`,
//!   MDX over XMLA).
//! - [`OidcConfig`] + token caching — `OAuth2` client-credentials grant with
//!   token caching and refresh-on-expiry.
//!
//! ## Secret handling
//!
//! `OidcConfig.client_secret_env_var` carries the *name* of an environment
//! variable, not the secret itself. The secret is read from the environment at
//! token-fetch time and is never stored in any struct field or logged.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod backend;
pub mod engine;
pub mod error;
pub mod executor;
pub mod fixture;
pub mod oidc;
pub mod xmla;

pub use backend::Backend;
pub use engine::{
    Engine, EngineResult, DEFAULT_MAX_RESULT_ROWS, HARD_ROW_CAP, MAX_RESULT_ROWS_CEILING,
};
pub use error::EngineError;
pub use executor::{
    EndpointConfig, LiveExecutor, RowSource, DEFAULT_QUERY_DEADLINE_MAX_SECS,
    DEFAULT_QUERY_DEADLINE_SECS, DEADLINE_EXCEEDED_HINT,
};
pub use fixture::FixtureEngine;
pub use oidc::OidcConfig;
