# Changelog

## v0.3.0

### Cross-fact date-role binding + null-path rejection

- **Per-measure date-role binding (FR-1):** new `bind_with_date_roles()` binds
  each measure to the date hierarchy whose fact intersects the measure's fact.
  `BoundMeasureExt` now carries `date_role_hierarchy`. A mixed inventory+sales
  query with both `Inventory Calendar Month` and `Sold Calendar Month` binds
  each measure to its own date role.
- **Cross-fact date incompatibility rejection (FR-2/FR-3):** when a multi-fact
  MQO names a single date level not conformed across the referenced facts
  (e.g. an inventory measure under a `sold_date_*` hierarchy), the binder
  returns a structured `BindResult::DateRoleIncompatible` with code
  `cross_fact_date_incompatible`, the offending measure, the requested level,
  and the valid date hierarchies for that measure. Classification is
  pre-execution and catalog-only (NFR-1) — reuses the `enriched-catalog.v1`
  column-group compatibility matrix.
- **No false rejections (FR-4):** single-fact queries, sales-only queries under
  `Sold Calendar Month`, inventory-only queries under `Inventory Calendar Month`,
  conformed measures, and non-date dimensions all bind unchanged. Date
  dimensions are excluded from the residual blanket pairwise compat check so a
  legitimate multi-role query is not flagged.
- **CLI:** `mqo-bind` now uses `bind_with_date_roles` when `--enriched-catalog`
  is supplied and emits `{"date_role_incompatible":[...]}` with exit code `6`.

## v0.2.0

- Cross-fact compatibility checking (`bind_with_compat`) via `enriched-catalog.v1`.
