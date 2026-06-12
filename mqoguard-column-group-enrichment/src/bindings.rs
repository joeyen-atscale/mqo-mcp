//! `FactBindings` — the declared mapping from measure/hierarchy `unique_names` to
//! column-group sets.
//!
//! The source of truth for bindings is declared externally (OQ1 — see PRD).
//! This module provides:
//! - The typed `FactBindings` struct and its error type.
//! - `FactBindings::tpcds_defaults()` — pre-built bindings for the TPC-DS
//!   benchmark model as used by the mcp-tuner corpus.
//! - `FactBindings::from_json` — deserialization from a JSON sidecar file.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Errors produced when deserializing or validating `FactBindings`.
#[derive(Debug, Error)]
pub enum FactBindingsError {
    /// The JSON payload could not be parsed.
    #[error("failed to parse fact bindings JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The bindings map is empty (likely a misconfigured input).
    #[error("fact bindings are empty — provide at least one mapping")]
    Empty,
}

/// Maps each catalog entity (by `unique_name` for measures, by `hierarchy` name
/// for dimension levels) to the set of column-group identifiers it belongs to.
///
/// Column-group identifiers are lowercase short names matching TPC-DS fact
/// table names: `store_sales`, `catalog_sales`, `web_sales`, `store_returns`,
/// `catalog_returns`, `web_returns`, `inventory`.
///
/// Conformed dimensions appear under multiple group names (FR2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactBindings {
    /// Measure `unique_name` → column-group set.
    ///
    /// Keys use the fully-qualified `unique_name` format, e.g.
    /// `tpcds_benchmark_model.inventory_quantity_on_hand`.
    #[serde(default)]
    pub measures: BTreeMap<String, BTreeSet<String>>,

    /// Hierarchy name → column-group set (for dimension levels).
    ///
    /// Every level in the hierarchy inherits the same column-group set (FR2).
    /// A hierarchy present in multiple facts (conformed dimension) maps to a
    /// multi-element set.
    #[serde(default)]
    pub hierarchies: BTreeMap<String, BTreeSet<String>>,
}

impl FactBindings {
    /// Deserialize from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns [`FactBindingsError::Json`] if the JSON is malformed.
    /// Returns [`FactBindingsError::Empty`] if both maps are empty after parsing.
    pub fn from_json(json: &str) -> Result<Self, FactBindingsError> {
        let bindings: Self = serde_json::from_str(json)?;
        if bindings.measures.is_empty() && bindings.hierarchies.is_empty() {
            return Err(FactBindingsError::Empty);
        }
        Ok(bindings)
    }

