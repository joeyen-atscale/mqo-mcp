## v0.3.0 — 2026-06-11

Fix clippy warning in render-feature CLI arg parsing (unused `mut` on `render_out` when `render` feature is disabled). Optional `render` Cargo feature adds `--render <out.svg|out.png>` flag for spec render-verification via `vl-convert`. PRD: mqo-vega-render-check.
