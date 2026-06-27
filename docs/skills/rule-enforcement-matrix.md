# Rule Enforcement Matrix — `query-semantic-layer` skill

**Version:** 1.0  
**Date:** 2026-06-27  
**Owner:** Joe Yen  
**Jira epic:** ATSCALE-47877  
**Source skill:** `.agents/skills/query-semantic-layer/SKILL.md` + `ENFORCEMENT.md` + `references/*.md`

---

## Classification Scheme

### Decidability classes

| Class | Meaning |
|---|---|
| `sql-only` | Detectable by parsing the SQL string alone — no model metadata needed. |
| `sql+metadata` | Requires the model-metadata surface (catalog facts, column roles, SML structure, certified flags, hierarchies). |
| `intent` | Requires the user's goal or prior dialog context that the server cannot access at validation time. |

### Enforcement tiers

| Tier | Meaning |
|---|---|
| `server-validator` | Can be enforced as a hard server-side rejection before warehouse execution (zero warehouse cost on violation). |
| `pre-execution-validation-finding` | Can be surfaced as a structured warning/finding during validation but requires metadata and cannot cleanly hard-reject (e.g., non-fatal warnings returned alongside rows). |
| `advisory` | Remains as agent-side prose instruction; no programmatic enforcement feasible without intent context. |
| `hybrid` | Straddles the boundary: one sub-predicate is programmatic, another is intent-only. Both recorded explicitly. |

### Cost ratings (H/M/L)

- **False-positive cost:** cost of incorrectly *rejecting* a valid query.
- **False-negative cost:** cost of incorrectly *admitting* a bad query (wrong answer served to user).

---

## Rule Matrix

### R1 — Resolve exact selections first

**Source:** `references/selection.md`  
**Verbatim excerpt:** _"Before any query call, map the question to exact `unique_name`s for every metric, dimension, hierarchy, and calculation … never guess a name you haven't seen."_

**Failure mode prevented:** Agent guesses a plausible-sounding column name that doesn't exist in the model, producing a path-validation error or — worse — a silently-wrong result if the name happens to match a different column.

| Field | Value |
|---|---|
| Decidability | `sql+metadata` |
| Enforcement tier | `pre-execution-validation-finding` |
| Tier rationale | `run_query` already validates the column path before executing (zero warehouse cost); a missing column returns a path-validation error, surfacing R1 violation as a finding. Full pre-query enforcement would need the server to confirm every name was seen via a catalog lookup in the session, which requires metadata. |
| False-positive cost | **L** — path validation only fires on genuinely unknown column names; valid queries pass. |
| False-negative cost | **H** — an unresolved name that accidentally matches a different column returns wrong data silently. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | No direct overlap; validator checks SQL structure, not column existence. |

---

### R2 — Surface ambiguity as a tight choice

**Source:** `references/selection.md`  
**Verbatim excerpt:** _"When the question has more than one reasonable reading … list the candidates … and ask the user to pick. Don't silently take the first match."_

**Failure mode prevented:** Agent silently picks one of multiple valid candidate columns (e.g. `Gross Sales` vs `Net Sales`), returning a number the user didn't intend.

| Field | Value |
|---|---|
| Decidability | `intent` |
| Enforcement tier | `advisory` |
| Tier rationale | Detecting ambiguity requires knowing whether the user's phrasing genuinely maps to multiple candidates — this requires semantic matching against the question, not SQL parsing or catalog lookup alone. |
| False-positive cost | N/A (advisory — cannot over-reject) |
| False-negative cost | **H** — wrong metric or dimension selected silently; user sees a confident wrong answer. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | No — validator does not inspect agent question-resolution logic. |

---

### R3 — Confirm even when certain

**Source:** `references/selection.md`  
**Verbatim excerpt:** _"Present your chosen selections with the alternatives you rejected and why, and wait for approval before calling `run_query`."_

**Failure mode prevented:** Agent runs a query without confirmation, returning a result the user would have corrected if asked.

| Field | Value |
|---|---|
| Decidability | `intent` |
| Enforcement tier | `advisory` |
| Tier rationale | Whether a confirmation step occurred requires tracking conversational turn state, not SQL content. A server-side gate (the TODO in `internal/tools/tools.go` for `skills.SessionKeySkillLoaded`) could enforce skill-load as a proxy, but cannot verify that confirmation was obtained. |
| False-positive cost | N/A (advisory) |
| False-negative cost | **M** — skipping confirmation occasionally catches a wrong selection; the failure rate depends on query clarity, so cost is medium. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | No direct overlap. |

