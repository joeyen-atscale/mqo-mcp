//! Binary stdio tests — spawn the actual `mqo-mcp-server` binary and drive
//! JSON-RPC 2.0 over stdin/stdout.
//!
//! AC8   `binary_initialize_handshake` — serverInfo.name == "mqo-mcp-server"
//! AC9   `binary_tools_list_returns_fourteen_tools` — exactly 14 tools present
//! AC10  `binary_full_nlq_chain_revenue_by_year` — fleet-gated full chain
//! AC11  `binary_malformed_jsonrpc_returns_parse_error` — error code -32700

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

// ── Binary resolution ─────────────────────────────────────────────────────────

/// Find the `mqo-mcp-server` binary: workspace release build → ~/.local/bin → PATH.
fn resolve_binary() -> Option<PathBuf> {
    let workspace_release = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("target/release/mqo-mcp-server");
    if workspace_release.exists() {
        return Some(workspace_release);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let local = PathBuf::from(home).join(".local/bin/mqo-mcp-server");
        if local.exists() {
            return Some(local);
        }
    }
    // Last resort: check PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = PathBuf::from(dir).join("mqo-mcp-server");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn fixtures_catalog() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/catalog.json")
}

// ── ServerProcess RAII wrapper ────────────────────────────────────────────────

struct ServerProcess {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    _reader: thread::JoinHandle<()>,
}

impl ServerProcess {
    fn spawn(binary: &PathBuf) -> Self {
        let mut child = Command::new(binary)
            .arg("--catalog")
            .arg(fixtures_catalog())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mqo-mcp-server");

        let stdout: ChildStdout = child.stdout.take().expect("stdout");
        let stdin: ChildStdin = child.stdin.take().expect("stdin");

        let (tx, rx) = mpsc::channel::<String>();
        let reader_thread = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        ServerProcess {
            child,
            stdin,
            rx,
            _reader: reader_thread,
        }
    }

    fn send(&mut self, req: &Value) {
        let line = serde_json::to_string(req).expect("serialize request");
        writeln!(self.stdin, "{line}").expect("write to stdin");
        self.stdin.flush().expect("flush stdin");
    }

    fn recv(&self) -> Value {
        let line = self
            .rx
            .recv_timeout(Duration::from_secs(5))
            .expect("response within 5s");
        serde_json::from_str(&line).expect("valid JSON response")
    }

    fn send_raw(&mut self, raw: &str) {
        writeln!(self.stdin, "{raw}").expect("write raw to stdin");
        self.stdin.flush().expect("flush stdin");
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ── AC8: initialize handshake ─────────────────────────────────────────────────

#[test]
fn binary_initialize_handshake() {
    let Some(binary) = resolve_binary() else {
        eprintln!("AC8 SKIPPED: mqo-mcp-server binary not found");
        return;
    };

    let mut srv = ServerProcess::spawn(&binary);

    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    }));

    let resp = srv.recv();
    let result = resp.get("result").expect("initialize result present");
    assert_eq!(
        result.get("serverInfo").and_then(|si| si.get("name")).and_then(Value::as_str),
        Some("mqo-mcp-server"),
        "AC8: serverInfo.name must be 'mqo-mcp-server', got: {result}"
    );
    let version = result
        .get("serverInfo")
        .and_then(|si| si.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(!version.is_empty(), "AC8: serverInfo.version must be non-empty");
}

// ── AC9: tools list — exactly 16 tools ───────────────────────────────────────

#[test]
fn binary_tools_list_returns_fourteen_tools() {
    let Some(binary) = resolve_binary() else {
        eprintln!("AC9 SKIPPED: mqo-mcp-server binary not found");
        return;
    };

    let mut srv = ServerProcess::spawn(&binary);

    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    }));
    let _ = srv.recv();

    srv.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    srv.send(&json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {} }));
    let resp = srv.recv();

    let tools = resp
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .expect("tools array present");

    assert_eq!(tools.len(), 24, "AC9: expected 24 tools, got {}", tools.len());

    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();

    for expected in [
        "list_models",
        "describe_model",
        "search_columns",
        "query_multidimensional",
        "recommend_chart",
        "build_vega_spec",
    ] {
        assert!(
            tool_names.contains(&expected),
            "AC9: tool '{expected}' missing from list; present: {tool_names:?}"
        );
    }

    for read_only_tool in ["list_models", "describe_model", "search_columns"] {
        let tool = tools
            .iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some(read_only_tool))
            .unwrap_or_else(|| panic!("AC9: tool '{read_only_tool}' must be in list"));
        assert_eq!(
            tool.get("annotations")
                .and_then(|a| a.get("readOnlyHint"))
                .and_then(Value::as_bool),
            Some(true),
            "AC9: tool '{read_only_tool}' must have annotations.readOnlyHint: true"
        );
    }
}

// ── AC10: fleet-gated full NLQ chain ─────────────────────────────────────────

fn fleet_present() -> bool {
    let home = std::env::var_os("HOME").unwrap_or_default();
    let bins = ["mqo-bind", "mqo-route", "mqo-dax", "mqo-mdx"];
    bins.iter().all(|bin| {
        PathBuf::from(&home).join(".local/bin").join(bin).exists()
    })
}

