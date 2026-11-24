// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use serde::Serialize;
use std::collections::BTreeMap;

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
            other => Err(format!(
                "不支持的主题模式：{other}（可选 light|dark|system）"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelRegion {
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceholderKind {
    Backlinks,
    Changes,
    Terminal,
}

impl PlaceholderKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Backlinks => "backlinks",
            Self::Changes => "changes",
            Self::Terminal => "terminal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "backlinks" => Some(Self::Backlinks),
            "changes" => Some(Self::Changes),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Backlinks => "反链",
            Self::Changes => "改动",
            Self::Terminal => "终端",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelPlacement {
    locations: BTreeMap<PlaceholderKind, PanelRegion>,
}

impl Default for PanelPlacement {
    fn default() -> Self {
        let mut locations = BTreeMap::new();
        locations.insert(PlaceholderKind::Backlinks, PanelRegion::Right);
        locations.insert(PlaceholderKind::Changes, PanelRegion::Right);
        locations.insert(PlaceholderKind::Terminal, PanelRegion::Bottom);
        Self { locations }
    }
}

impl PanelPlacement {
    pub fn region_of(&self, kind: PlaceholderKind) -> PanelRegion {
        *self.locations.get(&kind).unwrap_or(&PanelRegion::Right)
    }

    pub fn toggle_region(&mut self, kind: PlaceholderKind) -> PanelRegion {
        let next = match self.region_of(kind) {
            PanelRegion::Right => PanelRegion::Bottom,
            PanelRegion::Bottom => PanelRegion::Right,
        };
        self.locations.insert(kind, next);
        next
    }

    pub fn views_in(&self, region: PanelRegion) -> Vec<PlaceholderKind> {
        [
            PlaceholderKind::Backlinks,
            PlaceholderKind::Changes,
            PlaceholderKind::Terminal,
        ]
        .into_iter()
        .filter(|kind| self.region_of(*kind) == region)
        .collect()
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
    fn panel_placement_moves_placeholders_between_right_and_bottom() {
        let mut placement = PanelPlacement::default();
        assert_eq!(
            placement.views_in(PanelRegion::Right),
            vec![PlaceholderKind::Backlinks, PlaceholderKind::Changes]
        );
        assert_eq!(
            placement.views_in(PanelRegion::Bottom),
            vec![PlaceholderKind::Terminal]
        );

        assert_eq!(
            placement.toggle_region(PlaceholderKind::Backlinks),
            PanelRegion::Bottom
        );
        assert_eq!(
            placement.views_in(PanelRegion::Right),
            vec![PlaceholderKind::Changes]
        );
        assert_eq!(
            placement.views_in(PanelRegion::Bottom),
            vec![PlaceholderKind::Backlinks, PlaceholderKind::Terminal]
        );
    }
}
