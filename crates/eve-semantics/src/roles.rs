//! Semantic roles: curated type/name/label → meaning mappings, so agents
//! read `role: "hud.open_cargo"` instead of reverse-engineering
//! `LeftSideButtonCargo`.
//!
//! Every entry was verified against the live 曙光 client (2026-09-12
//! verification sessions). The client's internal type names are stable
//! and self-describing — this table just makes them cheap to consume.
//! Unknown elements fall back to their raw type name; the table grows as
//! new controls are observed.

/// Resolve a semantic role for an element from its type name, internal
/// name, and visible label (text or hint).
pub fn semantic_role(
    type_name: &str,
    name: Option<&str>,
    label: Option<&str>,
) -> Option<String> {
    // Type-name exact matches (HUD and window chrome).
    let role = match type_name {
        // --- in-space HUD ---
        "StopButton" => Some("hud.stop_ship"),
        "MaxSpeedButton" => Some("hud.max_speed"),
        "SafetyButton" => Some("hud.safety_level"),
        "LeftSideButtonCargo" => Some("hud.open_cargo"),
        "LeftSideButtonTactical" => Some("hud.toggle_tactical_view"),
        "LeftSideButtonScanner" => Some("hud.open_scanner"),
        "LeftSideButtonAutopilot" => Some("hud.toggle_autopilot"),
        "LeftSideButtonCameraTactical" => Some("hud.camera_tactical"),
        "LeftSideButtonCameraOrbit" => Some("hud.camera_orbit"),
        "LeftSideButtonCameraPOV" => Some("hud.camera_first_person"),
        "QuickLockButton" => Some("targeting.quick_lock"),
        "LockStopButton" => Some("targeting.stop_locking"),
        "LockOption" => Some("targeting.options"),
        "FireFocusButton" => Some("weapons.focus_fire"),
        "FireStopButton" => Some("weapons.stop_focus_fire"),
        "FireOption" => Some("weapons.options"),

        // --- station ---
        "UndockButton" => Some("station.undock"),
        "CorvetteButton" => Some("station.board_rookie_ship"),

        // --- character selection ---
        "SmallCharacterSlot" => Some("charselect.select_character"),
        "SmallEmptySlot" => Some("charselect.new_character"),
        "OpenStoreButton" => Some("store.open"),

        // --- windows / lists ---
        "RedeemButton" => Some("redeem.open_panel"),
        "DraggableRedeemItem" => Some("redeem.item"),
        _ => None,
    };
    if role.is_some() {
        return role.map(str::to_string);
    }

    // Internal-name matches (buttons whose type is generic).
    if let Some(name) = name {
        let role = match (type_name, name) {
            ("StationServiceBtn", service) => {
                return Some(format!("station.service.{service}"))
            }
            ("ControlButton", "takeControlBtn") => Some("station.take_structure_control"),
            ("ModeButton", "dockedModeBtn") => Some("station.docked_mode"),
            (_, "undockButton") => Some("station.undock"),
            _ => None,
        };
        if let Some(role) = role {
            return Some(role.to_string());
        }
    }

    // Visible-label matches (localized checkboxes and named toggles).
    if let Some(label) = label {
        let role = match label.trim() {
            "自动锁定" => Some("targeting.auto_lock_back"),
            "自动集火" => Some("weapons.auto_focus_fire"),
            _ => None,
        };
        if let Some(role) = role {
            return Some(role.to_string());
        }
    }
    None
}
