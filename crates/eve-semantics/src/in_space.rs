//! In-space objects: all brackets on the space view (the agent's
//! "what do I see" root). Each bracket is a clickable celestial/NPC/
//! structure with a category icon, optional shadow label (name +
//! distance shown on hover), and the bracket's own item ID encoded in
//! the node name.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::strip_markup;
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// One in-space object bracket.
#[derive(Clone, Debug, Serialize)]
pub struct SpaceObject {
    /// Bracket kind: `celestial` (InSpaceBracket/MyShipBracket) or
    /// `anomaly` (AnomalyBracket) or `site` (StaticSiteBracket).
    pub kind: String,
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Item ID from the node name (`__inflightbracket_<itemID>`) —
    /// the same ID the overview carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<u64>,
    /// Category icon resource path (asteroidBelt.png / station.png /
    /// npcfrigate_16.png / ownShip.png / diamond2.png …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Semantic icon name (wordy stem from the resource path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_name: Option<String>,
    /// Shadow label text when the bracket has a visible label
    /// ("星空会实验室 1,461 km" style).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_text: Option<String>,
}

/// The collection of all in-space object brackets.
#[derive(Clone, Debug, Serialize)]
pub struct InSpaceObjects {
    pub objects: Vec<SpaceObject>,
}

/// All bracket types that represent in-space objects.
const BRACKET_TYPES: &[&str] = &[
    "InSpaceBracket",
    "MyShipBracket",
    "AnomalyBracket",
    "StaticSiteBracket",
];

fn bracket_kind(type_name: &str) -> &'static str {
    match type_name {
        "InSpaceBracket" | "MyShipBracket" => "celestial",
        "AnomalyBracket" => "anomaly",
        "StaticSiteBracket" => "site",
        _ => "celestial",
    }
}

/// Extract item ID from the bracket node name.
fn item_id_from(name: &str) -> Option<u64> {
    name.strip_prefix("__inflightbracket_")
        .or_else(|| name.strip_prefix("__inflight"))
        .and_then(|rest| rest.parse().ok())
}

/// The bracket's category icon: first Sprite child's texture, or the
/// innerIcon for sensor-suite brackets.
fn bracket_icon(tree: &RegionedTree<'_>, bracket: &RegionedNode<'_>) -> Option<String> {
    tree.descendants(bracket)
        .filter(|node| node.name() == Some("iconSprite") || node.name() == Some("innerIcon"))
        .next()
        .or_else(|| {
            tree.descendants(bracket)
                .find(|node| node.type_name() == "Sprite")
        })
        .and_then(|sprite| {
            let entries = sprite.entries();
            entries
                .get("_texturePath")
                .or_else(|| entries.get("texturePath"))
                .and_then(|v| match v {
                    eve_memory::NodeValue::Str(path) => Some(path.clone()),
                    _ => None,
                })
        })
}

/// The bracket's shadow label (hover text with name + distance).
fn bracket_label(tree: &RegionedTree<'_>, parent: &RegionedNode<'_>) -> Option<String> {
    tree.descendants(parent)
        .filter(|node| node.type_name() == "LabelCore" || node.type_name() == "Label")
        .filter_map(|node| node.text())
        .map(|text| strip_markup(&text))
        .find(|text| !text.is_empty())
}

/// Extract all in-space objects. The shadow labels live as siblings of
/// the brackets under `l_bracket` — associate each label to the nearest
/// bracket by region proximity.
pub fn extract_in_space(tree: &RegionedTree<'_>) -> InSpaceObjects {
    // Collect bracket-layer shadow labels first.
    let shadow_labels: Vec<(DisplayRegion, String)> = tree
        .find_by_type("BracketShadowLabel")
        .filter_map(|label| {
            let text = bracket_label(tree, label)?;
            Some((label.total_region, text))
        })
        .collect();

    let mut objects = Vec::new();
    for type_name in BRACKET_TYPES {
        for bracket in tree.find_by_type(type_name) {
            let name = bracket.name().unwrap_or("");
            let icon = bracket_icon(tree, bracket);
            let icon_name = icon
                .as_deref()
                .and_then(crate::icons::semantic_icon_name);
            // Associate the nearest shadow label (within 30px of the
            // bracket center).
            let (bcx, bcy) = bracket.total_region.center();
            let label_text = shadow_labels
                .iter()
                .find(|(region, _)| {
                    let (lcx, lcy) = region.center();
                    (lcx - bcx).abs() < 30 && (lcy - bcy).abs() < 30
                })
                .map(|(_, text)| text.clone());

            objects.push(SpaceObject {
                kind: bracket_kind(type_name).to_string(),
                region: bracket.total_region,
                interaction: interaction_info(tree, bracket),
                item_id: item_id_from(name),
                icon,
                icon_name,
                label_text,
            });
        }
    }
    InSpaceObjects { objects }
}
