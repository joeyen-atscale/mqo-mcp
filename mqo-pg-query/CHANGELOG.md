# Changelog — mqo-pg-query

## v0.1.0 — 2026-06-17

**PRD:** PRD-mqo-pgwire-query-cli — verbatim PgSql executor over AtScale OIDC + PGWire

Adds a new `mqo-pg-query` CLI binary that executes verbatim SQL against the AtScale
PGWire endpoint using `mqo-auth-bridge`'s existing OIDC + TLS connection path.
This is the gold-oracle primitive for the eval harness (`qwf20-ground-truth-oracle`):
it mints the reference result table by running `expected_sql` directly against the
live engine without going through the MQO pipeline.

### What's in this release

- **`mqo-pg-query --sql '<SQL>'`** — prints `{"columns": [...], "rows": [[...]]}` on success.
- **Row cap** — `--max-result-rows` (default 50 000, ceiling 200 000) prevents unbounded
  streaming; results over the cap emit `{"oversize": {"observed_at_least": N, "cap": C}}`.
- **Structured JSON error** — any auth/connection/SQL failure emits `{"error": {"message": "..."}}`,
  exits non-zero, and leaks no credentials.
- **OIDC + ROPC auth** — `--oidc-client-secret-env` / `--oidc-username` / `--oidc-password-env`
  support both client-credentials and Resource Owner Password Credentials grants, mirroring
  the `mqo-mcp-server` flag conventions.
- **Direct PGWire credentials** — `--pg-user` / `--pg-pass-env` bypass OIDC for direct
  user auth (the `qwf20-ground-truth-oracle` "direct user creds" path).
- **TLS enforced** — connects with `sslmode=require`; no plaintext fallback.
- **Stdin support** — SQL may be piped on stdin when `--sql` is omitted.
- **No new auth code** — reuses `mqo-auth-bridge`'s `LiveExecutor` and `pgwire_execute` entirely.

### Non-goals (deferred)

- MCP `run_query` tool on `mqo-mcp-server` — follow-on PRD.
- Python gold-cache wiring (`PRD-mqoeval-gold-via-cli`) — consumes this binary.
- Result caching / persistence — follow-on.
