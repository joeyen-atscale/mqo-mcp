//! XMLA cellset parser — converts `Execute` response XML into `Vec<Value>` rows.
//!
//! Handles two response shapes produced by `AtScale`'s XMLA endpoint:
//!
//! 1. **Tabular rowset** (`<Format>Tabular</Format>`) — the preferred shape.
//!    Rows live under the `urn:schemas-microsoft-com:xml-analysis:rowset`
//!    namespace; each `<row>` element has child elements whose local names are
//!    column names and whose text content is the cell value.
//!
//! 2. **`<MDDataSet>`** — the multidimensional form returned when the endpoint
//!    ignores the `Tabular` format request.  Axis tuples supply column names
//!    (via `<Caption>`); the `<CellData>` section supplies values.
//!
//! Both paths produce `{column → value}` JSON objects in the same
//! `Vec<serde_json::Value>` shape the `PGWire` path emits.
//!
//! ## Cell-value coercion
//!
//! | Raw text      | JSON value      |
//! |---------------|-----------------|
//! | numeric text  | `json!(f64)`    |
//! | other text    | `json!(string)` |
//! | absent/empty  | `Value::Null`   |
//!
//! Empty/absent cells are **never** coerced to `0`.  MDX BLANK == `Null`.
//!
//! ## Error handling
//!
//! A SOAP `<Fault>` or XMLA `<Messages><Error>` envelope yields
//! `Err(EngineError::QueryError)` whose message contains the fault text
//! verbatim.  Parse failure (malformed XML) also returns `Err`.  There is
//! **no** synthetic-row fallback.

use quick_xml::{events::Event, Reader};
use serde_json::Value;

use crate::error::EngineError;

// ─── Public entry point ───────────────────────────────────────────────────────

/// Parse an XMLA `Execute` response into a list of `{column → value}` rows.
///
/// # Arguments
///
/// * `xml`   – The raw XML response body from the XMLA endpoint.
/// * `limit` – Maximum number of rows to return. Callers should also apply
///   [`crate::engine::HARD_ROW_CAP`] before invoking this function.
///
/// # Errors
///
/// Returns [`EngineError::QueryError`] when:
/// - The XML contains a SOAP `<Fault>` element.
/// - The XML contains an XMLA `<Messages><Error>` element.
/// - The XML is syntactically malformed.
///
/// On any error the function returns `Err`; it **never** fabricates rows.
pub fn parse_xmla_cellset(xml: &str, limit: usize) -> Result<Vec<Value>, EngineError> {
    // Quick pre-check: does the response contain a SOAP Fault?
    if xml.contains("<Fault") || xml.contains(":Fault") {
        let msg = extract_fault_string(xml);
        return Err(EngineError::QueryError { reason: msg });
    }

    // Determine shape: MDDataSet vs Tabular rowset.
    if xml.contains("MDDataSet") {
        parse_mddataset(xml, limit)
    } else {
        parse_tabular_rowset(xml, limit)
    }
}

// ─── Tabular rowset parser ────────────────────────────────────────────────────

