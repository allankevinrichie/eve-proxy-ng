//! Runtime resource store: typeID → (localized name, icon) resolved
//! through a three-layer cascade.
//!
//! ```text
//! L2  user overlay    %LOCALAPPDATA%\eve_proxy_ng\resources\types.<flavor>.json
//!                     (or $EVE_NG_DATA_DIR) — written by
//!                     `eve-cli icons update --source client`
//! L1  manual overrides  curated icon → display-name fixes (code)
//! L0  embedded baseline  data/types.<flavor>.json.gz, derived from the
//!                     local client's own FSD data via its official
//!                     loaders (scripts/derive_client_resources.py)
//! ```
//!
//! The baseline (and every overlay produced by the client source) is
//! **localized** — Chinese names and 国服-exclusive types straight from
//! the installed game — which the legacy fuzzwork TQ SDE download
//! could never provide. Lookups never touch the network or the game
//! process: load is a one-time decompress+parse (~20 ms).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Baseline for 曙光/infinity, derived from the local client
/// (types × iconids × groups × categories × zh localization).
static BASELINE_INFINITY_GZ: &[u8] = include_bytes!("../../../data/types.infinity.json.gz");

/// Live-verified overrides: icon (normalized) → display name. Highest
/// priority; deliberate human curation beats both overlay and baseline.
const MANUAL_OVERRIDES: &[(&str, &str)] = &[
    // 曙光 live calibration 2026-09-12 (fitting-window cross-reference).
    ("res:/ui/texture/icons/13_64_5.png", "民用加特林磁轨炮"),
    ("res:/ui/texture/icons/12_64_8.png", "民用采矿器"),
    ("res:/ui/texture/icons/3_64_2.png", "1MN民用加力燃烧器"),
];

pub struct ResourceStore {
    /// Sorted by typeID: (typeID, name, icon).
    types: Vec<(u64, Box<str>, Option<Box<str>>)>,
    /// Sorted by icon: (icon, representative type name of the lowest
    /// typeID sharing it — the module-family name for HUD buttons).
    icons: Vec<(Box<str>, Box<str>)>,
    /// Diagnostics: which layer satisfied the load.
    pub loaded_from: String,
    pub type_count: usize,
}

