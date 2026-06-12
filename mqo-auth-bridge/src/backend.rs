//! Query backend variants.

/// The `AtScale` query backend to execute against.
///
/// - [`Backend::Sql`] is routed to the `PGWire` path (port 15432 by default).
/// - [`Backend::Dax`] and [`Backend::Mdx`] are both routed to the XMLA/engine
///   path (`/v1/xmla` on the cluster's public hostname by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// DAX query — sent via XMLA POST.
    Dax,
    /// MDX query — sent via XMLA POST.
    Mdx,
    /// SQL query — sent over `PGWire`.
    Sql,
}

impl Backend {
    /// Returns the canonical lowercase string used in fixture value synthesis.
    #[must_use]
    pub fn as_fixture_key(self) -> &'static str {
        match self {
            Self::Dax => "dax",
            Self::Mdx => "mdx",
            Self::Sql => "sql",
        }
    }
}