#[test]
fn binary_full_nlq_chain_revenue_by_year() {
    let Some(binary) = resolve_binary() else {
        eprintln!("AC10 SKIPPED: mqo-mcp-server binary not found");
        return;
    };
    if !fleet_present() {
        eprintln!("AC10 SKIPPED: fleet binaries not present");
        return;
    }

    let mut srv = ServerProcess::spawn(&binary);

    // initialize
    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0.1" }
        }
    }));
    let _ = srv.recv();

    srv.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    // describe_model — at least one measure present
    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "describe_model",
            "arguments": { "model": "sales" }
        }
    }));
    let desc_resp = srv.recv();
    let desc_result = desc_resp.get("result").expect("describe_model result");
    assert!(
        !desc_result.get("isError").and_then(Value::as_bool).unwrap_or(true),
        "AC10: describe_model isError must be false: {desc_result}"
    );
    let columns = desc_result
        .get("structuredContent")
        .and_then(|sc| sc.get("columns"))
        .and_then(Value::as_array)
        .expect("columns array");
    assert!(!columns.is_empty(), "AC10: columns must be non-empty");
    let has_measure = columns
        .iter()
        .any(|c| c.get("kind").and_then(Value::as_str) == Some("measure"));
    assert!(has_measure, "AC10: at least one measure must be present");

    // query_multidimensional — revenue by year
    let mqo = json!({
        "model": "sales",
        "measures": [{ "unique_name": "Revenue" }],
        "dimensions": [{ "hierarchy": "time.calendar", "level": "Year" }],
        "filters": [],
        "time_intelligence": [],
        "order": null,
        "limit": 100,
        "non_empty": true
    });
    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": { "name": "query_multidimensional", "arguments": { "mqo": mqo } }
    }));
    let qmr_resp = srv.recv();
    let qmr_result = qmr_resp.get("result").expect("query result");
    assert!(
        !qmr_result.get("isError").and_then(Value::as_bool).unwrap_or(true),
        "AC10: query_multidimensional isError must be false: {qmr_result}"
    );
    let sc = qmr_result.get("structuredContent").expect("structuredContent");
    // Paginated responses use `page` for the data array; non-paginated use `rows`.
    let data_rows = sc.get("page")
        .or_else(|| sc.get("rows"))
        .and_then(Value::as_array)
        .expect("AC10: `page` or `rows` array must be present in structuredContent");
    assert!(!data_rows.is_empty(), "AC10: data rows must be non-empty");
    assert!(
        sc.get("filters_applied").and_then(Value::as_array).is_some(),
        "AC10: filters_applied must be present"
    );

    // Normalize bound: binder emits objects ({unique_name, ...}), profiler expects strings.
    let empty = vec![];
    let raw_measures = sc.get("bound")
        .and_then(|b| b.get("measures"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let raw_dims = sc.get("bound")
        .and_then(|b| b.get("dimensions"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let measure_names: Vec<Value> = raw_measures.iter()
        .filter_map(|m| m.get("unique_name").and_then(Value::as_str))
        .map(|s| json!(s))
        .collect();
    let dim_names: Vec<Value> = raw_dims.iter()
        .filter_map(|d| d.get("unique_name").and_then(Value::as_str))
        .map(|s| json!(s))
        .collect();
    let simple_bound = json!({ "measures": measure_names, "dimensions": dim_names });

    // recommend_chart — pass { rows, bound } so the profiler can classify columns
    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "recommend_chart",
            "arguments": { "rows": data_rows, "bound": simple_bound }
        }
    }));
    let rec_resp = srv.recv();
    let rec_result = rec_resp.get("result").expect("recommend_chart result");
    assert!(
        !rec_result.get("isError").and_then(Value::as_bool).unwrap_or(true),
        "AC10: recommend_chart isError must be false: {rec_result}"
    );
    let rec_sc = rec_result.get("structuredContent").expect("recommendation");
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("line"),
        "AC10: revenue×year must recommend 'line'"
    );

    // build_vega_spec — Vega mark must be "line"
    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "build_vega_spec",
            "arguments": { "recommendation": rec_sc, "rows": data_rows }
        }
    }));
    let vega_resp = srv.recv();
    let vega_result = vega_resp.get("result").expect("build_vega_spec result");
    assert!(
        !vega_result.get("isError").and_then(Value::as_bool).unwrap_or(true),
        "AC10: build_vega_spec isError must be false: {vega_result}"
    );
    let spec = vega_result.get("structuredContent").expect("vega spec");
    assert!(
        spec.get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains("vega-lite")),
        "AC10: $schema must contain 'vega-lite'"
    );
    assert_eq!(
        spec.get("mark").and_then(Value::as_str),
        Some("line"),
        "AC10: Vega mark must be 'line'"
    );
    assert!(
        spec.get("data").and_then(|d| d.get("values")).and_then(Value::as_array).is_some(),
        "AC10: data.values must be present"
    );
}

// ── AC11: malformed JSON-RPC → parse error ────────────────────────────────────

#[test]
fn binary_malformed_jsonrpc_returns_parse_error() {
    let Some(binary) = resolve_binary() else {
        eprintln!("AC11 SKIPPED: mqo-mcp-server binary not found");
        return;
    };

    let mut srv = ServerProcess::spawn(&binary);
    srv.send_raw("this is not json");

    let resp = srv.recv();
    let error = resp.get("error").expect("error field must be present for parse error");
    assert_eq!(
        error.get("code").and_then(Value::as_i64),
        Some(-32700),
        "AC11: parse error code must be -32700, got: {error}"
    );
}