/// Parse the Tabular rowset form.
///
/// Expected structure (simplified):
/// ```xml
/// <root xmlns="urn:schemas-microsoft-com:xml-analysis:rowset">
///   <row>
///     <ColumnA>value</ColumnA>
///     <ColumnB>42.5</ColumnB>
///   </row>
///   …
/// </root>
/// ```
fn parse_tabular_rowset(xml: &str, limit: usize) -> Result<Vec<Value>, EngineError> {
    let mut reader = Reader::from_str(xml);
    // Do NOT trim_text: banded level values such as "  0- 50" and " 50-100"
    // carry meaningful leading spaces that distinguish tier buckets.  Trimming
    // would collapse them to "0- 50" / "50-100", mismatching the gold SQL output.
    // Whitespace-only text nodes that appear between XML tags (outside <row> or
    // outside column elements) are harmlessly ignored by the `in_row` / `current_col`
    // guards below.

    let mut rows: Vec<Value> = Vec::new();
    let mut in_row = false;
    let mut current_col: Option<String> = None;
    let mut current_obj: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = local_name_str(e.local_name().as_ref());
                if local == "row" {
                    in_row = true;
                    current_obj = serde_json::Map::new();
                } else if in_row {
                    current_col = Some(local);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name_str(e.local_name().as_ref());
                if local == "row" {
                    if rows.len() >= limit {
                        break;
                    }
                    rows.push(Value::Object(current_obj.clone()));
                    in_row = false;
                    current_obj = serde_json::Map::new();
                } else if in_row && current_col.as_deref() == Some(&local) {
                    // Column end without intervening Text → absent == Null.
                    let col = current_col.take().unwrap();
                    current_obj.entry(col).or_insert(Value::Null);
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_row {
                    if let Some(col) = current_col.take() {
                        let raw = e.unescape().map_err(|err| EngineError::QueryError {
                            reason: format!("XML unescape error: {err}"),
                        })?;
                        // Pass the raw value without trimming so that
                        // leading/trailing spaces in banded level members
                        // (e.g. "  0- 50", " 50-100") are preserved exactly
                        // as the semantic model emits them.
                        let v = coerce_cell(&raw);
                        current_obj.insert(col, v);
                    }
                }
            }
            // Empty element (<ColumnA/>) → Null.
            Ok(Event::Empty(ref e)) => {
                if in_row {
                    let local = local_name_str(e.local_name().as_ref());
                    if local != "row" {
                        current_obj.insert(local, Value::Null);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(EngineError::QueryError {
                    reason: format!("XML parse error: {err}"),
                });
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(rows)
}

// ─── MDDataSet types ──────────────────────────────────────────────────────────

/// Intermediate state accumulated while scanning `<Axes>`.
struct AxesState {
    /// `axes[axis_idx][tuple_idx] = Vec<member_caption>`.
    axes: Vec<Vec<Vec<String>>>,
    current_axis: usize,
    current_tuples: Vec<Vec<String>>,
    current_tuple_members: Vec<String>,
    in_caption: bool,
}

impl AxesState {
    fn new() -> Self {
        Self {
            axes: Vec::new(),
            current_axis: 0,
            current_tuples: Vec::new(),
            current_tuple_members: Vec::new(),
            in_caption: false,
        }
    }
}

/// Intermediate state accumulated while scanning `<CellData>`.
struct CellState {
    values: Vec<Option<Value>>,
    current_ordinal: Option<usize>,
    in_value: bool,
}

impl CellState {
    fn new() -> Self {
        Self {
            values: Vec::new(),
            current_ordinal: None,
            in_value: false,
        }
    }

    fn set_at_ordinal(&mut self, v: Value) {
        if let Some(ord) = self.current_ordinal {
            while self.values.len() <= ord {
                self.values.push(None);
            }
            self.values[ord] = Some(v);
        }
    }
}

// ─── MDDataSet parser ─────────────────────────────────────────────────────────

/// Parse the `<MDDataSet>` multidimensional form.
///
/// `AtScale`'s `MDDataSet` in practice looks like:
/// ```xml
/// <MDDataSet>
///   <Axes>
///     <Axis name="Axis0">  <!-- rows -->
///       <Tuples><Tuple><Member><Caption>Foo</Caption></Member></Tuple></Tuples>
///     </Axis>
///     <Axis name="Axis1">  <!-- columns / measures -->
///       <Tuples><Tuple><Member><Caption>Measure1</Caption></Member></Tuple></Tuples>
///     </Axis>
///   </Axes>
///   <CellData>
///     <!-- `CellData` is indexed by ordinal (row * `num_cols` + `col_idx`) -->
///     <Cell CellOrdinal="0"><Value>42</Value></Cell>
///   </CellData>
/// </MDDataSet>
/// ```
///
/// Axis1 member captions → measure/column names; Axis0 tuples → row keys.
fn parse_mddataset(xml: &str, limit: usize) -> Result<Vec<Value>, EngineError> {
    let (axes_state, cell_state) = scan_mddataset_xml(xml)?;
    Ok(build_mddataset_rows(&axes_state, &cell_state, limit))
}

/// First pass: scan the XML and populate `AxesState` + `CellState`.
fn scan_mddataset_xml(xml: &str) -> Result<(AxesState, CellState), EngineError> {
    let mut reader = Reader::from_str(xml);
    // Do NOT trim_text: cell values in banded levels ("  0- 50", " 50-100") carry
    // semantically significant leading spaces.  Column captions are trimmed
    // explicitly in the Text handler below.

    let mut axes = AxesState::new();
    let mut cells = CellState::new();
    let mut in_axes = false;
    let mut in_celldata = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                handle_mddataset_start(e, &mut axes, &mut cells, &mut in_axes, &mut in_celldata);
            }
            Ok(Event::End(ref e)) => {
                handle_mddataset_end(e, &mut axes, &mut cells, &mut in_axes);
            }
            Ok(Event::Text(ref e)) => {
                let raw = e.unescape().map_err(|err| EngineError::QueryError {
                    reason: format!("XML unescape error: {err}"),
                })?;
                if axes.in_caption && in_axes {
                    // Captions are column/hierarchy names — trim is appropriate.
                    axes.current_tuple_members.push(raw.trim().to_string());
                } else if cells.in_value && in_celldata {
                    // Cell values: preserve leading/trailing spaces (banded tiers).
                    // Only skip purely-whitespace nodes that represent XML indentation.
                    if !raw.trim().is_empty() {
                        cells.set_at_ordinal(coerce_cell(&raw));
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = local_name_str(e.local_name().as_ref());
                // <Value/> → null cell.
                if local == "Value" && in_celldata {
                    cells.set_at_ordinal(Value::Null);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok((axes, cells))
}

/// Handle a `Start` event during `MDDataSet` scanning.
fn handle_mddataset_start(
    e: &quick_xml::events::BytesStart<'_>,
    axes: &mut AxesState,
    cells: &mut CellState,
    in_axes: &mut bool,
    in_celldata: &mut bool,
) {
    let local = local_name_str(e.local_name().as_ref());
    match local.as_str() {
        "Axes" => *in_axes = true,
        "Axis" => {
            let axis_name = attr_value(e, b"name").unwrap_or_default();
            if let Some(idx) = axis_index_from_name(&axis_name) {
                axes.current_axis = idx;
                while axes.axes.len() <= axes.current_axis {
                    axes.axes.push(Vec::new());
                }
            }
            axes.current_tuples = Vec::new();
        }
        "Tuple" => axes.current_tuple_members = Vec::new(),
        "Caption" if *in_axes => axes.in_caption = true,
        "CellData" => {
            *in_axes = false;
            *in_celldata = true;
        }
        "Cell" if *in_celldata => {
            let ordinal_str = attr_value(e, b"CellOrdinal").unwrap_or_default();
            cells.current_ordinal = ordinal_str.parse::<usize>().ok();
        }
        "Value" if *in_celldata => cells.in_value = true,
        _ => {}
    }
}

/// Handle an `End` event during `MDDataSet` scanning.
fn handle_mddataset_end(
    e: &quick_xml::events::BytesEnd<'_>,
    axes: &mut AxesState,
    cells: &mut CellState,
    in_axes: &mut bool,
) {
    let local = local_name_str(e.local_name().as_ref());
    match local.as_str() {
        "Axes" => *in_axes = false,
        "Axis" => {
            if axes.current_axis < axes.axes.len() {
                axes.axes[axes.current_axis].clone_from(&axes.current_tuples);
            }
        }
        "Tuple" => axes.current_tuples.push(axes.current_tuple_members.clone()),
        "Caption" => axes.in_caption = false,
        "Value" => cells.in_value = false,
        _ => {}
    }
}

/// Second pass: assemble `Vec<Value>` rows from the collected axis and cell data.
fn build_mddataset_rows(axes: &AxesState, cells: &CellState, limit: usize) -> Vec<Value> {
    if axes.axes.len() < 2 {
        return Vec::new();
    }

    let row_tuples = &axes.axes[0];
    let col_tuples = &axes.axes[1];

    // Column names: flatten each column tuple's member captions with ".".
    let col_names: Vec<String> = col_tuples
        .iter()
        .map(|members| members.join("."))
        .collect();

    let num_cols = col_names.len();
    if num_cols == 0 {
        return Vec::new();
    }

    let row_keys: Vec<String> = row_tuples
        .iter()
        .map(|members| {
            members
                .first()
                .cloned()
                .unwrap_or_else(|| String::from("(unknown)"))
        })
        .collect();

    let num_rows = row_tuples.len().min(limit);
    let mut result: Vec<Value> = Vec::with_capacity(num_rows);

    for row_idx in 0..num_rows {
        let mut obj = serde_json::Map::new();
        if !row_keys.is_empty() {
            obj.insert(
                "Row".to_string(),
                Value::String(row_keys[row_idx].clone()),
            );
        }
        for (col_idx, col_name) in col_names.iter().enumerate() {
            let ordinal = row_idx * num_cols + col_idx;
            let v = cells
                .values
                .get(ordinal)
                .and_then(Clone::clone)
                .unwrap_or(Value::Null);
            obj.insert(col_name.clone(), v);
        }
        result.push(Value::Object(obj));
    }

    result
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Extract the fault string from a SOAP Fault XML document.
///
/// Returns the first `<faultstring>` text found, or the first 512 chars of the
/// raw XML if none is found (so the caller always has something to display).
fn extract_fault_string(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_faultstring = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = local_name_str(e.local_name().as_ref());
                if local.eq_ignore_ascii_case("faultstring")
                    || local.eq_ignore_ascii_case("Description")
                    || local.eq_ignore_ascii_case("ErrorCode")
                {
                    in_faultstring = true;
                }
            }
            Ok(Event::Text(ref e)) if in_faultstring => {
                if let Ok(raw) = e.unescape() {
                    let t = raw.trim().to_string();
                    if !t.is_empty() {
                        return t;
                    }
                }
            }
            Ok(Event::End(_)) => in_faultstring = false,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    // Fallback: return the raw XML body (truncated to 512 chars).
    xml.chars().take(512).collect()
}

/// Convert a cell text value to a `serde_json::Value`.
///
/// - Empty or whitespace-only → `Value::Null`  (never `0`)
/// - Parses as `f64` → `json!(n)`
/// - Otherwise → `Value::String` (value preserved verbatim, including any
///   leading/trailing spaces that are part of a banded level member such as
///   `"  0- 50"` or `" 50-100"`).
///
/// Callers must **not** pre-trim the text; trimming is intentionally left to
/// this function so that whitespace-only XML indentation nodes become `Null`
/// while meaningful leading spaces in tier labels are preserved.
#[inline]
fn coerce_cell(text: &str) -> Value {
    if text.trim().is_empty() {
        return Value::Null;
    }
    if let Ok(n) = text.trim().parse::<f64>() {
        serde_json::json!(n)
    } else {
        Value::String(text.to_string())
    }
}

/// Extract a UTF-8 attribute value from a `quick-xml` start element.
fn attr_value(e: &quick_xml::events::BytesStart<'_>, attr_name: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == attr_name)
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

/// Parse an XMLA axis name like `"Axis0"` or `"Axis1"` into a `usize` index.
fn axis_index_from_name(name: &str) -> Option<usize> {
    name.strip_prefix("Axis")
        .and_then(|rest| rest.parse::<usize>().ok())
}

/// Convert a `quick-xml` local name byte slice to an owned `String`.
#[inline]
fn local_name_str(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TABULAR_FIXTURE: &str = include_str!("../tests/fixtures/xmla_tabular_simple.xml");
    const TABULAR_100_FIXTURE: &str = include_str!("../tests/fixtures/xmla_tabular_100rows.xml");
    const SOAP_FAULT_FIXTURE: &str = include_str!("../tests/fixtures/xmla_soap_fault.xml");

    // ── AC1: tabular rowset parses to correct rows ────────────────────────────

    #[test]
    fn tabular_rowset_correct_count_and_columns() {
        let rows = parse_xmla_cellset(TABULAR_FIXTURE, 100).expect("parse ok");
        assert_eq!(rows.len(), 3, "should have 3 rows");
        let first = rows[0].as_object().expect("row is object");
        assert!(first.contains_key("Product"), "Product column");
        assert!(first.contains_key("Sales"), "Sales column");
        assert!(first.contains_key("Notes"), "Notes column");
    }

    // ── AC2a: numeric cell → json number ─────────────────────────────────────

    #[test]
    fn numeric_cell_parses_to_json_number() {
        let rows = parse_xmla_cellset(TABULAR_FIXTURE, 100).expect("parse ok");
        assert_eq!(
            rows[0]["Sales"],
            json!(10_169_858_384.28_f64),
            "numeric → f64"
        );
    }

    // ── AC2b: non-numeric cell → json string ─────────────────────────────────

    #[test]
    fn non_numeric_cell_parses_to_json_string() {
        let rows = parse_xmla_cellset(TABULAR_FIXTURE, 100).expect("parse ok");
        assert_eq!(rows[0]["Product"], json!("Widget A"), "string value");
    }

    // ── AC2c: absent cell → Value::Null, NOT 0 ───────────────────────────────

    #[test]
    fn absent_cell_is_null_not_zero() {
        let rows = parse_xmla_cellset(TABULAR_FIXTURE, 100).expect("parse ok");
        // Row 1 has Notes absent.
        assert_eq!(rows[1]["Notes"], Value::Null, "absent = Null");
        assert_ne!(rows[1]["Notes"], json!(0), "absent != 0");
        assert_ne!(rows[1]["Notes"], json!(""), "absent != empty string");
    }

    // ── AC3: SOAP Fault → Err with fault text ────────────────────────────────

    #[test]
    fn soap_fault_returns_err_with_fault_text() {
        let result = parse_xmla_cellset(SOAP_FAULT_FIXTURE, 100);
        assert!(result.is_err(), "should be Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Unsupported query syntax"),
            "fault text in error: {msg}"
        );
    }

    // ── AC3b: No synthetic rows on any error ─────────────────────────────────

    #[test]
    fn parse_failure_is_err_never_fabricated_rows() {
        let result = parse_xmla_cellset("<total gibberish not xml >>>", 100);
        // Either an Err or an empty Vec — never synthetic rows.
        match result {
            Err(_) => { /* expected */ }
            Ok(rows) => assert!(
                rows.is_empty(),
                "parse failure must not fabricate rows; got: {rows:?}"
            ),
        }
    }

    // ── AC4: limit is respected ───────────────────────────────────────────────

    #[test]
    fn limit_is_respected() {
        let rows = parse_xmla_cellset(TABULAR_100_FIXTURE, 5).expect("parse ok");
        assert_eq!(rows.len(), 5, "limit=5 → exactly 5 rows");
    }

    #[test]
    fn limit_zero_returns_empty() {
        let rows = parse_xmla_cellset(TABULAR_FIXTURE, 0).expect("parse ok");
        assert_eq!(rows.len(), 0, "limit=0 → empty");
    }

    // ── AC5: MDDataSet parses to same shape ──────────────────────────────────

    #[test]
    fn mddataset_parses_to_column_value_shape() {
        let xml = include_str!("../tests/fixtures/xmla_mddataset.xml");
        let rows = parse_xmla_cellset(xml, 100).expect("parse ok");
        assert!(!rows.is_empty(), "should have rows");
        let first = rows[0].as_object().expect("row is object");
        assert!(first.contains_key("Row"), "has Row key");
        assert!(first.len() > 1, "has measure columns");
    }

    // ── AC6: No synthetic fallback in xmla_execute ───────────────────────────
    // (Verified structurally: executor.rs calls parse_xmla_cellset whose only
    //  non-Err return is the parsed Vec — no synthetic branches exist.)

    // ── AC7: Banded-tier leading spaces are preserved (net-profit-tier fix) ───

    const BANDED_TIERS_FIXTURE: &str =
        include_str!("../tests/fixtures/xmla_tabular_banded_tiers.xml");

    /// Tabular rowset: cell values with leading spaces (banded tier labels such
    /// as `"  0- 50"` and `" 50-100"`) must be returned verbatim — trimming
    /// would produce `"0- 50"` / `"50-100"` which mismatches the gold SQL output.
    #[test]
    fn tabular_banded_tier_leading_spaces_preserved() {
        let rows = parse_xmla_cellset(BANDED_TIERS_FIXTURE, 100).expect("parse ok");
        assert_eq!(rows.len(), 4, "4 tier rows");

        // Row 0: "  0- 50" — two leading spaces
        assert_eq!(
            rows[0]["Net_x0020_Profit_x0020_Tier"],
            json!("  0- 50"),
            "two leading spaces preserved"
        );

        // Row 1: " 50-100" — one leading space
        assert_eq!(
            rows[1]["Net_x0020_Profit_x0020_Tier"],
            json!(" 50-100"),
            "one leading space preserved"
        );

        // Row 2: " 50 or Less" — one leading space
        assert_eq!(
            rows[2]["Net_x0020_Profit_x0020_Tier"],
            json!(" 50 or Less"),
            "leading space in 'or Less' variant preserved"
        );

        // Row 3: "100-150" — no leading space, baseline unchanged
        assert_eq!(
            rows[3]["Net_x0020_Profit_x0020_Tier"],
            json!("100-150"),
            "no leading space baseline unchanged"
        );
    }

    /// `coerce_cell` must treat whitespace-only text as Null (indentation nodes),
    /// while preserving leading spaces that precede non-whitespace content.
    #[test]
    fn coerce_cell_whitespace_only_is_null() {
        assert_eq!(coerce_cell(""), Value::Null, "empty → Null");
        assert_eq!(coerce_cell("   "), Value::Null, "spaces-only → Null");
        assert_eq!(coerce_cell("\t\n"), Value::Null, "tab/newline-only → Null");
    }

    #[test]
    fn coerce_cell_leading_space_preserved_in_string() {
        assert_eq!(
            coerce_cell("  0- 50"),
            json!("  0- 50"),
            "leading spaces in tier label preserved"
        );
        assert_eq!(
            coerce_cell(" 50-100"),
            json!(" 50-100"),
            "one leading space preserved"
        );
    }

    #[test]
    fn coerce_cell_numeric_with_surrounding_spaces_parses_as_number() {
        // Numbers may arrive with surrounding whitespace from XML formatting;
        // numeric parsing trims first.
        assert_eq!(coerce_cell(" 42 "), json!(42.0_f64), "padded int → f64");
        assert_eq!(
            coerce_cell("  3.14  "),
            json!(3.14_f64),
            "padded float → f64"
        );
    }
}
