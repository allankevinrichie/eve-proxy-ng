//! Ship UI extraction: module buttons (high/mid/low rows), capacitor,
//! hitpoints, maneuver indication, speed.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::parse_number_truncating_fraction;
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// One module (slot) button.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleButton {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Exact module typeID from the node name (`ModuleButton_<typeID>`).
    /// Uniquely identifies the variant (incl. meta level) — the precise
    /// identification channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_id: Option<u64>,
    /// Icon resource path — family-level fingerprint (variants share
    /// icons).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Icon family name (本地化; incl. live-verified overrides) — kept
    /// alongside the exact name since meta variants share it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_name: Option<String>,
    /// The per-module overload arc above the button (click target +
    /// state), when the slot carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overload: Option<ModuleOverload>,
    /// Module display name: exact typeID lookup (localized, precise
    /// variant) → icon family fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_name: Option<String>,
    pub is_active: Option<bool>,
    /// Slot shows the busy sprite.
    pub is_busy: bool,
    /// Slot shows the hilite sprite.
    pub is_hilite: bool,
    /// 0..1000 ramp progress (both ramps averaged), when readable.
    pub ramp_rotation_milli: Option<i64>,
}

/// The icon fingerprint of a module button (first `res:` texture below it).
pub fn module_button_icon(tree: &RegionedTree<'_>, button: &RegionedNode<'_>) -> Option<String> {
    tree.subtree_iter(button.index)
        .skip(1)
        .find_map(|node| {
            node.entries()
                .get("_texturePath")
                .or_else(|| node.entries().get("texturePath"))
        })
        .and_then(|value| match value {
            eve_memory::NodeValue::Str(path) if path.starts_with("res:") => Some(path.clone()),
            _ => None,
        })
}

/// Hitpoint percentages; all three gauges or nothing.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Hitpoints {
    pub shield_percent: i64,
    pub armor_percent: i64,
    pub structure_percent: i64,
}

/// What the ship is doing right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum ManeuverType {
    Warp,
    Jump,
    Orbit,
    Approach,
}

/// The complete ship HUD.
#[derive(Clone, Debug, Serialize)]
pub struct ShipUi {
    pub region: DisplayRegion,
    pub module_buttons_high: Vec<ModuleButton>,
    pub module_buttons_mid: Vec<ModuleButton>,
    pub module_buttons_low: Vec<ModuleButton>,
    pub capacitor_percent: Option<i64>,
    pub hitpoints: Option<Hitpoints>,
    pub indication: Option<ManeuverType>,
    /// Client-formatted speed text (locale-correct by construction).
    pub speed_text: Option<String>,
    /// The full HUD control cluster: lock/fire buttons, auto-lock and
    /// auto-focus-fire checkboxes, side function buttons (cargo,
    /// tactical view, autopilot…), matrix slots, the safety button and
    /// the three rack-overload buttons.
    pub hud_buttons: Vec<HudButton>,
}

/// Per-module overload control: the thin arc ABOVE the module button
/// (inside the round slot). Clicking the arc toggles overload; the arc
/// texture carries the state.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleOverload {
    /// The arc's own click rectangle (absolute game coords).
    pub region: DisplayRegion,
    /// `disabled` (module cannot overheat), `off` (ready), `on`
    /// (overloading), or `blink` (overheating/damage states).
    pub state: String,
    /// The arc texture path (raw ground truth).
    pub texture: String,
}

fn overload_state(texture: &str) -> &'static str {
    let lowered = texture.to_ascii_lowercase();
    if lowered.contains("disabled") {
        "disabled"
    } else if lowered.contains("blink") {
        "blink"
    } else if lowered.contains("on") && !lowered.contains("off") {
        // slotOverloadOn.png — careful: "off" checked first below.
        "on"
    } else if lowered.contains("off") {
        "off"
    } else {
        "off"
    }
}

