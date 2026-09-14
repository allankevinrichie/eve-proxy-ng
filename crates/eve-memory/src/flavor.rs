//! Server flavor (game edition) identification.
//!
//! The project targets all three EVE Online editions:
//!
//! | Flavor         | Operator     | Common name | Internal tag  |
//! |----------------|--------------|-------------|---------------|
//! | [`Flavor::Infinity`]    | NetEase (CN) | 曙光服      | `infinity`    |
//! | [`Flavor::Serenity`]    | NetEase (CN) | 经典服/晨曦 | `serenity`    |
//! | [`Flavor::Tranquility`] | CCP         | 国际服      | `tranquility` |
//!
//! The flavor is detected from the client window title (e.g.
//! `星战前夜：晨曦 [Infinity] - aiyoggle`) or the install path
//! (`C:\EVE\SharedCache\infinity\bin64\exefile.exe`).
//! The semantic layer uses it to dispatch version-specific extractors and
//! falls back to the 曙光 (Infinity) implementation when no extractor is
//! registered for a flavor.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Flavor {
    /// NetEase China "曙光" server (internal name `infinity`).
    /// This is the baseline flavor for semantic extraction.
    Infinity,

    /// NetEase China "经典服" (晨曦, internal name `serenity`).
    Serenity,

    /// International server operated by CCP (internal name `tranquility`).
    Tranquility,

    /// Flavor could not be identified; consumers fall back to the baseline.
    #[default]
    Unknown,
}

impl Flavor {
    /// Parse the `[Tag]` component of a window title.
    pub fn parse_tag(tag: &str) -> Flavor {
        match tag.trim().to_ascii_lowercase().as_str() {
            "infinity" => Flavor::Infinity,
            "serenity" => Flavor::Serenity,
            "tranquility" => Flavor::Tranquility,
            _ => Flavor::Unknown,
        }
    }

    /// Detect the flavor from a client window title such as
    /// `星战前夜：晨曦 [Infinity] - aiyoggle`.
    pub fn from_window_title(title: &str) -> Flavor {
        let Some(open) = title.rfind('[') else {
            return Flavor::Unknown;
        };
        let Some(close) = title[open..].find(']') else {
            return Flavor::Unknown;
        };
        Flavor::parse_tag(&title[open + 1..open + close])
    }

    /// Detect the flavor from the executable path, e.g.
    /// `C:\EVE\SharedCache\infinity\bin64\exefile.exe`.
    pub fn from_install_path(path: &std::path::Path) -> Flavor {
        for component in path.components().rev().skip(1) {
            let flavor = Flavor::parse_tag(&component.as_os_str().to_string_lossy());
            if flavor != Flavor::Unknown {
                return flavor;
            }
        }
        Flavor::Unknown
    }

    /// The internal tag used by the game launcher, if known.
    pub fn internal_tag(self) -> Option<&'static str> {
        match self {
            Flavor::Infinity => Some("infinity"),
            Flavor::Serenity => Some("serenity"),
            Flavor::Tranquility => Some("tranquility"),
            Flavor::Unknown => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Flavor::Infinity => "曙光服 (Infinity)",
            Flavor::Serenity => "经典服 (Serenity)",
            Flavor::Tranquility => "国际服 (Tranquility)",
            Flavor::Unknown => "未知服务器",
        }
    }
}

/// Extra facts parsed from a client window title.
#[derive(Clone, Debug, Default)]
pub struct TitleInfo {
    pub flavor: Flavor,
    /// Character name, from the ` - <name>` suffix.
    pub character_name: Option<String>,
}

pub fn parse_window_title(title: &str) -> TitleInfo {
    let flavor = Flavor::from_window_title(title);
    let character_name = title.rfind(']').and_then(|close| {
        let rest = &title[close + 1..];
        let rest = rest.trim_start();
        rest.strip_prefix("- ").map(|name| name.trim().to_string())
    });
    TitleInfo {
        flavor,
        character_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_netease_infinity_title() {
        let info = parse_window_title("星战前夜：晨曦 [Infinity] - aiyoggle");
        assert_eq!(info.flavor, Flavor::Infinity);
        assert_eq!(info.character_name.as_deref(), Some("aiyoggle"));
    }

    #[test]
    fn parses_international_title() {
        let info = parse_window_title("EVE - aiyoggle");
        assert_eq!(info.flavor, Flavor::Unknown);
        assert_eq!(info.character_name, None);
    }

    #[test]
    fn parses_install_path() {
        let flavor = Flavor::from_install_path(std::path::Path::new(
            r"C:\EVE\SharedCache\infinity\bin64\exefile.exe",
        ));
        assert_eq!(flavor, Flavor::Infinity);
    }
}
