//! mqo-session-footprint-meter — core classification and token-counting logic.
//!
//! Classifies every byte of every JSON-RPC frame in an `mqo-mcp-server` stdio
//! session into five context classes and estimates token cost using the shared
//! `chars/4` convention (same as `slai-context-budget-profiler`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Token arithmetic ──────────────────────────────────────────────────────────

/// Estimate tokens from a char count using `ceil(chars / chars_per_token)`.
///
/// # Errors
/// Returns an error if `chars_per_token` is zero.
pub fn tokens_from_chars(chars: usize, chars_per_token: u32) -> Result<u64, MeterError> {
    if chars_per_token == 0 {
        return Err(MeterError::ZeroCharsPerToken);
    }
    let cpt = u64::from(chars_per_token);
    // chars: usize — safe to widen to u64 on all target platforms (usize <= 64 bits).
    #[allow(clippy::as_conversions)]
    let c = chars as u64;
    Ok(c.div_ceil(cpt))
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors produced by the meter.
#[derive(Debug)]
pub enum MeterError {
    /// `--chars-per-token` was 0.
    ZeroCharsPerToken,
    /// A JSON-RPC frame could not be parsed.
    BadFrame(serde_json::Error),
    /// A `--pg-pass` literal was detected in the server command.
    LiteralPgPass,
    /// I/O error from subprocess or capture.
    Io(std::io::Error),
}

impl std::fmt::Display for MeterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroCharsPerToken => f.write_str("--chars-per-token must be > 0"),
            Self::BadFrame(e) => write!(f, "JSON-RPC parse error: {e}"),
            Self::LiteralPgPass => f.write_str(
                "security: --pg-pass literal detected in --server command; \
                 use --pg-pass-env <ENV_VAR> instead",
            ),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for MeterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BadFrame(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::ZeroCharsPerToken | Self::LiteralPgPass => None,
        }
    }
}

impl From<serde_json::Error> for MeterError {
    fn from(e: serde_json::Error) -> Self {
        Self::BadFrame(e)
    }
}

impl From<std::io::Error> for MeterError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ── Context classes ───────────────────────────────────────────────────────────

/// The five context classes to which every session byte is attributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenClass {
    /// MCP `initialize` / `tools/list` envelope (server capabilities).
    SystemPrompt,
    /// `describe_model` response body (catalog content).
    CatalogDescribeModel,
    /// Request frames and non-rows portions of response envelopes.
    ToolCall,
    /// The `rows` array in `query_multidimensional` responses.
    ToolResultRows,
    /// Any assistant-visible dialogue / text not attributed elsewhere.
    Dialogue,
}

impl std::fmt::Display for TokenClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::SystemPrompt => "system_prompt",
            Self::CatalogDescribeModel => "catalog_describe_model",
            Self::ToolCall => "tool_call",
            Self::ToolResultRows => "tool_result_rows",
            Self::Dialogue => "dialogue",
        };
        f.write_str(s)
    }
}

// ── Per-turn result ───────────────────────────────────────────────────────────

/// Token attribution for one session turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnFootprint {
    /// 1-based turn index.
    pub turn: u32,
    /// Operation name (`describe_model`, `query_multidimensional`, `system`, …).
    pub op: String,
    /// Total tokens for this turn.
    pub tokens: u64,
}

// ── Class token counts ────────────────────────────────────────────────────────

/// Token counts per context class.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClassTokens {
    /// Tokens attributed to the MCP handshake / tool-schema class.
    pub system_prompt: u64,
    /// Tokens attributed to `describe_model` catalog content.
    pub catalog_describe_model: u64,
    /// Tokens attributed to tool-call request frames and response envelopes.
    pub tool_call: u64,
    /// Tokens attributed to `rows` arrays in query responses.
    pub tool_result_rows: u64,
    /// Tokens attributed to dialogue / free text.
    pub dialogue: u64,
}

impl ClassTokens {
    /// Sum across all five classes.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.system_prompt
            + self.catalog_describe_model
            + self.tool_call
            + self.tool_result_rows
            + self.dialogue
    }

    /// Add `tokens` to the given class.
    pub fn add(&mut self, class: TokenClass, tokens: u64) {
        match class {
            TokenClass::SystemPrompt => self.system_prompt += tokens,
            TokenClass::CatalogDescribeModel => self.catalog_describe_model += tokens,
            TokenClass::ToolCall => self.tool_call += tokens,
            TokenClass::ToolResultRows => self.tool_result_rows += tokens,
            TokenClass::Dialogue => self.dialogue += tokens,
        }
    }
}

