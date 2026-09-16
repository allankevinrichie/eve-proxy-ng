//! Screen-status indicators: ship alerts, timers, and the
//! notification center — floating overlays that are neither windows
//! nor HUD controls but carry real game-state information.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// One ship-alert banner (warp protection countdown, etc.).
#[derive(Clone, Debug, Serialize)]
pub struct ShipAlert {
    pub region: DisplayRegion,
    /// Alert texture basename (`warped2.png` = warp protection).
    pub texture: String,
}

/// All visible ship-alert banners.
#[derive(Clone, Debug, Serialize)]
pub struct ShipAlerts {
    pub alerts: Vec<ShipAlert>,
}

/// One on-screen timer/indicator (at-war, milestone timers, safety LED).
#[derive(Clone, Debug, Serialize)]
pub struct ScreenTimer {
    /// Indicator kind: `at_war` / `milestone` / `safety_led`.
    pub kind: String,
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Icon texture when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// All visible timers and status indicators.
#[derive(Clone, Debug, Serialize)]
pub struct ScreenTimers {
    pub timers: Vec<ScreenTimer>,
}

/// The notification center (floating overlay with notification list).
#[derive(Clone, Debug, Serialize)]
pub struct NotificationCenter {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// Whether the notification panel is expanded (scroll container
    /// has meaningful height).
    pub is_expanded: bool,
}

/// Extract ship-alert banners (warp protection, etc.).
pub fn extract_ship_alerts(tree: &RegionedTree<'_>) -> ShipAlerts {
    let alerts = tree
        .find_by_type("Container")
        .filter(|node| node.name() == Some("shipAlerts"))
        .flat_map(|container| tree.descendants(container))
        .filter(|node| node.type_name() == "Sprite")
        .filter_map(|sprite| {
            let entries = sprite.entries();
            let tex = entries
                .get("_texturePath")
                .or_else(|| entries.get("texturePath"))?;
            if let eve_memory::NodeValue::Str(path) = tex {
                Some(ShipAlert {
                    region: sprite.total_region,
                    texture: path.rsplit('/').next().unwrap_or(path).to_string(),
                })
            } else {
                None
            }
        })
        .collect();
    ShipAlerts { alerts }
}

/// Extract screen timers and status indicators.
pub fn extract_screen_timers(tree: &RegionedTree<'_>) -> ScreenTimers {
    let mut timers = Vec::new();
    // At-war icon (TimerContainer → AtWarCont → GlowSprite:atWarIcon)
    for icon in tree.find_by_type("GlowSprite")
        .filter(|node| node.name() == Some("atWarIcon"))
    {
        timers.push(ScreenTimer {
            kind: "at_war".into(),
            region: icon.total_region,
            interaction: interaction_info(tree, icon),
            icon: Some("atWar_64.png".into()),
        });
    }
    // MilestoneTimer (ISK / clone / insurance timers)
    for timer in tree.find_by_type("MilestoneTimer") {
        let icon = tree
            .descendants(timer)
            .find(|node| node.type_name() == "Sprite")
            .and_then(|sprite| {
                sprite.entries().get("_texturePath")
                    .and_then(|v| match v {
                        eve_memory::NodeValue::Str(p) => Some(p.rsplit('/').next().unwrap_or(p).to_string()),
                        _ => None,
                    })
            });
        timers.push(ScreenTimer {
            kind: "milestone".into(),
            region: timer.total_region,
            interaction: interaction_info(tree, timer),
            icon,
        });
    }
    // SafetyButton LED (the level indicator on the HUD)
    for led in tree.find_by_type("SafetyButton") {
        timers.push(ScreenTimer {
            kind: "safety_led".into(),
            region: led.total_region,
            interaction: interaction_info(tree, led),
            icon: None,
        });
    }
    ScreenTimers { timers }
}

/// Extract the notification center.
pub fn extract_notification_center(tree: &RegionedTree<'_>) -> Option<NotificationCenter> {
    let container = tree.find_by_type("NotificationContainer").next()?;
    let region = container.total_region;
    // Expanded = the scroll container has meaningful height.
    let is_expanded = tree
        .descendants(container)
        .find(|node| node.type_name() == "NotificationScrollContainer")
        .map(|sc| sc.region.height > 10)
        .unwrap_or(false);
    Some(NotificationCenter {
        region,
        interaction: interaction_info(tree, container),
        is_expanded,
    })
}
