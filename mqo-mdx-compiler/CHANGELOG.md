# Changelog

## v0.2.0 — 2026-06-10

add MDX pre-flight syntax validator (validate_mdx_syntax): balanced delimiters, SELECT/axis/FROM/NON-EMPTY structural checks; --skip-syntax-check escape hatch; MdxCompileError::SyntaxCheckFailed variant; mirrors DAX compiler's existing validate_dax_syntax.
