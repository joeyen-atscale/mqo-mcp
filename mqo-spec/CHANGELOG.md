# Changelog

## v0.2.0 — 2026-06-14

Relaxed the EmptyMeasures invariant: an Mqo with projection: true and >= 1 dimension/level is now valid without measures. Adds is_projection() helper. Zero measures + zero dimensions still fires EmptyMeasures. Adds related_attributes to level metadata. (PRD-mqo-attribute-projection)