impl ResourceStore {
    /// The process-wide store (default flavor: infinity).
    pub fn global() -> &'static ResourceStore {
        static GLOBAL: OnceLock<ResourceStore> = OnceLock::new();
        GLOBAL.get_or_init(|| ResourceStore::load("infinity"))
    }

    /// Load the cascade for one flavor: overlay when present and
    /// valid, else the embedded baseline.
    pub fn load(flavor: &str) -> ResourceStore {
        Self::load_with_data_dir(data_dir().as_deref(), flavor)
    }

    /// Same cascade with an explicit data dir (tests, embedding).
    pub fn load_with_data_dir(dir: Option<&Path>, flavor: &str) -> ResourceStore {
        if let Some(dir) = dir {
            let path = dir.join("resources").join(format!("types.{flavor}.json"));
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if let Some(store) = ResourceStore::from_json(&bytes) {
                        return ResourceStore::with_overrides(
                            store,
                            &format!("overlay:{}", path.display()),
                        );
                    }
                    tracing::warn!(
                        path = %path.display(),
                        "resource overlay is invalid; falling back to embedded baseline"
                    );
                }
                Err(_) => {} // no overlay — normal case
            }
        }
        let json = decompress_baseline(flavor).unwrap_or_else(|| {
            tracing::warn!(flavor, "no embedded baseline for flavor; store is empty");
            Vec::new()
        });
        let store = ResourceStore::from_json(&json).unwrap_or_else(|| ResourceStore {
            types: Vec::new(),
            icons: Vec::new(),
            loaded_from: "empty".into(),
            type_count: 0,
        });
        ResourceStore::with_overrides(store, "embedded-baseline")
    }

    fn from_json(bytes: &[u8]) -> Option<ResourceStore> {
        #[derive(serde::Deserialize)]
        struct RawType {
            name: String,
            icon: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct RawDoc {
            types: std::collections::HashMap<String, RawType>,
        }
        let doc: RawDoc = serde_json::from_slice(bytes).ok()?;
        let mut types: Vec<(u64, Box<str>, Option<Box<str>>)> = doc
            .types
            .into_iter()
            .filter_map(|(id, t)| {
                let id: u64 = id.parse().ok()?;
                Some((id, t.name.into_boxed_str(), t.icon.map(String::into_boxed_str)))
            })
            .collect();
        types.sort_unstable_by_key(|(id, _, _)| *id);
        // Family table: representative = lowest typeID per icon.
        let mut icons: Vec<(Box<str>, Box<str>)> = Vec::new();
        for (_, name, icon) in &types {
            if let Some(icon) = icon {
                match icons.binary_search_by(|(i, _)| (**i).cmp(icon.as_ref())) {
                    Ok(_) => {}
                    Err(at) => icons.insert(at, (icon.clone(), name.clone())),
                }
            }
        }
        Some(ResourceStore {
            type_count: types.len(),
            types,
            icons,
            loaded_from: String::new(),
        })
    }

    fn with_overrides(mut store: ResourceStore, source: &str) -> ResourceStore {
        for (icon, name) in MANUAL_OVERRIDES {
            match store
                .icons
                .binary_search_by(|(i, _)| (**i).cmp(*icon))
            {
                Ok(at) => store.icons[at].1 = (*name).into(),
                Err(at) => store.icons.insert(at, ((*icon).into(), (*name).into())),
            }
        }
        store.loaded_from = source.to_string();
        store
    }

    /// Icon resource path (normalized) → module family display name.
    pub fn module_name(&self, path: &str) -> Option<&str> {
        let normalized = path.replace('\\', "/").to_ascii_lowercase();
        self.icons
            .binary_search_by(|(icon, _)| (**icon).cmp(normalized.as_str()))
            .ok()
            .map(|at| self.icons[at].1.as_ref())
    }

    /// typeID → localized type name (exact; `ModuleButton_<typeID>`
    /// node names resolve through this).
    pub fn type_name(&self, type_id: u64) -> Option<&str> {
        self.types
            .binary_search_by(|(id, _, _)| id.cmp(&type_id))
            .ok()
            .map(|at| self.types[at].1.as_ref())
    }

    /// typeID → icon resource path, when the tables record one.
    pub fn type_icon(&self, type_id: u64) -> Option<&str> {
        self.types
            .binary_search_by(|(id, _, _)| id.cmp(&type_id))
            .ok()
            .and_then(|at| self.types[at].2.as_deref())
    }

    /// All icon entries (family table incl. overrides) for iteration
    /// and export.
    pub fn icon_entries(&self) -> Vec<(&str, &str)> {
        self.icons.iter().map(|(i, n)| (i.as_ref(), n.as_ref())).collect()
    }
}

fn decompress_baseline(flavor: &str) -> Option<Vec<u8>> {
    let gz: &[u8] = match flavor {
        "infinity" => BASELINE_INFINITY_GZ,
        _ => return None,
    };
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// Where `icons update` writes overlays: `$EVE_NG_DATA_DIR` when set
/// (tests, portable setups), else `%LOCALAPPDATA%\eve_proxy_ng`.
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("EVE_NG_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var("LOCALAPPDATA")
        .ok()
        .map(|local| Path::new(&local).join("eve_proxy_ng"))
}

/// Overlay location for one flavor (`<data_dir>/resources/types.<flavor>.json`).
pub fn overlay_path(flavor: &str) -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("resources").join(format!("types.{flavor}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_loads_and_resolves() {
        let store = ResourceStore::load("infinity");
        assert_eq!(store.loaded_from, "embedded-baseline");
        assert!(store.type_count > 60_000, "baseline missing: {}", store.type_count);
        // Chinese names straight from the client's zh localization.
        assert_eq!(store.type_name(645), Some("多米尼克斯级"));
        assert_eq!(store.type_name(34), Some("三钛合金"));
        assert_eq!(
            store.type_icon(34),
            Some("res:/ui/texture/icons/6_64_14.png")
        );
        // 国服-exclusive typeID beyond the TQ SDE range.
        assert!(store.type_name(60_001).is_some());
        // Family lookup + manual override precedence.
        let name = store.module_name("res:/ui/texture/icons/12_64_8.png");
        assert_eq!(name, Some("民用采矿器"));
    }

    #[test]
    fn overlay_takes_precedence_when_valid() {
        let dir = std::env::temp_dir().join(format!("eve-ng-test-{}", std::process::id()));
        let res = dir.join("resources");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(
            res.join("types.infinity.json"),
            r#"{"meta":{},"types":{"34":{"name":"三钛合金-测试","icon":null}}}"#,
        )
        .unwrap();
        let store = ResourceStore::load_with_data_dir(Some(&dir), "infinity");
        assert!(store.loaded_from.starts_with("overlay:"), "{}", store.loaded_from);
        assert_eq!(store.type_name(34), Some("三钛合金-测试"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
