//! Locked targets: the target bar on screen (TargetInBar entries),
//! each with name / type text / distance / health bar visibility /
//! active-target indicator.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::strip_markup;
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// One locked target in the target bar.
#[derive(Clone, Debug, Serialize)]
pub struct LockedTarget {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// 目标名称（第一行文本，如 "基索加 7 - 星空会实验室"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 类型/从属文本（第二行，如 "国立军事学院"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    /// 距离文本（第三行，如 "573 m"；原文）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_text: Option<String>,
    /// 是否为当前激活目标（ActiveTargetIndicator 可见）。
    pub is_active: bool,
}

/// The target bar: all currently locked targets.
#[derive(Clone, Debug, Serialize)]
pub struct TargetBar {
    pub region: DisplayRegion,
    pub targets: Vec<LockedTarget>,
}

/// Extract the target bar when targets are locked (None with 0 locked).
pub fn extract_targets(tree: &RegionedTree<'_>) -> Option<TargetBar> {
    let bars: Vec<&RegionedNode<'_>> = tree
        .find_by_type("TargetInBar")
        .collect();
    if bars.is_empty() {
        return None;
    }
    let mut targets = Vec::new();
    for bar in bars {
        // The label container's EveLabelSmall children carry
        // name / type / distance in order.
        let labels: Vec<String> = tree
            .descendants(bar)
            .filter(|node| node.type_name() == "EveLabelSmall")
            .filter_map(|node| node.text())
            .map(|text| strip_markup(&text))
            .filter(|text| !text.is_empty())
            .collect();
        let is_active = tree
            .descendants(bar)
            .any(|node| node.type_name() == "ActiveTargetIndicator");
        targets.push(LockedTarget {
            region: bar.total_region,
            interaction: interaction_info(tree, bar),
            name: labels.first().cloned(),
            type_text: labels.get(1).cloned(),
            distance_text: labels.get(2).cloned(),
            is_active,
        });
    }
    let region = targets
        .first()
        .map(|t| t.region)
        .unwrap_or(DisplayRegion::EMPTY);
    Some(TargetBar { region, targets })
}
