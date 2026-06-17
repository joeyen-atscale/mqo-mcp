# Changelog — aso-tbox

## v0.1.0 — 2026-06-16

Initial release: AtScale Semantic-model Ontology (`aso:`) TBox v0.1.0.

BFO 2020-grounded OWL2-DL vocabulary for the AtScale OLAP element taxonomy (PRD-osl-atscale-tbox).

- Ships `ontology/aso.ttl` embedded via `include_str!` — 14 named classes (12 required FR1 kinds + SemiAdditiveMeasure + FullyAdditiveMeasure), 8 object properties (rollsUpTo, playsRoleOf, additiveOver, hasDimension, hasMeasure, hasHierarchy, hasLevel, hasKey).
- `aso:rollsUpTo` declared `owl:TransitiveProperty`; `aso:playsRoleOf` and all others declared `owl:ObjectProperty`.
- `owl:imports` BFO 2020 by reference IRI (`http://purl.obolibrary.org/obo/bfo/2020/bfo.owl`).
- Exposes all class and property IRIs as `&'static str` constants in `aso_tbox::iris`.
- `load_tbox()` parses the embedded TTL into `oxrdf::Graph` via oxttl; `load_tbox_from(src)` public for extension.
- `triples_for_subject(graph, iri)` helper for subject-keyed lookup.
- 14 unit tests covering PRD acceptance criteria AC1–AC3, round-trip, and BFO-import check.
