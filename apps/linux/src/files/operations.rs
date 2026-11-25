// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 文件树操作辅助：设备检测、跨库安全目录选取、路径归属判定与错误信息摘要。

use std::path::{Path, PathBuf};

/// 原子保存与冲突副本的临时目录：优先 `HORTUS_CACHE_DIR`，始终位于笔记库之外。
pub fn prepare_edit_temp_dir() -> PathBuf {
    let base = std::env::var_os("HORTUS_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let candidate = base.join("edit-tmp");
    if std::fs::create_dir_all(&candidate).is_ok() && candidate.is_dir() {
        candidate
    } else {
        std::env::temp_dir()
    }
}

pub fn device_of(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|meta| meta.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

pub fn save_temp_for(temp_dir: &Path, vault_root: &str, rel: &str) -> PathBuf {
    let note_abs = Path::new(vault_root).join(rel);
    let parent = note_abs.parent().unwrap_or(Path::new(vault_root));
    let parent_device = device_of(parent);
    let root = Path::new(vault_root);
    let mut candidates = vec![temp_dir.to_path_buf(), std::env::temp_dir()];
    if let Some(grand) = root.parent() {
        candidates.push(grand.join(".hortus-save"));
    }
    for candidate in candidates {
        if candidate.starts_with(root) {
            continue;
        }
        if !candidate.is_dir() && std::fs::create_dir_all(&candidate).is_err() {
            continue;
        }
        if parent_device.is_none() || device_of(&candidate) == parent_device {
            return candidate;
        }
    }
    std::env::temp_dir()
}

/// 错误提示只保留简短原因，避免长路径/英文 io 错误把通知条撑宽。
pub fn short_error(prefix: &str, error: &str) -> String {
    let sep = if crate::i18n::current_lang() == crate::i18n::Lang::ZhHans {
        "："
    } else {
        ": "
    };
    let reason: String = error.chars().take(32).collect();
    if error.chars().count() > 32 {
        format!("{prefix}{sep}{reason}…")
    } else {
        format!("{prefix}{sep}{reason}")
    }
}

#[cfg(feature = "automation")]
pub fn count_dir_files(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}
