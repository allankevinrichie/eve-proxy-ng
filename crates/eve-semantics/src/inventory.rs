//! Inventory windows: cargo and item containers with capacity gauges.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::{parse_capacity_gauge, split_row_cells};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// Window type names (client renamed across versions).
pub const INVENTORY_WINDOW_TYPES: &[&str] = &["InventoryPrimary", "ActiveShipCargo"];

/// Selected-container type names.
pub const CONTAINER_TYPES: &[&str] = &[
    "ShipCargo",
    "ShipDroneBay",
    "ShipGeneralMiningHold",
    "StationItems",
    "ShipFleetHangar",
    "StructureItemHangar",
];

#[derive(Clone, Debug, Serialize)]
pub struct InventoryWindow {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// e.g. `1,211.9/5,000.0 m³` (parse with
    /// [`parse_capacity_gauge`]-backed `capacity_used_total`).
    pub capacity_gauge_text: Option<String>,
    pub capacity_used_total: Option<(i64, i64)>,
    /// Currently selected container type name.
    pub selected_container_type: Option<String>,
    pub items: Vec<InventoryItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InventoryItem {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub name: Option<String>,
    pub quantity: Option<i64>,
    pub is_selected: bool,
}

pub fn extract_inventory_windows(tree: &RegionedTree<'_>) -> Vec<InventoryWindow> {
    tree.all_regioned()
        .filter(|node| INVENTORY_WINDOW_TYPES.contains(&node.type_name()))
        .filter_map(|window| extract_window(tree, window))
        .collect()
}

fn extract_window(tree: &RegionedTree<'_>, window: &RegionedNode<'_>) -> Option<InventoryWindow> {
    let capacity_gauge_text = tree
        .descendants(window)
        .find(|node| node.type_name().contains("CapacityGauge"))
        .and_then(|gauge| gauge.text());
    let capacity_used_total = capacity_gauge_text.as_deref().and_then(parse_capacity_gauge);

    // The selected container sits in the "right" container.
    let selected_container_type = tree
        .descendants(window)
        .find(|node| {
            node.type_name() == "Container"
                && node.name().is_some_and(|name| name.to_lowercase().contains("right"))
        })
        .and_then(|right| {
            tree.descendants(right)
                .find(|node| CONTAINER_TYPES.contains(&node.type_name()))
                .map(|node| node.type_name().to_string())
        });

    let items = tree
        .descendants(window)
        .filter(|node| node.type_name() == "Item" || node.type_name() == "InvItem")
        .filter_map(|item| item_of(tree, item))
        .collect();

    Some(InventoryWindow {
        region: window.total_region,
        interaction: interaction_info(tree, window),
        capacity_gauge_text,
        capacity_used_total,
        selected_container_type,
        items,
    })
}

fn item_of(tree: &RegionedTree<'_>, item: &RegionedNode<'_>) -> Option<InventoryItem> {
    // Row text: on the node itself or a child label (`InventoryItemLOL`
    // in details view). Tab-tag rows: one element, `<t>`-separated
    // columns; icon-view items have an `itemNameLabel` child plus a
    // `qtypar` quantity badge (observed 曙光 2026-09).
    let row_text = item.text().or_else(|| {
        tree.subtree_iter(item.index)
            .skip(1)
            .filter(|child| child.depth <= item.depth + 3)
            .find_map(|child| child.text())
    });
    let (name, quantity) = match row_text.as_deref() {
        Some(text) if text.contains("<t>") => {
            let cells = split_row_cells(text);
            (
                cells.first().cloned(),
                cells.get(1).and_then(|cell| {
                    cell.trim()
                        .parse::<i64>()
                        .ok()
                        .or_else(|| crate::parsing::parse_number_truncating_fraction(cell))
                }),
            )
        }
        _ => {
            let name = tree
                .subtree_iter(item.index)
                .skip(1)
                .find(|node| node.name() == Some("itemNameLabel"))
                .and_then(|node| node.text())
                .map(|text| crate::parsing::strip_markup(&text))
                .or_else(|| {
                    // Fallback: first contained text.
                    tree.subtree_iter(item.index)
                        .skip(1)
                        .filter_map(|node| node.text())
                        .next()
                        .map(|text| crate::parsing::strip_markup(&text))
                });
            let quantity = tree
                .subtree_iter(item.index)
                .skip(1)
                .find(|node| node.name() == Some("qtypar"))
                .and_then(|badge| {
                    tree.subtree_iter(badge.index)
                        .skip(1)
                        .filter_map(|node| node.text())
                        .next()
                })
                .and_then(|text| crate::parsing::parse_number_truncating_fraction(&text));
            (name, quantity)
        }
    };
    Some(InventoryItem {
        region: item.total_region,
        interaction: interaction_info(tree, item),
        name,
        quantity,
        is_selected: item.boolean("isSelected").unwrap_or(false)
            || item.boolean("_selected").unwrap_or(false),
    })
}
