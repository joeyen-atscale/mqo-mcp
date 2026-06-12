# mqo-from-sql

Reverse-compile AtScale SQL projections into [MQO](https://github.com/joeyen-atscale/mqo-spec) (Multidimensional Query Object) JSON.

## What it does

`mqo-from-sql` parses the flat SQL shape that AtScale's SQL backend emits:

```sql
SELECT "store_sales.Total Store Sales", "time_dim.calendar.[Year]"
FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model"
GROUP BY "time_dim.calendar.[Year]"
LIMIT 100
```

…and reverse-compiles it into a `BoundMqo` JSON that can be validated, stored as a golden test case, or fed back into the MQO fleet.

## Usage

```sh
# Single SQL string
mqo-from-sql --catalog snapshot.json 'SELECT ...'

# Batch mode (one SQL per line, JSONL output)
mqo-from-sql --catalog snapshot.json --batch queries.jsonl --format jsonl

# Write to a file
mqo-from-sql --catalog snapshot.json --output out.json 'SELECT ...'

# Live catalog fetch (password via env var, never on the CLI)
mqo-from-sql --pg-pass-env ATSCALE_PG_PASS --catalog snapshot.json 'SELECT ...'
```

**AC-critical**: `--pg-pass` does not exist. Passwords must only be supplied via `--pg-pass-env <VARNAME>`.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Parse/resolve error (batch: at least one line failed) |
| 2 | Usage error (bad flags, missing required input) |

## SQL shape supported

```
SELECT "<measure_unique_name>"[, ...]
FROM "atscale_catalogs"."<catalog_id>"."<model_id>"
[WHERE <col> = <val> [AND ...]]
[GROUP BY "<dimension_level_unique_name>"[, ...]]
[LIMIT <integer>]
```

## Development

```sh
cargo test --release
```

## Related crates

- [`mqo-spec`](https://github.com/joeyen-atscale/mqo-spec) — MQO schema and validation
- [`mqo-catalog-binder`](https://github.com/joeyen-atscale/mqo-catalog-binder) — catalog snapshot types and binding
- [`mqo-backend-router`](https://github.com/joeyen-atscale/mqo-backend-router) — routes BoundMqo to SQL/DAX/MDX
