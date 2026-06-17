# aso-lift Changelog

## v0.1.0 — 2026-06-16

**PRD:** PRD-osl-engine-xml-rdf-lift — Lift the engine model XML into a typed RDF knowledge graph.

### Summary

First release of `aso-lift`: a deterministic, offline engine-model XML → RDF transform that
types each AtScale model element as an `owl:NamedIndividual` in the `aso:` OWL2-DL vocabulary.

### What's included

- `lift(xml, opts)` — parse engine model XML and emit Turtle RDF.
- IRI minting from XSD `id` attributes (not mutable names) for stable cross-run identity (FR3).
- `owl:NamedIndividual` + `rdf:type aso:<Class>` + `rdfs:label` for every element (FR2).
- Role-playing `keyed-attribute-ref` emitted as `aso:playsRoleOf` triples (FR4).
- Hierarchy level order emitted as `aso:rollsUpTo` triples (FR4).
- Semi-additive measure additivity (`LastNonEmpty` etc.) emitted via `aso:additiveOver` (FR5).
- `owl:imports` the `aso:` TBox (and transitively BFO) in the emitted ontology header (FR6).
- `UnsupportedSchema` error for pre-2.0 schema versions with an actionable migration message (FR1/AC7).
- Unknown element kinds fall back to `aso:Attribute` with a logged warning, not silent drop (AC6).
- Deterministic, sorted triple emission — byte-identical output on rerun of identical input (NFR1).
- No warehouse credentials required — metadata-only transform (NFR2).
- `aso-lift` CLI binary: `aso-lift <model.xml> [--base-iri <IRI>] [--output <file.ttl>]`.

### Test fixture caveat

**Tests run against a SYNTHETIC fixture** (`tests/fixtures/synthetic-model.xml`), hand-crafted to
be representative of the real engine schema shape per the PRD and ontological-semantic-layer vision.
It is NOT the production `sales-insights-project.xml`. Real-XSD validation against an exported
engine model is deferred per PRD open question OQ2.

### Acceptance criteria coverage (against synthetic fixture)

| AC | Status | Note |
|----|--------|------|
| AC1 | Covered | Turtle round-trips; `owl:imports` aso: TBox present |
| AC2 | Covered | All 11 elements → typed NamedIndividual + label |
| AC3 | Covered | IRI keyed on `id`; stable after caption rename |
| AC4 | Covered | `rpr_order_date` → `aso:playsRoleOf` dim_date |
| AC5 | Covered | Two runs byte-identical |
| AC6 | Covered | Unknown element → `aso:Attribute` fallback + logged |
| AC7 | Covered | `project_1_1` → `LiftError::UnsupportedSchema` with migration message |
| AC8 | Covered | Succeeds with no warehouse env vars |

### Dependencies

- `aso-tbox` (path dep — must be on `build/osl-atscale-tbox` branch or later)
- `oxrdf 0.3` / `oxttl 0.2` — RDF model + Turtle serialisation
- `quick-xml 0.37` — XML parsing
- `thiserror 1` — error types
