//! Interaction state: is an element clickable, and what covers it?
//!
//! Two mechanisms, both grounded in client-maintained signals rather than
//! guessed geometry:
//!
//! 1. **Hit routing** ([`hit_test`]): the root's layer children come
//!    pre-sorted topmost-first by the client (observed in the 曙光 sample
//!    2026-09-12: `l_hint` → `l_dragging` → `l_menu` → … → `l_main` →
//!    `l_viewstate`), and the same tree order is used within containers.
//!    Descent steers by the client's own hit-testing flag `_pickState`:
//!    `0` = subtree takes no input (the click falls through),
//!    `1` = takes input,
//!    `2` = passes input to children.
//!    The deepest `_pickState == 1` node containing the point is the hit.
//!    A flavor can override the layer order via
//!    [`crate::FlavorProfile::layer_order_topmost_first`].
//!
//! 2. **Sampled occlusion** ([`interaction_info`]): probe an element's
//!    region on a 3×3 grid; every probe whose hit lands outside the
//!    element's subtree names an occluder. This yields honest
//!    `occluded_percent` / `occluded_by` values and an `is_interactable`
//!    verdict for the element's center.
//!
//! Caveat kept visible on purpose: child order is a convention, not a
//! guarantee (the reference project hardcoded observed occluder types for
//! exactly this reason). `occluded_by` always reports *which* node won the
//! probe, so a wrong verdict is inspectable.

use serde::Serialize;

use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// Layers, topmost first — OBSERVED tree order of the 曙光 sample's root
/// children (2026-09-12), kept for reference. The live tree order is used
/// by default; a flavor overrides via
/// [`crate::FlavorProfile::layer_order_topmost_first`] only when a client
/// stops pre-sorting its layers.
pub const LAYER_ORDER_TOPMOST_FIRST: &[&str] = &[
    "l_hint",
    "l_dragging",
    "l_menu",
    "l_mloading",
    "l_modal",
    "l_utilmenu",
    "l_alwaysvisible",
    "l_abovemain",
    "l_videoOverlay",
    "l_infoBubble",
    "l_main",
    "l_viewstate",
];

/// Probe grid per axis for occlusion sampling.
const PROBES_PER_AXIS: usize = 3;

/// Client hit-testing flag values (observed 2026-07-27).
pub const PICK_STATE_DEAD: i64 = 0;
pub const PICK_STATE_TAKES: i64 = 1;
pub const PICK_STATE_PASSES: i64 = 2;

impl RegionedNode<'_> {
    /// `_pickState`: 0 dead, 1 takes input, 2 passes to children.
    /// (Precomputed at tree build.)
    pub fn pick_state(&self) -> Option<i64> {
        self.pick_state
    }

    /// `_opacity` when present; values below ~0.5 mean the client hides
    /// the element (notification badges, settings button).
    pub fn is_opacity_hidden(&self) -> bool {
        self.opacity.is_some_and(|opacity| opacity < 0.5)
    }
}

/// The node a click at a point would reach.
#[derive(Clone, Debug)]
pub struct HitTarget<'a> {
    pub node: &'a RegionedNode<'a>,
}

/// Where a click at `(x, y)` lands, topmost routing first.
/// `None` means no input-taking node covers the point.
///
/// `layer_order` overrides the root layer order; `None` trusts the tree
/// order (topmost first, as the client maintains it).
pub fn hit_test<'a>(
    tree: &'a RegionedTree<'a>,
    x: i64,
    y: i64,
    layer_order: Option<&[&str]>,
) -> Option<HitTarget<'a>> {
    // Hot path (occlusion probing calls this thousands of times per
    // snapshot): the live tree order needs no layer sort, so iterate the
    // root's children directly without collecting.
    match layer_order {
        None => {
            for layer in tree.children_of(tree.root()) {
                if !layer.total_region.contains_point(x, y) {
                    continue;
                }
                if let Some(hit) = descend(tree, layer, x, y, None) {
                    return Some(hit);
                }
            }
            None
        }
        Some(order) => {
            for layer in layers_topmost_first(tree, Some(order)) {
                if !layer.total_region.contains_point(x, y) {
                    continue;
                }
                if let Some(hit) = descend(tree, layer, x, y, None) {
                    return Some(hit);
                }
            }
            None
        }
    }
}