/// One HUD control outside the module racks.
#[derive(Clone, Debug, Serialize)]
pub struct HudButton {
    /// Semantic id (`hud.lock_all`, `hud.auto_lock`, `hud.cargo_hold`,
    /// `hud.overload_high`, `hud.safety`, …).
    pub kind: String,
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// User-facing label: the checkbox text or the button hint (which
    /// carries toggle state wording, e.g. 开启/关闭自动导航).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_name: Option<String>,
    /// Toggle/checkbox state when detectable (checkbox `_checked`; the
    /// flipped hint 关闭/隐藏 prefix means ON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_on: Option<bool>,
    /// Gauge-style buttons: RAW fill-strip height in px (cargo button:
    /// the busyContainer strip grows with bay occupancy; empty bay
    /// reads a 3px baseline).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_strip_px: Option<i64>,
    /// Derived fill percent (cargo button). Three-point calibration:
    /// 0%=3px, 4%(5/125m³)=4px, 12%(15/125m³)=7px → slope ≈0.34px/%
    /// → percent ≈ (px-3)×3, resolution ±~1.5%.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_percent: Option<i64>,
}

/// Node → semantic kind mapping for HUD controls (type-name or
/// node-name based; both are stable client structure).
fn hud_kind(node: &RegionedNode<'_>) -> Option<&'static str> {
    let type_name = node.type_name();
    let name = node.name().unwrap_or("");
    Some(match (type_name, name) {
        ("QuickLockButton", _) => "hud.lock_all",
        ("CheckboxWithTooltip", "autolockcheckbox") => "hud.auto_lock",
        ("CheckboxWithTooltip", "autofirecheckbox") => "hud.auto_focus_fire",
        ("FireFocusButton", _) => "hud.fire",
        ("LockOption", _) => "hud.lock_settings",
        ("LockStopButton", _) => "hud.unlock_all",
        ("FireOption", _) => "hud.fire_settings",
        ("FireStopButton", _) => "hud.stop_attack",
        ("SafetyButton", _) => "hud.safety",
        ("LeftSideButtonCameraTactical", _) => "hud.camera_tactical",
        ("LeftSideButtonCameraOrbit", _) => "hud.camera_orbit",
        ("LeftSideButtonCameraPOV", _) => "hud.camera_pov",
        ("LeftSideButtonCargo", _) => "hud.cargo_hold",
        ("LeftSideButtonTactical", _) => "hud.tactical_view",
        ("LeftSideButtonScanner", _) => "hud.scanner",
        ("LeftSideButtonAutopilot", _) => "hud.autopilot",
        ("LeftSideButtonMiningScan", _) => "hud.mining_scan",
        ("ButtonIcon", _) if node
            .name().is_none() => return None, // matrix icons matched by parent below
        ("OverloadBtn", "overloadBtnHi") => "hud.overload_high",
        ("OverloadBtn", "overloadBtnMed") => "hud.overload_mid",
        ("OverloadBtn", "overloadBtnLo") => "hud.overload_low",
        _ => return None,
    })
}

/// Does a `busy` sprite exist under the node? For the camera radio
/// group only the active mode's button has one; toggles (autopilot,
/// tactical view) also light it while on.
fn busy_present(tree: &RegionedTree<'_>, node: &RegionedNode<'_>) -> bool {
    tree.subtree_iter(node.index)
        .skip(1)
        .any(|child| child.type_name() == "Sprite" && child.name() == Some("busy"))
}

fn extract_hud_buttons(
    tree: &RegionedTree<'_>,
    ship_node: &RegionedNode<'_>,
) -> Vec<HudButton> {
    let mut buttons = Vec::new();
    for node in tree.subtree_iter(ship_node.index).skip(1) {
        if let Some(kind) = hud_kind(node) {
            let hint = node.hint().map(str::to_string);
            // Checkbox labels come from their text child; other buttons
            // carry the state-worded hint.
            let label = if node.type_name() == "CheckboxWithTooltip" {
                tree.subtree_iter(node.index)
                    .find(|child| child.name() == Some("text"))
                    .and_then(|child| child.text())
                    .map(|text| crate::parsing::strip_markup(&text))
                    .or(hint.clone())
            } else {
                hint.clone()
            };
            let is_on = match node.type_name() {
                "CheckboxWithTooltip" => node.boolean("_checked"),
                // Toggle buttons flip their hint when on: 关闭自动导航 /
                // 隐藏战术视图 mean the feature is currently ON.
                "LeftSideButtonAutopilot" | "LeftSideButtonTactical" => {
                    let flipped = hint
                        .as_deref()
                        .map(|h| h.starts_with("关闭") || h.starts_with("隐藏"))
                        .unwrap_or(false);
                    Some(flipped || busy_present(tree, node))
                }
                // Camera modes are a radio group: the ACTIVE mode's
                // button carries a busy sprite child, the inactive ones
                // have none (verified: orbit on → busy present, the
                // other two absent).
                "LeftSideButtonCameraTactical"
                | "LeftSideButtonCameraOrbit"
                | "LeftSideButtonCameraPOV" => Some(busy_present(tree, node)),
                // cargo/mining/scanner: idle buttons also carry a
                // dim busy sprite — no calibrated on/off signal yet.
                _ => None,
            };
            let icon = crate::icons::element_icon(tree, node);
            let icon_name = icon
                .as_deref()
                .and_then(crate::icons::semantic_icon_name);
            // The cargo button doubles as an occupancy gauge: its
            // busyContainer strip (a cropped glow sprite) grows with
            // the bay's fill level. Report the RAW strip height — the
            // empty bay baseline is 3px, so percent needs calibration.
            let (fill_strip_px, fill_percent) = if node.type_name() == "LeftSideButtonCargo" {
                tree.children_of(node)
                    .find(|child| child.name() == Some("busyContainer"))
                    .map(|strip| {
                        const EMPTY_BASE: i64 = 3; // measured: 0% = 3px
                        // measured slope: 0.34px/% (4%→4px, 12%→7px)
                        let px = strip.region.height;
                        let percent = ((px - EMPTY_BASE).max(0) * 3).min(100);
                        (Some(px), Some(percent))
                    })
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };
            buttons.push(HudButton {
                kind: kind.to_string(),
                region: node.total_region,
                interaction: crate::interaction::interaction_info(tree, node),
                label,
                icon,
                icon_name,
                is_on,
                fill_strip_px,
                fill_percent,
            });
        }
    }
    // Matrix slots: the two unnamed ButtonIcons under matrixslotButtons.
    if let Some(container) = tree
        .subtree_iter(ship_node.index)
        .find(|node| node.name() == Some("matrixslotButtons"))
    {
        for (index, node) in tree
            .children_of(container)
            .filter(|child| child.type_name() == "ButtonIcon")
            .enumerate()
        {
            let icon = crate::icons::element_icon(tree, node);
            let icon_name = icon
                .as_deref()
                .and_then(crate::icons::semantic_icon_name);
            buttons.push(HudButton {
                kind: format!("hud.matrix_{}", index + 1),
                region: node.total_region,
                interaction: crate::interaction::interaction_info(tree, node),
                label: icon_name.clone(),
                icon,
                icon_name,
                is_on: None,
                fill_strip_px: None,
                fill_percent: None,
            });
        }
    }
    buttons
}

/// The HUD shows modules as icons only. Two complementary
/// identification channels:
///
/// 1. [`ModuleButton::module_role`] — icon fingerprint table
///    ([`crate::icons`]), instant but only as complete as its
///    calibration.
/// 2. [`ModuleButtonTooltip`] — while a module button is hovered, the
///    client shows its name and keyboard shortcut in the `l_hint` layer.
///    The executor layer can hover-then-read to identify any module.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleButtonTooltip {
    pub region: DisplayRegion,
    /// Module display name (first text line of the tooltip).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_name: Option<String>,
    /// Keyboard shortcut like `CTRL-F3`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    /// All tooltip text lines, verbatim (markup stripped).
    pub text_lines: Vec<String>,
}

