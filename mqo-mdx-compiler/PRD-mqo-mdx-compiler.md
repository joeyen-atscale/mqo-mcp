# PRD-mqo-mdx-compiler — compile a bound MQO to MDX (secondary backend)

*Source: `/Users/jsy/Documents/PRDs/ARCHIVE/PRD-mqo-mdx-compiler.md` — this copy adds AC labels required by `ac-traceability`.*

- Status: Draft v0.1
- build_target: rust-cli
- Owner: Joe Yen
- Date: 2026-06-07

## TL;DR

Some requests are inherently multidimensional — asymmetric axes, ragged /
parent-child hierarchy navigation, drill-through, or a client that wants a true
cellset (Excel/pivot). For those, the MQO compiles to MDX. This CLI takes a
`BoundMqo` and emits MDX honoring the gate's structural rules: fully-qualified
cube/three-part names (R10), `NON EMPTY` (R13), calc-group member literals (R7),
MDX-dependency hierarchies for calculated measures (R6), and semi-additive trigger
levels (R11).

## Acceptance criteria

**AC1** — ≥6 golden `BoundMqo` → MDX pairs compile exactly (normalized
string-equal). Covers measure-only, measure+dim, multi-dim CROSSJOIN, three-part
cube name, Member filter in WHERE, two measures on COLUMNS, full query with
WITH+WHERE+dims.

Tests: `ac1_minimal_measure_only`, `ac1_measure_with_one_dimension`,
`ac1_measure_with_two_dimensions_crossjoin`, `ac1_three_part_cube_name`,
`ac1_member_filter_in_where`, `ac1_two_measures_on_columns`,
`ac1_full_query_golden`.

**AC2** — Every emitted MDX includes `NON EMPTY` on the row axis and a
fully-qualified cube name (R10 / R13).

Tests: `ac2_non_empty_on_rows`, `ac2_fully_qualified_cube_two_part`,
`ac2_non_empty_only_on_rows_not_columns`.

**AC3** — A calculated measure pulls its MDX-dependency hierarchies onto the
row axis (R6); dep-hierarchies already covered by a bound dimension are not
duplicated; two calc measures with the same dep hierarchy emit it exactly once.

Tests: `ac3_calc_measure_adds_dependency_hierarchy`,
`ac3_dependency_hierarchy_deduped_with_bound_dims`,
`ac3_non_calc_measure_no_dep_hierarchies`,
`ac3_multi_calc_dep_hierarchy_dedup`.

**AC4** — A calc-group member literal is emitted verbatim from bound metadata
into a `WITH MEMBER` clause (R7); a `CalcGroupMember` filter in `mqo.filters`
appears in the `WHERE` slicer; no `WITH MEMBER` is emitted when
`calc_group_members` is empty.

Tests: `ac4_calc_group_member_emitted_verbatim`,
`ac4_no_calc_group_member_no_with_clause`, `ac4_calc_group_filter_in_where`.

**AC5** — A semi-additive measure without a trigger level produces
`MdxCompileError::SemiAdditiveMissingTrigger` and exits non-zero (R11); the
error message names the offending measure; empty-measure input returns
`MdxCompileError::EmptyMeasures`.

Tests: `ac5_semi_additive_missing_trigger_is_error`,
`ac5_semi_additive_with_trigger_compiles`, `ac5_error_contains_measure_name`,
`ac5_empty_measures_error`.

**AC6** — `cargo test --release --workspace` passes; `cargo clippy --workspace
-- -D warnings` passes.

**AC7** — Integration CLI test: `mqo-mdx` binary invoked via
`std::process::Command` compiles a golden `BoundMqo` JSON file and asserts on
stdout/exit-code; semi-additive-missing-trigger exits 1; missing `--bound` flag
exits non-zero; non-existent file exits 2.

Tests: `acceptance_cli_golden_compile`, `acceptance_cli_semi_additive_exit_nonzero`,
`acceptance_cli_missing_bound_flag_exits_nonzero`,
`acceptance_cli_nonexistent_bound_file_exits_2`.

**AC8** (MAY) — Range filters in `mqo.filters` are handled gracefully (not
silently dropped without documentation). The `build_where_clause` function
documents the intentional omission with a comment explaining why `Range` filters
are not expressible as MDX slicer members.