/// Scroll viewports clip their scrollable content: an entry's region is
/// laid out in content coordinates and extends beyond the visible window,
/// but only the part inside the viewport is clickable.
fn is_scroll_clipper(node: &RegionedNode<'_>) -> bool {
    matches!(
        node.type_name(),
        "Scroll" | "ScrollContainer" | "BasicDynamicScroll" | "NotificationScrollContainer"
    ) || node.name() == Some("__clipper")
}

fn descend<'a>(
    tree: &'a RegionedTree<'a>,
    node: &'a RegionedNode<'a>,
    x: i64,
    y: i64,
    clip: Option<DisplayRegion>,
) -> Option<HitTarget<'a>> {
    let clip = if is_scroll_clipper(node) {
        Some(node.total_region)
    } else {
        clip
    };
    match node.pick_state() {
        // Dead subtree: takes no input and disables its children; the
        // click falls through to siblings and lower layers.
        Some(PICK_STATE_DEAD) => return None,
        _ => {}
    }
    // Tree order is topmost first: earlier children win.
    for child in tree.children_of(node) {
        let inside_clip = clip.is_none_or(|region| region.contains_point(x, y));
        if inside_clip && child.total_region.contains_point(x, y) {
            if let Some(hit) = descend(tree, child, x, y, clip) {
                return Some(hit);
            }
        }
    }
    match node.pick_state() {
        Some(PICK_STATE_TAKES) => Some(HitTarget { node }),
        // Pass-through (2) or unknown: the click continues below.
        _ => None,
    }
}

/// Root's layer children, topmost first: the live tree order unless a
/// flavor overrides it (then by table rank, unknown layers last).
fn layers_topmost_first<'a>(
    tree: &'a RegionedTree<'a>,
    layer_order: Option<&[&str]>,
) -> Vec<&'a RegionedNode<'a>> {
    let children: Vec<&RegionedNode<'a>> = tree.children_of(tree.root()).collect();
    match layer_order {
        None => children,
        Some(order) => {
            let mut layers: Vec<(usize, usize, &RegionedNode<'a>)> = children
                .iter()
                .enumerate()
                .map(|(index, layer)| {
                    let rank = layer
                        .name()
                        .and_then(|name| order.iter().position(|entry| *entry == name))
                        .unwrap_or(order.len());
                    (rank, index, *layer)
                })
                .collect();
            layers.sort_by_key(|&(rank, index, _)| (rank, index));
            layers.into_iter().map(|(_, _, layer)| layer).collect()
        }
    }
}

/// One covering node, as reported to agents.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OccluderRef {
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub address: String,
    pub region: DisplayRegion,
    /// The occluder's `_pickState`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick_state: Option<i64>,
}

impl<'a> From<&'a RegionedNode<'a>> for OccluderRef {
    fn from(node: &'a RegionedNode<'a>) -> OccluderRef {
        OccluderRef {
            type_name: node.type_name().to_string(),
            name: node.name().map(str::to_string),
            address: node.node.address.0.to_string(),
            region: node.total_region,
            pick_state: node.pick_state(),
        }
    }
}

/// Interaction verdict for one element; embed into snapshot structs with
/// `#[serde(flatten)]`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct InteractionInfo {
    /// A click on the element's center reaches the element's subtree, the
    /// element is not a dead zone, and it is not opacity-hidden.
    pub is_interactable: bool,
    /// Percent of probed points whose hit landed outside the subtree.
    pub occluded_percent: i64,
    /// Distinct winners of the occluded probes, deepest first.
    pub occluded_by: Vec<OccluderRef>,
    /// The element's own `_pickState`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick_state: Option<i64>,
    /// The element's own `_opacity`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
}

/// Compute the interaction verdict for `node` by probing its region.
pub fn interaction_info(tree: &RegionedTree<'_>, node: &RegionedNode<'_>) -> InteractionInfo {
    let layer_order = crate::FlavorProfile::for_flavor(crate::Flavor::Infinity)
        .layer_order_topmost_first;
    interaction_info_with_order(tree, node, layer_order)
}