---

### R4 — Never fabricate results

**Source:** `references/results.md`  
**Verbatim excerpt:** _"Every number you present must come verbatim from a `run_query` result this conversation. Don't invent rows, round before the engine returns, or paraphrase a remembered figure as if freshly queried."_

**Failure mode prevented:** Agent presents a recalled, estimated, or invented number as if it were a live query result, causing silent data-quality failures.

| Field | Value |
|---|---|
| Decidability | `intent` |
| Enforcement tier | `advisory` |
| Tier rationale | Detecting fabrication requires comparing agent output against query results in the session transcript — impossible at the server's SQL-validation layer. Mentioned explicitly in `preQueryGate` as one of three always-on prose rules. |
| False-positive cost | N/A (advisory) |
| False-negative cost | **H** — fabricated numbers may be plausible, go undetected, and inform real business decisions. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | No. |

---

### R5 — On empty or unexpected results, don't self-compute

**Source:** `references/results.md`  
**Verbatim excerpt:** _"When a query returns zero rows, null measures, or contradictory values, report it verbatim and ask how to proceed … Don't substitute your own arithmetic."_

**Failure mode prevented:** Agent back-fills empty results with estimated values or combines multiple queries' numbers, presenting arithmetic the engine never ran.

| Field | Value |
|---|---|
| Decidability | `intent` |
| Enforcement tier | `advisory` |
| Tier rationale | Detecting whether the agent is self-computing requires inspecting the agent's response against the set of returned rows — not available at query-validation time. |
| False-positive cost | N/A (advisory) |
| False-negative cost | **H** — self-computed fill-ins can look authoritative; downstream decisions based on fabricated reconciliation are high-risk. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | No. |

---

### R6 — Read a calculated measure's MDX and include its dependencies

**Source:** `references/calculations.md`  
**Verbatim excerpt:** _"When `calculation` is non-empty, read the MDX before querying … Pull out: Hierarchy dependencies … each must appear in the SELECT, or the calc resolves against `[All]` and silently returns an incorrect result."_

**Failure mode prevented:** Calc measure queried without its required hierarchy dependency resolves against `[All]`, returning a silently wrong aggregate.

| Field | Value |
|---|---|
| Decidability | `sql+metadata` |
| Enforcement tier | `hybrid` |
| Tier rationale | Sub-predicate split below. |
| **Hybrid split** | **Programmatic:** detect that a column has `calculation != ""` (metadata-checkable) and that the SELECT lacks any column from the calc's dependency hierarchies (metadata + SQL parse). **Intent:** choosing *which* hierarchy to include when multiple are valid remains agent judgment. |
| False-positive cost | **M** — incorrectly requiring a dependency that isn't mandatory would block valid queries; metadata parsing must be precise. |
| False-negative cost | **H** — missing hierarchy means silently wrong time-period or level-scoped calc result. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | Partial — validator checks column-group path conformance but does not inspect MDX dependency chains. |

---

### R7 — Prefer existing calculations over hand-built logic

**Source:** `references/calculations.md`  
**Verbatim excerpt:** _"When the question implies a derivation … search the catalog for a calc that already encodes it before composing your own."_

**Failure mode prevented:** Agent reinvents time-intelligence or ratio logic from base measures, diverging from the model's defined NULL handling, scope, and division guards — producing wrong or inconsistent results.

| Field | Value |
|---|---|
| Decidability | `intent` |
| Enforcement tier | `advisory` |
| Tier rationale | Determining whether the agent searched the catalog exhaustively before building hand-logic requires examining the agent's dialog and search steps — not inspectable at SQL-validation time. |
| False-positive cost | N/A (advisory) |
| False-negative cost | **M** — hand-built calcs diverge from model intent and produce wrong answers, but the failure is often detectable by the user comparing expected vs. returned values. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | No. |

---

### R8 — Verify a filter literal's format and existence

**Source:** `references/filters.md`  
**Verbatim excerpt:** _"Before filtering on a literal you haven't seen this conversation, preview it … A text column named `Order Custom Year` could hold `'2008'`, `'CY2008'`, `'Reporting Calendar 2008'`, or whatever its underlying column stores. Guessing runs clean and returns zero rows."_

