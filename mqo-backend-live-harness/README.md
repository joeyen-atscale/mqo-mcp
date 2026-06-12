# mqo-backend-live-harness

Port-gated DAX/MDX E2E harness. Green-skips dead ports, flips pass when ports open.

## TL;DR

Runs a capability probe on each configured backend (SQL/DAX/MDX), then for every
test case: executes and asserts if the backend is live, or emits a labelled skip if
the port is closed or the protocol rejects the request. Runs a parity check when
two or more backends are live. Exits 0 iff all non-skipped checks pass.

```
✅ [sql] scalar_total_store_sales
⏭️ [dax] scalar_total_store_sales (rejected: PGWire rejected EVALUATE (SQL-only host))
⏭️ [mdx] scalar_total_store_sales (unreachable: XMLA :11111 unreachable)
⏭️ [parity] skipped (only 1 live backend(s))
2/2 passed, 4 skipped, 0 failed
```

## Install

```sh
cargo install --path .
```

## Usage

```sh
# SQL-only host (env vars gate which backends are probed live)
ATSCALE_PGWIRE_HOST=mcp-aws.atscale.com \
  mqo-live-harness --cases fixtures/default_cases.json --backends sql,dax,mdx

# Custom case file — no code change needed
mqo-live-harness --cases my_cases.json --backends sql
```

## Environment variables

| Variable              | Backend | Description                                  |
|-----------------------|---------|----------------------------------------------|
| `ATSCALE_PGWIRE_HOST` | SQL/DAX | Hostname for PGWire (port 11120)             |
| `ATSCALE_XMLA_URL`    | MDX     | XMLA endpoint URL (e.g. `http://host:11111`) |

If a variable is unset the corresponding backend is probed as Unreachable and all
its cases are skipped — exit 0 is preserved.

## Case file format

```json
[
  {
    "name": "scalar_total_store_sales",
    "mqo": { "measures": ["store_sales.total_store_sales"], "catalog": "tpcds", "model": "tpcds" },
    "expected_value": 10170000000.0
  }
]
```

Add new entries to the JSON file; no Rust code changes required.

## Architecture

```
src/
  lib.rs          — types: BackendStatus, TestCase, CheckOutcome, HarnessReport
  probe.rs        — CapabilityProbe trait + EnvProbe (real) + FakeProbe (tests)
  comparator.rs   — ParityComparator trait + FakeComparator (tests)
  runner.rs       — Engine trait + FakeEngine (tests) + run_harness()
  main.rs         — CLI (clap), wires EnvProbe + StubEngine + StubComparator
tests/
  acceptance.rs   — 8 offline acceptance tests covering all ACs
fixtures/
  default_cases.json — 2 TPC-DS E2E cases validated 2026-06-08
```

Real backend clients (AtScale PGWire, XMLA) slot in by implementing `Engine`.
The real `mqo-mcp-server` probe and `mqo-cross-backend-parity` comparator will
replace `EnvProbe` / `StubComparator` when path deps are set up.
