// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

use crate::core::model::ColorSchemeChoice;
use std::path::PathBuf;

/// L2 截图场景：由 `--edit-knowledge-scene` 在截图前装配真实界面状态。
#[cfg(feature = "automation")]
pub const L2_SCENES: [&str; 5] = [
    "search-backlinks",
    "tags",
    "quick-open",
    "edit-find-completion",
    "rename-links",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliOptions {
    pub vault: Option<PathBuf>,
    pub open_notes: Vec<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub color_scheme: Option<ColorSchemeChoice>,
    pub new_instance: bool,
    pub show_help: bool,
    #[cfg(feature = "automation")]
    pub overview: bool,
    #[cfg(feature = "automation")]
    pub hide_top_bar: bool,
    #[cfg(feature = "automation")]
    pub edit_mode: bool,
    #[cfg(feature = "automation")]
    pub focus_diagram: Option<u32>,
    #[cfg(feature = "automation")]
    pub scroll_fraction: Option<f64>,
    #[cfg(feature = "automation")]
    pub smoke_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub reading_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub screenshot: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub quit_after_ms: Option<u64>,
    #[cfg(feature = "automation")]
    pub edit_knowledge_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub edit_knowledge_scene: Option<String>,
    #[cfg(feature = "automation")]
    pub wikilink_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub layout_width_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub layout_hide_sidebars: bool,
    #[cfg(feature = "automation")]
    pub live_resize_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub terminal_agent_json: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub terminal_agent_media: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub terminal_agent_vault2: Option<PathBuf>,
    #[cfg(feature = "automation")]
    pub restore_session_json: Option<PathBuf>,
}

impl CliOptions {
    /// 是否需要隔离的单次测试/自动化进程（`NON_UNIQUE`）。
    pub fn wants_non_unique(&self) -> bool {
        if self.new_instance
            || self.vault.is_some()
            || !self.open_notes.is_empty()
            || self.width.is_some()
            || self.height.is_some()
            || self.color_scheme.is_some()
        {
            return true;
        }
        #[cfg(feature = "automation")]
        if self.has_automation_mode() {
            return true;
        }
        false
    }

    #[cfg(feature = "automation")]
    pub fn has_automation_mode(&self) -> bool {
        self.smoke_json.is_some()
            || self.reading_json.is_some()
            || self.edit_knowledge_json.is_some()
            || self.edit_knowledge_scene.is_some()
            || self.wikilink_json.is_some()
            || self.layout_width_json.is_some()
            || self.live_resize_json.is_some()
            || self.terminal_agent_json.is_some()
            || self.restore_session_json.is_some()
            || self.screenshot.is_some()
            || self.focus_diagram.is_some()
            || self.edit_mode
            || self.scroll_fraction.is_some()
            || self.overview
            || self.hide_top_bar
            || self.quit_after_ms.is_some()
    }

    #[cfg(not(feature = "automation"))]
    pub fn has_automation_mode(&self) -> bool {
        false
    }

    #[cfg(feature = "automation")]
    pub fn locks_window_size(&self) -> bool {
        self.width.is_some()
            && self.height.is_some()
            && self.live_resize_json.is_none()
            && (self.screenshot.is_some()
                || self.smoke_json.is_some()
                || self.reading_json.is_some())
    }

    #[cfg(not(feature = "automation"))]
    pub fn locks_window_size(&self) -> bool {
        false
    }

    pub fn parse<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut options = Self::default();
        let mut iter = args.into_iter().map(Into::into).skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--help" | "-h" => {
                    options.show_help = true;
                    return Ok(options);
                }
                "--vault" => {
                    let value = iter.next().ok_or_else(|| {
                        crate::i18n::tr(crate::i18n::Key::CliMissingVaultArg).to_string()
                    })?;
                    options.vault = Some(PathBuf::from(value));
                }
                "--open" => {
                    let value = iter.next().ok_or_else(|| {
                        crate::i18n::tr(crate::i18n::Key::CliMissingOpenArg).to_string()
                    })?;
                    options.open_notes.push(value);
                }
                "--width" => {
                    let value = iter.next().ok_or_else(|| {
                        crate::i18n::tr(crate::i18n::Key::CliMissingWidthArg).to_string()
                    })?;
                    let parsed = value.parse::<i32>().map_err(|_| {
                        crate::i18n::fmt1(crate::i18n::Key::CliInvalidWidthArg, &value)
                    })?;
                    if parsed < 320 {
                        return Err(crate::i18n::fmt1(
                            crate::i18n::Key::CliSmallWidthArg,
                            parsed,
                        ));
                    }
                    options.width = Some(parsed);
                }
                "--height" => {
                    let value = iter.next().ok_or_else(|| {
                        crate::i18n::tr(crate::i18n::Key::CliMissingHeightArg).to_string()
                    })?;
                    let parsed = value.parse::<i32>().map_err(|_| {
                        crate::i18n::fmt1(crate::i18n::Key::CliInvalidHeightArg, &value)
                    })?;
                    if parsed < 240 {
                        return Err(crate::i18n::fmt1(
                            crate::i18n::Key::CliSmallHeightArg,
                            parsed,
                        ));
                    }
                    options.height = Some(parsed);
                }
                "--color-scheme" => {
                    let value = iter.next().ok_or_else(|| {
                        crate::i18n::tr(crate::i18n::Key::CliMissingColorSchemeArg).to_string()
                    })?;
                    options.color_scheme = Some(ColorSchemeChoice::parse(&value)?);
                }
                "--new-instance" => {
                    options.new_instance = true;
                }
                #[cfg(feature = "automation")]
                "--overview" => options.overview = true,
                #[cfg(feature = "automation")]
                "--hide-top-bar" => options.hide_top_bar = true,
                #[cfg(feature = "automation")]
                "--edit" => options.edit_mode = true,
                #[cfg(feature = "automation")]
                "--focus-diagram" => {
                    let value = iter
                        .next()
                        .ok_or("--focus-diagram requires a 1-based integer")?;
                    let parsed = value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --focus-diagram value: {value}"))?;
                    if parsed == 0 {
                        return Err("--focus-diagram must be >= 1".into());
                    }
                    options.focus_diagram = Some(parsed);
                }
                #[cfg(feature = "automation")]
                "--scroll-fraction" => {
                    let value = iter
                        .next()
                        .ok_or("--scroll-fraction requires a float in 0..1")?;
                    let parsed = value
                        .parse::<f64>()
                        .map_err(|_| format!("invalid --scroll-fraction value: {value}"))?;
                    options.scroll_fraction = Some(parsed.clamp(0.0, 1.0));
                }
                #[cfg(feature = "automation")]
                "--smoke-json" => {
                    let value = iter.next().ok_or("--smoke-json requires a path")?;
                    options.smoke_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--reading-json" => {
                    let value = iter.next().ok_or("--reading-json requires a path")?;
                    options.reading_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--screenshot" => {
                    let value = iter.next().ok_or("--screenshot requires a PNG path")?;
                    options.screenshot = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--quit-after-ms" => {
                    let value = iter.next().ok_or("--quit-after-ms requires milliseconds")?;
                    let parsed = value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --quit-after-ms value: {value}"))?;
                    options.quit_after_ms = Some(parsed);
                }
                #[cfg(feature = "automation")]
                "--edit-knowledge-json" => {
                    let value = iter.next().ok_or("--edit-knowledge-json requires a path")?;
                    options.edit_knowledge_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--edit-knowledge-scene" => {
                    let value = iter
                        .next()
                        .ok_or("--edit-knowledge-scene requires a scene name")?;
                    if !L2_SCENES.contains(&value.as_str()) {
                        return Err(format!("unknown L2 scene: {value}"));
                    }
                    options.edit_knowledge_scene = Some(value);
                }
                #[cfg(feature = "automation")]
                "--wikilink-json" => {
                    let value = iter.next().ok_or("--wikilink-json requires a path")?;
                    options.wikilink_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--layout-width-json" => {
                    let value = iter.next().ok_or("--layout-width-json requires a path")?;
                    options.layout_width_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--layout-hide-sidebars" => {
                    options.layout_hide_sidebars = true;
                }
                #[cfg(feature = "automation")]
                "--live-resize-json" => {
                    let value = iter.next().ok_or("--live-resize-json requires a path")?;
                    options.live_resize_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--terminal-agent-json" => {
                    let value = iter.next().ok_or("--terminal-agent-json requires a path")?;
                    options.terminal_agent_json = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--terminal-agent-media" => {
                    let value = iter
                        .next()
                        .ok_or("--terminal-agent-media requires a directory path")?;
                    options.terminal_agent_media = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--terminal-agent-vault2" => {
                    let value = iter
                        .next()
                        .ok_or("--terminal-agent-vault2 requires a directory path")?;
                    options.terminal_agent_vault2 = Some(PathBuf::from(value));
                }
                #[cfg(feature = "automation")]
                "--restore-session-json" => {
                    let value = iter
                        .next()
                        .ok_or("--restore-session-json requires a path")?;
                    options.restore_session_json = Some(PathBuf::from(value));
                }
                other => return Err(crate::i18n::fmt1(crate::i18n::Key::CliUnknownArg, other)),
            }
        }
        Ok(options)
    }

    pub fn usage() -> String {
        #[cfg(feature = "automation")]
        {
            format!(
                "{}{}",
                crate::i18n::tr(crate::i18n::Key::CliUsageProd),
                crate::i18n::tr(crate::i18n::Key::CliUsageAutoSuffix)
            )
        }
        #[cfg(not(feature = "automation"))]
        {
            crate::i18n::tr(crate::i18n::Key::CliUsageProd).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_options_parse_and_reject_invalid_values() {
        let parsed = CliOptions::parse([
            "hortus-gtk",
            "--vault",
            "/tmp/vault",
            "--open",
            "Start here.md",
            "--open",
            "Learning/Study loop.md",
            "--width",
            "1280",
            "--height",
            "820",
            "--color-scheme",
            "dark",
        ])
        .unwrap();
        assert_eq!(parsed.vault, Some(PathBuf::from("/tmp/vault")));
        assert_eq!(
            parsed.open_notes,
            vec!["Start here.md", "Learning/Study loop.md"]
        );
        assert_eq!(parsed.width, Some(1280));
        assert_eq!(parsed.height, Some(820));
        assert_eq!(parsed.color_scheme, Some(ColorSchemeChoice::Dark));

        assert!(CliOptions::parse(["hortus-gtk", "--unknown"]).is_err());
        assert!(CliOptions::parse(["hortus-gtk", "--width", "100"]).is_err());
        assert!(CliOptions::parse(["hortus-gtk", "--color-scheme", "sepia"]).is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn automation_cli_options_parse() {
        let parsed = CliOptions::parse([
            "hortus-gtk",
            "--overview",
            "--smoke-json",
            "/tmp/smoke.json",
            "--screenshot",
            "/tmp/out.png",
            "--live-resize-json",
            "/tmp/resize.json",
        ])
        .unwrap();
        assert!(parsed.overview);
        assert_eq!(parsed.smoke_json, Some(PathBuf::from("/tmp/smoke.json")));
        assert_eq!(parsed.screenshot, Some(PathBuf::from("/tmp/out.png")));
        assert_eq!(
            parsed.live_resize_json,
            Some(PathBuf::from("/tmp/resize.json"))
        );
    }
}
