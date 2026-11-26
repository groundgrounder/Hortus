// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 外观配色持久化：统一读写 `config.toml` 的 `color_scheme`，并保留一次性从旧 `linux-ui.toml` 迁移的能力。

use crate::core::ColorSchemeChoice;
use hortus_ffi::WorkspaceCore;
use std::path::{Path, PathBuf};

const LEGACY_UI_PREFS_FILE: &str = "linux-ui.toml";

fn env_path(key: &str) -> Option<PathBuf> {
    if key == "HOME" {
        return std::env::home_dir().filter(|p| !p.as_os_str().is_empty());
    }
    std::env::var_os(key)
        .filter(|val| !val.is_empty())
        .map(PathBuf::from)
}

pub fn ui_prefs_path_with(lookup: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let get = |key: &str| lookup(key).filter(|p| !p.as_os_str().is_empty());
    let config_dir = if let Some(dir) = get("HORTUS_CONFIG_DIR") {
        dir
    } else if let Some(xdg) = get("XDG_CONFIG_HOME") {
        xdg.join("hortus")
    } else if let Some(home) = get("HOME") {
        home.join(".config").join("hortus")
    } else {
        return None;
    };
    Some(config_dir.join(LEGACY_UI_PREFS_FILE))
}

pub fn ui_prefs_path() -> Option<PathBuf> {
    ui_prefs_path_with(env_path)
}

/// 经 `WorkspaceCore` 读取外观配色；若 `config.toml` 尚未显式设置 `color_scheme` 且旧 `linux-ui.toml`
/// 存在有效非 `system` 值，则一次性迁移写回 `config.toml`。
pub fn load_color_scheme_with_core(core: &WorkspaceCore) -> ColorSchemeChoice {
    let current = core
        .color_scheme()
        .ok()
        .and_then(|s| ColorSchemeChoice::parse(s.trim()).ok())
        .unwrap_or(ColorSchemeChoice::System);

    if current != ColorSchemeChoice::System {
        return current;
    }

    let config_path = core
        .editable_config()
        .ok()
        .map(|info| PathBuf::from(info.config_path));
    let Some(config_path) = config_path else {
        return current;
    };
    if config_has_explicit_color_scheme(&config_path) {
        return current;
    }

    let Some(legacy_path) = config_path
        .parent()
        .map(|dir| dir.join(LEGACY_UI_PREFS_FILE))
    else {
        return current;
    };
    let legacy = load_color_scheme_from(&legacy_path);
    if legacy != ColorSchemeChoice::System {
        let _ = core.save_color_scheme(legacy.as_str().to_string());
        return legacy;
    }
    current
}

/// 在尚无共享 `WorkspaceCore` 实例时读取（并按需迁移）外观配色。
pub fn load_saved_color_scheme() -> ColorSchemeChoice {
    if let Ok(core) = WorkspaceCore::new() {
        return load_color_scheme_with_core(&core);
    }
    ui_prefs_path()
        .map(|path| load_color_scheme_from(&path))
        .unwrap_or(ColorSchemeChoice::System)
}

/// 经 `WorkspaceCore` 持久化外观配色至 `config.toml`，不再写 `linux-ui.toml`。
pub fn save_color_scheme_with_core(
    core: &WorkspaceCore,
    choice: ColorSchemeChoice,
) -> Result<(), String> {
    core.save_color_scheme(choice.as_str().to_string())
        .map_err(|e| e.to_string())
}

/// 在无现成 `WorkspaceCore` 时打开默认配置并保存外观配色至 `config.toml`。
pub fn save_color_scheme(choice: ColorSchemeChoice) -> Result<(), String> {
    let core = WorkspaceCore::new().map_err(|e| e.to_string())?;
    save_color_scheme_with_core(&core, choice)
}

fn config_has_explicit_color_scheme(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    for line in raw.lines() {
        let trimmed = strip_unquoted_comment(line).trim();
        if trimmed.starts_with('[') {
            // 顶层键必须在首个表头之前
            break;
        }
        if let Some((key, _)) = trimmed.split_once('=')
            && key.trim() == "color_scheme"
        {
            return true;
        }
    }
    false
}

