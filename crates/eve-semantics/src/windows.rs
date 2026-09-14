//! Layers, generic windows, message boxes, neocom, chat, info panels,
//! selected item — plus the generic-window fallback ("every window renders
//! whether or not we have a specialized view").

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// Window type names claimed by specialized extractors; everything else
/// with a `content` child falls back to the generic window shape.
pub const SPECIALIZED_WINDOW_TYPES: &[&str] = &[
    "OverView",
    "OverviewWindow",
    "OverviewWindowOld",
    "InventoryPrimary",
    "ActiveShipCargo",
    "ShipUI",
    "MessageBox",
    "NeocomContainer",
    "InfoPanelContainer",
    "ChatWindowStack",
    "ActiveItem",
    "SelectedItemWnd",
    "MarketOrdersWnd",
];

#[derive(Clone, Debug, Serialize)]
pub struct LayerSummary {
    pub name: String,
    pub region: DisplayRegion,
    pub window_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct GenericWindow {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub type_name: String,
    pub caption: Option<String>,
    /// Contained text nodes, top-left first, capped for very dense windows
    /// (`truncated_texts` reports the remainder).
    pub texts: Vec<String>,
    pub truncated_texts: usize,
}

const GENERIC_WINDOW_TEXT_CAP: usize = 200;

#[derive(Clone, Debug, Serialize)]
pub struct MessageBox {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub caption: Option<String>,
    pub content_text: Option<String>,
    pub buttons: Vec<MessageBoxButton>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MessageBoxButton {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Neocom {
    pub region: DisplayRegion,
    pub buttons: Vec<NeocomButton>,
    /// Minutes since midnight.
    pub clock_minutes: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NeocomButton {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub name: Option<String>,
    pub hint: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatWindowStack {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub windows: Vec<ChatWindow>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatWindow {
    pub caption: Option<String>,
    pub users: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InfoPanelContainerSummary {
    pub region: DisplayRegion,
    pub panels: Vec<InfoPanelSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InfoPanelSummary {
    pub type_name: String,
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub title_text: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelectedItemWindow {
    pub region: DisplayRegion,
    pub buttons: Vec<SelectedItemButton>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelectedItemButton {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub hint: Option<String>,
}

/// Every root layer, in tree order (= z order, topmost first). Includes
/// login/character-selection layers the presentation list never knew.
pub fn extract_layers(tree: &RegionedTree<'_>) -> Vec<LayerSummary> {
    tree.children_of(tree.root())
        .filter(|node| node.type_name() == "LayerCore")
        .map(|layer| LayerSummary {
            name: layer
                .name()
                .map(str::to_string)
                .unwrap_or_else(|| layer.type_name().to_string()),
            region: layer.total_region,
            window_count: tree.children_of(layer).count(),
        })
        .collect()
}

/// Control types that are not windows even though they may own a
/// `content`-shaped child (observed in the 曙光 snapshot 2026-09-12).
const NON_WINDOW_TYPES: &[&str] = &[
    "ButtonIndicator",
    "ButtonWindow",
    "ButtonInventory",
    "ButtonIcon",
    "Button",
    "Icon",
    "Sprite",
    "ContainerAutoSize",
];

/// Windows: nodes with a `content` child (dockable windows), plus nodes
/// whose type names claim them. Order: topmost first (last child of a
/// layer draws on top).
pub fn extract_generic_windows(tree: &RegionedTree<'_>) -> Vec<GenericWindow> {
    let mut windows: Vec<GenericWindow> = tree
        .all_regioned()
        .filter(|node| node.depth >= 2)
        .filter(|node| {
            !SPECIALIZED_WINDOW_TYPES.contains(&node.type_name())
                && !NON_WINDOW_TYPES.contains(&node.type_name())
                && !node.type_name().starts_with("Button")
                && tree
                    .children_of(node)
                    .any(|child| child.name() == Some("content"))
        })
        .map(|window| {
            let texts: Vec<String> = tree
                .subtree_iter(window.index)
                .filter_map(|node| node.text())
                .filter(|text| !text.is_empty() && !text.starts_with("Failed to read"))
                .collect();
            let truncated = texts.len().saturating_sub(GENERIC_WINDOW_TEXT_CAP);
            GenericWindow {
                region: window.total_region,
                interaction: interaction_info(tree, window),
                type_name: window.type_name().to_string(),
                caption: caption_of(tree, window),
                texts: texts.into_iter().take(GENERIC_WINDOW_TEXT_CAP).collect(),
                truncated_texts: truncated,
            }
        })
        .collect();
    windows.reverse();
    windows
}

/// Caption chain: `WindowCaption` > `TextHeadline` > header label.
fn caption_of(tree: &RegionedTree<'_>, window: &RegionedNode<'_>) -> Option<String> {
    if let Some(caption) = tree
        .descendants(window)
        .find(|node| node.type_name() == "WindowCaption")
        .and_then(|node| node.text())
    {
        return Some(caption);
    }
    tree.descendants(window)
        .find(|node| node.type_name() == "TextHeadline")
        .and_then(|node| node.text())
}

/// MessageBox nodes plus HybridWindows inside modal layers.
pub fn extract_message_boxes(tree: &RegionedTree<'_>) -> Vec<MessageBox> {
    let mut boxes: Vec<MessageBox> = tree
        .all_regioned()
        .filter(|node| node.type_name() == "MessageBox")
        .filter_map(|node| message_box_of(tree, node))
        .collect();
    let modal_hybrids = tree
        .all_regioned()
        .filter(|node| node.type_name() == "LayerCore")
        .filter(|node| {
            node.name()
                .is_some_and(|name| name.to_lowercase().contains("modal"))
        })
        .flat_map(|layer| tree.descendants(layer))
        .filter(|node| node.type_name() == "HybridWindow")
        .filter_map(|node| message_box_of(tree, node));
    boxes.extend(modal_hybrids);
    boxes
}

fn message_box_of(tree: &RegionedTree<'_>, message_box: &RegionedNode<'_>) -> Option<MessageBox> {
    let caption = caption_of(tree, message_box);
    let content_text = tree
        .subtree_iter(message_box.index)
        .filter_map(|node| node.text())
        .filter(|text| !text.is_empty() && !text.starts_with("Failed to read"))
        .max_by_key(|text| text.len());
    let buttons = tree
        .descendants(message_box)
        .filter(|node| node.type_name().contains("ButtonGroup"))
        .flat_map(|group| tree.descendants(group))
        .filter(|node| node.type_name().contains("Button"))
        .filter_map(|button| {
            let text = tree
                .subtree_iter(button.index)
                .filter_map(|node| node.text())
                .max_by_key(|text| text.len());
            Some(MessageBoxButton {
                region: button.total_region,
                interaction: interaction_info(tree, button),
                text,
            })
        })
        // The same physical button appears at several nesting levels
        // (button > underlay > caption); keep one per hit area.
        .fold(Vec::new(), |mut unique, button| {
            if !unique
                .iter()
                .any(|existing: &MessageBoxButton| existing.region == button.region)
            {
                unique.push(button);
            }
            unique
        });
    Some(MessageBox {
        region: message_box.total_region,
        interaction: interaction_info(tree, message_box),
        caption,
        content_text,
        buttons,
    })
}

pub fn extract_neocom(tree: &RegionedTree<'_>) -> Option<Neocom> {
    let container = tree.find_by_type("NeocomContainer").next()?;
    let buttons = tree
        .descendants(container)
        .filter(|node| node.type_name().starts_with("Button"))
        .filter(|node| node.name().is_some())
        .map(|button| NeocomButton {
            region: button.total_region,
            interaction: interaction_info(tree, button),
            name: button.name().map(str::to_string),
            hint: button.hint().map(str::to_string),
        })
        .collect();
    let clock_minutes = tree
        .descendants(container)
        .find(|node| node.type_name() == "InGameClock")
        .and_then(|clock| clock.text().as_deref().map(crate::parsing::parse_clock_minutes))
        .flatten();
    Some(Neocom {
        region: container.total_region,
        buttons,
        clock_minutes,
    })
}

pub fn extract_chat_window_stacks(tree: &RegionedTree<'_>) -> Vec<ChatWindowStack> {
    tree.find_by_type("ChatWindowStack")
        .filter_map(|stack| {
            let windows = tree
                .descendants(stack)
                .filter(|node| node.type_name() == "XmppChatWindow")
                .filter_map(|window| {
                    let caption = window.text().or_else(|| caption_of(tree, window));
                    let users = tree
                        .descendants(window)
                        .filter(|node| {
                            matches!(
                                node.type_name(),
                                "XmppChatSimpleUserEntry" | "XmppChatUserEntry"
                            )
                        })
                        .filter_map(|user| user.text())
                        .collect();
                    Some(ChatWindow { caption, users })
                })
                .collect();
            Some(ChatWindowStack {
                region: stack.total_region,
                interaction: interaction_info(tree, stack),
                windows,
            })
        })
        .collect()
}

/// Info panels: the specialized panels plus `mainCont` children as
/// "other" panels (degrade-don't-disappear).
pub fn extract_info_panels(tree: &RegionedTree<'_>) -> Option<InfoPanelContainerSummary> {
    let container = tree
        .find_by_type("InfoPanelContainer")
        .max_by(|a, b| tree.count_descendants(a).cmp(&tree.count_descendants(b)))?;
    let mut panels = Vec::new();
    for panel in tree.descendants(container) {
        match panel.type_name() {
            "InfoPanelLocationInfo" | "InfoPanelRoute" | "InfoPanelAgentMissions" => {
                panels.push(info_panel_summary(tree, panel));
            }
            _ if panel.name() == Some("mainCont") => {
                // Unclaimed mainCont children surface as other panels when
                // they look like panels (type name), not loose controls.
                for child in tree.children_of(panel) {
                    if !matches!(
                        child.type_name(),
                        "InfoPanelLocationInfo" | "InfoPanelRoute" | "InfoPanelAgentMissions"
                    ) && child.type_name().contains("Panel")
                    {
                        panels.push(info_panel_summary(tree, child));
                    }
                }
            }
            _ => {}
        }
    }
    Some(InfoPanelContainerSummary {
        region: container.total_region,
        panels,
    })
}

fn info_panel_summary(tree: &RegionedTree<'_>, panel: &RegionedNode<'_>) -> InfoPanelSummary {
    InfoPanelSummary {
        type_name: panel.type_name().to_string(),
        region: panel.total_region,
        interaction: interaction_info(tree, panel),
        title_text: tree
            .subtree_iter(panel.index)
            .filter_map(|node| node.text())
            .find(|text| !text.is_empty() && !text.starts_with("Failed to read")),
    }
}

pub fn extract_selected_item(tree: &RegionedTree<'_>) -> Option<SelectedItemWindow> {
    // The client renamed this window across versions; accept both.
    let window = tree
        .find_by_type("ActiveItem")
        .chain(tree.find_by_type("SelectedItemWnd"))
        .next()?;
    let buttons = tree
        .descendants(window)
        .filter(|node| node.hint().is_some() || node.type_name() == "ButtonIcon")
        .map(|button| SelectedItemButton {
            region: button.total_region,
            interaction: interaction_info(tree, button),
            hint: button.hint().map(str::to_string),
        })
        .collect();
    Some(SelectedItemWindow {
        region: window.total_region,
        buttons,
    })
}