// ── Section detail (catalog) ──────────────────────────────────────────────────

/// Optional per-section token breakdown for the `catalog_describe_model` class,
/// mirroring the seven-section schema of `slai-context-budget-profiler`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogSections {
    /// Measure definitions.
    pub measures: u64,
    /// Dimension definitions.
    pub dimensions: u64,
    /// Calculated fields.
    pub calcs: u64,
    /// Hierarchy definitions.
    pub hierarchies: u64,
    /// Raw SQL / DAX expression strings.
    pub raw_expressions: u64,
    /// Semantic summary / description text.
    pub summaries: u64,
    /// Structural scaffolding / IDs.
    pub scaffolding: u64,
}

impl CatalogSections {
    /// Sum across all seven sections.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.measures
            + self.dimensions
            + self.calcs
            + self.hierarchies
            + self.raw_expressions
            + self.summaries
            + self.scaffolding
    }

    /// Build from a `describe_model` JSON `Value` by sizing each key group.
    ///
    /// Strategy: serialize each top-level value independently, measure chars,
    /// then apply `ceil(chars / chars_per_token)`.
    ///
    /// # Errors
    /// Returns an error if `chars_per_token` is zero.
    pub fn from_value(v: &Value, chars_per_token: u32) -> Result<Self, MeterError> {
        if chars_per_token == 0 {
            return Err(MeterError::ZeroCharsPerToken);
        }

        let Some(obj) = v.as_object() else {
            // Not an object — put everything in scaffolding.
            let chars = v.to_string().len();
            return Ok(Self {
                scaffolding: tokens_from_chars(chars, chars_per_token)?,
                ..Default::default()
            });
        };

        let mut measures = 0usize;
        let mut dimensions = 0usize;
        let mut calcs = 0usize;
        let mut hierarchies = 0usize;
        let mut raw_expressions = 0usize;
        let mut summaries = 0usize;
        let mut scaffolding = 0usize;

        for (key, val) in obj {
            let vstr = val.to_string();
            let vlen = vstr.len();
            let klen = key.len() + 4; // key + quotes + colon + space
            let total_len = klen + vlen;

            let k = key.as_str();
            // Classify by key name heuristics (mirrors context-budget-profiler).
            if k.contains("measure") || k.contains("metric") || k.contains("kpi") {
                measures += total_len;
            } else if k.contains("dimension") || k.contains("attribute") || k.contains("level") {
                dimensions += total_len;
            } else if k.contains("calc") || k.contains("formula") || k.contains("expression")
                || k.contains("computed")
            {
                calcs += total_len;
            } else if k.contains("hierarchy") || k.contains("hier") {
                hierarchies += total_len;
            } else if k.contains("sql") || k.contains("dax") || k.contains("mdx")
                || k.contains("query")
            {
                raw_expressions += total_len;
            } else if k.contains("description")
                || k.contains("summary")
                || k.contains("label")
                || k.contains("caption")
                || k.contains("display")
            {
                summaries += total_len;
            } else {
                scaffolding += total_len;
            }
        }

        Ok(Self {
            measures: tokens_from_chars(measures, chars_per_token)?,
            dimensions: tokens_from_chars(dimensions, chars_per_token)?,
            calcs: tokens_from_chars(calcs, chars_per_token)?,
            hierarchies: tokens_from_chars(hierarchies, chars_per_token)?,
            raw_expressions: tokens_from_chars(raw_expressions, chars_per_token)?,
            summaries: tokens_from_chars(summaries, chars_per_token)?,
            scaffolding: tokens_from_chars(scaffolding, chars_per_token)?,
        })
    }
}

// ── Session footprint ─────────────────────────────────────────────────────────

/// The top-level output of one meter run: token counts per class, per turn,
/// and optional catalog section detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFootprint {
    /// Model name from the first `describe_model` response, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Number of scripted turns executed.
    pub turns: u32,
    /// Characters-per-token divisor used.
    pub chars_per_token: u32,
    /// Total token estimate across all classes.
    pub total_tokens: u64,
    /// Token counts per context class.
    pub classes: ClassTokens,
    /// Optional catalog section breakdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_sections: Option<CatalogSections>,
    /// Per-turn breakdown.
    pub per_turn: Vec<TurnFootprint>,
}

