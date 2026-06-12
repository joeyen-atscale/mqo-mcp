//! SQL parser: extracts the AtScale projection shape from a flat SELECT statement.
//!
//! The expected SQL shape is:
//! ```sql
//! SELECT "<measure_unique_name>"[, ...]
//! FROM "atscale_catalogs"."<catalog_id>"."<model_id>"
//! [WHERE <col> = <val> [AND ...]]
//! [GROUP BY "<dimension_level_unique_name>"[, ...]]
//! [LIMIT <integer>]
//! ```

use sqlparser::ast::{
    BinaryOperator, Expr, FunctionArg, FunctionArgExpr, GroupByExpr, Query, Select, SelectItem,
    SetExpr, Statement, TableFactor, Value as SqlValue,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser as SqlParser;

use crate::error::ParseError;

/// The parsed structural representation of an AtScale SQL projection.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedProjection {
    /// Raw unique_names from SELECT (double-quotes stripped).
    pub measures: Vec<String>,
    /// The catalog_id from FROM part[1] (e.g. `"tpcds_Snowflake"` → `tpcds_Snowflake`).
    pub catalog_id: String,
    /// The model_id from FROM part[2].
    pub model_id: String,
    /// Raw unique_names from GROUP BY.
    pub dimensions: Vec<String>,
    /// Predicates from WHERE.
    pub filters: Vec<FilterClause>,
    /// Optional LIMIT.
    pub limit: Option<u64>,
}

/// A single WHERE predicate.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterClause {
    pub col: String,
    pub op: String,
    pub value: serde_json::Value,
}

/// Parse an AtScale SQL projection string into a `ParsedProjection`.
///
/// # Errors
///
/// Returns `ParseError` when the SQL cannot be parsed or does not match the
/// expected AtScale shape.
pub fn parse_sql(sql: &str) -> Result<ParsedProjection, ParseError> {
    let dialect = GenericDialect {};
    let stmts = SqlParser::parse_sql(&dialect, sql)
        .map_err(|e| ParseError::SqlSyntax(format!("{e}")))?;

    if stmts.len() != 1 {
        return Err(ParseError::SqlSyntax(format!(
            "expected exactly 1 statement, got {}",
            stmts.len()
        )));
    }

    let stmt = stmts.into_iter().next().unwrap();
    let query = match stmt {
        Statement::Query(q) => q,
        other => {
            return Err(ParseError::SqlSyntax(format!(
                "expected SELECT query, got {other}"
            )))
        }
    };

    extract_projection(&query)
}

fn extract_projection(query: &Query) -> Result<ParsedProjection, ParseError> {
    let select = match query.body.as_ref() {
        SetExpr::Select(s) => s,
        other => {
            return Err(ParseError::SqlSyntax(format!(
                "expected SELECT body, got {other}"
            )))
        }
    };

    let limit = extract_limit(query)?;
    let (catalog_id, model_id) = extract_from(select)?;
    let measures = extract_select_columns(select)?;
    let dimensions = extract_group_by(select)?;
    let filters = extract_where(select)?;

    Ok(ParsedProjection {
        measures,
        catalog_id,
        model_id,
        dimensions,
        filters,
        limit,
    })
}

// ── SELECT ─────────────────────────────────────────────────────────────────

fn extract_select_columns(select: &Select) -> Result<Vec<String>, ParseError> {
    let mut cols = Vec::new();
    for item in &select.projection {
        match item {
            SelectItem::UnnamedExpr(expr) => {
                cols.push(expr_to_unique_name(expr)?);
            }
            SelectItem::ExprWithAlias { expr, .. } => {
                // Accept aliased columns too (e.g. SUM("x") AS "x")
                cols.push(expr_to_unique_name(expr)?);
            }
            SelectItem::Wildcard(_) => {
                return Err(ParseError::SqlSyntax(
                    "wildcard SELECT (*) not supported".to_string(),
                ))
            }
            other => {
                return Err(ParseError::SqlSyntax(format!(
                    "unsupported SELECT item: {other}"
                )))
            }
        }
    }
    if cols.is_empty() {
        return Err(ParseError::SqlSyntax("SELECT has no columns".to_string()));
    }
    Ok(cols)
}

fn expr_to_unique_name(expr: &Expr) -> Result<String, ParseError> {
    match expr {
        // Bare quoted identifier: "Total Store Sales"
        Expr::Identifier(ident) => Ok(ident.value.clone()),
        // Compound identifier: "schema"."column" — join with '.'
        Expr::CompoundIdentifier(parts) => {
            Ok(parts.iter().map(|i| i.value.clone()).collect::<Vec<_>>().join("."))
        }
        // SUM("label") — unwrap the inner ident (sqlparser 0.44: Function.args is Vec<FunctionArg>)
        Expr::Function(f) => {
            if f.args.len() == 1 {
                if let FunctionArg::Unnamed(ref fae) = f.args[0] {
                    if let FunctionArgExpr::Expr(ref inner) = fae {
                        return expr_to_unique_name(inner);
                    }
                }
            }
            Err(ParseError::SqlSyntax(format!(
                "unsupported function expression in SELECT: {expr}"
            )))
        }
        other => Err(ParseError::SqlSyntax(format!(
            "unsupported expression in SELECT/GROUP BY: {other}"
        ))),
    }
}

