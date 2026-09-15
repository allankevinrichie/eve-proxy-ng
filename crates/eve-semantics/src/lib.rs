//! # eve-semantics
//!
//! Semantic layer: maps the raw UI tree ([`eve_memory::UiNode`]) to typed
//! game meaning — overview rows, module buttons, capacitor, windows, menus.
//!
//! Structure knowledge is ported from the Sanderling project's
//! `ParseUserInterface.elm` (BlindGuyNW fork 2026-07) and dispatched per
//! server flavor with fallback to the 曙光 (Infinity) implementation.
//!
//! Layering:
//!
//! ```text
//! UiNode ──▶ RegionedTree (regions, occlusion, subtree indexing)
//!                │
//!                ▼
//!        extractors (ship / overview / menu / inventory / windows)
//!                │
//!                ▼
//!           UiSnapshot (serde → JSON / Python)
//! ```

pub mod character_select;
pub mod fitting;
pub mod icons;
pub mod interaction;
pub mod inventory;
pub mod menu;
pub mod overview;
pub mod parsing;
pub mod region;
pub mod resources;
pub mod roles;
pub mod scroll;
pub mod ship;
pub mod station;
pub mod windows;

use serde::Serialize;

use eve_memory::{Flavor, UiNode};
use fitting::FittingWindow;
use inventory::InventoryWindow;
use menu::{ContextMenu, UtilMenu};
use overview::OverviewWindow;
use region::RegionedTree;
use scroll::ScrollView;
use ship::{ModuleButtonTooltip, ShipUi};
use station::StationWindow;
use windows::{
    ChatWindowStack, GenericWindow, InfoPanelContainerSummary, LayerSummary, MessageBox,
    Neocom, SelectedItemWindow,
};

/// Which screen the client is showing right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    /// Character selection (pre-game).
    CharacterSelect,
    /// Docked in a station or structure.
    Docked,
    /// Flying in space.
    InSpace,
    /// Unclassified (loading, login edge cases).
    Unknown,
}

/// One-glance game state: what screen, what blocks input.
#[derive(Clone, Debug, Serialize)]
pub struct GameState {
    pub screen: Screen,
    /// Caption of the topmost modal message box, when one is open —
    /// while set, nothing behind it is clickable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by_modal: Option<String>,
}

/// Parse a UI tree into the semantic snapshot for the given flavor.
pub fn parse_ui_tree(tree: &UiNode, flavor: Flavor) -> UiSnapshot {
    parse_ui_tree_timed(tree, flavor).0
}

/// Phase breakdown of [`parse_ui_tree`] for performance work (µs).
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseTiming {
    pub regioned_us: u64,
    pub interaction_us: u64,
    pub extractors_us: u64,
    pub total_us: u64,
}

/// [`parse_ui_tree`] with per-phase timings: tree building, interaction
/// occlusion probing, and the remaining extractors.
pub fn parse_ui_tree_timed(tree: &UiNode, flavor: Flavor) -> (UiSnapshot, ParseTiming) {
    let total_start = std::time::Instant::now();
    let profile = FlavorProfile::for_flavor(flavor);
    let regioned_start = std::time::Instant::now();
    let regioned = RegionedTree::build(tree);
    let regioned_us = regioned_start.elapsed().as_micros() as u64;

    let interaction_start = std::time::Instant::now();
    let mut interaction_elements = interaction::extract_interaction_elements(&regioned);
    let mut other_windows = windows::extract_generic_windows(&regioned);
    // Window membership: every interaction element joins its enclosing
    // top-level generic window (smallest containing rect, center test) —
    // the container grouping agents use to tell what belongs together.
    for element in &mut interaction_elements {
        let (cx, cy) = element.region.center();
        let mut best: Option<usize> = None;
        let mut best_area = i64::MAX;
        for (index, window) in other_windows.iter().enumerate() {
            if window.region.contains_point(cx, cy) {
                let area = window.region.area();
                if area < best_area {
                    best_area = area;
                    best = Some(index);
                }
            }
        }
        if let Some(index) = best {
            other_windows[index].element_addresses.push(element.address.clone());
            element.window_address = Some(other_windows[index].address.clone());
        }
    }
    let interaction_us = interaction_start.elapsed().as_micros() as u64;

    let extractors_start = std::time::Instant::now();
    let ship_ui = ship::extract_ship_ui(&regioned);
    let station_window = station::extract_station_window(&regioned);
    let message_boxes = windows::extract_message_boxes(&regioned);
    let game_state = classify_game_state(&regioned, ship_ui.is_some(), station_window.is_some(), &message_boxes);
    let client_size = (!regioned.root().region.is_empty()).then_some(ClientSize {
        width: regioned.root().region.width,
        height: regioned.root().region.height,
    });
    let snapshot = UiSnapshot {
        flavor: profile.flavor.internal_tag().unwrap_or("unknown").to_string(),
        node_count: eve_memory::uitree::count_nodes(tree),
        client_size,
        game_state,
        ship_ui,
        // Module tooltip while a module button is hovered (the
        // hover-then-read identification channel).
        module_button_tooltip: ship::extract_module_button_tooltip(&regioned),
        overview_windows: overview::extract_overview_windows(&regioned, profile),
        context_menus: menu::extract_context_menus(&regioned),
        util_menus: menu::extract_util_menus(&regioned),
        inventory_windows: inventory::extract_inventory_windows(&regioned),
        message_boxes,
        neocom: windows::extract_neocom(&regioned),
        info_panels: windows::extract_info_panels(&regioned),
        selected_item_window: windows::extract_selected_item(&regioned),
        chat_window_stacks: windows::extract_chat_window_stacks(&regioned),
        fitting_window: fitting::extract_fitting_window(&regioned),
        station_window,
        character_select: character_select::extract_character_select(&regioned),
        scrollable_views: scroll::extract_scrollable_views(&regioned),
        layers: windows::extract_layers(&regioned),
        other_windows,
        interaction_elements,
    };
    let extractors_us = extractors_start.elapsed().as_micros() as u64;
    (
        snapshot,
        ParseTiming {
            regioned_us,
            interaction_us,
            extractors_us,
            total_us: total_start.elapsed().as_micros() as u64,
        },
    )
}

