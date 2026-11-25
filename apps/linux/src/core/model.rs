// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::i18n::{Key, fmt1, tr};
use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "io.github.groundgrounder.Hortus";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSchemeChoice {
    System,
    Light,
    Dark,
}

impl ColorSchemeChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            other => Err(fmt1(Key::CliUnsupportedColorScheme, other)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelRegion {
    Left,
    Right,
    Bottom,
}

impl PanelRegion {
    pub const ALL: [Self; 3] = [Self::Left, Self::Right, Self::Bottom];

    pub fn id(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Bottom => "bottom",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Left => tr(Key::RegionLeft),
            Self::Right => tr(Key::RegionRight),
            Self::Bottom => tr(Key::RegionBottom),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaticToolKind {
    Files,
    Search,
    Tags,
    Outline,
    Backlinks,
    Changes,
}

impl StaticToolKind {
    pub const ALL: [Self; 6] = [
        Self::Files,
        Self::Search,
        Self::Tags,
        Self::Outline,
        Self::Backlinks,
        Self::Changes,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Search => "search",
            Self::Tags => "tags",
            Self::Outline => "outline",
            Self::Backlinks => "backlinks",
            Self::Changes => "changes",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "files" => Some(Self::Files),
            "search" => Some(Self::Search),
            "tags" => Some(Self::Tags),
            "outline" => Some(Self::Outline),
            "backlinks" => Some(Self::Backlinks),
            "changes" => Some(Self::Changes),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Files => tr(Key::ToolFiles),
            Self::Search => tr(Key::ToolSearch),
            Self::Tags => tr(Key::ToolTags),
            Self::Outline => tr(Key::ToolOutline),
            Self::Backlinks => tr(Key::ToolBacklinks),
            Self::Changes => tr(Key::ToolChanges),
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Files => "folder-symbolic",
            Self::Search => "system-search-symbolic",
            Self::Tags => "starred-symbolic",
            Self::Outline => "view-list-symbolic",
            Self::Backlinks => "insert-link-symbolic",
            Self::Changes => "document-edit-symbolic",
        }
    }

    pub fn default_region(self) -> PanelRegion {
        match self {
            Self::Files | Self::Search | Self::Tags => PanelRegion::Left,
            Self::Outline | Self::Backlinks | Self::Changes => PanelRegion::Right,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntryItem {
    pub path: String,
    pub parent: String,
    pub name: String,
    pub directory: bool,
}

impl From<&hortus_ffi::TreeEntry> for TreeEntryItem {
    fn from(entry: &hortus_ffi::TreeEntry) -> Self {
        Self {
            path: entry.path.clone(),
            parent: entry.parent.clone(),
            name: entry.name.clone(),
            directory: entry.directory,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingItem {
    pub level: u8,
    pub title: String,
    pub start: u64,
}

impl From<hortus_ffi::DocumentHeading> for HeadingItem {
    fn from(heading: hortus_ffi::DocumentHeading) -> Self {
        Self {
            level: heading.level,
            title: heading.title,
            start: heading.start,
        }
    }
}

/// 按内核规则排序目录树节点：目录优先，其次使用 `hortus_vault::compare_names`。
pub fn sort_tree_entries(entries: &mut [TreeEntryItem]) {
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| hortus_vault::compare_names(&a.name, &b.name))
    });
}

pub fn is_note_file_name(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("md")
                || ext.eq_ignore_ascii_case("markdown")
                || ext.eq_ignore_ascii_case("mmd")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_tree_entries_puts_directories_first_naturally() {
        let mut entries = vec![
            TreeEntryItem {
                path: "note10.md".into(),
                parent: String::new(),
                name: "note10.md".into(),
                directory: false,
            },
            TreeEntryItem {
                path: "note2.md".into(),
                parent: String::new(),
                name: "note2.md".into(),
                directory: false,
            },
            TreeEntryItem {
                path: "Assets".into(),
                parent: String::new(),
                name: "Assets".into(),
                directory: true,
            },
        ];
        sort_tree_entries(&mut entries);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["Assets", "note2.md", "note10.md"]
        );
    }

    #[test]
    fn static_tool_and_region_roundtrip() {
        for region in PanelRegion::ALL {
            assert_eq!(PanelRegion::parse(region.id()), Some(region));
        }
        for tool in StaticToolKind::ALL {
            assert_eq!(StaticToolKind::parse(tool.id()), Some(tool));
        }
        assert_eq!(StaticToolKind::Files.default_region(), PanelRegion::Left);
        assert_eq!(StaticToolKind::Outline.default_region(), PanelRegion::Right);
    }
}