/// Extract the module tooltip currently on screen (`None` when no module
/// is hovered). Mirrors the reference parser's tooltip handling.
pub fn extract_module_button_tooltip(tree: &RegionedTree<'_>) -> Option<ModuleButtonTooltip> {
    let tooltip = tree.find_by_type("ModuleButtonTooltip").next()?;
    let lines: Vec<String> = tree
        .subtree_iter(tooltip.index)
        .filter_map(|node| node.text())
        .map(|text| crate::parsing::strip_markup(&text))
        .filter(|text| !text.is_empty() && !text.starts_with("Failed to read"))
        .collect();
    let module_name = lines.first().cloned();
    let shortcut = lines
        .iter()
        .find_map(|line| parse_shortcut(line))
        .map(str::to_string);
    Some(ModuleButtonTooltip {
        region: tooltip.total_region,
        module_name,
        shortcut,
        text_lines: lines,
    })
}

/// `"CTRL-F3"` / `"STRG-F4"` (German client) / `"UMSCH-F6"` style
/// shortcuts: a dash-separated pair of 1-5 uppercase letter groups.
fn parse_shortcut(line: &str) -> Option<&str> {
    let mut parts = line.split('-');
    let (head, tail) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let is_key = |s: &str| (2..=5).contains(&s.len()) && s.chars().all(|c| c.is_ascii_uppercase());
    if is_key(head) && is_key(tail) {
        Some(line)
    } else {
        None
    }
}