pub fn interaction_info_with_order(
    tree: &RegionedTree<'_>,
    node: &RegionedNode<'_>,
    layer_order: Option<&[&str]>,
) -> InteractionInfo {
    let mut info = InteractionInfo {
        pick_state: node.pick_state,
        opacity: node.opacity,
        ..InteractionInfo::default()
    };
    let region = node.total_region;
    if region.is_empty() {
        return info;
    }
    let center = region.center();
    let center_reaches = hit_test(tree, center.0, center.1, layer_order)
        .is_some_and(|hit| subtree_contains(tree, node, hit.node));
    let mut occluders: Vec<OccluderRef> = Vec::new();
    let mut occluded_probes = 0usize;
    let mut probes = 0usize;
    for probe in probe_points(&region) {
        probes += 1;
        match hit_test(tree, probe.0, probe.1, layer_order) {
            // The probe reaches the node's subtree — not occluded.
            Some(hit) if subtree_contains(tree, node, hit.node) => {}
            // Routing stopped on an ANCESTOR of the node (a pick-routing
            // container above it): the probe still lands inside the
            // node's own visual area, so this is coverage by the node's
            // own chain, not by a foreign node in front. Ancestors are
            // never occluders.
            Some(hit) if subtree_contains(tree, hit.node, node) => {}
            Some(hit) => {
                occluded_probes += 1;
                let winner = OccluderRef::from(hit.node);
                if !occluders.contains(&winner) {
                    occluders.push(winner);
                }
            }
            // No input-taking node at the probe (dead space or empty).
            None => occluded_probes += 1,
        }
    }
    info.occluded_percent = if probes == 0 {
        0
    } else {
        (occluded_probes as i64 * 100) / probes as i64
    };
    info.occluded_by = occluders;
    info.is_interactable = center_reaches
        && node.pick_state() != Some(PICK_STATE_DEAD)
        && !node.is_opacity_hidden();
    info
}

/// 3×3 grid of points inside the region (single center when degenerate).
fn probe_points(region: &DisplayRegion) -> Vec<(i64, i64)> {
    if region.width <= PROBES_PER_AXIS as i64 || region.height <= PROBES_PER_AXIS as i64 {
        return vec![region.center()];
    }
    let mut points = Vec::with_capacity(PROBES_PER_AXIS * PROBES_PER_AXIS);
    for row in 0..PROBES_PER_AXIS {
        for column in 0..PROBES_PER_AXIS {
            let x = region.x + region.width * (2 * column as i64 + 1) / (2 * PROBES_PER_AXIS as i64);
            let y = region.y + region.height * (2 * row as i64 + 1) / (2 * PROBES_PER_AXIS as i64);
            points.push((x, y));
        }
    }
    points
}

/// Is `node` an ancestor-or-self of `other` in the pre-order layout?
pub fn subtree_contains(tree: &RegionedTree<'_>, node: &RegionedNode<'_>, other: &RegionedNode<'_>) -> bool {
    other.index >= node.index && other.index < tree.subtree_end(node.index)
}

/// Cap for [`extract_interaction_elements`] (dense in-space trees carry
/// hundreds of brackets and icons; UI trees stay far below this).
const MAX_INTERACTION_ELEMENTS: usize = 2000;

/// A discrete input-taking element — the generic inventory of "things you
/// can click", available on ANY screen (in-space, station, character
/// selection, login), including UI states no specialized extractor
/// covers.
#[derive(Clone, Debug, Serialize)]
pub struct InteractionElement {
    pub type_name: String,
    /// Decimal node address — the stable identity used for frame-to-frame
    /// keyed diffs in the recorder.
    pub address: String,
    /// Address of the enclosing top-level window (`other_windows[].address`)
    /// — the container this element visually belongs to. None for HUD /
    /// layer-level elements outside any window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Semantic role from the curated table (`hud.open_cargo`,
    /// `station.undock`, ...); `None` for unmapped types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// First contained text (markup stripped), for identification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Icon resource path the element presents (`res:/…`), when its
    /// shallow subtree carries one (HUD/neocom buttons, menus…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Semantic name of that icon — type family name (中文名) from the
    /// resource tables, or the wordy bracket/chrome stem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_name: Option<String>,
    /// The human-readable identification string — what a person would
    /// call this control. Priority: `text` > `role` > `hint` > `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub region: DisplayRegion,
    /// The region actually visible on screen: the raw region clipped
    /// by enclosing scroll viewports / window frames (virtualized
    /// strips lay cells out far beyond the frame). Some only when it
    /// differs from `region`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_region: Option<DisplayRegion>,
    /// False when the element lies entirely outside the client area
    /// (off-screen cells of virtualized lists).
    pub is_on_screen: bool,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
}

