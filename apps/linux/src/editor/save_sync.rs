// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 磁盘指纹对账与文档保存辅助。

use std::path::Path;

pub fn read_disk_stamp(vault_root: &str, path: &str) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(Path::new(vault_root).join(path)).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    Some((meta.len(), modified))
}
