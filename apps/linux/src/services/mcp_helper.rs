// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 内置 `hortus` CLI 路径解析与标准 MCP JSON 配置生成。

use crate::terminal::process::discover_hortus_cli_dir;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const DEFAULT_VAULT_PLACEHOLDER: &str = "/path/to/folder";

/// 解析 `hortus` 可执行文件路径：优先同级 / `HORTUS_CLI_DIR`，其次 `PATH`，否则回退 `"hortus"`。
pub fn resolve_hortus_cli_command() -> String {
    resolve_hortus_cli_command_with(
        discover_hortus_cli_dir(),
        std::env::var_os("PATH").as_deref(),
    )
}

pub fn resolve_hortus_cli_command_with(
    cli_dir: Option<PathBuf>,
    path_env: Option<&OsStr>,
) -> String {
    if let Some(dir) = cli_dir {
        let candidate = dir.join("hortus");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    if let Some(paths) = path_env {
        for dir in std::env::split_paths(paths) {
            if !dir.is_absolute() {
                continue;
            }
            let candidate = dir.join("hortus");
            if is_executable_file(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    "hortus".to_string()
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 生成与 macOS `M10-2` 一致的标准 MCP JSON 配置。
pub fn build_mcp_config_json(vault_path: Option<&str>, command: &str) -> String {
    let resolved_vault = vault_path
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_VAULT_PLACEHOLDER);
    let payload = serde_json::json!({
        "mcpServers": {
            "hortus": {
                "command": command,
                "args": ["mcp", "--vault", resolved_vault],
            }
        }
    });
    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
}

pub fn mcp_config_json_for_vault(vault_path: Option<&str>) -> String {
    let command = resolve_hortus_cli_command();
    build_mcp_config_json(vault_path, &command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_cli_priority_sibling_then_path_then_fallback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sibling_dir = tmp.path().join("sibling");
        let path_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&sibling_dir).expect("mkdir sibling");
        std::fs::create_dir_all(&path_dir).expect("mkdir bin");

        let sibling_bin = sibling_dir.join("hortus");
        let path_bin = path_dir.join("hortus");
        std::fs::write(&sibling_bin, "#!/bin/sh\n").expect("write sibling");
        std::fs::write(&path_bin, "#!/bin/sh\n").expect("write path");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&sibling_bin, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::set_permissions(&path_bin, std::fs::Permissions::from_mode(0o755));
        }

        // 1. 优先同级目录
        let cmd1 = resolve_hortus_cli_command_with(Some(sibling_dir), Some(path_dir.as_os_str()));
        assert_eq!(cmd1, sibling_bin.to_string_lossy());

        // 2. 其次 PATH
        let cmd2 = resolve_hortus_cli_command_with(None, Some(path_dir.as_os_str()));
        assert_eq!(cmd2, path_bin.to_string_lossy());

        // 3. 回退 "hortus"
        let cmd3 = resolve_hortus_cli_command_with(None, None);
        assert_eq!(cmd3, "hortus");
    }

    #[test]
    fn builds_standard_mcp_json_with_vault_or_placeholder() {
        let with_vault = build_mcp_config_json(Some("/tmp/demo-notes"), "/usr/bin/hortus");
        let parsed: serde_json::Value = serde_json::from_str(&with_vault).expect("valid json");
        assert_eq!(parsed["mcpServers"]["hortus"]["command"], "/usr/bin/hortus");
        assert_eq!(
            parsed["mcpServers"]["hortus"]["args"],
            serde_json::json!(["mcp", "--vault", "/tmp/demo-notes"])
        );

        let without_vault = build_mcp_config_json(None, "hortus");
        let parsed_empty: serde_json::Value =
            serde_json::from_str(&without_vault).expect("valid json");
        assert_eq!(parsed_empty["mcpServers"]["hortus"]["command"], "hortus");
        assert_eq!(
            parsed_empty["mcpServers"]["hortus"]["args"],
            serde_json::json!(["mcp", "--vault", "/path/to/folder"])
        );

        let empty_str = build_mcp_config_json(Some(""), "hortus");
        let parsed_blank: serde_json::Value = serde_json::from_str(&empty_str).expect("valid json");
        assert_eq!(
            parsed_blank["mcpServers"]["hortus"]["args"],
            serde_json::json!(["mcp", "--vault", "/path/to/folder"])
        );
    }

    #[test]
    fn preserves_vault_path_boundary_spaces_and_special_chars() {
        let samples = [
            " /tmp/leading-space",
            "/tmp/trailing-space ",
            " /tmp/both spaces ",
            "/tmp/my \"notes\" \\ dir ",
        ];
        for sample in samples {
            let json = build_mcp_config_json(Some(sample), "/usr/bin/hortus");
            let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
            let args = parsed["mcpServers"]["hortus"]["args"]
                .as_array()
                .expect("args array");
            assert_eq!(args.len(), 3);
            assert_eq!(args[0].as_str(), Some("mcp"));
            assert_eq!(args[1].as_str(), Some("--vault"));
            assert_eq!(args[2].as_str(), Some(sample));
        }
    }
}
