//! OIDC client-credentials token flow with caching.
//!
//! # Secret handling
//!
//! `OidcConfig.client_secret_env_var` holds the *name* of an environment
//! variable; the actual secret is read from the environment at fetch time.
//! The secret value is **never** stored in any struct and will not appear in
//! `Debug` or `Display` output for any type in this module.

use std::{
    env,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::error::EngineError;

/// Configuration for OIDC client-credentials grant.
///
/// # Debug output
///
/// `client_secret_env_var` is the environment variable *name*, not the secret.
/// The secret value is never stored in this struct.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Full URL to the OIDC token endpoint, e.g.
    /// `http://localhost:8080/realms/community-identity/protocol/openid-connect/token`.
    pub token_url: String,
    /// `OAuth2` client identifier.
    pub client_id: String,
    /// Name of the environment variable that holds the client secret.
    /// The secret itself is never stored here.
    pub client_secret_env_var: String,
    /// Keycloak realm name (informational; embedded in `token_url` for most
    /// flows, but stored here for observability).
    pub realm: String,
    /// ROPC username. When `Some`, the token fetch uses `grant_type=password`
    /// (Resource Owner Password Credentials) instead of `client_credentials`.
    /// Required for clusters that do not support service-account OIDC.
    pub username: Option<String>,
    /// Name of the environment variable holding the ROPC user password.
    /// Only used when `username` is `Some`. Never stored as a value.
    pub password_env_var: Option<String>,
}

/// How many seconds before expiry we consider the token stale and refresh.
const SKEW_SECONDS: u64 = 30;

/// A cached bearer token with its expiry instant.
#[derive(Debug, Clone)]
pub struct Token {
    /// The bearer token string.
    pub access_token: String,
    /// When this token expires (wall-clock monotonic).
    pub expires_at: Instant,
}

impl Token {
    /// Returns `true` if the token is still valid (not within the skew window).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        Instant::now() + Duration::from_secs(SKEW_SECONDS) < self.expires_at
    }
}

/// Shared token cache used inside [`TokenCache`].
#[derive(Debug, Default)]
struct Inner {
    cached: Option<Token>,
}

/// Thread-safe token cache.
///
/// Wraps the OIDC config and keeps a cached token that is refreshed on expiry.
#[derive(Debug, Clone)]
pub struct TokenCache {
    config: OidcConfig,
    inner: Arc<Mutex<Inner>>,
}

/// Raw response shape from the OIDC token endpoint.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl TokenCache {
    /// Create a new token cache for the given OIDC config.
    #[must_use]
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Return a valid bearer token, fetching or refreshing as needed.
    ///
    /// Uses an async HTTP client (reqwest). Call from a Tokio runtime.
    ///
    /// # Errors
    ///
    /// - [`EngineError::MissingSecret`] if the env var is absent.
    /// - [`EngineError::AuthFailure`] if the token endpoint returns a non-2xx
    ///   response or an unparseable body.
    /// - [`EngineError::Http`] for transport-level failures.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should never happen in
    /// normal use).
    pub async fn fetch_token(&self) -> Result<Token, EngineError> {
        // Check cache under lock (clone out to drop lock before async I/O).
        let cached = {
            let guard = self.inner.lock().expect("token cache mutex poisoned");
            guard.cached.clone()
        };

        if let Some(t) = cached {
            if t.is_valid() {
                return Ok(t);
            }
        }

        // Cache miss or expired — fetch a new token.
        let token = self.do_fetch().await?;

        // Store in cache.
        {
            let mut guard = self.inner.lock().expect("token cache mutex poisoned");
            guard.cached = Some(token.clone());
        }

        Ok(token)
    }

    /// Perform the actual token POST — `client_credentials` or ROPC depending on config.
    async fn do_fetch(&self) -> Result<Token, EngineError> {
        // Read the client secret from the environment — never store it.
        let secret = env::var(&self.config.client_secret_env_var).map_err(|_| {
            EngineError::MissingSecret {
                var_name: self.config.client_secret_env_var.clone(),
            }
        })?;

        let client = reqwest::Client::new();

        // Use ROPC when a username is configured; otherwise fall back to client_credentials.
        let resp = if let Some(ref username) = self.config.username {
            let password_var = self.config.password_env_var.as_deref().unwrap_or("");
            let password = env::var(password_var).map_err(|_| EngineError::MissingSecret {
                var_name: password_var.to_string(),
            })?;
            client
                .post(&self.config.token_url)
                .form(&[
                    ("grant_type", "password"),
                    ("client_id", &self.config.client_id),
                    ("client_secret", &secret),
                    ("username", username),
                    ("password", &password),
                ])
                .send()
                .await?
        } else {
            client
                .post(&self.config.token_url)
                .form(&[
                    ("grant_type", "client_credentials"),
                    ("client_id", &self.config.client_id),
                    ("client_secret", &secret),
                ])
                .send()
                .await?
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(EngineError::AuthFailure {
                reason: format!("token endpoint returned {status}: {body}"),
            });
        }

        let tr: TokenResponse = resp.json().await.map_err(|e| EngineError::AuthFailure {
            reason: format!("failed to parse token response: {e}"),
        })?;

        let expires_at = Instant::now() + Duration::from_secs(tr.expires_in);
        Ok(Token {
            access_token: tr.access_token,
            expires_at,
        })
    }
}
