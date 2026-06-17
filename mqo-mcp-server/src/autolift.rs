//! Auto-lift: on-demand fetch-and-lift of a live model's engine XML into the
//! in-process RDF triple store.
//!
//! ## What it does
//!
//! When `autolift_base_url` is configured, the first `query_model_graph` call
//! for a model triggers:
//!   1. A `GET {base_url}/{catalog_id}.xml` with an OIDC bearer token.
//!   2. `aso_lift::lift()` on the returned XML body.
//!   3. Storage of the resulting `oxrdf::Graph` in a shared in-process cache,
//!      keyed on `(catalog_id, last_schema_update)`.
//!
//! Subsequent calls for the same `(catalog_id, LAST_SCHEMA_UPDATE)` hit the
//! cache without re-lifting.  When `LAST_SCHEMA_UPDATE` advances the cache
//! entry is evicted and a re-lift occurs on the next call.
//!
//! ## PRD coverage (PRD-osl-live-autolift)
//!
//! | FR/NFR | Coverage |
//! |--------|----------|
//! | FR1 | `try_autolift` fetches and lifts on first OSL tool call |
//! | FR2 | OIDC bearer token via `LiveExecutor::fetch_token_sync` |
//! | FR3 | `AutoliftCache` keyed on `(catalog_id, last_schema_update)` |
//! | FR4 | Cache miss on `LAST_SCHEMA_UPDATE` advance → re-lift |
//! | FR5 | Disabled by default (`autolift_base_url = None`); returns not-available |
//! | FR6 | HTTP/parse errors → `None` (not-available, no crash, no hang) |
//! | NFR1 | Off hot path: `query_multidimensional` never calls this module |
//! | NFR2 | Bearer token via existing OIDC env-var config; never logged |

use mqo_auth_bridge::LiveExecutor;
use oxrdf::Graph;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ─────────────────────────────────────────────────────────────────────────────
//  Cache
// ─────────────────────────────────────────────────────────────────────────────

/// Cache entry: `(catalog_id, last_schema_update)` → loaded Graph.
///
/// `last_schema_update` is `"none"` when the engine does not populate
/// `LAST_SCHEMA_UPDATE` — still a valid key; causes no false re-lifts.
type CacheKey = (String, String);

/// In-memory auto-lift cache.
///
/// `Mutex<HashMap<CacheKey, Graph>>` — entries are cheap to evict (the Graph is
/// replaced on version change) and never flushed to disk (v1: in-memory only).
pub struct AutoliftCache {
    inner: Mutex<HashMap<CacheKey, Graph>>,
}

impl AutoliftCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Look up a cached graph. Returns a clone when present.
    ///
    /// Cloning an `oxrdf::Graph` is O(triples) — acceptable for the graph sizes
    /// this lift produces (typical models ≤ ~1000 triples).
    #[must_use]
    pub fn get(&self, catalog_id: &str, schema_update: &str) -> Option<Graph> {
        let key: CacheKey = (catalog_id.to_string(), schema_update.to_string());
        self.inner.lock().ok()?.get(&key).cloned()
    }

    /// Insert a graph. Evicts any previously cached graph for `catalog_id`
    /// (regardless of the old `schema_update` key) to avoid unbounded growth.
    pub fn insert(&self, catalog_id: &str, schema_update: &str, graph: Graph) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        // Evict any old entry for this catalog_id (different schema_update).
        guard.retain(|(cid, _), _| cid != catalog_id);
        guard.insert((catalog_id.to_string(), schema_update.to_string()), graph);
    }

    /// Number of cached entries (used in tests).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }
}

