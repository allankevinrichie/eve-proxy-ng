//! Overview window extraction: rows, columns, distances, indications.
//!
//! The header-to-cell assignment is geometric (a header claims a cell when
//! the cell lies within the header's horizontal span ±3 px), with the
//! tab-tag form (`"A<t>B<t>C"`) as fallback for single-element rows.
//! Derived fields avoid hard-coded English header names: the object name is
//! the leftmost cell, the distance the cell that parses with a unit.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::{parse_distance_meters, split_row_cells};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// Accepted type names (client renamed the window across versions).
pub const OVERVIEW_WINDOW_TYPES: &[&str] = &["OverView", "OverviewWindow", "OverviewWindowOld"];

#[derive(Clone, Debug, Serialize)]
pub struct OverviewWindow {
    pub region: DisplayRegion,
    /// 本窗口拥有的交互元素地址（树成员判定：元素祖先链上最近的
    /// 窗口节点拥有它；装配时填充）。
    pub element_addresses: Vec<String>,
    /// 本窗口节点地址（与 element.window_address / element_tree join）。
    pub address: String,

    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Client-announced caption, e.g. `Overview (General: General)`
    /// (tab then preset, observed 2026-07-22).
    pub caption: Option<String>,
    pub tabs: Vec<OverviewTab>,
    pub entries: Vec<OverviewEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OverviewTab {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub name: Option<String>,
    pub is_selected: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OverviewEntry {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Column name → cell text (header text verbatim, any locale).
    pub cells_texts: BTreeMap<String, String>,
    /// Leftmost cell (the object name in every observed preset).
    pub object_name: Option<String>,
    /// Cell parsing as a distance with unit.
    pub distance_meters: Option<i64>,
    /// Bracket icon the entry presents (`res:/…/Brackets/stargate.png`)
    /// — the object-class channel when names are ambiguous.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Semantic name of that icon (`stargate`, `citadelLarge`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_name: Option<String>,
    pub indications: CommonIndications,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct CommonIndications {
    pub targeting: bool,
    pub targeted_by_me: bool,
    pub is_jamming_me: bool,
    pub is_warp_disrupting_me: bool,
}

pub fn extract_overview_windows(
    tree: &RegionedTree<'_>,
    profile: &crate::FlavorProfile,
) -> Vec<OverviewWindow> {
    tree.all_regioned()
        .filter(|node| OVERVIEW_WINDOW_TYPES.contains(&node.type_name()))
        .filter_map(|window| extract_window(tree, window, profile))
        .collect()
}

fn extract_window(
    tree: &RegionedTree<'_>,
    window: &RegionedNode<'_>,
    profile: &crate::FlavorProfile,
) -> Option<OverviewWindow> {
    let descendants: Vec<&RegionedNode<'_>> = tree.descendants(window).collect();

    let caption = descendants
        .iter()
        .copied()
        .find(|node| node.type_name() == "WindowCaption")
        .and_then(|node| {
            node.text().or_else(|| {
                tree.subtree_iter(node.index)
                    .filter_map(|child| child.text())
                    .next()
            })
        });

    let tabs = descendants
        .iter()
        .copied()
        .filter(|node| node.type_name() == "OverviewTab")
        .map(|tab| OverviewTab {
            region: tab.total_region,
            interaction: interaction_info(tree, tab),
            name: tab.text().or_else(|| {
                tree.subtree_iter(tab.index)
                    .filter_map(|child| child.text())
                    .next()
            }),
            is_selected: tab.boolean("isSelected").unwrap_or(false)
                || tab.boolean("_selected").unwrap_or(false),
        })
        .collect();

    // Column headers: texts of the "…headers…" container.
    let headers: Vec<(String, DisplayRegion)> = descendants
        .iter()
        .copied()
        .find(|node| node.type_name().to_lowercase().contains("headers"))
        .map(|headers_container| header_texts(tree, headers_container))
        .unwrap_or_default();

    let entries = descendants
        .iter()
        .copied()
        .filter(|node| node.type_name() == "OverviewScrollEntry")
        .filter_map(|entry| extract_entry(tree, entry, &headers, profile))
        .collect();

    Some(OverviewWindow {
        element_addresses: Vec::new(),
        address: window.node.address.0.to_string(),
        region: window.total_region,
        interaction: interaction_info(tree, window),
        caption,
        tabs,
        entries,
    })
}

fn header_texts(tree: &RegionedTree<'_>, container: &RegionedNode<'_>) -> Vec<(String, DisplayRegion)> {
    tree.subtree_iter(container.index)
        .filter_map(|node| node.text().map(|text| (text, node.total_region)))
        .filter(|(text, region)| !text.is_empty() && region.width > 0)
        .collect()
}

fn extract_entry(
    tree: &RegionedTree<'_>,
    entry: &RegionedNode<'_>,
    headers: &[(String, DisplayRegion)],
    profile: &crate::FlavorProfile,
) -> Option<OverviewEntry> {
    let cells = entry_cells(tree, entry, headers);
    if cells.is_empty() {
        return None;
    }
    // Column semantics come from the flavor's header spellings; the
    // geometric heuristics only act as fallback.
    let cell_by_header = |spellings: &[&str]| -> Option<&EntryCell> {
        cells.iter().find(|cell| {
            spellings
                .iter()
                .any(|spelling| cell.header_name.trim() == *spelling)
        })
    };
    let object_name = cell_by_header(profile.object_name_headers)
        .map(|cell| cell.text.clone())
        .or_else(|| {
            cells
                .iter()
                .min_by_key(|cell| (cell.region.x, cell.region.y))
                .map(|cell| cell.text.clone())
        });
    let distance_meters = cell_by_header(profile.distance_headers)
        .and_then(|cell| parse_distance_meters(&cell.text))
        .or_else(|| {
            cells
                .iter()
                .filter_map(|cell| {
                    parse_distance_meters(&cell.text).map(|meters| (unit_rank(&cell.text), meters))
                })
                .max_by_key(|&(rank, _)| rank)
                .map(|(_, meters)| meters)
        });
    let icon = tree
        .subtree_iter(entry.index)
        .find(|node| node.type_name() == "SpaceObjectIcon")
        .and_then(|space_object_icon| crate::icons::element_icon(tree, space_object_icon));
    let icon_name = icon.as_deref().and_then(crate::icons::semantic_icon_name);
    Some(OverviewEntry {
        region: entry.total_region,
        interaction: interaction_info(tree, entry),
        cells_texts: cells
            .into_iter()
            .map(|cell| (cell.header_name, cell.text))
            .collect(),
        object_name,
        distance_meters,
        icon,
        icon_name,
        indications: indications_of(tree, entry),
    })
}

/// km/AU outrank plain meters: meters also appear in size columns.
fn unit_rank(text: &str) -> u8 {
    if text.contains("km") || text.contains("AU") {
        2
    } else {
        1
    }
}

/// One matched cell of an entry row.
struct EntryCell {
    /// Column name (header text, or the cell's own text when unmatched).
    header_name: String,
    text: String,
    region: DisplayRegion,
}

/// Text-bearing children of an entry, left to right, matched to headers.
fn entry_cells(
    tree: &RegionedTree<'_>,
    entry: &RegionedNode<'_>,
    headers: &[(String, DisplayRegion)],
) -> Vec<EntryCell> {
    let mut cells: Vec<(DisplayRegion, String)> = tree
        .subtree_iter(entry.index)
        .skip(1)
        .filter(|node| node.depth <= entry.depth + 4)
        .filter(|node| node.type_name().contains("Label") || node.type_name() == "Textbody")
        .filter_map(|node| node.text().map(|text| (node.total_region, text)))
        .filter(|(region, text)| !text.is_empty() && region.width > 0)
        .collect();
    cells.sort_by_key(|(region, _)| (region.y, region.x));
    let leftmost = cells.iter().map(|(r, _)| r.x).min().unwrap_or(0);

    let mut assigned: Vec<EntryCell> = Vec::new();
    for (region, text) in cells {
        let header = headers.iter().find(|(_, header_region)| {
            header_region.x < region.x + 3
                && header_region.x + header_region.width > region.x + region.width - 3
        });
        match header {
            Some((name, _)) => assigned.push(EntryCell {
                header_name: name.clone(),
                text,
                region,
            }),
            None => {
                // Tab-tag form: a cell ≥4 px right of the leftmost header
                // carries several values separated by `<t>`.
                if region.x >= leftmost + 4 {
                    for (i, cell) in split_row_cells(&text).into_iter().enumerate() {
                        let name = headers
                            .get(i)
                            .map(|(name, _)| name.clone())
                            .unwrap_or_else(|| format!("column {i}"));
                        assigned.push(EntryCell {
                            header_name: name,
                            text: cell,
                            region,
                        });
                    }
                } else {
                    assigned.push(EntryCell {
                        header_name: text.clone(),
                        text,
                        region,
                    });
                }
            }
        }
    }
    assigned
}

fn indications_of(tree: &RegionedTree<'_>, entry: &RegionedNode<'_>) -> CommonIndications {
    // Names under the first SpaceObjectIcon drive targeting flags.
    let space_object_icon = tree
        .subtree_iter(entry.index)
        .find(|node| node.type_name() == "SpaceObjectIcon");
    let mut indications = CommonIndications::default();
    if let Some(icon) = space_object_icon {
        for node in tree.subtree_iter(icon.index).skip(1) {
            match node.name() {
                Some("targeting") => indications.targeting = true,
                Some("targetedByMeIndicator") => indications.targeted_by_me = true,
                _ => {}
            }
        }
    }
    // Right-aligned icon container hints carry EWAR text (English observed;
    // China-client wording to be added per flavor).
    for node in tree.subtree_iter(entry.index).skip(1) {
        if node.name() == Some("rightAlignedIconContainer") {
            for hint in tree.subtree_iter(node.index).skip(1).filter_map(|n| n.hint()) {
                let lowered = hint.to_lowercase();
                if lowered.contains("is jamming me") {
                    indications.is_jamming_me = true;
                }
                if lowered.contains("is warp disrupting me") {
                    indications.is_warp_disrupting_me = true;
                }
            }
        }
    }
    indications
}
