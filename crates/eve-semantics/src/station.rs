//! Station window (LobbyWnd) extraction: services, undock, structure
//! control, and the guest/agent/office/hangar tabs.
//!
//! Observed structure (曙光 2026-09-12): `LobbyWnd` carries
//! `StationServiceBtn` children whose `_name` is a stable service id
//! (`lpstore`, `industry`, `market`, ...), an `UndockButton`,
//! `takeControlBtn` / `dockedModeBtn`, and `Tab` children whose hints
//! describe the panel (`查看现在停靠在空间站中的飞行员`...).

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// One station service button (`_name` is a stable internal id).
#[derive(Clone, Debug, Serialize)]
pub struct StationService {
    /// Stable service id: lpstore / industry / market / fitting / ...
    pub id: String,
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
}

/// A generic button reference (undock / control / tabs).
#[derive(Clone, Debug, Serialize)]
pub struct StationButton {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
}

/// The station services panel while docked.
#[derive(Clone, Debug, Serialize)]
pub struct StationWindow {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub services: Vec<StationService>,
    pub undock: Option<StationButton>,
    pub take_control: Option<StationButton>,
    pub docked_mode: Option<StationButton>,
    pub tabs: Vec<StationButton>,
}

pub fn extract_station_window(tree: &RegionedTree<'_>) -> Option<StationWindow> {
    let window = tree.find_by_type("LobbyWnd").next()?;

    let services = tree
        .descendants(window)
        .filter(|node| node.type_name() == "StationServiceBtn")
        .filter_map(|button| {
            Some(StationService {
                id: button.name()?.to_string(),
                region: button.total_region,
                interaction: interaction_info(tree, button),
            })
        })
        .collect();

    let mut undock = None;
    let mut take_control = None;
    let mut docked_mode = None;
    for node in tree.descendants(window) {
        let button = |node: &RegionedNode<'_>| StationButton {
            name: node.name().map(str::to_string),
            hint: node.hint().map(str::to_string),
            region: node.total_region,
            interaction: interaction_info(tree, node),
        };
        match (node.type_name(), node.name()) {
            ("UndockButton", _) => undock = Some(button(node)),
            ("ControlButton", Some("takeControlBtn")) => take_control = Some(button(node)),
            ("ModeButton", Some("dockedModeBtn")) => docked_mode = Some(button(node)),
            _ => {}
        }
    }

    let tabs = tree
        .descendants(window)
        .filter(|node| node.type_name() == "Tab" && node.hint().is_some())
        .map(|tab| StationButton {
            name: tab.name().map(str::to_string),
            hint: tab.hint().map(str::to_string),
            region: tab.total_region,
            interaction: interaction_info(tree, tab),
        })
        .collect();

    Some(StationWindow {
        region: window.total_region,
        interaction: interaction_info(tree, window),
        services,
        undock,
        take_control,
        docked_mode,
        tabs,
    })
}
