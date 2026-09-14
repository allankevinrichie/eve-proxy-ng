//! Display regions: rectangles and the region-annotated tree every
//! extractor navigates.
//!
//! Ported from the reference semantic layer:
//!
//! - A node's own region needs **all four** `_displayX/_displayY/
//!   _displayWidth/_displayHeight`; the `_left/_top/_width/_height` family
//!   is a static fallback (used to rebuild e.g. notification entries).
//! - `_displayX/_displayY` are **parent-relative offsets** accumulated down
//!   the tree into `total_region`.
//! - A child without any region is invisible: its whole subtree disappears
//!   from `descendants()` iteration (use [`RegionedTree::all_nodes`] for
//!   raw access).
//!
//! Occlusion and clickability are NOT modeled here: see
//! [`crate::interaction`], which routes hits through the client's own
//! `_pickState` flag instead of guessing from geometry.

use eve_memory::{NodeValue, UiNode};

/// Integer rectangle in window-client coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
pub struct DisplayRegion {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl DisplayRegion {
    pub const EMPTY: DisplayRegion = DisplayRegion {
        x: -1,
        y: -1,
        width: 0,
        height: 0,
    };

    pub fn right(&self) -> i64 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(&self) -> i64 {
        self.y.saturating_add(self.height)
    }

    pub fn area(&self) -> i64 {
        // Saturate: corrupt snapshots can carry absurd `_display*` values.
        self.width.max(0).saturating_mul(self.height.max(0))
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn contains_point(&self, x: i64, y: i64) -> bool {
        self.x <= x && x < self.right() && self.y <= y && y < self.bottom()
    }

    pub fn center(&self) -> (i64, i64) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    fn intersect(&self, other: &DisplayRegion) -> Option<DisplayRegion> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            None
        } else {
            Some(DisplayRegion {
                x,
                y,
                width: right - x,
                height: bottom - y,
            })
        }
    }

    /// Largest rectangle of `self` remaining after removing `hole`
    /// (4-way split, overlaps allowed, deduplicated).
    pub fn subtract(&self, hole: &DisplayRegion) -> Vec<DisplayRegion> {
        let Some(intersection) = self.intersect(hole) else {
            return vec![*self];
        };
        if intersection == *self {
            return Vec::new();
        }
        let mut parts = Vec::with_capacity(4);
        if self.x < intersection.x {
            parts.push(DisplayRegion {
                x: self.x,
                y: self.y,
                width: intersection.x - self.x,
                height: self.height,
            });
        }
        if intersection.right() < self.right() {
            parts.push(DisplayRegion {
                x: intersection.right(),
                y: self.y,
                width: self.right() - intersection.right(),
                height: self.height,
            });
        }
        if self.y < intersection.y {
            parts.push(DisplayRegion {
                x: intersection.x,
                y: self.y,
                width: intersection.width,
                height: intersection.y - self.y,
            });
        }
        if intersection.bottom() < self.bottom() {
            parts.push(DisplayRegion {
                x: intersection.x,
                y: intersection.bottom(),
                width: intersection.width,
                height: self.bottom() - intersection.bottom(),
            });
        }
        parts.retain(|part| !part.is_empty());
        let mut unique: Vec<DisplayRegion> = Vec::with_capacity(parts.len());
        for part in parts {
            if !unique.contains(&part) {
                unique.push(part);
            }
        }
        unique
    }

    /// Largest-by-area rectangle left after subtracting every occluder.
    pub fn largest_visible_after(&self, occluders: &[DisplayRegion]) -> DisplayRegion {
        let mut candidates = vec![*self];
        for occluder in occluders {
            let mut next = Vec::new();
            for candidate in &candidates {
                next.extend(candidate.subtract(occluder));
            }
            candidates = next;
            if candidates.is_empty() {
                return DisplayRegion::EMPTY;
            }
        }
        candidates
            .into_iter()
            .max_by_key(|part| (part.area(), part.x, part.y))
            .unwrap_or(DisplayRegion::EMPTY)
    }
}

