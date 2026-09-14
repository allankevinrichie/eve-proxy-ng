//! Static module icon mapping: icon resource path → module display
//! name, and typeID → exact localized name.
//!
//! The HUD shows module buttons as icons only. The icon fingerprint is
//! game-wide data — the same icon means the same module type on every
//! ship — so a table lookup resolves it deterministically, with no
//! runtime state. Everything is served by [`crate::resources`]:
//! client-derived baseline (中文名/国服独有), optional user overlay,
//! plus live-verified manual overrides on top. When the HUD shows an
//! unmapped icon, identify it once via the fitting window (slot labels
//! carry names) or a hover tooltip, then add the pair to the overrides
//! in [`crate::resources`].

/// Resolve a module display name from its icon resource path.
pub fn module_name_from_icon(path: &str) -> Option<&'static str> {
    crate::resources::ResourceStore::global().module_name(path)
}

/// Resolve the exact localized type name from a typeID
/// (`ModuleButton_<typeID>` node names carry it).
pub fn type_name_from_type_id(type_id: u64) -> Option<&'static str> {
    crate::resources::ResourceStore::global().type_name(type_id)
}

/// The icon resource of a typeID, when the tables record one.
pub fn icon_from_type_id(type_id: u64) -> Option<&'static str> {
    crate::resources::ResourceStore::global().type_icon(type_id)
}

/// Parse the `ModuleButton_<typeID>` naming convention; returns the
/// typeID when the node name carries one.
pub fn type_id_from_node_name(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("ModuleButton_")?;
    rest.parse::<u64>().ok()
}

/// All known icon entries (baseline/overlay plus manual overrides) —
/// consumers use this to export or iterate the full mapping.
pub fn all_icon_entries() -> Vec<(&'static str, &'static str)> {
    crate::resources::ResourceStore::global().icon_entries()
}

#[cfg(test)]
mod tests {
    #[test]
    fn semantic_icon_name_channels() {
        // Family table: 中文 type-family name.
        assert_eq!(
            super::semantic_icon_name("res:/UI/Texture/Icons/6_64_14.png").as_deref(),
            Some("三钛合金")
        );
        // Bracket / chrome stems, lowercased, size suffix trimmed.
        assert_eq!(
            super::semantic_icon_name("res:/UI/Texture/Shared/Brackets/stargate.png").as_deref(),
            Some("stargate")
        );
        assert_eq!(
            super::semantic_icon_name(
                "res:/UI/Texture/eveicon/bracket_icons/skyhook_bracket_16px.png"
            )
            .as_deref(),
            Some("skyhook_bracket")
        );
        // Purely numeric stems carry no readable name.
        assert_eq!(
            super::semantic_icon_name("res:/UI/Texture/Icons/99_99_99.png"),
            None
        );
    }
}

/// Shallowest `res:/` texture below a node — the icon the element
/// visually presents (buttons carry their icon on themselves or an
/// immediate sprite child). Bounded BFS: depth ≤ 4, ≤ 64 nodes, so
/// container-sized elements stay cheap.
pub fn element_icon(
    tree: &crate::region::RegionedTree<'_>,
    node: &crate::region::RegionedNode<'_>,
) -> Option<String> {
    fn texture_of(node: &crate::region::RegionedNode<'_>) -> Option<String> {
        let entries = node.entries();
        let value = entries.get("_texturePath").or_else(|| entries.get("texturePath"))?;
        match value {
            eve_memory::NodeValue::Str(path) if path.starts_with("res:") => Some(path.clone()),
            _ => None,
        }
    }
    if let Some(path) = texture_of(node) {
        return Some(path);
    }
    let mut level: Vec<&crate::region::RegionedNode<'_>> = tree.children_of(node).collect();
    let mut budget = 64usize;
    for _ in 0..4 {
        if level.is_empty() || budget == 0 {
            break;
        }
        let mut next = Vec::new();
        for child in level {
            if budget == 0 {
                break;
            }
            budget -= 1;
            if let Some(path) = texture_of(child) {
                return Some(path);
            }
            next.extend(tree.children_of(child));
        }
        level = next;
    }
    None
}

/// Semantic name for an icon resource path: the type family table
/// first, then the wordy basename stem for bracket/chrome icons
/// (`…/Brackets/stargate.png` → `stargate`), else None
/// (`73_16_50`-style numeric names carry no readable stem).
pub fn semantic_icon_name(path: &str) -> Option<String> {
    if let Some(name) = crate::resources::ResourceStore::global().module_name(path) {
        return Some(name.to_string());
    }
    let stem = path.rsplit('/').next()?;
    let stem = stem
        .strip_suffix(".png")
        .or_else(|| stem.strip_suffix(".dds"))
        .unwrap_or(stem);
    // Drop one trailing size segment (`_16px`, `_64`): `skyhook_bracket_16px`
    // → `skyhook_bracket`; numeric-only stems (`73_16_50`) stay unreadable.
    let mut trimmed = stem;
    if let Some(pos) = trimmed.rfind('_') {
        let tail = trimmed[pos + 1..].trim_end_matches("px");
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            trimmed = &trimmed[..pos];
        }
    }
    let wordy = trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3;
    wordy.then(|| trimmed.to_ascii_lowercase())
}
