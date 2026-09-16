//! Selected-item window actions: the orbit/approach/warp/dock/lock
//! button bar that appears when a space object is selected.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// One action button in the selected-item window (stable node name →
/// semantic kind).
#[derive(Clone, Debug, Serialize)]
pub struct SelectedItemAction {
    pub kind: String,
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Raw node name (`selectedItemApproach` → `approach`).
    pub name: String,
}

/// Stable node-name → semantic kind mapping.
fn action_kind(name: &str) -> Option<&'static str> {
    Some(match name {
        "selectedItemApproach" => "approach",
        "selectedItemWarpTo" => "warp_to",
        "selectedItemDock" => "dock",
        "selectedItemOrbit" => "orbit",
        "selectedItemKeepAtRange" => "keep_at_range",
        "selectedItemLockTarget" => "lock_target",
        "selectedItemLookAt" => "look_at",
        "selectedItemSetInterest" => "set_interest",
        "selectedItemShowInfo" => "show_info",
        _ => return None,
    })
}

/// All action buttons under the selected-item window.
pub fn extract_selected_item_actions(
    tree: &RegionedTree<'_>,
) -> Vec<SelectedItemAction> {
    tree.find_by_type("SelectedItemButton")
        .filter_map(|button| {
            let name = button.name()?;
            let kind = action_kind(name)?;
            Some(SelectedItemAction {
                kind: kind.to_string(),
                region: button.total_region,
                interaction: interaction_info(tree, button),
                name: name.to_string(),
            })
        })
        .collect()
}