// ── Frame classification ──────────────────────────────────────────────────────

/// A single classified JSON-RPC frame.
#[derive(Debug)]
pub struct ClassifiedFrame {
    /// The raw JSON string of the frame.
    pub raw: String,
    /// The primary class assigned to this frame.
    pub class: TokenClass,
    /// Char count of the `rows` portion (only non-zero for query responses).
    pub rows_chars: usize,
    /// Char count of the non-rows portion (only meaningful for query responses).
    pub envelope_chars: usize,
}

/// Classify a JSON-RPC frame arriving from `mqo-mcp-server`.
///
/// The `op` argument indicates which tool produced the frame:
/// - `"system"` → `SystemPrompt`
/// - `"describe_model"` → `CatalogDescribeModel`
/// - `"query_multidimensional"` → split rows vs. envelope
/// - anything else → `Dialogue`
///
/// For tool-call *request* frames, pass `op = "request"`.
#[must_use]
pub fn classify_frame(raw: &str, op: &str) -> ClassifiedFrame {
    match op {
        "system" => ClassifiedFrame {
            raw: raw.to_owned(),
            class: TokenClass::SystemPrompt,
            rows_chars: 0,
            envelope_chars: raw.len(),
        },
        "describe_model" => ClassifiedFrame {
            raw: raw.to_owned(),
            class: TokenClass::CatalogDescribeModel,
            rows_chars: 0,
            envelope_chars: raw.len(),
        },
        "request" => ClassifiedFrame {
            raw: raw.to_owned(),
            class: TokenClass::ToolCall,
            rows_chars: 0,
            envelope_chars: raw.len(),
        },
        "query_multidimensional" => {
            // Extract the `rows` array and measure its serialized length.
            let (rows_chars, envelope_chars) = split_query_response(raw);
            ClassifiedFrame {
                raw: raw.to_owned(),
                class: TokenClass::ToolCall, // envelope part
                rows_chars,
                envelope_chars,
            }
        }
        _ => ClassifiedFrame {
            raw: raw.to_owned(),
            class: TokenClass::Dialogue,
            rows_chars: 0,
            envelope_chars: raw.len(),
        },
    }
}

/// Split a `query_multidimensional` response JSON string into
/// `(rows_chars, envelope_chars)`.
///
/// If the JSON can be parsed and contains a `rows` array (possibly nested
/// inside `result.content[].text` as another JSON string), extract and measure
/// the rows; the remainder is the envelope.
pub fn split_query_response(raw: &str) -> (usize, usize) {
    // Try to parse as JSON.
    let Ok(v): Result<Value, _> = serde_json::from_str(raw) else {
        return (0, raw.len());
    };

    // mqo-mcp-server wraps results in MCP content:
    //   { "result": { "content": [{ "type": "text", "text": "<json-str>" }] } }
    // Try to extract the inner text payload.
    let inner_text = v
        .pointer("/result/content/0/text")
        .and_then(Value::as_str);

    let inner_val: Option<Value> = inner_text.and_then(|t| serde_json::from_str(t).ok());

    // Extract rows from either the inner payload or the top-level value.
    let rows_val = inner_val
        .as_ref()
        .and_then(|iv| iv.get("rows"))
        .or_else(|| v.get("rows"))
        .or_else(|| v.pointer("/result/rows"));

    let rows_chars = rows_val.map_or(0, |r| r.to_string().len());

    let envelope_chars = raw.len().saturating_sub(rows_chars);
    (rows_chars, envelope_chars)
}

// ── Accumulator ───────────────────────────────────────────────────────────────

/// Accumulates classified frames into a `SessionFootprint`.
pub struct FootprintAccumulator {
    chars_per_token: u32,
    model: Option<String>,
    turns: u32,
    classes: ClassTokens,
    catalog_sections: Option<CatalogSections>,
    per_turn: Vec<TurnFootprint>,
    /// Running sum of per-frame token costs (the `total_tokens` invariant anchor).
    total_tokens: u64,
}