pub fn load_color_scheme_from(path: &Path) -> ColorSchemeChoice {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return ColorSchemeChoice::System;
    };
    parse_color_scheme_toml(&raw)
}

pub fn parse_color_scheme_toml(raw: &str) -> ColorSchemeChoice {
    let mut found = ColorSchemeChoice::System;
    for line in raw.lines() {
        let trimmed = strip_unquoted_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, val)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() != "color_scheme" {
            continue;
        }
        if let Some(unquoted) = unquote_toml_string(val.trim())
            && let Ok(parsed) = ColorSchemeChoice::parse(unquoted.trim())
        {
            found = parsed;
        } else {
            // 格式损坏时安全回退为 system。
            found = ColorSchemeChoice::System;
        }
    }
    found
}

fn strip_unquoted_comment(line: &str) -> &str {
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            }
        } else {
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '#' => return &line[..idx],
                _ => {}
            }
        }
    }
    line
}

fn unquote_toml_string(val: &str) -> Option<&str> {
    let is_quoted = (val.starts_with('"') && val.ends_with('"'))
        || (val.starts_with('\'') && val.ends_with('\''));
    if val.len() >= 2 && is_quoted {
        Some(&val[1..val.len() - 1])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_config_dir_priority() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h_dir = dir.path().join("custom-hortus");
        let xdg_dir = dir.path().join("xdg");
        let home_dir = dir.path().join("home");

        let from_hortus = ui_prefs_path_with(|k| match k {
            "HORTUS_CONFIG_DIR" => Some(h_dir.clone()),
            "XDG_CONFIG_HOME" => Some(xdg_dir.clone()),
            "HOME" => Some(home_dir.clone()),
            _ => None,
        });
        assert_eq!(from_hortus, Some(h_dir.join("linux-ui.toml")));

        let from_xdg = ui_prefs_path_with(|k| match k {
            "XDG_CONFIG_HOME" => Some(xdg_dir.clone()),
            "HOME" => Some(home_dir.clone()),
            _ => None,
        });
        assert_eq!(from_xdg, Some(xdg_dir.join("hortus/linux-ui.toml")));
    }

    #[test]
    fn migrates_legacy_linux_ui_toml_once_and_persists_to_config_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&cfg).expect("mkdir cfg");
        let legacy_path = cfg.join("linux-ui.toml");
        std::fs::write(&legacy_path, "color_scheme = \"dark\"\n").expect("write legacy");

        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .expect("core");

        // 首次加载触发一次性迁移并写回 config.toml
        assert_eq!(load_color_scheme_with_core(&core), ColorSchemeChoice::Dark);
        assert_eq!(core.color_scheme().expect("color_scheme"), "dark");

        // 显式切回 system 后，即使旧 linux-ui.toml 仍是 dark 也不再重复覆盖
        save_color_scheme_with_core(&core, ColorSchemeChoice::System).expect("save system");
        assert_eq!(
            load_color_scheme_with_core(&core),
            ColorSchemeChoice::System
        );
        // 后续保存不改写 linux-ui.toml
        save_color_scheme_with_core(&core, ColorSchemeChoice::Light).expect("save light");
        assert_eq!(core.color_scheme().expect("color_scheme"), "light");
        assert_eq!(
            std::fs::read_to_string(&legacy_path).expect("read legacy"),
            "color_scheme = \"dark\"\n"
        );
    }

    #[test]
    fn malformed_toml_falls_back_to_system_safely() {
        assert_eq!(
            parse_color_scheme_toml("color_scheme = \"neon\"\n"),
            ColorSchemeChoice::System
        );
        assert_eq!(
            parse_color_scheme_toml("[[[ broken toml ==="),
            ColorSchemeChoice::System
        );
        assert_eq!(
            parse_color_scheme_toml("other_key = 42\ncolor_scheme = dark # unquoted"),
            ColorSchemeChoice::System
        );
        assert_eq!(
            parse_color_scheme_toml("other_key = 'keep'\ncolor_scheme = 'dark' # valid"),
            ColorSchemeChoice::Dark
        );
    }
}