**Failure mode prevented:** Agent uses an unverified literal in a WHERE clause; the query runs without error but returns zero rows (wrong result, no diagnostic signal).

| Field | Value |
|---|---|
| Decidability | `sql+metadata` |
| Enforcement tier | `hybrid` |
| Tier rationale | Sub-predicate split below. |
| **Hybrid split** | **Programmatic:** detect that the SQL contains a literal filter (`WHERE "col" = 'value'`) on a text/varchar column whose member set is not returned by catalog metadata tools (column type = `text`, not `calculation_group`). Flag as a finding requiring preview. **Intent:** whether a literal was "seen this conversation" requires session-turn tracking, not SQL parsing. Whether to require a preview for numeric range-confirmed values is agent judgment. |
| False-positive cost | **M** — overly aggressive flagging of every string literal would block queries where the value is obviously correct (e.g., a literal sourced from a prior result this conversation). |
| False-negative cost | **H** — wrong literal runs cleanly, returns zero rows, and the agent may interpret the empty result as "no data" rather than a filter mismatch. |
| Jira cross-ref | ATSCALE-48423 (path compat / filter-literal related) |
| `mqo-param-validator` overlap | Partial — validator may flag unrecognized filter patterns; this rule is broader (existence + format, not just syntax). |

---

### R9 — Stay in the semantic layer

**Source:** `references/query-construction.md`  
**Verbatim excerpt:** _"Every answer shown to the user must come from a `run_query` result against a registered model — the `catalog`/`schema`/`table` tuple in `list_models`."_

**Failure mode prevented:** Agent queries raw warehouse tables, bypassing semantic-layer aggregate routing, certified-flag rules, semi-additive logic, and calc-defined NULL/scope handling — producing wrong numbers that look correct.

| Field | Value |
|---|---|
| Decidability | `sql+metadata` |
| Enforcement tier | `server-validator` |
| Tier rationale | The FROM clause's catalog/schema can be checked against the registered model list at validation time (metadata lookup). A FROM that does not match any `atscale_catalogs` entry can be rejected hard. Mentioned as one of three always-on rules in `preQueryGate`. |
| False-positive cost | **L** — a legitimate AtScale query always uses the `atscale_catalogs` prefix; a non-conforming FROM is always wrong in this context. |
| False-negative cost | **H** — raw-warehouse bypass silently omits all semantic-layer logic, returning plausibly wrong numbers with no error. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | Yes — validator's catalog-check intent aligns; R9 is the canonical prose statement this validator rule expresses. |

---

### R10 — Fully qualify every model reference

**Source:** `references/query-construction.md`  
**Verbatim excerpt:** _"The single `FROM` target uses the three-part name `"<catalog>"."<schema>"."<table>"` exactly as `list_models` returns it … Do not write `JOIN`, `UNION`/`UNION ALL`/`INTERSECT`/`EXCEPT`, subqueries, or CTEs (`WITH`)."_

**Failure mode prevented:** (a) Under-qualified FROM can resolve to a different or non-existent model. (b) JOIN/UNION/CTE/subquery bypasses the semantic layer's single-virtual-table contract, producing errors or silently wrong cross-model data.

| Field | Value |
|---|---|
| Decidability | `sql-only` |
| Enforcement tier | `server-validator` |
| Tier rationale | Already enforced: `internal/tools/query_validator.go` rejects queries that break R10 (no JOIN/CTE/subquery; FROM must name one registered model, fully qualified) before they hit the warehouse. The reference implementation from PR #79. Error message cites the rule. |
| False-positive cost | **L** — the check is structural (SQL parse); a fully-qualified single-FROM query is never mis-rejected. |
| False-negative cost | **H** — a JOIN/CTE silently combines model data with raw warehouse data or crosses model boundaries, producing undefined results. |
| Jira cross-ref | ATSCALE-48466 (multi-statement class-cast via run_query path) |
| `mqo-param-validator` overlap | Yes — `mqo-param-validator` covers structural SQL checks; R10's FROM/JOIN enforcement in `query_validator.go` is the canonical implementation this validator pattern mirrors. |

---

### R11 — Include a trigger hierarchy for semi-additive measures

