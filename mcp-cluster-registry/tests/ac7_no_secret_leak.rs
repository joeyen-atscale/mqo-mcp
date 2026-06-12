//! AC7 (CRITICAL): AuthConfig serializes ONLY env-var *names*, never the values.
//!
//! The serializer must NOT call `std::env::var()`.  This test intentionally
//! does NOT set any environment variables; it verifies that the round-trip
//! JSON contains the env-var name strings but not any fabricated values.

use mcp_cluster_registry::{AuthConfig, ClusterEntry, ClusterRegistry};

fn make_registry_with_oidc() -> ClusterRegistry {
    ClusterRegistry {
        clusters: vec![ClusterEntry {
            name: "test".to_string(),
            endpoint: "test.example.com:15432".to_string(),
            xmla_url: None,
            auth: AuthConfig::Oidc {
                token_url: "https://auth.example.com/token".to_string(),
                client_id: "my-client".to_string(),
                realm: "MyRealm".to_string(),
                client_secret_env: "MY_OIDC_SECRET_ENV_VAR_NAME".to_string(),
            },
            supported_backends: vec!["sql".to_string()],
            model_filter: None,
            priority: 0,
            required: true,
            tags: vec![],
        }],
    }
}

fn make_registry_with_direct() -> ClusterRegistry {
    ClusterRegistry {
        clusters: vec![ClusterEntry {
            name: "direct-test".to_string(),
            endpoint: "direct.example.com:15432".to_string(),
            xmla_url: None,
            auth: AuthConfig::Direct {
                pg_user: "MY_PG_USER_ENV_VAR_NAME".to_string(),
                pg_pass_env: "MY_PG_PASS_ENV_VAR_NAME".to_string(),
            },
            supported_backends: vec!["sql".to_string()],
            model_filter: None,
            priority: 0,
            required: true,
            tags: vec![],
        }],
    }
}

#[test]
fn oidc_json_contains_env_var_name_not_value() {
    let reg = make_registry_with_oidc();
    let json = reg.to_json();

    // The JSON must contain the env-var NAME.
    assert!(
        json.contains("MY_OIDC_SECRET_ENV_VAR_NAME"),
        "JSON must contain the env-var name string"
    );

    // The JSON must NOT contain any env-var *value* (we set none, but the
    // serializer must not attempt to read them — checked structurally by
    // confirming the field name appears, not a resolved value).
    // Since we did not set MY_OIDC_SECRET_ENV_VAR_NAME in the environment,
    // if the serializer called std::env::var() it would produce an empty
    // string or panic — the only correct behavior is the literal name.
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let secret_field = &parsed["clusters"][0]["auth"]["client_secret_env"];
    assert_eq!(
        secret_field.as_str().unwrap(),
        "MY_OIDC_SECRET_ENV_VAR_NAME",
        "client_secret_env must serialize as the env-var name, not its value"
    );
}

#[test]
fn direct_json_contains_env_var_names_not_values() {
    let reg = make_registry_with_direct();
    let json = reg.to_json();

    assert!(
        json.contains("MY_PG_PASS_ENV_VAR_NAME"),
        "JSON must contain pg_pass_env var name"
    );

    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let pass_field = &parsed["clusters"][0]["auth"]["pg_pass_env"];
    assert_eq!(
        pass_field.as_str().unwrap(),
        "MY_PG_PASS_ENV_VAR_NAME",
        "pg_pass_env must serialize as the env-var name, not its value"
    );
}

#[test]
fn oidc_round_trip_preserves_env_var_names() {
    // No env vars set in this process for these names — confirm round-trip
    // stores the name correctly.
    let original = make_registry_with_oidc();
    let json = original.to_json();
    let restored = ClusterRegistry::from_json(&json).expect("JSON parse");

    assert_eq!(original, restored, "OIDC AuthConfig must round-trip identically");

    if let AuthConfig::Oidc { client_secret_env, .. } = &restored.clusters[0].auth {
        assert_eq!(client_secret_env, "MY_OIDC_SECRET_ENV_VAR_NAME");
    } else {
        panic!("expected OIDC auth variant");
    }
}

#[test]
fn direct_round_trip_preserves_env_var_names() {
    let original = make_registry_with_direct();
    let json = original.to_json();
    let restored = ClusterRegistry::from_json(&json).expect("JSON parse");

    assert_eq!(original, restored, "Direct AuthConfig must round-trip identically");

    if let AuthConfig::Direct { pg_pass_env, .. } = &restored.clusters[0].auth {
        assert_eq!(pg_pass_env, "MY_PG_PASS_ENV_VAR_NAME");
    } else {
        panic!("expected Direct auth variant");
    }
}