impl FootprintAccumulator {
    /// Create a new accumulator.
    ///
    /// # Errors
    /// Returns an error if `chars_per_token` is 0.
    pub fn new(chars_per_token: u32) -> Result<Self, MeterError> {
        if chars_per_token == 0 {
            return Err(MeterError::ZeroCharsPerToken);
        }
        Ok(Self {
            chars_per_token,
            model: None,
            turns: 0,
            classes: ClassTokens::default(),
            catalog_sections: None,
            per_turn: Vec::new(),
            total_tokens: 0,
        })
    }

    /// Set the model name from the first `describe_model` response.
    pub fn set_model(&mut self, name: impl Into<String>) {
        if self.model.is_none() {
            self.model = Some(name.into());
        }
    }

    /// Push a classified frame into the accumulator.
    ///
    /// Token attribution uses `ceil(raw.len() / chars_per_token)` as the frame
    /// budget, then distributes it across classes.  For `query_multidimensional`
    /// frames the frame budget is split as:
    ///   `rows_tokens = ceil(rows_chars / cpt)`
    ///   `envelope_tokens = frame_tokens - rows_tokens`
    /// This guarantees `rows_tokens + envelope_tokens = frame_tokens` exactly,
    /// so `sum(classes) = sum(per-frame tokens) = total_tokens` with zero drift.
    ///
    /// # Errors
    /// Returns an error if `chars_per_token` is 0 (can't happen post-construction,
    /// but kept for forward compatibility).
    pub fn push(&mut self, frame: &ClassifiedFrame, op: &str) -> Result<(), MeterError> {
        self.turns += 1;

        // Compute the frame-level token budget: ceil(raw.len() / cpt).
        let frame_tokens = tokens_from_chars(frame.raw.len(), self.chars_per_token)?;
        self.total_tokens += frame_tokens;

        match frame.class {
            TokenClass::SystemPrompt => {
                self.classes.add(TokenClass::SystemPrompt, frame_tokens);
            }
            TokenClass::CatalogDescribeModel => {
                self.classes.add(TokenClass::CatalogDescribeModel, frame_tokens);
            }
            TokenClass::ToolCall => {
                // For query responses: split rows vs. envelope within the frame budget.
                if frame.rows_chars > 0 {
                    let rows_t = tokens_from_chars(frame.rows_chars, self.chars_per_token)?;
                    // Clamp to avoid underflow if rows_t somehow exceeds frame_tokens.
                    let env_t = frame_tokens.saturating_sub(rows_t);
                    self.classes.add(TokenClass::ToolResultRows, rows_t);
                    self.classes.add(TokenClass::ToolCall, env_t);
                } else {
                    self.classes.add(TokenClass::ToolCall, frame_tokens);
                }
            }
            TokenClass::ToolResultRows => {
                self.classes.add(TokenClass::ToolResultRows, frame_tokens);
            }
            TokenClass::Dialogue => {
                self.classes.add(TokenClass::Dialogue, frame_tokens);
            }
        }

        // Every turn contributes exactly frame_tokens regardless of class.
        self.per_turn.push(TurnFootprint {
            turn: self.turns,
            op: op.to_owned(),
            tokens: frame_tokens,
        });

        Ok(())
    }

    /// Attach catalog section detail for the `catalog_describe_model` class.
    pub fn set_catalog_sections(&mut self, sections: CatalogSections) {
        self.catalog_sections = Some(sections);
    }

    /// Finalize and produce a `SessionFootprint`.
    ///
    /// `total_tokens` is the running sum of per-frame `ceil(raw/cpt)` values,
    /// which equals `sum(classes)` exactly by construction.
    ///
    /// # Errors
    /// This method is infallible after successful construction, but returns
    /// `Result` for future compatibility.
    pub fn finalize(self) -> Result<SessionFootprint, MeterError> {
        Ok(SessionFootprint {
            model: self.model,
            turns: self.turns,
            chars_per_token: self.chars_per_token,
            total_tokens: self.total_tokens,
            classes: self.classes,
            catalog_sections: self.catalog_sections,
            per_turn: self.per_turn,
        })
    }
}

// ── Server command security check ─────────────────────────────────────────────