/// Every `_pickState == 1` element with its own hit area, excluding
/// screen-sized containers (layers, backgrounds, overlays — their children
/// carry the real targets). Tree order = topmost first, so overlapping
/// elements appear in click precedence order.
///
/// Element computation is embarrassingly parallel (per-element occlusion
/// probing over the shared read-only tree) and runs under rayon; the
/// indexed collect keeps the tree (z-) order of the output.
pub fn extract_interaction_elements(tree: &RegionedTree<'_>) -> Vec<InteractionElement> {
    use rayon::prelude::*;
    let root = tree.root();
    let root_area = root.total_region.area().max(1);
    // The client area = the root's OWN rect (UIRoot display size) —
    // virtualized strips lay cells out far beyond it.
    let client = crate::region::DisplayRegion {
        x: 0,
        y: 0,
        width: root.region.width,
        height: root.region.height,
    };
    // Parent index map (pre-order): one O(n) pass for cheap ancestor
    // walks during clip computation.
    let mut parent = vec![0usize; tree.all_nodes().len()];
    for node in tree.all_nodes() {
        for child in tree.children_of(node) {
            parent[child.index] = node.index;
        }
    }
    let candidates: Vec<&crate::region::RegionedNode<'_>> = tree
        .all_regioned()
        .filter(|node| node.depth >= 1)
        .filter(|node| node.pick_state() == Some(PICK_STATE_TAKES))
        .filter(|node| node.total_region.area() * 2 < root_area)
        .take(MAX_INTERACTION_ELEMENTS)
        .collect();
    candidates
        .into_par_iter()
        .map(|node| {
            let text = node
                .text()
                .or_else(|| {
                    tree.subtree_iter(node.index)
                        .skip(1)
                        .filter(|child| child.depth <= node.depth + 3)
                        .find_map(|child| child.text())
                })
                .map(|text| crate::parsing::strip_markup(&text))
                .filter(|text| !text.is_empty());
            let hint = node.hint().map(str::to_string);
            let icon = crate::icons::element_icon(tree, node);
            let icon_name = icon
                .as_deref()
                .and_then(crate::icons::semantic_icon_name);
            let role = crate::roles::semantic_role(
                node.type_name(),
                node.name(),
                text.as_deref().or(hint.as_deref()),
            );
            // The human-readable identification string, priority
            // text > role > hint > name (hint is user-facing wording,
            // beats dev node names like CloseButtonIcon).
            let label = text
                .clone()
                .or_else(|| role.clone().map(String::from))
                .or_else(|| hint.clone())
                .or_else(|| node.name().map(str::to_string));
            let (visible_region, is_on_screen) = visible_geometry(tree, &parent, node, client);
            InteractionElement {
                type_name: node.type_name().to_string(),
                address: node.node.address.0.to_string(),
                window_address: None,
                name: node.name().map(str::to_string),
                role,
                label,
                text,
                hint,
                icon,
                icon_name,
                region: node.total_region,
                visible_region,
                is_on_screen,
                interaction: interaction_info(tree, node),
            }
        })
        .collect()
}

/// An ancestor that clips its subtree's rendering: scroll viewports
/// and window-style containers (in-game, window content is cut to the
/// window frame).
fn is_ancestor_clipper(node: &RegionedNode<'_>) -> bool {
    is_scroll_clipper(node) || {
        let t = node.type_name();
        t.contains("Wnd") || t.contains("Window")
    }
}

/// A node's own absolute rect: the total region's origin (totals begin
/// with the own rect) with the own region's extents.
fn own_abs(node: &RegionedNode<'_>) -> crate::region::DisplayRegion {
    crate::region::DisplayRegion {
        x: node.total_region.x,
        y: node.total_region.y,
        width: node.region.width.max(0),
        height: node.region.height.max(0),
    }
}