impl Default for AutoliftCache {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Fetch + lift
// ─────────────────────────────────────────────────────────────────────────────

/// Attempt to fetch the engine model XML for `catalog_id` and lift it into an
/// `oxrdf::Graph`.
///
/// Returns `Some(graph)` on success, `None` on any failure (HTTP error,
/// non-200 status, empty body, parse/lift error). Errors are logged to stderr
/// via `eprintln!` but never propagated as panics.
///
/// ## Security
/// The OIDC bearer token is read from the env-var configured in `executor` and
/// is never stored in the returned `Graph`, logged, or placed in an error message.
#[must_use]
pub fn try_autolift(
    catalog_id: &str,
    base_url: &str,
    executor: &LiveExecutor,
) -> Option<Graph> {
    // Build the catalog XML URL: `{base_url}/{catalog_id}.xml`.
    // Normalize: strip trailing slash from base.
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/{catalog_id}.xml");

    // Fetch a bearer token — reuses the existing OIDC token provider.
    let token = match executor.fetch_token_sync() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("mqo-mcp-server: autolift: token fetch error for {catalog_id}: {e}");
            return None;
        }
    };

    // Perform the HTTP GET synchronously (blocking reqwest — fine here because
    // this is called from the synchronous MCP request handler, not inside an
    // async context). NFR1: only reachable from OSL tool dispatch, never from
    // query_multidimensional.
    let xml_body = fetch_xml_blocking(&url, &token.access_token, catalog_id)?;

    // Run aso-lift on the XML body.
    let opts = aso_lift::LiftOptions::default();
    match aso_lift::lift(&xml_body, &opts) {
        Ok(output) => {
            // Parse the Turtle output into an oxrdf::Graph using the same
            // parser used by ModelGraphStore::load_turtle.
            match turtle_to_graph(&output.turtle) {
                Ok(graph) => {
                    eprintln!(
                        "mqo-mcp-server: autolift: lifted {catalog_id} \
                         ({} triples)",
                        graph.len()
                    );
                    Some(graph)
                }
                Err(e) => {
                    eprintln!(
                        "mqo-mcp-server: autolift: Turtle parse error for {catalog_id}: {e}"
                    );
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("mqo-mcp-server: autolift: lift error for {catalog_id}: {e}");
            None
        }
    }
}

/// Shared helper: ensure a lifted `oxrdf::Graph` is available for `(catalog_id,
/// schema_update)`.
///
/// Checks the `cache` first; on a miss calls [`try_autolift`] and populates the
/// cache before returning.  Returns `None` when auto-lift is disabled (no
/// `base_url`) or when the fetch/lift fails.
///
/// This is the single point called by all three OSL tools (`query_model_graph`,
/// `describe_grounding`, `validate_query_ontology`) so cache-population logic is
/// not duplicated.
#[must_use]
pub fn ensure_graph(
    catalog_id: &str,
    schema_update: &str,
    base_url: &str,
    executor: &LiveExecutor,
    cache: &Arc<AutoliftCache>,
) -> Option<Graph> {
    // Cache hit — serve without re-lifting.
    if let Some(graph) = cache.get(catalog_id, schema_update) {
        return Some(graph);
    }

    // Cache miss → fetch + lift + insert.
    eprintln!(
        "mqo-mcp-server: autolift: cache miss for {catalog_id} \
         (schema_update={schema_update}); lifting…"
    );
    let graph = try_autolift(catalog_id, base_url, executor)?;
    cache.insert(catalog_id, schema_update, graph.clone());
    Some(graph)
}

/// Serialize an `oxrdf::Graph` to a Turtle string.
///
/// Used when a downstream consumer (e.g. `aso-ground`) needs a Turtle string
/// rather than a `Graph` object directly.
///
/// # Panics
///
/// Panics if the `oxttl::TurtleSerializer` fails to finish — in practice this
/// cannot happen with an in-memory writer.
#[must_use]
pub fn graph_to_turtle(graph: &Graph) -> String {
    use oxrdf::Triple;
    use oxttl::TurtleSerializer;
    let mut buf: Vec<u8> = Vec::new();
    let mut ser = TurtleSerializer::new().for_writer(&mut buf);
    // Collect into owned Triples; sort for determinism.
    let mut triples: Vec<Triple> = graph.iter().map(oxrdf::TripleRef::into_owned).collect();
    triples.sort_by_key(|t| (t.subject.to_string(), t.predicate.to_string(), t.object.to_string()));
    for triple in &triples {
        let _ = ser.serialize_triple(triple.as_ref());
    }
    ser.finish().expect("TurtleSerializer finish should not fail");
    String::from_utf8(buf).unwrap_or_default()
}

/// Derive an autolift base URL from a resolved XMLA URL.
///
/// Replaces a trailing `/v1/xmla` (or `.../xmla`) suffix with `/v1/catalogs`.
///
/// Examples:
/// - `https://mcp-aws.atscaleinternal.com/v1/xmla` → `https://mcp-aws.atscaleinternal.com/v1/catalogs`
/// - `https://mcp-aws.atscaleinternal.com/xmla` → `https://mcp-aws.atscaleinternal.com/v1/catalogs`
///
/// Returns `None` when `xmla_url` is empty or does not contain a recognizable
/// XMLA suffix (autolift stays disabled).
#[must_use]
pub fn derive_autolift_base_url(xmla_url: &str) -> Option<String> {
    let url = xmla_url.trim();
    if url.is_empty() {
        return None;
    }
    // Strip trailing `/v1/xmla` first (most specific pattern).
    if let Some(base) = url.strip_suffix("/v1/xmla") {
        return Some(format!("{base}/v1/catalogs"));
    }
    // Strip trailing `/xmla` (the bare suffix without the version prefix).
    if let Some(base) = url.strip_suffix("/xmla") {
        return Some(format!("{base}/v1/catalogs"));
    }
    None
}

/// Perform a blocking `GET {url}` with a bearer token and return the response
/// body on HTTP 200. Returns `None` on non-200, connection error, or empty body.
fn fetch_xml_blocking(url: &str, bearer_token: &str, catalog_id: &str) -> Option<String> {
    // Build a blocking reqwest client.
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new());

    let resp = match client
        .get(url)
        .header("Authorization", format!("Bearer {bearer_token}"))
        .header("Accept", "application/xml, text/xml, */*")
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "mqo-mcp-server: autolift: HTTP GET {url} failed for {catalog_id}: {e}"
            );
            return None;
        }
    };

    let status = resp.status();
    if !status.is_success() {
        eprintln!(
            "mqo-mcp-server: autolift: HTTP GET {url} returned {status} for {catalog_id}"
        );
        return None;
    }

    match resp.text() {
        Ok(body) if body.trim().is_empty() => {
            eprintln!(
                "mqo-mcp-server: autolift: HTTP GET {url} returned empty body for {catalog_id}"
            );
            None
        }
        Ok(body) => Some(body),
        Err(e) => {
            eprintln!(
                "mqo-mcp-server: autolift: failed to read body from {url} for {catalog_id}: {e}"
            );
            None
        }
    }
}