// ── FROM ──────────────────────────────────────────────────────────────────

fn extract_from(select: &Select) -> Result<(String, String), ParseError> {
    if select.from.len() != 1 {
        return Err(ParseError::SqlSyntax(format!(
            "expected exactly 1 FROM table, got {}",
            select.from.len()
        )));
    }
    let table_factor = &select.from[0].relation;
    match table_factor {
        TableFactor::Table { name, .. } => {
            // name.0 is a Vec<Ident>; we expect [atscale_catalogs, catalog_id, model_id]
            let parts: Vec<String> = name.0.iter().map(|i| i.value.clone()).collect();
            if parts.len() < 2 {
                return Err(ParseError::SqlSyntax(format!(
                    "FROM clause must have at least 2 parts (catalog.model), got: {}",
                    parts.join(".")
                )));
            }
            // Support both 2-part (catalog.model) and 3-part (atscale_catalogs.catalog.model)
            if parts.len() >= 3 {
                // [atscale_catalogs, catalog_id, model_id] → parts[1], parts[2]
                Ok((parts[1].clone(), parts[2].clone()))
            } else {
                // 2-part: [catalog_id, model_id]
                Ok((parts[0].clone(), parts[1].clone()))
            }
        }
        other => Err(ParseError::SqlSyntax(format!(
            "expected a table reference in FROM, got: {other}"
        ))),
    }
}

// ── WHERE ─────────────────────────────────────────────────────────────────

fn extract_where(select: &Select) -> Result<Vec<FilterClause>, ParseError> {
    match &select.selection {
        None => Ok(vec![]),
        Some(expr) => collect_predicates(expr),
    }
}

fn collect_predicates(expr: &Expr) -> Result<Vec<FilterClause>, ParseError> {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let mut left_preds = collect_predicates(left)?;
            let right_preds = collect_predicates(right)?;
            left_preds.extend(right_preds);
            Ok(left_preds)
        }
        Expr::BinaryOp { left, op, right } => {
            let col = expr_to_col_name(left)?;
            let op_str = binary_op_str(op)?;
            let val = expr_to_json_value(right)?;
            Ok(vec![FilterClause {
                col,
                op: op_str,
                value: val,
            }])
        }
        Expr::InList { expr, list, negated } => {
            let col = expr_to_col_name(expr)?;
            let op_str = if *negated { "NOT IN" } else { "IN" }.to_string();
            let vals: Result<Vec<serde_json::Value>, ParseError> =
                list.iter().map(expr_to_json_value).collect();
            Ok(vec![FilterClause {
                col,
                op: op_str,
                value: serde_json::Value::Array(vals?),
            }])
        }
        other => Err(ParseError::SqlSyntax(format!(
            "unsupported WHERE expression: {other}"
        ))),
    }
}

fn expr_to_col_name(expr: &Expr) -> Result<String, ParseError> {
    match expr {
        Expr::Identifier(ident) => Ok(ident.value.clone()),
        Expr::CompoundIdentifier(parts) => {
            Ok(parts.iter().map(|i| i.value.clone()).collect::<Vec<_>>().join("."))
        }
        other => Err(ParseError::SqlSyntax(format!(
            "expected column name in predicate, got: {other}"
        ))),
    }
}

fn binary_op_str(op: &BinaryOperator) -> Result<String, ParseError> {
    Ok(match op {
        BinaryOperator::Eq => "=",
        BinaryOperator::NotEq => "!=",
        BinaryOperator::Lt => "<",
        BinaryOperator::Gt => ">",
        BinaryOperator::LtEq => "<=",
        BinaryOperator::GtEq => ">=",
        other => {
            return Err(ParseError::SqlSyntax(format!(
                "unsupported binary operator in WHERE: {other}"
            )))
        }
    }
    .to_string())
}

fn expr_to_json_value(expr: &Expr) -> Result<serde_json::Value, ParseError> {
    match expr {
        Expr::Value(v) => sql_value_to_json(v),
        Expr::UnaryOp {
            op: sqlparser::ast::UnaryOperator::Minus,
            expr,
        } => {
            // Handle negative numbers: -42 or -3.14
            match expr.as_ref() {
                Expr::Value(v) => {
                    let positive = sql_value_to_json(v)?;
                    match positive {
                        serde_json::Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                Ok(serde_json::Value::Number(serde_json::Number::from(-i)))
                            } else if let Some(f) = n.as_f64() {
                                Ok(serde_json::json!(-f))
                            } else {
                                Err(ParseError::SqlSyntax(
                                    "cannot negate number".to_string(),
                                ))
                            }
                        }
                        other => Ok(other),
                    }
                }
                other => Err(ParseError::SqlSyntax(format!(
                    "unsupported unary minus operand: {other}"
                ))),
            }
        }
        Expr::Identifier(ident) => Ok(serde_json::Value::String(ident.value.clone())),
        other => Err(ParseError::SqlSyntax(format!(
            "unsupported value expression in WHERE: {other}"
        ))),
    }
}