**Source:** `references/semi-additive.md`  
**Verbatim excerpt:** _"When you select a semi-additive measure, include at least one level from one of its `relationships[].hierarchy` values, or one of its `degenerate_dimensions`. Without one: the engine falls back to the declared `aggregation` … `run_query` returns a non-fatal warning alongside the rows — surface it."_

**Failure mode prevented:** Semi-additive measure queried without its trigger hierarchy silently falls back to base aggregation (e.g., `Max` instead of `last_non_empty`), returning a wrong value with no error — only a non-fatal warning.

| Field | Value |
|---|---|
| Decidability | `sql+metadata` |
| Enforcement tier | `pre-execution-validation-finding` |
| Tier rationale | `run_query` already returns a non-fatal warning when a semi-additive trigger gap is detected (per ENFORCEMENT.md: _"run_query warns when a semi-additive measure is missing its trigger hierarchy (R11)"_). This is the correct tier — hard rejection would be a false positive when the user explicitly approves querying without a trigger. The finding surfaces the gap for agent + user decision. |
| False-positive cost | **M** — hard rejection of trigger-less semi-additive queries would block valid approved cases; warning tier is correct. |
| False-negative cost | **H** — silent fallback to wrong aggregation (e.g., `Max` headcount across all dates instead of `last_non_empty` by date) produces plausible-looking wrong numbers. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | Partial — semi-additive trigger detection requires model metadata (`semi_additive` block); `mqo-param-validator` structural checks do not inspect this. |

---

### R12 — Handle the row-cap advisory

**Source:** `references/results.md`  
**Verbatim excerpt:** _"Over the cap the query still runs (credits spent), returns zero rows, and the server replies `IsError: true` with a JSON advisory … never auto-pick; the user owns the trade-off."_

**Failure mode prevented:** Agent silently retries a row-cap-hit query with a self-chosen rewrite (e.g., auto-adding `LIMIT 25 ORDER BY desc`), presenting partial results as if they were the complete answer.

| Field | Value |
|---|---|
| Decidability | `intent` |
| Enforcement tier | `advisory` |
| Tier rationale | The server already *surfaces* the row-cap advisory (IsError + JSON payload). R12's mandate is about agent *behavior after* the signal: not auto-retrying. That behavior is post-execution intent, not enforceable at the SQL-validation layer. |
| False-positive cost | N/A (advisory) |
| False-negative cost | **M** — a silently-narrowed retry returns partial data presented as complete; user may not notice. Medium (not high) because the cap signal is visible in the transcript if the user looks. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | No. |

---

### R13 — Return bounded results

**Source:** `references/query-construction.md`  
**Verbatim excerpt:** _"Default to result sets that answer the question concisely … Add `LIMIT` plus an `ORDER BY` matched to intent for human-readable results; disclose the limit."_

**Failure mode prevented:** Agent returns an unbounded result set (potentially millions of rows), burning context tokens, risking the engine row cap (R12), and obscuring the useful signal in noise.

| Field | Value |
|---|---|
| Decidability | `sql-only` |
| Enforcement tier | `hybrid` |
| Tier rationale | Sub-predicate split below. |
| **Hybrid split** | **Programmatic:** detect that a query SELECTs a near-unique column (Customer ID, Order Number, Session ID) without a `LIMIT` clause — rejectable or warn-able from SQL parse alone. **Intent:** determining whether the user explicitly asked for per-row enumeration (in which case LIMIT is not required) needs dialog context. |
| False-positive cost | **M** — flagging every non-LIMIT query would block legitimate unbounded aggregate queries (e.g., `SELECT "Region", "Sales Amount" GROUP BY "Region"` — 10 rows, no LIMIT needed). Rule applies to near-unique GROUP BY columns. |
| False-negative cost | **M** — unbounded large result sets are expensive and noisy but don't produce wrong *values*, just too many of them; R12 is the safety backstop. |
| Jira cross-ref | — |
| `mqo-param-validator` overlap | Yes — `mqo-param-validator` row-threshold check (`--row-threshold`) enforces a version of this at the MQO layer. R13 is the query-construction-time statement of the same constraint. |

---

## Coverage note: ENFORCEMENT.md authoring gates

`ENFORCEMENT.md` contains authoring/workflow gates (`preQueryGate` prose, Workflow Gate / STOP / CHECKLIST_ACK pattern) directed at skill authors/reviewers, not at the agent. Per OQ-B (PRD §9), these are tracked separately as authoring-side rules and are **not** enumerated in this matrix as agent-facing gate rules. The three rules surfaced in `preQueryGate` (R3 confirm, R4 no fabrication, R9 semantic-layer-only) are fully classified above.