pub type Entries = std::collections::BTreeMap<String, NodeValue>;

/// Read a region from `_displayX/_displayY/_displayWidth/_displayHeight`.
pub fn region_from_display_entries(entries: &Entries) -> Option<DisplayRegion> {
    Some(DisplayRegion {
        x: read_i64(entries, "_displayX")?,
        y: read_i64(entries, "_displayY")?,
        width: read_i64(entries, "_displayWidth")?,
        height: read_i64(entries, "_displayHeight")?,
    })
}

/// Static fallback region from `_left/_top/_width/_height`.
pub fn region_from_left_top_entries(entries: &Entries) -> Option<DisplayRegion> {
    Some(DisplayRegion {
        x: read_i64(entries, "_left")?,
        y: read_i64(entries, "_top")?,
        width: read_i64(entries, "_width")?,
        height: read_i64(entries, "_height")?,
    })
}

/// The reference format may encode integers as numbers, wide-int objects
/// (`{int, int_low32}`), or digit strings. Client int objects often carry
/// garbage in the upper 32 bits — the meaningful value is the low 32 bits
/// (this is exactly why the reference JSON has the `int_low32` field; its
/// decoder prefers it too).
pub fn read_i64(entries: &Entries, key: &str) -> Option<i64> {
    match entries.get(key)? {
        NodeValue::Int(value) => Some(low_32_if_wide(*value)),
        NodeValue::WideInt { int_low32, .. } => Some(*int_low32 as i64),
        NodeValue::Str(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

pub fn read_f64(entries: &Entries, key: &str) -> Option<f64> {
    match entries.get(key)? {
        NodeValue::Float(value) => Some(*value),
        NodeValue::Int(value) => Some(low_32_if_wide(*value) as f64),
        NodeValue::WideInt { int_low32, .. } => Some(*int_low32 as f64),
        NodeValue::Str(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Values outside the 32-bit range are corrupt snapshots; the client keeps
/// the real number in the low 32 bits.
fn low_32_if_wide(value: i64) -> i64 {
    if (i32::MIN as i64..=i32::MAX as i64).contains(&value) {
        value
    } else {
        value as u32 as i32 as i64
    }
}

pub fn read_bool(entries: &Entries, key: &str) -> Option<bool> {
    match entries.get(key)? {
        NodeValue::Bool(value) => Some(*value),
        NodeValue::Int(value) => Some(*value != 0),
        _ => None,
    }
}

pub fn read_str<'a>(entries: &'a Entries, key: &str) -> Option<&'a str> {
    match entries.get(key)? {
        NodeValue::Str(text) => Some(text.as_str()),
        _ => None,
    }
}

/// Color with channels in integer percent.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct ColorPercent {
    pub a: Option<i64>,
    pub r: Option<i64>,
    pub g: Option<i64>,
    pub b: Option<i64>,
}

pub fn read_color(entries: &Entries, key: &str) -> Option<ColorPercent> {
    let channel = |value: &Option<i32>| value.map(|v| v as i64);
    match entries.get(key)? {
        NodeValue::Color { a, r, g, b } => Some(ColorPercent {
            a: channel(a),
            r: channel(r),
            g: channel(g),
            b: channel(b),
        }),
        _ => None,
    }
}

/// A node with precomputed regions; the unit of semantic navigation.
#[derive(Clone, Debug)]
pub struct RegionedNode<'a> {
    pub node: &'a UiNode,
    pub depth: u32,
    /// Region relative to the parent (display entries preferred, static
    /// `_left` family as fallback).
    pub region: DisplayRegion,
    /// Absolute region (parent total + own offsets).
    pub total_region: DisplayRegion,
    /// Position in [`RegionedTree::all_nodes`] (pre-order).
    pub index: usize,
    /// `_pickState` extracted once at build (hot path: read on every
    /// descend during hit-testing).
    pub pick_state: Option<i64>,
    /// `_opacity` extracted once at build.
    pub opacity: Option<f64>,
}

impl<'a> RegionedNode<'a> {
    pub fn type_name(&self) -> &str {
        &self.node.type_name
    }

    pub fn entries(&self) -> &Entries {
        &self.node.entries
    }

    pub fn name(&self) -> Option<&str> {
        read_str(self.entries(), "_name")
    }

    /// `_text`, or `_setText` when it is a plain string or a `Link` whose
    /// `_text` carries the text (observed 2024-05-26).
    pub fn text(&self) -> Option<String> {
        if let Some(text) = read_str(self.entries(), "_text") {
            return Some(text.to_string());
        }
        match self.entries().get("_setText") {
            Some(NodeValue::Str(text)) => Some(text.clone()),
            Some(NodeValue::Node(link)) => read_str(&link.entries, "_text").map(str::to_string),
            _ => None,
        }
    }

    pub fn hint(&self) -> Option<&str> {
        read_str(self.entries(), "_hint")
    }

    pub fn number(&self, key: &str) -> Option<i64> {
        read_i64(self.entries(), key)
    }

    pub fn float(&self, key: &str) -> Option<f64> {
        read_f64(self.entries(), key)
    }

    pub fn boolean(&self, key: &str) -> Option<bool> {
        read_bool(self.entries(), key)
    }

    pub fn color(&self, key: &str) -> Option<ColorPercent> {
        read_color(self.entries(), key)
    }

    /// Contained readable texts (own text first, then descendants' in tree
    /// order), for captions and combined readouts. Reader failure markers
    /// are dropped.
    pub fn contained_texts(&self, tree: &RegionedTree<'a>) -> Vec<String> {
        let mut texts = Vec::new();
        for descendant in tree.subtree_iter(self.index) {
            if let Some(text) = descendant.text() {
                if !text.is_empty() && !text.starts_with("Failed to read") {
                    texts.push(text);
                }
            }
        }
        texts
    }
}

/// Precomputed, indexed view of a UI tree with regions and occlusion.
///
/// Nodes are stored in pre-order, so every subtree is a contiguous slice.
pub struct RegionedTree<'a> {
    nodes: Vec<RegionedNode<'a>>,
    /// Children slice per node (`nodes[start..end]`), aligned with indices.
    child_ranges: Vec<(usize, usize)>,
    /// Direct-child indices per node (dense, no depth filtering at query
    /// time — the hot path of hit-test descends).
    child_indices: Vec<Box<[u32]>>,
    /// Indices of nodes visible to `descendants()` (own region present and
    /// the whole ancestor chain regioned).
    regioned_indices: Vec<usize>,
}

impl<'a> RegionedTree<'a> {
    /// Build from a raw tree. The root participates even when regionless
    /// (it gets an empty region, like the reference).
    pub fn build(root: &'a UiNode) -> RegionedTree<'a> {
        let mut nodes: Vec<RegionedNode<'a>> = Vec::new();
        let mut child_ranges = Vec::new();
        let mut regioned_indices = Vec::new();
        build_node(
            root,
            DisplayRegion {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            0,
            true,
            &mut nodes,
            &mut child_ranges,
            &mut regioned_indices,
        );
        regioned_indices.sort_unstable();
        // Dense direct-child lists from the ranges (kept for subtree_end).
        let mut child_indices: Vec<Box<[u32]>> = Vec::with_capacity(nodes.len());
        for (index, (start, end)) in child_ranges.iter().enumerate() {
            let child_depth = nodes[index].depth + 1;
            child_indices.push(
                nodes[*start..*end]
                    .iter()
                    .filter(|child| child.depth == child_depth)
                    .map(|child| child.index as u32)
                    .collect(),
            );
        }
        RegionedTree {
            nodes,
            child_ranges,
            child_indices,
            regioned_indices,
        }
    }

    pub fn root(&self) -> &RegionedNode<'a> {
        &self.nodes[0]
    }

    pub fn all_nodes(&self) -> &[RegionedNode<'a>] {
        &self.nodes
    }

    /// Direct children of `node`, in tree order — a dense index list,
    /// no per-query depth filtering.
    pub fn children_of<'s>(
        &'s self,
        node: &RegionedNode<'a>,
    ) -> impl DoubleEndedIterator<Item = &'s RegionedNode<'a>> {
        self.child_indices[node.index]
            .iter()
            .map(|&index| &self.nodes[index as usize])
    }

    /// End index (exclusive) of the pre-order subtree rooted at `index`.
    pub fn subtree_end(&self, index: usize) -> usize {
        let (start, end) = self.child_ranges[index];
        if start == end {
            index + 1
        } else {
            self.subtree_end(end - 1)
        }
    }

    /// Subtree of `node` in pre-order (including itself), regardless of
    /// region presence.
    pub fn subtree_iter(&self, index: usize) -> impl Iterator<Item = &RegionedNode<'a>> {
        let end = self.subtree_end(index);
        self.nodes[index..end].iter()
    }

    /// `node` plus its regioned descendants, in tree order — mirrors
    /// `listDescendantsWithDisplayRegion` (self included, regionless
    /// subtrees skipped).
    pub fn descendants(&self, node: &RegionedNode<'a>) -> impl Iterator<Item = &RegionedNode<'a>> {
        let start = node.index;
        let end = self.subtree_end(start);
        self.regioned_indices
            .iter()
            .filter(move |&&index| index >= start && index < end)
            .map(|&index| &self.nodes[index])
    }

    /// Every regioned node in the whole tree, tree order.
    pub fn all_regioned(&self) -> impl Iterator<Item = &RegionedNode<'a>> {
        self.regioned_indices.iter().map(|&index| &self.nodes[index])
    }

    pub fn find_by_type(&self, type_name: &str) -> impl Iterator<Item = &RegionedNode<'a>> {
        self.all_regioned()
            .filter(move |node| node.type_name() == type_name)
    }

    pub fn find_by_name(&self, name: &str) -> impl Iterator<Item = &RegionedNode<'a>> {
        self.all_regioned()
            .filter(move |node| node.name() == Some(name))
    }

    pub fn count_descendants(&self, node: &RegionedNode<'a>) -> usize {
        self.subtree_end(node.index) - node.index
    }
}