/// ShipUI requires the hull node, the capacitor, and all three hitpoint
/// gauges — mirroring the all-or-nothing rule of the reference parser.
pub fn extract_ship_ui(tree: &RegionedTree<'_>) -> Option<ShipUi> {
    let ship_node = tree.find_by_type("ShipUI").next()?;
    let ship_descendants: Vec<&RegionedNode<'_>> = tree.descendants(ship_node).collect();

    let capacitor = ship_descendants
        .iter()
        .copied()
        .find(|node| node.type_name() == "CapacitorContainer")?;

    let hitpoints = hitpoints_from(&ship_descendants);
    let (high, mid, low) = module_rows(tree, ship_node, capacitor);
    Some(ShipUi {
        region: ship_node.total_region,
        module_buttons_high: high,
        module_buttons_mid: mid,
        module_buttons_low: low,
        capacitor_percent: capacitor_percent(tree, capacitor),
        hitpoints,
        indication: indication_from(tree, ship_node),
        speed_text: speed_text_from(&ship_descendants),
        hud_buttons: extract_hud_buttons(tree, ship_node),
    })
}

/// Geometric row grouping: modules above/below the capacitor's vertical
/// center (±20 px) are high/low slots, the rest middle.
fn module_rows(
    tree: &RegionedTree<'_>,
    ship_node: &RegionedNode<'_>,
    capacitor: &RegionedNode<'_>,
) -> (Vec<ModuleButton>, Vec<ModuleButton>, Vec<ModuleButton>) {
    let capacitor_center =
        capacitor.total_region.y + capacitor.total_region.height / 2;
    let mut high = Vec::new();
    let mut mid = Vec::new();
    let mut low = Vec::new();
    for slot in tree.descendants(ship_node).filter(|n| n.type_name() == "ShipSlot") {
        let Some(button) = tree
            .descendants(slot)
            .find(|n| n.type_name() == "ModuleButton")
        else {
            continue;
        };
        let module = module_button(tree, slot, button);
        let center = button.total_region.y + button.total_region.height / 2;
        if center < capacitor_center - 20 {
            high.push(module);
        } else if center > capacitor_center + 20 {
            low.push(module);
        } else {
            mid.push(module);
        }
    }
    (high, mid, low)
}

/// One module (slot) button, with its icon resolved through the static
/// icon table.
fn module_button(
    tree: &RegionedTree<'_>,
    slot: &RegionedNode<'_>,
    button: &RegionedNode<'_>,
) -> ModuleButton {
    let sprite_named = |name: &str| {
        tree.descendants(slot)
            .any(|node| node.type_name() == "Sprite" && node.name() == Some(name))
    };
    let icon = module_button_icon(tree, button);
    let type_id = button
        .name()
        .and_then(crate::icons::type_id_from_node_name);
    let icon_name = icon
        .as_deref()
        .and_then(crate::icons::module_name_from_icon)
        .map(str::to_string);
    // The overload arc: the ShipSlot's overloadBtn sprite sibling.
    let overload = tree
        .children_of(slot)
        .find(|child| child.name() == Some("overloadBtn"))
        .and_then(|arc| {
            let entries = arc.entries();
            let texture = entries
                .get("_texturePath")
                .or_else(|| entries.get("texturePath"))
                .and_then(|value| match value {
                    eve_memory::NodeValue::Str(path) => Some(path.clone()),
                    _ => None,
                })?;
            Some(ModuleOverload {
                region: arc.total_region,
                state: overload_state(&texture).to_string(),
                texture,
            })
        });
    // Name priority: exact typeID lookup (localized, precise variant)
    // → icon family fallback (incl. live-verified overrides).
    let module_name = type_id
        .and_then(crate::icons::type_name_from_type_id)
        .map(str::to_string)
        .or(icon_name.clone());
    ModuleButton {
        region: button.total_region,
        interaction: interaction_info(tree, button),
        type_id,
        module_name,
        icon,
        icon_name,
        overload,
        is_active: button.boolean("ramp_active"),
        is_busy: sprite_named("busy"),
        is_hilite: sprite_named("hilite"),
        ramp_rotation_milli: ramp_rotation_milli(tree, slot),
    }
}

