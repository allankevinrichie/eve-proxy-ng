//! Scrollable list views: the viewport/content/handle triple plus
//! per-entry visibility and scroll math, so an agent can decide "this
//! entry is off-screen, scroll N px (or drag the handle M px) to reveal
//! it".
//!
//! Model (validated against the live client 2026-09-12):
//! - The scroll node's region is the viewport; entry regions are laid out
//!   in content coordinates and extend beyond it (clipped visually and
//!   for input — see [`crate::interaction`] clipping).
//! - The scrollbar `ScrollHandle` position encodes the current offset:
//!   `offset = (handle.y - track.y) / (track.h - handle.h) * (content -
//!   viewport)`. Measured 153px (handle) vs 156px (content) on a random
//!   wheel scroll — within 0.5%.
//! - To fully reveal an entry: scroll down by `entry.bottom -
//!   viewport.bottom` px (negative: scroll up by `viewport.top -
//!   entry.top`).

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};
use eve_memory::NodeValue;

/// Scroll viewport types whose content scrolls (the clipper family).
const SCROLL_VIEW_TYPES: &[&str] = &[
    "Scroll",
    "ScrollContainer",
    "BasicDynamicScroll",
    "NotificationScrollContainer",
];

/// One scrollable list on screen.
#[derive(Clone, Debug, Serialize)]
pub struct ScrollView {
    /// The viewport (visible window into the content).
    pub viewport: DisplayRegion,
    /// Top-to-bottom extent of the laid-out entries, in px.
    pub content_extent_px: i64,
    /// Scrollbar handle region, when a scrollbar exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollbar_handle: Option<DisplayRegion>,
    /// Scrollbar track region, when a scrollbar exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollbar_track: Option<DisplayRegion>,
    /// Current scroll offset in px (0 = top), derived from the handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_offset_px: Option<i64>,
    pub entries: Vec<ScrollEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScrollEntry {
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// First contained text (row label).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub region: DisplayRegion,
    /// Fully inside the viewport right now.
    pub is_visible: bool,
    /// Signed px to scroll (down positive) for full visibility; 0 when
    /// already visible.
    pub scroll_to_reveal_px: i64,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
}

/// Extract every scrollable list (hangar tree, chat user list, market
/// rows, overview rows, ...) with visibility and scroll predictions.
pub fn extract_scrollable_views(tree: &RegionedTree<'_>) -> Vec<ScrollView> {
    tree.all_regioned()
        .filter(|node| SCROLL_VIEW_TYPES.contains(&node.type_name()))
        .filter_map(|scroll| build_view(tree, scroll))
        .collect()
}

fn build_view(tree: &RegionedTree<'_>, scroll: &RegionedNode<'_>) -> Option<ScrollView> {
    let viewport = scroll.total_region;
    if viewport.is_empty() {
        return None;
    }
    // Rows: entry-typed descendants (lists, trees) plus inventory items
    // (`InvItem` icon grids, `Item` detail/list rows), with a sane
    // height. Note: detail/list views are virtualized — only the rows
    // near the viewport exist in the tree; paginate by scrolling.
    let mut entries: Vec<&RegionedNode<'_>> = tree
        .descendants(scroll)
        .filter(|node| {
            (node.type_name().contains("Entry")
                || node.type_name() == "InvItem"
                || node.type_name() == "Item")
                && (16..=400).contains(&node.total_region.height)
        })
        .collect();
    // Deduplicate nested entries (e.g. TreeViewEntry inside
    // TreeViewEntryInventory): keep the outermost.
    entries.sort_by_key(|node| node.index);
    entries.dedup_by(|a, b| a.index > b.index && tree.subtree_end(b.index) > a.index);
    if entries.is_empty() {
        return None;
    }

    let content_top = entries.iter().map(|e| e.total_region.y).min()?;
    let content_bottom = entries
        .iter()
        .map(|e| e.total_region.y + e.total_region.height)
        .max()?;
    let content_extent = content_bottom - content_top;

    // Scrollbar: a `Scrollbar` descendant whose child `ScrollHandle`
    // gives the position.
    let mut handle: Option<(DisplayRegion, DisplayRegion)> = None;
    for node in tree.descendants(scroll) {
        if node.type_name().contains("Scrollbar") {
            if let Some(handle_node) = tree
                .descendants(node)
                .find(|n| n.type_name().contains("ScrollHandle"))
            {
                handle = Some((handle_node.total_region, node.total_region));
            }
        }
    }
    let scroll_offset_px = handle.map(|(handle_region, track)| {
        let movable = (track.height - handle_region.height).max(1);
        let ratio = (handle_region.y - track.y).clamp(0, movable) as f64 / movable as f64;
        let overscroll = (content_extent - viewport.height).max(0);
        (ratio * overscroll as f64).round() as i64
    });

    let entries = entries
        .into_iter()
        .map(|entry| {
            let region = entry.total_region;
            let bottom = region.y + region.height;
            let scroll_to_reveal = if bottom <= viewport.y + viewport.height && region.y >= viewport.y {
                0
            } else if bottom > viewport.y + viewport.height {
                bottom - (viewport.y + viewport.height)
            } else {
                region.y - viewport.y
            };
            ScrollEntry {
                type_name: entry.type_name().to_string(),
                name: entry.name().map(str::to_string),
                label: contained_label(tree, entry),
                region,
                is_visible: scroll_to_reveal == 0,
                scroll_to_reveal_px: scroll_to_reveal,
                interaction: interaction_info(tree, entry),
            }
        })
        .collect();

    Some(ScrollView {
        viewport,
        content_extent_px: content_extent,
        scrollbar_handle: handle.map(|(handle_region, _)| handle_region),
        scrollbar_track: handle.map(|(_, track)| track),
        scroll_offset_px,
        entries,
    })
}

/// First contained display text, markup stripped; prefers the
/// `itemNameLabel` child (icon-grid rows put their quantity badge before
/// the name in tree order).
fn contained_label(tree: &RegionedTree<'_>, entry: &RegionedNode<'_>) -> Option<String> {
    tree.subtree_iter(entry.index)
        .find(|node| node.name() == Some("itemNameLabel"))
        .and_then(|node| node.text())
        .map(|text| crate::parsing::strip_markup(&text))
        .or_else(|| {
            tree.subtree_iter(entry.index)
                .filter_map(|node| node.text())
                .map(|text| crate::parsing::strip_markup(&text))
                .find(|text| !text.is_empty() && !text.starts_with("Failed to read"))
        })
        .filter(|text| !text.is_empty())
}

/// Wide integers never appear as labels; silences unused-import lint for
/// builds without tests.
#[allow(dead_code)]
fn _unused(_v: NodeValue) {}