/// `(visible_region, is_on_screen)`: the element's region intersected
/// with every enclosing clipper's own rect, and with the client area.
/// `visible_region` is Some only when that differs from the raw region.
fn visible_geometry(
    tree: &RegionedTree<'_>,
    parent: &[usize],
    node: &RegionedNode<'_>,
    client: crate::region::DisplayRegion,
) -> (Option<crate::region::DisplayRegion>, bool) {
    let raw = node.total_region;
    let mut visible = raw;
    let mut clipped = false;
    let mut index = node.index;
    while index != 0 {
        index = parent[index];
        let ancestor = &tree.all_nodes()[index];
        if !ancestor.region.is_empty() && is_ancestor_clipper(ancestor) {
            match visible.intersect(&own_abs(ancestor)) {
                Some(inter) => {
                    visible = inter;
                    clipped = true;
                }
                // Entirely outside an enclosing viewport/window frame:
                // not rendered by the game at all.
                None => return (None, false),
            }
        }
    }
    let on_screen = visible
        .intersect(&client)
        .is_some_and(|r| r.width > 0 && r.height > 0);
    (clipped.then_some(visible).filter(|v| *v != raw), on_screen)
}

/// Serializable form of [`HitTarget`] for CLI/Python.
#[derive(Clone, Debug, Serialize)]
pub struct HitTestReport {
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub address: String,
    pub region: DisplayRegion,
    pub pick_state: Option<i64>,
    /// Contained text, for identifying the hit control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl<'a> From<HitTarget<'a>> for HitTestReport {
    fn from(hit: HitTarget<'a>) -> HitTestReport {
        HitTestReport {
            type_name: hit.node.type_name().to_string(),
            name: hit.node.name().map(str::to_string),
            address: hit.node.node.address.0.to_string(),
            region: hit.node.total_region,
            pick_state: hit.node.pick_state(),
            text: hit.node.text(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::RegionedTree;
    use eve_memory::{Address, NodeValue, UiNode};
    use std::collections::BTreeMap;

    fn entries(pairs: &[(&str, NodeValue)]) -> BTreeMap<String, NodeValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn region_entries(x: i64, y: i64, w: i64, h: i64, pick: Option<i64>) -> BTreeMap<String, NodeValue> {
        let mut map = entries(&[
            ("_displayX", NodeValue::Int(x)),
            ("_displayY", NodeValue::Int(y)),
            ("_displayWidth", NodeValue::Int(w)),
            ("_displayHeight", NodeValue::Int(h)),
        ]);
        if let Some(pick) = pick {
            map.insert("_pickState".to_string(), NodeValue::Int(pick));
        }
        map
    }

    fn node(type_name: &str, map: BTreeMap<String, NodeValue>, children: Vec<UiNode>) -> UiNode {
        UiNode {
            address: Address(0),
            type_name: type_name.to_string(),
            entries: map,
            other_entries_keys: None,
            children: Some(children),
        }
    }

    fn layer(name: &str, children: Vec<UiNode>) -> UiNode {
        let mut map = region_entries(0, 0, 2000, 2000, Some(PICK_STATE_PASSES));
        map.insert("_name".to_string(), NodeValue::Str(name.into()));
        node("LayerCore", map, children)
    }

    #[test]
    fn modal_layer_occludes_main_window_button() {
        // Tree order is topmost first: the modal layer comes first.
        // l_main holds a pass-through container with a button.
        let button = node(
            "ModuleButton",
            region_entries(100, 100, 40, 20, Some(PICK_STATE_TAKES)),
            vec![],
        );
        let container = node(
            "Container",
            region_entries(0, 0, 400, 400, Some(PICK_STATE_PASSES)),
            vec![button],
        );
        // l_modal holds a panel overlapping the button.
        let panel = node(
            "Panel",
            region_entries(90, 90, 200, 200, Some(PICK_STATE_TAKES)),
            vec![],
        );
        let root = node(
            "UIRoot",
            BTreeMap::new(),
            vec![
                layer("l_modal", vec![panel]),
                layer("l_main", vec![container]),
            ],
        );

        let tree = RegionedTree::build(&root);

        // Click on the button's center lands on the modal panel instead.
        let hit = hit_test(&tree, 120, 110, None).expect("hit");
        assert_eq!(hit.node.type_name(), "Panel");

        // The button therefore reports itself occluded and not operable.
        let button_view = tree.find_by_type("ModuleButton").next().unwrap();
        let info = interaction_info_with_order(&tree, button_view, None);
        assert!(!info.is_interactable);
        assert_eq!(info.occluded_percent, 100);
        assert_eq!(info.occluded_by.len(), 1);
        assert_eq!(info.occluded_by[0].type_name, "Panel");

        // A click away from the panel only reaches the pass-through
        // container: no input-taking node.
        assert!(hit_test(&tree, 300, 300, None).is_none());
    }

    #[test]
    fn dead_zone_disables_subtree_but_falls_through() {
        let button = node(
            "Button",
            region_entries(10, 10, 30, 30, Some(PICK_STATE_TAKES)),
            vec![],
        );
        let dead = node(
            "Container",
            region_entries(0, 0, 100, 100, Some(PICK_STATE_DEAD)),
            vec![button],
        );
        // A live sibling below the dead container still receives clicks.
        let live = node(
            "Button",
            region_entries(50, 50, 30, 30, Some(PICK_STATE_TAKES)),
            vec![],
        );
        let root = node(
            "UIRoot",
            BTreeMap::new(),
            vec![layer("l_main", vec![dead, live])],
        );
        let tree = RegionedTree::build(&root);

        // The dead subtree yields no hit; the overlapping live button
        // (earlier sibling, drawn on top anyway) takes the click.
        let hit = hit_test(&tree, 60, 60, None).expect("hit");
        assert_eq!(hit.node.type_name(), "Button");

        // A point only covered by the dead container hits nothing.
        assert!(hit_test(&tree, 5, 5, None).is_none());

        // The dead button is not operable.
        let dead_button = tree
            .all_regioned()
            .find(|n| n.type_name() == "Button" && n.total_region.x == 10)
            .unwrap();
        let info = interaction_info_with_order(&tree, dead_button, None);
        assert!(!info.is_interactable);
    }

    #[test]
    fn scroll_viewport_clips_content() {
        // A 100px viewport over 300px of content: the button at content
        // y 250..280 is outside the clip and must not receive clicks,
        // while the one at y 10..40 stays clickable.
        let far_button = node(
            "Button",
            region_entries(10, 250, 30, 30, Some(PICK_STATE_TAKES)),
            vec![],
        );
        let near_button = node(
            "Button",
            region_entries(10, 10, 30, 30, Some(PICK_STATE_TAKES)),
            vec![],
        );
        let mut scroll_map = region_entries(0, 0, 100, 100, Some(PICK_STATE_PASSES));
        scroll_map.insert("_name".to_string(), NodeValue::Str("scroll".into()));
        let scroll = node("BasicDynamicScroll", scroll_map, vec![far_button, near_button]);
        let root = node("UIRoot", BTreeMap::new(), vec![layer("l_main", vec![scroll])]);
        let tree = RegionedTree::build(&root);

        // In-viewport button: clickable.
        let hit = hit_test(&tree, 25, 25, None).expect("hit");
        assert_eq!(hit.node.total_region.y, 10);

        // Off-viewport button: the click falls through (nothing there).
        assert!(hit_test(&tree, 25, 265, None).is_none());

        // Its probes report occlusion, and the verdict is not operable.
        let far_view = tree
            .all_regioned()
            .find(|n| n.type_name() == "Button" && n.total_region.y == 250)
            .unwrap();
        let info = interaction_info_with_order(&tree, far_view, None);
        assert!(!info.is_interactable);
        assert_eq!(info.occluded_percent, 100);
    }

    #[test]
    fn unobstructed_button_is_interactable() {
        let button = node(
            "Button",
            region_entries(10, 10, 30, 30, Some(PICK_STATE_TAKES)),
            vec![],
        );
        let root = node("UIRoot", BTreeMap::new(), vec![layer("l_main", vec![button])]);
        let tree = RegionedTree::build(&root);
        let button_view = tree.find_by_type("Button").next().unwrap();
        let info = interaction_info_with_order(&tree, button_view, None);
        assert!(info.is_interactable);
        assert_eq!(info.occluded_percent, 0);
        assert!(info.occluded_by.is_empty());
    }

    #[test]
    fn opacity_hidden_element_not_interactable() {
        let mut map = region_entries(10, 10, 30, 30, Some(PICK_STATE_TAKES));
        map.insert("_opacity".to_string(), NodeValue::Float(0.0));
        let button = node("Button", map, vec![]);
        let root = node("UIRoot", BTreeMap::new(), vec![layer("l_main", vec![button])]);
        let tree = RegionedTree::build(&root);
        let button_view = tree.find_by_type("Button").next().unwrap();
        let info = interaction_info_with_order(&tree, button_view, None);
        assert!(!info.is_interactable);
        assert_eq!(info.opacity, Some(0.0));
    }
}
