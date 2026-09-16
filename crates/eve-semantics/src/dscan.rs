//! D-Scan (directional scanner) window: result entries with name /
//! type / distance, plus the scan button.

use serde::Serialize;

use crate::interaction::{interaction_info, InteractionInfo};
use crate::parsing::strip_markup;
use crate::region::{DisplayRegion, RegionedNode, RegionedTree};

/// One D-Scan result row.
#[derive(Clone, Debug, Serialize)]
pub struct DscanEntry {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    /// 名称（第一列，如 "基索加 7 - 星空会实验室"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 类型/从属（第二列，如 "跨学科研究协会（星空会）哨站"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    /// 距离文本（第三列，如 "1,461 km"；原文）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_text: Option<String>,
    /// bracket 图标路径（类别信号）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// The D-Scan window when open.
#[derive(Clone, Debug, Serialize)]
pub struct DscanWindow {
    pub region: DisplayRegion,
    #[serde(flatten)]
    pub interaction: InteractionInfo,
    pub entries: Vec<DscanEntry>,
    /// 扫描按钮（region 可注入操作）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_button_region: Option<DisplayRegion>,
}

/// Extract the D-Scan window when open (DirectionalScanner node).
pub fn extract_dscan(tree: &RegionedTree<'_>) -> Option<DscanWindow> {
    let window = tree.find_by_type("DirectionalScanner").next()?;
    let region = window.total_region;

    let mut entries = Vec::new();
    for entry in tree.find_by_type("DirectionalScanResultEntry") {
        let labels: Vec<String> = tree
            .descendants(entry)
            .filter(|node| node.type_name() == "Label")
            .filter_map(|node| node.text())
            .map(|text| strip_markup(&text))
            .filter(|text| !text.is_empty())
            .collect();
        let icon = tree
            .descendants(entry)
            .find(|node| node.type_name() == "Sprite")
            .and_then(|sprite| {
                sprite.entries().get("_texturePath")
                    .or_else(|| sprite.entries().get("texturePath"))
                    .and_then(|v| match v {
                        eve_memory::NodeValue::Str(path) => Some(path.clone()),
                        _ => None,
                    })
            });
        entries.push(DscanEntry {
            region: entry.total_region,
            interaction: interaction_info(tree, entry),
            name: labels.first().cloned(),
            type_text: labels.get(1).cloned(),
            distance_text: labels.get(2).cloned(),
            icon,
        });
    }

    let scan_button = tree
        .find_by_type("ScanButton")
        .next()
        .map(|button| button.total_region);

    Some(DscanWindow {
        region,
        interaction: interaction_info(tree, window),
        entries,
        scan_button_region: scan_button,
    })
}