fn sql_value_to_json(v: &SqlValue) -> Result<serde_json::Value, ParseError> {
    match v {
        SqlValue::Number(s, _) => {
            // Try integer first, then float
            if let Ok(i) = s.parse::<i64>() {
                Ok(serde_json::Value::Number(serde_json::Number::from(i)))
            } else if let Ok(f) = s.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| ParseError::SqlSyntax(format!("non-finite number: {s}")))
            } else {
                Err(ParseError::SqlSyntax(format!("cannot parse number: {s}")))
            }
        }
        SqlValue::SingleQuotedString(s) | SqlValue::DoubleQuotedString(s) => {
            Ok(serde_json::Value::String(s.clone()))
        }
        SqlValue::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        SqlValue::Null => Ok(serde_json::Value::Null),
        other => Err(ParseError::SqlSyntax(format!(
            "unsupported SQL literal: {other}"
        ))),
    }
}

// ── GROUP BY ──────────────────────────────────────────────────────────────

fn extract_group_by(select: &Select) -> Result<Vec<String>, ParseError> {
    match &select.group_by {
        GroupByExpr::Expressions(exprs) => {
            exprs.iter().map(expr_to_unique_name).collect()
        }
        GroupByExpr::All => Err(ParseError::SqlSyntax(
            "GROUP BY ALL is not supported".to_string(),
        )),
    }
}

// ── LIMIT ─────────────────────────────────────────────────────────────────

fn extract_limit(query: &Query) -> Result<Option<u64>, ParseError> {
    match &query.limit {
        None => Ok(None),
        Some(expr) => match expr {
            Expr::Value(SqlValue::Number(s, _)) => s.parse::<u64>().map(Some).map_err(|_| {
                ParseError::SqlSyntax(format!("invalid LIMIT value: {s}"))
            }),
            other => Err(ParseError::SqlSyntax(format!(
                "unsupported LIMIT expression: {other}"
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_select() {
        let sql = r#"SELECT "store_sales.Total Store Sales" FROM "atscale_catalogs"."tpcds"."tpcds_model""#;
        let p = parse_sql(sql).unwrap();
        assert_eq!(p.measures, vec!["store_sales.Total Store Sales"]);
        assert_eq!(p.catalog_id, "tpcds");
        assert_eq!(p.model_id, "tpcds_model");
        assert!(p.dimensions.is_empty());
        assert!(p.filters.is_empty());
        assert!(p.limit.is_none());
    }

    #[test]
    fn parse_with_group_by_and_limit() {
        let sql = r#"
            SELECT "sales.Revenue", "time.calendar.[Year]"
            FROM "atscale_catalogs"."tpcds"."sales_model"
            GROUP BY "time.calendar.[Year]"
            LIMIT 100
        "#;
        let p = parse_sql(sql).unwrap();
        assert!(p.measures.contains(&"sales.Revenue".to_string()));
        assert_eq!(p.dimensions, vec!["time.calendar.[Year]"]);
        assert_eq!(p.limit, Some(100));
    }

    #[test]
    fn parse_where_eq_filter() {
        let sql = r#"
            SELECT "sales.Revenue"
            FROM "atscale_catalogs"."cat"."model"
            WHERE "time.calendar.[Year]" = 2023
        "#;
        let p = parse_sql(sql).unwrap();
        assert_eq!(p.filters.len(), 1);
        assert_eq!(p.filters[0].col, "time.calendar.[Year]");
        assert_eq!(p.filters[0].op, "=");
        assert_eq!(p.filters[0].value, serde_json::json!(2023));
    }

    #[test]
    fn parse_where_and_multiple_filters() {
        let sql = r#"
            SELECT "sales.Revenue"
            FROM "atscale_catalogs"."cat"."model"
            WHERE "time.calendar.[Year]" = 2023 AND "geo.country.[Country]" = 'USA'
        "#;
        let p = parse_sql(sql).unwrap();
        assert_eq!(p.filters.len(), 2);
    }

    #[test]
    fn parse_invalid_sql_returns_error() {
        let result = parse_sql("SELECT FROM WHERE");
        assert!(result.is_err());
    }

    #[test]
    fn parse_two_part_from() {
        let sql = r#"SELECT "sales.Revenue" FROM "cat"."model""#;
        let p = parse_sql(sql).unwrap();
        assert_eq!(p.catalog_id, "cat");
        assert_eq!(p.model_id, "model");
    }
}