    /// Returns the column-group set for the given measure `unique_name`, or an
    /// empty set if the measure has no binding.
    #[must_use]
    pub fn groups_for_measure(&self, unique_name: &str) -> BTreeSet<String> {
        self.measures
            .get(unique_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the column-group set for the given hierarchy name, or an empty
    /// set if the hierarchy has no binding.
    #[must_use]
    pub fn groups_for_hierarchy(&self, hierarchy: &str) -> BTreeSet<String> {
        self.hierarchies
            .get(hierarchy)
            .cloned()
            .unwrap_or_default()
    }

    /// Pre-built bindings for the TPC-DS benchmark model.
    ///
    /// Derived from the known TPC-DS fact table structure as used by the
    /// mcp-tuner corpus (`tasks/tpcds_failure_modes_100_nonprod.json`).
    ///
    /// Column-group identifiers used:
    /// - `store_sales` — store sales fact
    /// - `catalog_sales` — catalog sales fact
    /// - `web_sales` — web sales fact
    /// - `store_returns` — store returns fact
    /// - `catalog_returns` — catalog returns fact
    /// - `web_returns` — web returns fact
    /// - `inventory` — inventory fact
    ///
    /// Conformed dimensions (e.g. `product_dimension`, `customer_dimension`,
    /// `sold_date_dimensions`) appear under all applicable facts.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn tpcds_defaults() -> Self {
        let mut measures: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut hierarchies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        // Helper macro to insert a measure with given column-groups
        macro_rules! measure {
            ($name:expr, [$($g:expr),+]) => {
                measures.insert(
                    format!("tpcds_benchmark_model.{}", $name),
                    [$($g.to_string()),+].into_iter().collect(),
                );
            };
        }

        // Helper macro to insert a hierarchy with given column-groups
        macro_rules! hierarchy {
            ($name:expr, [$($g:expr),+]) => {
                hierarchies.insert(
                    $name.to_string(),
                    [$($g.to_string()),+].into_iter().collect(),
                );
            };
        }

        // --- Inventory measures ---
        measure!("inventory_quantity_on_hand", ["inventory"]);
        measure!("total_product_count", ["inventory"]);

        // --- Store sales measures ---
        measure!("total_store_sales", ["store_sales"]);
        measure!("store_quantity_sold", ["store_sales"]);
        measure!("store_net_paid_amount", ["store_sales"]);
        measure!("store_net_paid_incl_tax", ["store_sales"]);
        measure!("store_net_profit", ["store_sales"]);
        measure!("store_ext_sales_price", ["store_sales"]);
        measure!("store_ext_list_price", ["store_sales"]);
        measure!("store_ext_wholesale_cost", ["store_sales"]);
        measure!("store_ext_discount_amount", ["store_sales"]);
        measure!("store_ext_sales_tax", ["store_sales"]);
        measure!("store_coupon_amount", ["store_sales"]);
        measure!("store_customer_count", ["store_sales"]);
        measure!("store_sales_row_counter", ["store_sales"]);
        measure!("average_store_sales_sales_price", ["store_sales"]);
        measure!("average_store_sales_list_price", ["store_sales"]);
        measure!("average_store_sales_coupon_amount", ["store_sales"]);
        measure!("average_store_sales_quantity", ["store_sales"]);
        measure!("store_ext_sales_price_by_promotion", ["store_sales"]);
        measure!("store_sales_price_by_promotion", ["store_sales"]);
        measure!("avg_quarterly_store_sales_for_1998_1999", ["store_sales"]);
        measure!("store_revenue_ratio_by_product_class", ["store_sales"]);
        measure!("store_sales_increase", ["store_sales"]);

        // --- Store returns measures ---
        measure!("store_returns_count", ["store_returns"]);
        measure!("average_store_unit_net_profit", ["store_sales"]);

        // --- Catalog sales measures ---
        measure!("catalog_sales", ["catalog_sales"]);
        measure!("catalog_quantity_sold", ["catalog_sales"]);
        measure!("catalog_net_paid_amount", ["catalog_sales"]);
        measure!("catalog_net_paid_inc_tax_amount", ["catalog_sales"]);
        measure!("catalog_net_profit_amount", ["catalog_sales"]);
        measure!("catalog_ext_sales_price", ["catalog_sales"]);
        measure!("catalog_ext_list_price", ["catalog_sales"]);
        measure!("catalog_ext_wholesale_cost", ["catalog_sales"]);
        measure!("catalog_ext_discount_amount", ["catalog_sales"]);
        measure!("catalog_ext_sales_tax", ["catalog_sales"]);
        measure!("catalog_customer_count", ["catalog_sales"]);
        measure!("catalog_sales_row_counter", ["catalog_sales"]);
        measure!("catalog_sales_price", ["catalog_sales"]);
        measure!("catalog_sales_net_paid", ["catalog_sales"]);
        measure!("catalog_sales_average_coupon_amount", ["catalog_sales"]);
        measure!("catalog_sales_average_list_price", ["catalog_sales"]);
        measure!("catalog_sales_average_quantity_sold", ["catalog_sales"]);
        measure!("catalog_sales_average_sales_price", ["catalog_sales"]);
        measure!("average_catalog_unit_net_profit", ["catalog_sales"]);
        measure!("purchased_amount_in_catalog", ["catalog_sales"]);
        measure!("catalog_buyer", ["catalog_sales"]);

        // --- Web sales measures ---
        measure!("web_sales", ["web_sales"]);
        measure!("web_quantity_sold", ["web_sales"]);
        measure!("web_net_paid_amount", ["web_sales"]);
        measure!("web_net_paid_incl_ship", ["web_sales"]);
        measure!("web_net_paid_incl_tax", ["web_sales"]);
        measure!("web_net_paid_incl_tax_and_ship", ["web_sales"]);
        measure!("web_net_profit", ["web_sales"]);
        measure!("web_ext_sales_price", ["web_sales"]);
        measure!("web_ext_list_price", ["web_sales"]);
        measure!("web_ext_wholesale_cost", ["web_sales"]);
        measure!("web_ext_discount_amount", ["web_sales"]);
        measure!("web_ext_sales_tax", ["web_sales"]);
        measure!("web_ext_ship_cost", ["web_sales"]);
        measure!("web_customer_count", ["web_sales"]);
        measure!("web_sales_row_counter", ["web_sales"]);
        measure!("web_sales_net_paid", ["web_sales"]);
        measure!("average_web_unit_net_profit", ["web_sales"]);
        measure!("purchased_amount_on_web", ["web_sales"]);
        measure!("web_sales_increase", ["web_sales"]);

        // --- Cross-fact / derived measures (appear on multiple facts) ---
        // These are calc-group measures that aggregate across store + catalog + web sales
        measure!("average_ext_sales_price", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("average_ext_wholesale_cost", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_ext_discount_amount", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_ext_list_price", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_ext_sales_price", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_ext_sales_tax", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_ext_wholesale_cost", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_net_paid_amount", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_net_paid_incl_tax", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_net_profit", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("total_quantity_sold", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("customer_count", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("avg_quarter_sales_ratio", ["store_sales", "catalog_sales", "web_sales"]);
        measure!("purchased_amount_in_store", ["store_sales"]);
        measure!("catalog_and_web_sales", ["catalog_sales", "web_sales"]);
        measure!("catalog_and_web_sales_net", ["catalog_sales", "web_sales"]);
        measure!("store_and_web_purchased_amount", ["store_sales", "web_sales"]);
        measure!("web_and_catalog_sales", ["web_sales", "catalog_sales"]);
        measure!("web_and_catalog_sales_price_growth", ["web_sales", "catalog_sales"]);
        measure!("store_sales_by_promotion_ratio", ["store_sales"]);

        // --- Inventory hierarchies ---
        hierarchy!("inventory_date_dimensions", ["inventory"]);
        hierarchy!("inventory_date_week_hierarchy", ["inventory"]);
        hierarchy!("fulfilling_warehouse", ["inventory"]);

        // --- Store-sales-only hierarchies ---
        hierarchy!("store_dimension", ["store_sales", "store_returns"]);
        hierarchy!("sold_date_dimensions", [
            "store_sales", "catalog_sales", "web_sales"
        ]);
        hierarchy!("sold_date_week_hierarchy", [
            "store_sales", "catalog_sales", "web_sales"
        ]);
        hierarchy!("sold_time_dimension", [
            "store_sales", "catalog_sales", "web_sales"
        ]);
        hierarchy!("store_sales_ticket_number", ["store_sales"]);
        hierarchy!("store_item_product_dimension", ["store_sales", "store_returns"]);

        // --- Return-specific hierarchies ---
        hierarchy!("return_date_dimensions", [
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("return_date_week_hierarchy", [
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("return_time_dimension", [
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("return_customer_address", [
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("returns_time_tier", [
            "store_returns", "catalog_returns", "web_returns"
        ]);

        // --- Ship/fulfillment hierarchies (web/catalog) ---
        hierarchy!("ship_date_dimensions", ["web_sales", "catalog_sales"]);
        hierarchy!("ship_date_week_hierarchy", ["web_sales", "catalog_sales"]);
        hierarchy!("ship_customer_address", ["web_sales", "catalog_sales"]);
        hierarchy!("ship_mode", ["web_sales", "catalog_sales"]);
        hierarchy!("web_site", ["web_sales"]);

        // --- Conformed dimensions (multiple facts) ---
        hierarchy!("product_dimension", [
            "store_sales", "catalog_sales", "web_sales",
            "store_returns", "catalog_returns", "web_returns",
            "inventory"
        ]);
        hierarchy!("promotion_product_item_product_dimension", [
            "store_sales", "catalog_sales", "web_sales"
        ]);
        hierarchy!("customer_dimension", [
            "store_sales", "catalog_sales", "web_sales",
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("customer_address", [
            "store_sales", "catalog_sales", "web_sales",
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("sold_customer_address", [
            "store_sales", "catalog_sales", "web_sales"
        ]);
        hierarchy!("customer_demographics", [
            "store_sales", "catalog_sales", "web_sales",
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("household_demographics", [
            "store_sales", "catalog_sales", "web_sales",
            "store_returns", "catalog_returns", "web_returns"
        ]);
        hierarchy!("income_band", [
            "store_sales", "catalog_sales", "web_sales",
            "store_returns", "catalog_returns", "web_returns"
        ]);

        // --- Price-tier and analytical hierarchies ---
        // These are computed from sales facts; bind to all applicable sales facts.
        hierarchy!("sales_price_tier", [
            "store_sales", "catalog_sales", "web_sales"
        ]);
        hierarchy!("catalog_sales_price_tier", ["catalog_sales"]);
        hierarchy!("catalog_preferred", ["catalog_sales"]);
        hierarchy!("net_profit_tier", [
            "store_sales", "catalog_sales", "web_sales"
        ]);

        // --- Promotions hierarchy ---
        // Promotions are applicable to store, catalog, and web sales facts
        hierarchy!("promotions", [
            "store_sales", "catalog_sales", "web_sales"
        ]);

        Self {
            measures,
            hierarchies,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tpcds_defaults_non_empty() {
        let b = FactBindings::tpcds_defaults();
        assert!(!b.measures.is_empty(), "expected non-empty measure bindings");
        assert!(
            !b.hierarchies.is_empty(),
            "expected non-empty hierarchy bindings"
        );
    }

    #[test]
    fn from_json_empty_errors() {
        let result = FactBindings::from_json(r#"{"measures": {}, "hierarchies": {}}"#);
        assert!(
            matches!(result, Err(FactBindingsError::Empty)),
            "expected Empty error"
        );
    }

    #[test]
    fn from_json_malformed_errors() {
        let result = FactBindings::from_json("not json");
        assert!(
            matches!(result, Err(FactBindingsError::Json(_))),
            "expected Json error"
        );
    }

    #[test]
    fn groups_for_measure_missing_returns_empty() {
        let b = FactBindings::tpcds_defaults();
        let groups = b.groups_for_measure("tpcds_benchmark_model.does_not_exist");
        assert!(groups.is_empty());
    }

    #[test]
    fn groups_for_hierarchy_missing_returns_empty() {
        let b = FactBindings::tpcds_defaults();
        let groups = b.groups_for_hierarchy("does_not_exist");
        assert!(groups.is_empty());
    }
}
