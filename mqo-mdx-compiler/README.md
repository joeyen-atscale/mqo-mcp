# mqo-mdx-compiler

Some requests are inherently multidimensional — asymmetric axes, ragged /
parent-child hierarchy navigation, drill-through, or a client that wants a true
cellset (Excel/pivot). For those, the MQO compiles to MDX. This CLI takes a
`BoundMqo` and emits MDX honoring the gate's structural rules: fully-qualified
cube/three-part names (R10), `NON EMPTY` (R13), calc-group member literals (R7),
MDX-dependency hierarchies for calculated measures (R6), and semi-additive trigger
levels (R11).

Part of the MQO fleet.

**Execution path (confirmed 2026-06-10):** compiled MDX text is sent to `/v1/xmla`
via `mqo-auth-bridge`. The same endpoint also accepts DAX `EVALUATE`; both paths
use Bearer token auth (`atscale-mcp` client_credentials). Do not use port 11111
(firewalled externally) or `/xmla` on port 443 (routes to the Modeler UI).

## Install

```
cargo install --path .
```

## License

MIT OR Apache-2.0