fn classify_game_state(
    tree: &RegionedTree<'_>,
    has_ship_ui: bool,
    has_station: bool,
    message_boxes: &[MessageBox],
) -> GameState {
    // Strongest signals first: a leftover `l_charsel` layer with
    // `_display=true` can persist after login (observed in the dump
    // sample), so ShipUI/docked markers outrank it.
    let screen = if has_ship_ui {
        Screen::InSpace
    } else if has_station {
        Screen::Docked
    } else if tree
        .all_regioned()
        .any(|node| node.type_name() == "SmallCharacterSlot" || node.name() == Some("l_charsel"))
    {
        Screen::CharacterSelect
    } else {
        Screen::Unknown
    };
    GameState {
        screen,
        // The topmost modal is the first MessageBox (tree order).
        blocked_by_modal: message_boxes.first().map(|b| {
            b.caption.clone().unwrap_or_else(|| "modal".to_string())
        }),
    }
}

/// Per-flavor knowledge tables.
///
/// The baseline targets the 曙光 (Infinity) client — the only flavor
/// measured so far (2026-09). 经典服 (Serenity) and the international
/// client (Tranquility) reuse it until their differences are recorded;
/// when they diverge, override the relevant table in a dedicated profile
/// rather than branching inside extractors.
pub struct FlavorProfile {
    pub flavor: Flavor,
    /// Window-title texts of the object-name overview column.
    pub object_name_headers: &'static [&'static str],
    /// Window-title texts of the distance overview column.
    pub distance_headers: &'static [&'static str],
    /// Layer stack override, topmost first. `None` (default) trusts the
    /// live tree order — the client pre-sorts its root layers (observed
    /// 曙光 2026-09-12).
    pub layer_order_topmost_first: Option<&'static [&'static str]>,
}

impl FlavorProfile {
    pub fn for_flavor(flavor: Flavor) -> &'static FlavorProfile {
        // All flavors currently resolve to the 曙光 baseline (Chinese UI),
        // with English spellings kept as accepted aliases.
        static BASELINE: FlavorProfile = FlavorProfile {
            flavor: Flavor::Infinity,
            object_name_headers: &["名字", "Name", "名稱"],
            distance_headers: &["距离", "Distance", "距離"],
            layer_order_topmost_first: None,
        };
        let _ = flavor;
        &BASELINE
    }
}

/// The game window's client area (UIRoot's own display size) — the
/// coordinate space every region is expressed in.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ClientSize {
    pub width: i64,
    pub height: i64,
}

/// Top-level semantic extraction result.
#[derive(Clone, Debug, Serialize)]
pub struct UiSnapshot {
    pub flavor: String,
    pub node_count: usize,
    pub client_size: Option<ClientSize>,
    /// One glance: current screen and any game-wide input blocker.
    pub game_state: GameState,
    pub ship_ui: Option<ShipUi>,
    /// Present while a module button is hovered: module name + shortcut.
    pub module_button_tooltip: Option<ModuleButtonTooltip>,
    pub overview_windows: Vec<OverviewWindow>,
    pub context_menus: Vec<ContextMenu>,
    pub util_menus: Vec<UtilMenu>,
    pub inventory_windows: Vec<InventoryWindow>,
    pub message_boxes: Vec<MessageBox>,
    pub neocom: Option<Neocom>,
    pub info_panels: Option<InfoPanelContainerSummary>,
    pub selected_item_window: Option<SelectedItemWindow>,
    pub chat_window_stacks: Vec<ChatWindowStack>,
    /// Fitting window (in-flight fitting view) when open.
    pub fitting_window: Option<FittingWindow>,
    /// Station services panel when docked.
    pub station_window: Option<StationWindow>,
    /// Login character-select screen when present (slots with names
    /// and status lines).
    pub character_select: Option<character_select::CharacterSelect>,
    /// Every scrollable list with visibility and scroll predictions.
    pub scrollable_views: Vec<ScrollView>,
    pub layers: Vec<LayerSummary>,
    /// The generic-window safety net: everything with a `content` child
    /// that no specialized extractor claimed.
    pub other_windows: Vec<GenericWindow>,
    /// The universal click inventory: every input-taking element on
    /// screen (any UI state, including login/character selection).
    /// Tree order = topmost first.
    pub interaction_elements: Vec<interaction::InteractionElement>,
}
