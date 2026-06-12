//! AC3: OIDC client-credentials token flow against a wiremock stub.
//!
//! - `fetch_token()` performs a `grant_type=client_credentials` POST.
//! - A second call within lifetime is served from cache (exactly 1 HTTP call).
//! - An expired token triggers a refresh (2 HTTP calls total).

use std::{env, time::Duration};

use mqo_auth_bridge::{oidc::TokenCache, OidcConfig};
use wiremock::{
    matchers::{body_string_contains, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn make_config(token_url: String) -> OidcConfig {
    OidcConfig {
        token_url,
        client_id: "test-client".to_string(),
        client_secret_env_var: "TEST_OIDC_SECRET".to_string(),
        realm: "test-realm".to_string(),
        username: None,
        password_env_var: None,
    }
}

#[tokio::test]
async fn fetch_token_calls_token_endpoint() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "tok-abc",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .expect(1) // exactly one call
        .mount(&server)
        .await;

    env::set_var("TEST_OIDC_SECRET", "super-secret");
    let config = make_config(format!("{}/token", server.uri()));
    let cache = TokenCache::new(config);

    let token = cache.fetch_token().await.expect("should fetch token");
    assert_eq!(token.access_token, "tok-abc");

    // server mock expectation enforces exactly 1 call when MockServer drops
}

#[tokio::test]
async fn second_call_within_lifetime_is_cached() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "tok-cached",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .expect(1) // only ONE call total
        .mount(&server)
        .await;

    env::set_var("TEST_OIDC_SECRET", "super-secret");
    let config = make_config(format!("{}/token", server.uri()));
    let cache = TokenCache::new(config);

    let t1 = cache.fetch_token().await.expect("first fetch");
    let t2 = cache.fetch_token().await.expect("second fetch (cached)");
    assert_eq!(t1.access_token, t2.access_token);
    // wiremock verifies exactly 1 HTTP request when MockServer drops
}

#[tokio::test]
async fn expired_token_triggers_refresh() {
    let server = MockServer::start().await;

    // Return a token that expires in 1 second (< SKEW_SECONDS=30 → immediately
    // considered stale on next call after sleep).
    // Instead we use expires_in=0 to force immediate expiry.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "tok-refreshed",
            "expires_in": 0,
            "token_type": "Bearer"
        })))
        .expect(2) // two HTTP calls: initial + refresh
        .mount(&server)
        .await;

    env::set_var("TEST_OIDC_SECRET", "super-secret");
    let config = make_config(format!("{}/token", server.uri()));
    let cache = TokenCache::new(config);

    let _t1 = cache.fetch_token().await.expect("first fetch");
    // Token has expires_in=0, so it's immediately past the skew window.
    // Next call should refresh.
    let t2 = cache.fetch_token().await.expect("refresh fetch");
    assert_eq!(t2.access_token, "tok-refreshed");
    // wiremock verifies exactly 2 calls
}

#[tokio::test]
async fn auth_failure_on_non_2xx_response() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    env::set_var("TEST_OIDC_SECRET", "super-secret");
    let config = make_config(format!("{}/token", server.uri()));
    let cache = TokenCache::new(config);

    let err = cache.fetch_token().await.expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("401") || msg.contains("auth"),
        "error should mention 401 or auth: {msg}"
    );
}

/// Helper to silence the unused import warning on Duration when tokio sleep
/// feature is unavailable.
#[allow(dead_code)]
fn _use_duration(_: Duration) {}