#[allow(clippy::too_many_arguments)]
fn build_node<'a>(
    node: &'a UiNode,
    parent_total: DisplayRegion,
    depth: u32,
    chain_visible: bool,
    nodes: &mut Vec<RegionedNode<'a>>,
    child_ranges: &mut Vec<(usize, usize)>,
    regioned_indices: &mut Vec<usize>,
) {
    let own_region = region_from_display_entries(&node.entries)
        .or_else(|| region_from_left_top_entries(&node.entries));
    // Root (`depth == 0`) participates unconditionally; deeper nodes need
    // their own region AND a fully regioned ancestor chain.
    let visible = chain_visible && (depth == 0 || own_region.is_some());
    let total_region = match own_region {
        Some(region) => DisplayRegion {
            x: parent_total.x.saturating_add(region.x),
            y: parent_total.y.saturating_add(region.y),
            width: region.width,
            height: region.height,
        },
        None if visible => parent_total,
        None => DisplayRegion::EMPTY,
    };
    let index = nodes.len();
    nodes.push(RegionedNode {
        node,
        depth,
        region: own_region.unwrap_or(DisplayRegion::EMPTY),
        total_region,
        index,
        pick_state: read_i64(&node.entries, "_pickState"),
        opacity: read_f64(&node.entries, "_opacity"),
    });
    child_ranges.push((0, 0)); // placeholder, keeps index alignment
    if visible {
        regioned_indices.push(index);
    }
    let child_start = nodes.len();
    for child in node.children.as_deref().unwrap_or(&[]) {
        build_node(child, total_region, depth + 1, visible, nodes, child_ranges, regioned_indices);
    }
    let child_end = nodes.len();
    child_ranges[index] = (child_start, child_end);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_memory::NodeValue;

    fn entries(pairs: &[(&str, NodeValue)]) -> Entries {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn node(type_name: &str, pairs: &[(&str, NodeValue)], children: Vec<UiNode>) -> UiNode {
        UiNode {
            address: eve_memory::Address(0),
            type_name: type_name.to_string(),
            entries: entries(pairs),
            other_entries_keys: None,
            children: Some(children),
        }
    }

    #[test]
    fn rectangle_subtraction_keeps_largest_part() {
        let whole = DisplayRegion { x: 0, y: 0, width: 100, height: 100 };
        let hole = DisplayRegion { x: 0, y: 0, width: 40, height: 100 };
        let visible = whole.largest_visible_after(&[hole]);
        assert_eq!(
            visible,
            DisplayRegion { x: 40, y: 0, width: 60, height: 100 }
        );
    }

    #[test]
    fn total_subtraction_yields_empty() {
        let whole = DisplayRegion { x: 0, y: 0, width: 10, height: 10 };
        let hole = DisplayRegion { x: -5, y: -5, width: 20, height: 20 };
        assert_eq!(whole.largest_visible_after(&[hole]), DisplayRegion::EMPTY);
    }

    #[test]
    fn wide_int_takes_low_32_bits() {
        // Real client data: upper 32 bits are garbage, low 32 carry the value.
        let map = entries(&[
            ("a", NodeValue::WideInt { int: 4_606_758_548_477_577_750, int_low32: 2582 }),
            ("b", NodeValue::Str("42".into())),
        ]);
        assert_eq!(read_i64(&map, "a"), Some(2582));
        assert_eq!(read_i64(&map, "b"), Some(42));
        assert_eq!(read_i64(&map, "c"), None);
    }

    #[test]
    fn regions_accumulate_and_prune_regionless_subtrees() {
        // root -> childA (region) -> grandA (region)
        //      -> childB (no region) -> grandB (region, must stay hidden)
        let grand_a = node(
            "Label",
            &[("_displayX", NodeValue::Int(10)), ("_displayY", NodeValue::Int(20)),
              ("_displayWidth", NodeValue::Int(5)), ("_displayHeight", NodeValue::Int(6))],
            vec![],
        );
        let grand_b = node(
            "Label",
            &[("_displayX", NodeValue::Int(1)), ("_displayY", NodeValue::Int(2)),
              ("_displayWidth", NodeValue::Int(3)), ("_displayHeight", NodeValue::Int(4))],
            vec![],
        );
        let child_a = node(
            "Container",
            &[("_displayX", NodeValue::Int(100)), ("_displayY", NodeValue::Int(50)),
              ("_displayWidth", NodeValue::Int(30)), ("_displayHeight", NodeValue::Int(30))],
            vec![grand_a],
        );
        let child_b = node("Container", &[("_name", NodeValue::Str("no region".into()))], vec![grand_b]);
        let root = node("UIRoot", &[], vec![child_a, child_b]);

        let tree = RegionedTree::build(&root);
        assert_eq!(tree.all_nodes().len(), 5);
        let visible: Vec<&str> = tree.all_regioned().map(|n| n.type_name()).collect();
        assert_eq!(visible, vec!["UIRoot", "Container", "Label"]);

        // grandA total = root(0,0) + childA(100,50) + grandA(10,20)
        let grand_a_view = tree.all_regioned().nth(2).unwrap();
        assert_eq!(grand_a_view.total_region, DisplayRegion { x: 110, y: 70, width: 5, height: 6 });
    }

}