/// Average both module ramps into 0..1000 (ported formula; radians on the
/// `leftRamp`/`rightRamp` sprites).
fn ramp_rotation_milli(tree: &RegionedTree<'_>, slot: &RegionedNode<'_>) -> Option<i64> {
    let rotation_of = |name: &str| -> Option<f64> {
        tree.descendants(slot)
            .find(|node| node.name() == Some(name))?
            .float("_rotation")
    };
    let left = rotation_of("leftRamp")?;
    let right = rotation_of("rightRamp")?;
    let in_range = |r: f64| (0.0..std::f64::consts::PI * 2.01).contains(&r);
    if !in_range(left) || !in_range(right) {
        return None;
    }
    let combined = (left + right) * 500.0 / std::f64::consts::PI;
    Some((1000.0 - combined).round().clamp(0.0, 1000.0) as i64)
}

/// Capacitor level: filled `pmark` sprites (`_color.a < 20`) over total.
fn capacitor_percent(tree: &RegionedTree<'_>, capacitor: &RegionedNode<'_>) -> Option<i64> {
    let mut filled = 0usize;
    let mut total = 0usize;
    for pmark in tree.descendants(capacitor).filter(|n| n.name() == Some("pmark")) {
        let alpha = pmark.color("_color")?.a?;
        total += 1;
        if alpha < 20 {
            filled += 1;
        }
    }
    if total == 0 {
        None
    } else {
        Some((filled as i64 * 100) / total as i64)
    }
}

fn hitpoints_from(descendants: &[&RegionedNode<'_>]) -> Option<Hitpoints> {
    let gauge_percent = |name: &str| -> Option<i64> {
        let gauge = descendants
            .iter()
            .copied()
            .find(|node| node.name() == Some(name))?;
        let value = gauge.float("_lastValue")?;
        Some((value * 100.0).round() as i64)
    };
    Some(Hitpoints {
        shield_percent: gauge_percent("shieldGauge")?,
        armor_percent: gauge_percent("armorGauge")?,
        structure_percent: gauge_percent("structureGauge")?,
    })
}

/// Maneuver type from the indication container's texts. Keywords cover the
/// English, Korean, and Chinese clients; extend per flavor as observed.
fn indication_from(tree: &RegionedTree<'_>, ship_node: &RegionedNode<'_>) -> Option<ManeuverType> {
    let container = tree
        .descendants(ship_node)
        .find(|node| {
            node.name()
                .is_some_and(|name| name.to_lowercase().contains("indicationcontainer"))
        })?;
    let texts: Vec<String> = tree.subtree_iter(container.index).filter_map(|n| n.text()).collect();
    let keywords: &[(&str, ManeuverType)] = &[
        ("Warp", ManeuverType::Warp),
        ("워프 드라이브 가동", ManeuverType::Warp), // Korean client, 2022-05
        ("跃迁", ManeuverType::Warp),               // China clients
        ("Jump", ManeuverType::Jump),
        ("점프 중", ManeuverType::Jump),
        ("跳跃", ManeuverType::Jump),
        ("跳转", ManeuverType::Jump),
        ("Orbit", ManeuverType::Orbit),
        ("环绕", ManeuverType::Orbit),
        ("Approach", ManeuverType::Approach),
        ("接近", ManeuverType::Approach),
    ];
    texts.iter().find_map(|text| {
        keywords
            .iter()
            .find(|(keyword, _)| text.contains(keyword))
            .map(|(_, maneuver)| *maneuver)
    })
}

fn speed_text_from(descendants: &[&RegionedNode<'_>]) -> Option<String> {
    descendants
        .iter()
        .copied()
        .find(|node| node.name() == Some("speedLabel"))?
        .text()
}

/// `speedLabel` text may embed the number; expose the numeric part too.
pub fn speed_mps_from_text(text: &str) -> Option<i64> {
    parse_number_truncating_fraction(text)
}
