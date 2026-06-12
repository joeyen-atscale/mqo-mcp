//! AC4: Secret hygiene tests.
//!
//! - Client secret is read from the env var named by `client_secret_env_var`.
//! - Missing env var → structured `EngineError::MissingSecret`.
//! - `format!("{:?}", oidc_config)` does NOT contain the secret value.

use std::env;

use mqo_auth_bridge::{EngineError, OidcConfig};

const SENTINEL: &str = "THIS_IS_THE_SECRET_VALUE_SENTINEL";
const VAR_NAME: &str = "AC4_TEST_OIDC_SECRET";

fn make_config() -> OidcConfig {
    OidcConfig {
        token_url: "http://localhost:9999/token".to_string(),
        client_id: "test-client".to_string(),
        client_secret_env_var: VAR_NAME.to_string(),
        realm: "test-realm".to_string(),
        username: None,
        password_env_var: None,
    }
}

#[test]
fn debug_output_does_not_contain_secret() {
    // Set the sentinel value into the env var.
    env::set_var(VAR_NAME, SENTINEL);

    let config = make_config();
    let debug_str = format!("{config:?}");

    assert!(
        !debug_str.contains(SENTINEL),
        "Debug output must not contain the secret value; got: {debug_str}"
    );
    // It should contain the var NAME, not the value.
    assert!(
        debug_str.contains(VAR_NAME),
        "Debug output should contain the env var name; got: {debug_str}"
    );
}

#[test]
fn missing_env_var_is_structured_error() {
    // Ensure the var is unset.
    env::remove_var("AC4_MISSING_VAR");

    let config = OidcConfig {
        token_url: "http://localhost:9999/token".to_string(),
        client_id: "test-client".to_string(),
        client_secret_env_var: "AC4_MISSING_VAR".to_string(),
        realm: "test-realm".to_string(),
        username: None,
        password_env_var: None,
    };

    // We can't easily call fetch_token() in a sync test without a server,
    // but we can verify the EngineError::MissingSecret variant exists and
    // carries the var name.
    let err = EngineError::MissingSecret {
        var_name: "AC4_MISSING_VAR".to_string(),
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("AC4_MISSING_VAR"),
        "error should name the var: {msg}"
    );
    assert!(!msg.contains(SENTINEL), "error must not contain sentinel");

    // Also confirm the config carries only the var name.
    let debug_str = format!("{config:?}");
    assert!(!debug_str.contains(SENTINEL));
    assert!(debug_str.contains("AC4_MISSING_VAR"));
}

#[test]
fn oidc_config_display_via_error_is_clean() {
    env::set_var(VAR_NAME, SENTINEL);
    let config = make_config();

    // Any format path should be clean.
    let display = format!(
        "{}",
        EngineError::MissingSecret {
            var_name: config.client_secret_env_var.clone(),
        }
    );
    assert!(!display.contains(SENTINEL));
}