/// Parse a Turtle string into an `oxrdf::Graph`.
fn turtle_to_graph(turtle: &str) -> Result<Graph, String> {
    use oxttl::TurtleParser;
    let parser = TurtleParser::new()
        .with_base_iri("https://models.atscale.com")
        .map_err(|e| format!("parser init: {e}"))?;
    let mut graph = Graph::new();
    for result in parser.for_slice(turtle.as_bytes()) {
        match result {
            Ok(triple) => {
                graph.insert(&triple);
            }
            Err(e) => return Err(format!("parse error: {e}")),
        }
    }
    Ok(graph)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── URL derivation tests ──────────────────────────────────────────────────

    /// Derive from the canonical `/v1/xmla` path used on mcp-aws.
    #[test]
    fn derive_from_v1_xmla_suffix() {
        let result = derive_autolift_base_url("https://mcp-aws.atscaleinternal.com/v1/xmla");
        assert_eq!(
            result.as_deref(),
            Some("https://mcp-aws.atscaleinternal.com/v1/catalogs"),
            "must replace /v1/xmla with /v1/catalogs"
        );
    }

    /// Derive from a bare `/xmla` suffix (no version prefix).
    #[test]
    fn derive_from_bare_xmla_suffix() {
        let result = derive_autolift_base_url("https://example.com/xmla");
        assert_eq!(
            result.as_deref(),
            Some("https://example.com/v1/catalogs"),
            "must replace /xmla with /v1/catalogs"
        );
    }

    /// Explicit `--autolift-base-url` always wins (caller must prefer explicit over derived).
    /// The derivation is only invoked when there is no explicit flag — this test
    /// verifies `derive_autolift_base_url` itself returns the correct value; the
    /// override logic is in `build_autolift` (main.rs tests).
    #[test]
    fn explicit_override_is_returned_unchanged() {
        // If an explicit URL is passed directly (not through derive), no stripping occurs.
        // The caller's build_autolift function is responsible for the precedence.
        // Here we just verify derive doesn't mangle an already-correct base URL.
        let result = derive_autolift_base_url("https://mcp-aws.atscaleinternal.com/v1/catalogs");
        // `/v1/catalogs` has neither /v1/xmla nor /xmla → None (no derivation possible)
        assert!(
            result.is_none(),
            "a catalogs URL is not an XMLA URL; derive must return None: {result:?}"
        );
    }

    /// Empty xmla_url → disabled (returns None).
    #[test]
    fn empty_xmla_url_returns_none() {
        assert!(
            derive_autolift_base_url("").is_none(),
            "empty URL must return None"
        );
        assert!(
            derive_autolift_base_url("   ").is_none(),
            "whitespace-only URL must return None"
        );
    }

    /// Unrecognized suffix → disabled.
    #[test]
    fn unrecognized_suffix_returns_none() {
        let result = derive_autolift_base_url("https://example.com/some/other/path");
        assert!(
            result.is_none(),
            "unrecognized path must return None: {result:?}"
        );
    }

    // ── AC1: autolift disabled → cache empty, not available ───────────────────
    // (Tested via Server::query_model_graph in integration tests in mcp; here
    //  we verify the cache itself behaves correctly.)

    /// AC3: same (catalog_id, schema_update) → cache hit, no re-lift.
    #[test]
    fn ac3_cache_hit_same_schema_update() {
        let cache = AutoliftCache::new();

        // Synthesize a minimal valid graph using the fixture TTL from model_graph.
        let fixture_ttl = fixture_minimal_ttl();
        let graph = turtle_to_graph(fixture_ttl).expect("fixture must parse");
        let triple_count = graph.len();

        cache.insert("my_catalog", "2024-01-01", graph);

        // Second lookup for same key: must return a graph.
        let hit = cache.get("my_catalog", "2024-01-01");
        assert!(hit.is_some(), "cache must hit on same schema_update");
        let g = hit.unwrap();
        assert_eq!(g.len(), triple_count, "cached graph triple count must match");
    }

    /// AC4: LAST_SCHEMA_UPDATE changes → old entry evicted.
    #[test]
    fn ac4_schema_update_change_evicts_old_entry() {
        let cache = AutoliftCache::new();

        let g1 = turtle_to_graph(fixture_minimal_ttl()).expect("fixture must parse");
        cache.insert("cat1", "v1", g1);

        // Old entry present.
        assert!(cache.get("cat1", "v1").is_some());

        // Insert new version — evicts v1 entry.
        let g2 = turtle_to_graph(fixture_minimal_ttl()).expect("fixture must parse");
        cache.insert("cat1", "v2", g2);

        // Old key must be gone.
        assert!(
            cache.get("cat1", "v1").is_none(),
            "old schema_update entry must be evicted"
        );
        // New key must be present.
        assert!(
            cache.get("cat1", "v2").is_some(),
            "new schema_update entry must be present"
        );

        // Only one entry in the cache (no unbounded growth for same catalog_id).
        assert_eq!(cache.len(), 1, "cache must hold exactly 1 entry after eviction");
    }

    /// AC4: different catalog_ids coexist in the cache.
    #[test]
    fn different_catalog_ids_coexist() {
        let cache = AutoliftCache::new();
        let g1 = turtle_to_graph(fixture_minimal_ttl()).expect("fixture must parse");
        let g2 = turtle_to_graph(fixture_minimal_ttl()).expect("fixture must parse");
        cache.insert("cat_a", "v1", g1);
        cache.insert("cat_b", "v1", g2);

        assert!(cache.get("cat_a", "v1").is_some(), "cat_a must be cached");
        assert!(cache.get("cat_b", "v1").is_some(), "cat_b must be cached");
        assert_eq!(cache.len(), 2, "two distinct catalog_ids must coexist");
    }

    /// AC5: HTTP error (mocked by try_autolift with an unreachable URL) →
    /// returns None, no crash.
    ///
    /// We test this indirectly: `fetch_xml_blocking` with a bad URL returns None.
    #[test]
    fn ac5_http_error_returns_none_no_crash() {
        // This will fail to connect (no server at that address). Must return None.
        let result = fetch_xml_blocking(
            "http://127.0.0.1:1/nonexistent.xml",
            "fake-token",
            "test_catalog",
        );
        assert!(result.is_none(), "unreachable URL must return None");
    }

    /// Verify turtle_to_graph round-trips the fixture TTL.
    #[test]
    fn turtle_to_graph_parses_fixture() {
        let g = turtle_to_graph(fixture_minimal_ttl()).expect("fixture must parse");
        assert!(g.len() > 0, "parsed graph must be non-empty");
    }

    fn fixture_minimal_ttl() -> &'static str {
        r#"
@prefix aso:  <https://ontology.atscale.com/aso/> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

<https://models.atscale.com#hier-brand>
    rdf:type owl:NamedIndividual, aso:Hierarchy ;
    rdfs:label "Brand" .

<https://models.atscale.com#level-brand>
    rdf:type owl:NamedIndividual, aso:Level ;
    rdfs:label "Brand" .
"#
    }
}
