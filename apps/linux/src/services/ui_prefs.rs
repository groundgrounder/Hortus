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
/// 存在有效非 `system` 值，则一次性迁移写回 `config.toml`。写入失败时返回错误供调用方处理。
pub fn load_color_scheme_with_core_result(
    core: &WorkspaceCore,
) -> Result<ColorSchemeChoice, String> {
    match core.explicit_color_scheme() {
        Ok(Some(scheme)) => {
            Ok(ColorSchemeChoice::parse(scheme.trim()).unwrap_or(ColorSchemeChoice::System))
        }
        Ok(None) => {
            let Some(config_path) = core
                .editable_config()
                .ok()
                .map(|info| PathBuf::from(info.config_path))
            else {
                return Ok(ColorSchemeChoice::System);
            };
            let Some(legacy_path) = config_path
                .parent()
                .map(|dir| dir.join(LEGACY_UI_PREFS_FILE))
            else {
                return Ok(ColorSchemeChoice::System);
            };
            let legacy = load_color_scheme_from(&legacy_path);
            if legacy != ColorSchemeChoice::System {
                core.save_color_scheme(legacy.as_str().to_string())
                    .map_err(|e| e.to_string())?;
                return Ok(legacy);
            }
            Ok(ColorSchemeChoice::System)
        }
        Err(_) => Ok(ColorSchemeChoice::System),
    }
}

/// 经 `WorkspaceCore` 读取外观配色；迁移写回失败时记录日志并安全回退 `System`，不静默伪装已迁移。
pub fn load_color_scheme_with_core(core: &WorkspaceCore) -> ColorSchemeChoice {
    match load_color_scheme_with_core_result(core) {
        Ok(choice) => choice,
        Err(err) => {
            eprintln!("hortus: failed to migrate legacy color_scheme: {err}");
            ColorSchemeChoice::System
        }
    }
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

/// 经 `WorkspaceCore` 原子持久化外观配色并返回写前/写后摘要 `(before_digest, after_digest)`。
pub fn save_color_scheme_with_digests(
    core: &WorkspaceCore,
    choice: ColorSchemeChoice,
) -> Result<(String, String), String> {
    core.save_color_scheme_with_digests(choice.as_str().to_string())
        .map_err(|e| e.to_string())
}

/// 在无现成 `WorkspaceCore` 时打开默认配置并保存外观配色至 `config.toml`。
pub fn save_color_scheme(choice: ColorSchemeChoice) -> Result<(), String> {
    let core = WorkspaceCore::new().map_err(|e| e.to_string())?;
    save_color_scheme_with_core(&core, choice)
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

    #[test]
    fn explicit_quoted_key_blocks_legacy_migration() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&cfg).expect("mkdir cfg");
        let initial_toml = "\"color_scheme\" = \"system\"\nfont_size = 15.0\n";
        std::fs::write(cfg.join("config.toml"), initial_toml).expect("write config");
        std::fs::write(cfg.join("linux-ui.toml"), "color_scheme = \"dark\"\n")
            .expect("write legacy");

        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .expect("core");

        assert_eq!(
            load_color_scheme_with_core(&core),
            ColorSchemeChoice::System
        );
        assert_eq!(
            std::fs::read_to_string(cfg.join("config.toml")).expect("read config"),
            initial_toml
        );
    }

    #[test]
    fn commented_color_scheme_migrates_once_and_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&cfg).expect("mkdir cfg");
        std::fs::write(
            cfg.join("config.toml"),
            "# color_scheme = \"dark\"\nfont_size = 15.0\n",
        )
        .expect("write config");
        std::fs::write(cfg.join("linux-ui.toml"), "color_scheme = \"dark\"\n")
            .expect("write legacy");

        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .expect("core");

        assert_eq!(load_color_scheme_with_core(&core), ColorSchemeChoice::Dark);
        let after_first = std::fs::read_to_string(cfg.join("config.toml")).expect("read config");
        assert!(after_first.contains("color_scheme = \"dark\""));

        // 连续两次加载幂等
        assert_eq!(load_color_scheme_with_core(&core), ColorSchemeChoice::Dark);
        let after_second = std::fs::read_to_string(cfg.join("config.toml")).expect("read config");
        assert_eq!(after_first, after_second);
    }

    #[test]
    fn explicit_invalid_color_scheme_falls_back_without_legacy_overwrite() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&cfg).expect("mkdir cfg");
        let initial_toml = "color_scheme = \"neon\"\nfont_size = 15.0\n";
        std::fs::write(cfg.join("config.toml"), initial_toml).expect("write config");
        std::fs::write(cfg.join("linux-ui.toml"), "color_scheme = \"dark\"\n")
            .expect("write legacy");

        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .expect("core");

        assert_eq!(
            load_color_scheme_with_core(&core),
            ColorSchemeChoice::System
        );
        assert_eq!(
            std::fs::read_to_string(cfg.join("config.toml")).expect("read config"),
            initial_toml
        );
    }

    #[cfg(unix)]
    #[test]
    fn migration_write_failure_surfaces_error_and_falls_back_to_system() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("cfg");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&cfg).expect("mkdir cfg");
        std::fs::write(cfg.join("config.toml"), "font_size = 15.0\n").expect("write config");
        std::fs::write(cfg.join("linux-ui.toml"), "color_scheme = \"dark\"\n")
            .expect("write legacy");

        let core = WorkspaceCore::in_directories(
            cfg.to_string_lossy().into_owned(),
            cache.to_string_lossy().into_owned(),
        )
        .expect("core");

        // 设为只读目录以触发原子写回失败（root 用户不受 chmod 限制时跳过断言）
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o555))
            .expect("chmod readonly");
        let probe_writable = std::fs::write(cfg.join(".probe"), b"x").is_ok();
        if probe_writable {
            let _ = std::fs::remove_file(cfg.join(".probe"));
        } else {
            assert!(load_color_scheme_with_core_result(&core).is_err());
            assert_eq!(
                load_color_scheme_with_core(&core),
                ColorSchemeChoice::System
            );
        }
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o755))
            .expect("restore perms");
    }
}
