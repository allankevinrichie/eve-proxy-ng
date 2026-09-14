//! Fitting window extraction: module slots with installed module names,
//! and the ship stats panels (capacitor / offense / defense / targeting /
//! navigation / drones).
//!
//! Observed structure (曙光 2026-09-12): `FittingWindow` holds
//! `FittingSlot` children (slot number in `_name`, installed module name
//! as a contained label, `空闲中…` hints on empty slots) and
//! `ExpandableMenu` stat panels whose header row carries the stat name
//! and value (`电容 | 162.0 GJ / 58秒`).

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::strip_markup;
use crate::region::{DisplayRegion, RegionedTree};

/// One fitting slot (high/mid/low/rig; the row grouping is geometric and
/// belongs to the HUD extractor, here we report slots as laid out).
#[derive(Clone, Debug, Serialize)]
pub struct FittingSlot {
    /// Slot number from the client (`_name`, e.g. "27").
    pub slot: Option<String>,
    pub region: DisplayRegion,
    /// Installed module display name; `None` on empty slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// True when the slot shows an "空闲中…" (empty) hint.
    pub is_empty: bool,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
}

/// One stat readout (`电容 → 162.0 GJ / 58秒`).
#[derive(Clone, Debug, Serialize)]
pub struct FittingStat {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// The fitting window.
#[derive(Clone, Debug, Serialize)]
pub struct FittingWindow {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub slots: Vec<FittingSlot>,
    pub stats: Vec<FittingStat>,
}

pub fn extract_fitting_window(tree: &RegionedTree<'_>) -> Option<FittingWindow> {
    let window = tree.find_by_type("FittingWindow").next()?;

    let slots = tree
        .descendants(window)
        .filter(|node| node.type_name() == "FittingSlot")
        .filter(|node| node.total_region.width > 8)
        .filter_map(|slot| {
            let texts: Vec<String> = tree
                .subtree_iter(slot.index)
                .filter_map(|node| node.text())
                .map(|text| strip_markup(&text))
                .filter(|text| !text.is_empty() && !text.starts_with("Failed to read"))
                .collect();
            let slot_id = slot.name().map(str::to_string);
            // The module name is the first text that is neither the slot
            // number nor chrome ("underlay") nor an empty-slot hint.
            let is_empty = texts.iter().any(|t| t.contains("空闲"));
            let module = texts.iter().find(|t| {
                !t.contains("空闲")
                    && !t.contains("underlay")
                    && Some(t.as_str()) != slot_id.as_deref()
                    && t.parse::<i64>().is_err()
                    && t.len() >= 2
            });
            Some(FittingSlot {
                slot: slot_id,
                region: slot.total_region,
                module: module.cloned(),
                is_empty,
                interaction: interaction_info(tree, slot),
            })
        })
        .collect();

    let stats = tree
        .descendants(window)
        .filter(|node| node.type_name() == "ExpandableMenu")
        .filter_map(|menu| {
            // Header row: the first label is the stat name, the last the
            // value (e.g. 电容 | 162.0 GJ / 58秒).
            let labels: Vec<String> = tree
                .subtree_iter(menu.index)
                .take(12)
                .filter(|node| node.type_name().contains("Label"))
                .filter_map(|node| node.text())
                .map(|text| strip_markup(&text))
                .filter(|text| !text.is_empty() && !text.starts_with("Failed to read"))
                .collect();
            let name = labels.first()?.clone();
            let value = labels.get(1).cloned();
            Some(FittingStat { name, value })
        })
        .collect();

    Some(FittingWindow {
        region: window.total_region,
        interaction: interaction_info(tree, window),
        slots,
        stats,
    })
}
