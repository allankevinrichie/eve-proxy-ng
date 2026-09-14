//! Context menus (right-click) and util menus (filter panels).

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// Layer that hosts context menus.
pub const MENU_LAYER_NAME: &str = "l_menu";
/// Layer that hosts util menus.
pub const UTIL_MENU_LAYER_NAME: &str = "l_utilmenu";

#[derive(Clone, Debug, Serialize)]
pub struct ContextMenu {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub entries: Vec<MenuEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MenuEntry {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub text: Option<String>,
    pub hint: Option<String>,
    pub is_checked: bool,
    /// Unnamed right-edge sprite marks a submenu.
    pub has_submenu: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct UtilMenu {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Checkbox-style rows with their checked state.
    pub checkboxes: Vec<UtilMenuRow>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UtilMenuRow {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub text: Option<String>,
    pub is_checked: bool,
}

/// Context menus: children of `l_menu` whose type contains "menu", plus
/// plain containers holding combo-box rows (an open combo reads as a menu).
pub fn extract_context_menus(tree: &RegionedTree<'_>) -> Vec<ContextMenu> {
    tree.all_regioned()
        .filter(|node| {
            node.type_name() == "LayerCore" && node.name() == Some(MENU_LAYER_NAME)
        })
        .flat_map(|layer| tree.children_of(layer))
        .filter(|child| {
            child.type_name().to_lowercase().contains("menu")
                || tree
                    .subtree_iter(child.index)
                    .any(|node| node.type_name() == "ComboEntry")
        })
        .filter_map(|menu| context_menu_of(tree, menu))
        .collect()
}

fn context_menu_of(tree: &RegionedTree<'_>, menu: &RegionedNode<'_>) -> Option<ContextMenu> {
    let entries: Vec<MenuEntry> = tree
        .children_of(menu)
        .filter(|child| {
            child
                .type_name()
                .to_lowercase()
                .contains("menuentry")
        })
        .filter_map(|entry| menu_entry_of(tree, entry))
        .collect();
    Some(ContextMenu {
        region: menu.total_region,
        interaction: interaction_info(tree, menu),
        entries,
    })
}

fn menu_entry_of(tree: &RegionedTree<'_>, entry: &RegionedNode<'_>) -> Option<MenuEntry> {
    // Entry text: its largest contained text node.
    let text = tree
        .subtree_iter(entry.index)
        .filter_map(|node| node.text().map(|text| (node.total_region.area(), text)))
        .max_by_key(|(area, _)| *area)
        .map(|(_, text)| text);
    let is_checked = tree.subtree_iter(entry.index).any(|node| {
        node.name() == Some("self_ok")
            || node.boolean("_checked").unwrap_or(false)
            || node.boolean("isChecked").unwrap_or(false)
    });
    let has_submenu = tree.subtree_iter(entry.index).skip(1).any(|node| {
        node.type_name() == "Sprite" && node.name().is_none() && node.total_region.width > 0
    });
    Some(MenuEntry {
        region: entry.total_region,
        interaction: interaction_info(tree, entry),
        text,
        hint: entry.hint().map(str::to_string),
        is_checked,
        has_submenu,
    })
}

/// Util menus: children of `l_utilmenu` (seen: `ExpandedUtilMenu`), read as
/// the small panel they are — checkbox rows, not menu entries.
pub fn extract_util_menus(tree: &RegionedTree<'_>) -> Vec<UtilMenu> {
    tree.all_regioned()
        .filter(|node| {
            node.type_name() == "LayerCore" && node.name() == Some(UTIL_MENU_LAYER_NAME)
        })
        .flat_map(|layer| tree.children_of(layer))
        .filter(|child| child.type_name().to_lowercase().contains("utilmenu"))
        .filter_map(|menu| util_menu_of(tree, menu))
        .collect()
}

fn util_menu_of(tree: &RegionedTree<'_>, menu: &RegionedNode<'_>) -> Option<UtilMenu> {
    let checkboxes = tree
        .subtree_iter(menu.index)
        .filter(|node| {
            node.type_name().contains("Checkbox") || node.type_name().contains("UtilMenu")
        })
        .filter_map(|row| {
            let text = tree
                .subtree_iter(row.index)
                .skip(1)
                .filter_map(|node| node.text())
                .next();
            Some(UtilMenuRow {
                region: row.total_region,
                interaction: interaction_info(tree, row),
                text,
                is_checked: row
                    .boolean("_checked")
                    .or_else(|| row.boolean("isChecked"))
                    .unwrap_or(false),
            })
        })
        .collect();
    Some(UtilMenu {
        region: menu.total_region,
        interaction: interaction_info(tree, menu),
        checkboxes,
    })
}