---

## Priority-Ordered Migration List

Sorted by: **false-negative cost desc** (H > M > L), then **false-positive cost asc** (L < M < H).  
Target: rules where high false-negative cost + low false-positive cost = highest migration ROI.

Rules in `advisory` tier are included for completeness but marked as intent-blocked from code migration.

| Priority | Rule | False-Neg | False-Pos | Current Tier | Target Tier | Migration note |
|---|---|---|---|---|---|---|
| 1 | **R10** — Fully qualified FROM / no JOIN/CTE/subquery | H | L | `server-validator` | `server-validator` | **Already shipped** (PR #79, `query_validator.go`). Reference implementation for all future migrations. |
| 2 | **R9** — Stay in semantic layer (atscale_catalogs only) | H | L | `server-validator` | `server-validator` | **Already enforceable**; FROM catalog check should be validated in `query_validator.go` alongside R10 if not already present. |
| 3 | **R1** — Resolve exact selections first | H | L | `pre-execution-validation-finding` | `pre-execution-validation-finding` | Path-validation already fires on unknown column names (zero warehouse cost). No additional server migration needed; finding already surfaces the violation. |
| 4 | **R11** — Semi-additive trigger hierarchy | H | M | `pre-execution-validation-finding` | `pre-execution-validation-finding` | Non-fatal warning already emitted by `run_query`. Consider promoting to hard rejection when `semi_additive` block present + no trigger column in SELECT (metadata gate). False-positive cost rises to H if user explicitly approved trigger-less query — keep as warning. |
| 5 | **R4** — Never fabricate results | H | N/A | `advisory` | `advisory` | Intent-blocked. No server migration path. |
| 6 | **R5** — Don't self-compute on empty results | H | N/A | `advisory` | `advisory` | Intent-blocked. |
| 7 | **R2** — Surface ambiguity | H | N/A | `advisory` | `advisory` | Intent-blocked. |
| 8 | **R6** — MDX calc dependency inclusion | H | M | `hybrid` | `pre-execution-validation-finding` | Programmatic sub-predicate: detect calc column in SELECT, check MDX dependency hierarchy against SELECT columns using metadata. Migrate the programmatic sub-predicate to a validation finding. Intent sub-predicate (hierarchy choice) stays advisory. Jira: create PRD-2 ticket. |
| 9 | **R8** — Verify filter literal format/existence | H | M | `hybrid` | `pre-execution-validation-finding` | Programmatic sub-predicate: flag text-column literal filters for preview requirement. Session-turn tracking (was it "seen this conversation") stays advisory. Jira: ATSCALE-48423. |
| 10 | **R3** — Confirm before running | M | N/A | `advisory` | `advisory` (skill-load gate possible) | The TODO in `tools.go` for `skills.SessionKeySkillLoaded` could enforce skill-load as a proxy for R3, but not confirmation itself. Low-priority server migration. |
| 11 | **R12** — Row-cap advisory behavior | M | N/A | `advisory` | `advisory` | Cap *signal* already surfaced by server. Agent *behavior* post-signal is intent-only. |
| 12 | **R7** — Prefer existing calcs | M | N/A | `advisory` | `advisory` | Intent-blocked. |
| 13 | **R13** — Bounded results | M | M | `hybrid` | `pre-execution-validation-finding` | Programmatic sub-predicate: detect near-unique GROUP BY column without LIMIT. Intent sub-predicate: user asked for per-row detail. Migrate programmatic detection as a warning finding. `mqo-param-validator` row-threshold already covers the MQO-layer equivalent. |

### Migration summary

- **Already server-validated (no action needed):** R10  
- **Confirm server-validator coverage:** R9 (check `query_validator.go`)  
- **Already pre-execution finding (no action needed):** R1, R11  
- **Migrate programmatic sub-predicate to finding (PRD-2 scope):** R6, R8, R13  
- **Intent-blocked, remain advisory:** R2, R3, R4, R5, R7, R12  

---

*This matrix is the source of record for PRD-2 (server-validator migrations) and PRD-3 (typed-error wire schema). Changes to the tier vocabulary after PRD-2 starts must be versioned in this header.*