/// Validate that a server command string does not contain a `--pg-pass`
/// literal (as opposed to `--pg-pass-env`).
///
/// # Errors
/// Returns `MeterError::LiteralPgPass` if the command contains `--pg-pass`
/// without the `-env` suffix.
pub fn check_no_literal_pg_pass(server_cmd: &str) -> Result<(), MeterError> {
    // Split on whitespace and check each token.
    // Allow `--pg-pass-env` but reject `--pg-pass` as a standalone flag.
    for token in server_cmd.split_whitespace() {
        // token is exactly "--pg-pass" or starts with "--pg-pass=" (with value)
        // but NOT "--pg-pass-env"
        if token == "--pg-pass" || token.starts_with("--pg-pass=") {
            return Err(MeterError::LiteralPgPass);
        }
    }
    Ok(())
}

// ── Session script ────────────────────────────────────────────────────────────

/// One scripted session turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTurn {
    /// Operation name: `"describe_model"` or `"query_multidimensional"`.
    pub op: String,
    /// Model name for `describe_model` turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// MQO payload for `query_multidimensional` turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mqo: Option<Value>,
}

// ── Fixture-based offline processing ─────────────────────────────────────────

/// A recorded session frame (for offline replay).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFrame {
    /// Operation name.
    pub op: String,
    /// Raw JSON-RPC payload.
    pub payload: String,
}

/// Process a list of recorded `SessionFrame`s and return a `SessionFootprint`.
///
/// This is the offline/fixture path used by acceptance tests.
///
/// # Errors
/// Returns an error if `chars_per_token` is 0 or if a frame cannot be
/// attributed.
pub fn process_frames(
    frames: &[SessionFrame],
    chars_per_token: u32,
    with_section_detail: bool,
) -> Result<SessionFootprint, MeterError> {
    let mut acc = FootprintAccumulator::new(chars_per_token)?;

    for frame in frames {
        let classified = classify_frame(&frame.payload, &frame.op);

        // For catalog frames, optionally extract section detail.
        if frame.op == "describe_model" && with_section_detail {
            // Try to parse the payload as a describe_model response and extract
            // the model name + section detail.
            if let Ok(v) = serde_json::from_str::<Value>(&frame.payload) {
                // Extract model name.
                if let Some(name) = v
                    .pointer("/result/content/0/text")
                    .and_then(Value::as_str)
                    .and_then(|t| serde_json::from_str::<Value>(t).ok())
                    .and_then(|iv| iv.get("model_name").and_then(Value::as_str).map(String::from))
                    .or_else(|| v.get("model_name").and_then(Value::as_str).map(String::from))
                {
                    acc.set_model(name);
                }

                // Build section detail from the parsed value.
                let inner: Value = v
                    .pointer("/result/content/0/text")
                    .and_then(Value::as_str)
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or_else(|| v.clone());

                let sections = CatalogSections::from_value(&inner, chars_per_token)?;
                acc.set_catalog_sections(sections);
            }
        }

        acc.push(&classified, &frame.op)?;
    }

    acc.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_from_chars_basic() {
        // 12 chars / 4 = 3 tokens exactly.
        let t12 = tokens_from_chars(12, 4);
        assert!(t12.is_ok());
        assert_eq!(t12.ok(), Some(3));
        // 13 chars / 4 = ceil(13/4) = 4 tokens.
        let t13 = tokens_from_chars(13, 4);
        assert!(t13.is_ok());
        assert_eq!(t13.ok(), Some(4));
    }

    #[test]
    fn tokens_from_chars_zero_divisor() {
        assert!(tokens_from_chars(10, 0).is_err());
    }

    #[test]
    fn check_no_literal_pg_pass_ok() {
        assert!(check_no_literal_pg_pass("mqo-mcp-server --pg-pass-env ATSCALE_PG_PASS").is_ok());
    }

    #[test]
    fn check_no_literal_pg_pass_rejects_literal() {
        assert!(check_no_literal_pg_pass("mqo-mcp-server --pg-pass secret123").is_err());
        assert!(check_no_literal_pg_pass("mqo-mcp-server --pg-pass=secret123").is_err());
    }

    #[test]
    fn split_query_response_no_rows() {
        let raw = r#"{"id":1,"result":{"content":[{"type":"text","text":"{}"}]}}"#;
        let (rows, envelope) = split_query_response(raw);
        assert_eq!(rows, 0);
        assert_eq!(envelope, raw.len());
    }
}
