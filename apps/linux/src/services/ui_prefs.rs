// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! Linux 界面偏好持久化（`$XDG_CONFIG_HOME/hortus/linux-ui.toml` / `HORTUS_CONFIG_DIR`）。

use crate::core::ColorSchemeChoice;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const UI_PREFS_FILE: &str = "linux-ui.toml";
static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

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
    Some(config_dir.join(UI_PREFS_FILE))
}

pub fn ui_prefs_path() -> Option<PathBuf> {
    ui_prefs_path_with(env_path)
}

pub fn load_saved_color_scheme() -> ColorSchemeChoice {
    ui_prefs_path()
        .map(|path| load_color_scheme_from(&path))
        .unwrap_or(ColorSchemeChoice::System)
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

pub fn save_color_scheme(choice: ColorSchemeChoice) -> std::io::Result<()> {
    let Some(path) = ui_prefs_path() else {
        return Ok(());
    };
    save_color_scheme_to(&path, choice)
}

pub fn save_color_scheme_to(path: &Path, choice: ColorSchemeChoice) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other("ui_prefs.no_parent"));
    };
    std::fs::create_dir_all(parent)?;

    let existing = std::fs::read_to_string(path).ok();
    let updated = render_updated_toml(existing.as_deref(), choice);
    atomic_write(path, updated.as_bytes())
}

fn render_updated_toml(existing: Option<&str>, choice: ColorSchemeChoice) -> String {
    let entry = format!("color_scheme = \"{}\"", choice.as_str());
    let Some(raw) = existing else {
        return format!("# Hortus Linux UI preferences\n{entry}\n");
    };
    let mut lines = Vec::new();
    let mut replaced = false;
    for line in raw.lines() {
        let trimmed = strip_unquoted_comment(line).trim();
        let is_key = trimmed
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == "color_scheme");
        if is_key {
            if !replaced {
                lines.push(entry.clone());
                replaced = true;
            }
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.push(entry);
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("ui_prefs.no_parent"))?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(UI_PREFS_FILE);
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.tmp.{}.{seq}", std::process::id()));
    let write_res = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if write_res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_config_dir_priority_and_roundtrips_color_scheme() {
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

        let file = from_xdg.expect("xdg path");
        assert_eq!(load_color_scheme_from(&file), ColorSchemeChoice::System);

        save_color_scheme_to(&file, ColorSchemeChoice::Dark).expect("save dark");
        assert_eq!(load_color_scheme_from(&file), ColorSchemeChoice::Dark);

        save_color_scheme_to(&file, ColorSchemeChoice::Light).expect("save light");
        assert_eq!(load_color_scheme_from(&file), ColorSchemeChoice::Light);
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

        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("linux-ui.toml");
        std::fs::write(&file, "color_scheme = invalid_value\n").expect("write");
        assert_eq!(load_color_scheme_from(&file), ColorSchemeChoice::System);
        save_color_scheme_to(&file, ColorSchemeChoice::Dark).expect("overwrite");
        assert_eq!(load_color_scheme_from(&file), ColorSchemeChoice::Dark);
    }
}
