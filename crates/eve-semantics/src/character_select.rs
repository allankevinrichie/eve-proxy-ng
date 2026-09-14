//! Character-select screen: the slots shown at login, with each
//! character's name and status lines (skill training / SP / ISK /
//! mail / security / location / ship), all carried by EveLabel
//! `_setText` values under `SmallCharacterSlot characterSlot_<N>`.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::strip_markup;
use crate::region::{DisplayRegion, RegionedTree};

#[derive(Clone, Debug, Serialize)]
pub struct CharacterSlot {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Slot number from the node name (`characterSlot_0` → 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    /// Character name (the big bold label on the card).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Status lines under the name, in display order (skill training,
    /// SP, ISK, unread mail, security status, location, ship …).
    pub details: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CharacterSelect {
    pub region: DisplayRegion,
    pub slots: Vec<CharacterSlot>,
}

/// Extract the character-select screen when present (`l_charsel` /
/// `CharacterSelection`). `None` on every other screen.
pub fn extract_character_select(tree: &RegionedTree<'_>) -> Option<CharacterSelect> {
    let screen = tree
        .all_regioned()
        .find(|node| node.type_name() == "CharacterSelection")?;
    let mut slots = Vec::new();
    for slot in tree
        .subtree_iter(screen.index)
        .skip(1)
        .filter(|node| node.type_name() == "SmallCharacterSlot")
    {
        let index = slot
            .name()
            .and_then(|name| name.rsplit('_').next())
            .and_then(|tail| tail.parse().ok());
        // Labels under the slot: the first EveLabelLargeBold is the
        // character name; every other label text is a status line.
        let mut name = None;
        let mut details = Vec::new();
        for label in tree.subtree_iter(slot.index).skip(1) {
            let Some(text) = label.text() else { continue };
            let text = strip_markup(&text);
            if text.is_empty() {
                continue;
            }
            if name.is_none()
                && (label.type_name() == "EveLabelLargeBold"
                    || label.name() == Some("characterNameLabel"))
            {
                name = Some(text);
            } else if !details.contains(&text) {
                details.push(text);
            }
        }
        slots.push(CharacterSlot {
            region: slot.total_region,
            interaction: interaction_info(tree, slot),
            index,
            name,
            details,
        });
    }
    if slots.is_empty() {
        return None;
    }
    Some(CharacterSelect {
        region: screen.total_region,
        slots,
    })
}
