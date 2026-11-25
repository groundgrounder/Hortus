// Hortus - Linux GTK4 + libadwaita 原生界面
// Copyright (C) 2026  groundgrounder
// SPDX-License-Identifier: GPL-3.0-only

//! 终端环境构造、`hortus` CLI 路径发现与子进程组安全清理。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 发现可用 `hortus` CLI 目录：先查同级可执行文件目录，再查 `HORTUS_CLI_DIR`。
pub fn discover_hortus_cli_dir() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("hortus");
        if sibling.is_file() {
            return Some(dir.to_path_buf());
        }
    }
    if let Some(dir) = std::env::var_os("HORTUS_CLI_DIR").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(dir);
        if path.join("hortus").is_file() {
            return Some(path);
        }
    }
    None
}

/// 解析用户默认 Shell（读取 `$SHELL`，兜底 `/bin/sh`）。
pub fn user_shell_program() -> String {
    std::env::var("SHELL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && Path::new(s).exists())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

/// 带超时的子进程标准输出读取：超时后主动 `kill()` 并 `wait()` 回收，避免启动阻塞。
pub fn run_command_with_timeout(
    cmd: &mut Command,
    timeout: std::time::Duration,
) -> Option<Vec<u8>> {
    use std::io::Read;
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        })
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let bytes = reader
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                return status.success().then_some(bytes);
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(handle) = reader {
                        let _ = handle.join();
                    }
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(handle) = reader {
                    let _ = handle.join();
                }
                return None;
            }
        }
    }
}

/// 探测登录 shell PATH（供桌面菜单启动等缺失终端 PATH 的场景使用；非关键路径，1.5s 超时保护）。
pub fn resolve_login_path() -> Option<String> {
    let shell = user_shell_program();
    let name = Path::new(&shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("sh");
    if !matches!(name, "sh" | "bash" | "zsh" | "fish") {
        return None;
    }
    let mut cmd = Command::new(&shell);
    cmd.args(["-lc", "printf '%s' \"$PATH\""]);
    let bytes = run_command_with_timeout(&mut cmd, std::time::Duration::from_millis(1500))?;
    let path = String::from_utf8(bytes).ok()?;
    let trimmed = path.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn has_utf8_locale(val: Option<&String>) -> bool {
    val.is_some_and(|v| {
        let upper = v.to_ascii_uppercase();
        upper.contains("UTF-8") || upper.contains("UTF8")
    })
}

/// 构造子进程环境变量列表（`KEY=VALUE`）：保证 UTF-8、`TERM=xterm-256color`，并将 `hortus` CLI 目录追加至 `PATH` 尾部。
pub fn build_terminal_env(
    vault_root: Option<&str>,
    agent_session_id: Option<&str>,
    effective_path: Option<&str>,
) -> Vec<String> {
    build_terminal_env_from_vars(
        std::env::vars_os(),
        vault_root,
        agent_session_id,
        effective_path,
    )
}

pub fn build_terminal_env_from_vars(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    vault_root: Option<&str>,
    agent_session_id: Option<&str>,
    effective_path: Option<&str>,
) -> Vec<String> {
    let mut env_map: BTreeMap<String, String> = vars
        .into_iter()
        .filter_map(|(k, v)| {
            let key = k.into_string().ok()?;
            let val = v.to_string_lossy().into_owned();
            Some((key, val))
        })
        .collect();

    // 剥离父进程遗留的多余隔离变量
    env_map.remove("HORTUS_AGENT_SESSION");

    env_map.insert("TERM".into(), "xterm-256color".into());
    if !has_utf8_locale(env_map.get("LC_ALL")) && !has_utf8_locale(env_map.get("LANG")) {
        env_map.insert("LANG".into(), "C.UTF-8".into());
    }

    let mut path_parts: Vec<String> = effective_path
        .map(str::to_string)
        .or_else(|| env_map.get("PATH").cloned())
        .unwrap_or_else(|| "/usr/bin:/bin".into())
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    if let Some(cli_dir) = discover_hortus_cli_dir() {
        let cli_str = cli_dir.to_string_lossy().into_owned();
        if !path_parts.iter().any(|p| p == &cli_str) {
            path_parts.push(cli_str);
        }
    }
    env_map.insert("PATH".into(), path_parts.join(":"));

    if let Some(root) = vault_root.filter(|r| !r.is_empty()) {
        env_map.insert("HORTUS_VAULT".into(), root.to_string());
    }
    if let Some(id) = agent_session_id.filter(|s| !s.is_empty()) {
        env_map.insert("HORTUS_AGENT_SESSION".into(), id.to_string());
    }

    env_map
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect()
}

/// 检查子进程是否仍在存活（排除僵尸进程）。
pub fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    unsafe {
        // 先尝试非阻塞回收可能已经结束的直接子进程，避免僵尸状态误判存活。
        let mut status: libc::c_int = 0;
        let _ = libc::waitpid(pid, &mut status, libc::WNOHANG);
        libc::kill(pid, 0) == 0
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// 向子进程组发 SIGTERM；若 `force_kill` 为真则直接 SIGKILL 并 waitpid 回收。
pub fn signal_process_group(pid: i32, sigkill: bool) {
    if pid <= 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        let sig = if sigkill {
            libc::SIGKILL
        } else {
            libc::SIGTERM
        };
        let pgid = libc::getpgid(pid);
        if pgid > 0 {
            let _ = libc::kill(-pgid, sig);
        }
        let _ = libc::kill(-pid, sig);
        let _ = libc::kill(pid, sig);
        let mut status: libc::c_int = 0;
        let _ = libc::waitpid(pid, &mut status, libc::WNOHANG);
    }
}

/// 同步优雅终止一组进程（关闭窗口/退出应用时使用：先 SIGTERM，短等待后对仍存活者发 SIGKILL 并回收）。
pub fn terminate_pids_sync(pids: &[i32]) {
    let active: Vec<i32> = pids.iter().copied().filter(|&p| is_pid_alive(p)).collect();
    if active.is_empty() {
        return;
    }
    for &pid in &active {
        signal_process_group(pid, false);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(180);
    while std::time::Instant::now() < deadline {
        if active.iter().all(|&pid| !is_pid_alive(pid)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    for &pid in &active {
        if is_pid_alive(pid) {
            signal_process_group(pid, true);
        }
    }
    let kill_deadline = std::time::Instant::now() + std::time::Duration::from_millis(120);
    while std::time::Instant::now() < kill_deadline {
        if active.iter().all(|&pid| !is_pid_alive(pid)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn build_terminal_env_tolerates_non_utf8_os_strings() {
        use std::os::unix::ffi::OsStringExt;

        let vars = vec![
            (
                OsString::from_vec(vec![0xff, b'B', b'A', b'D']),
                OsString::from("ignored"),
            ),
            (
                OsString::from("BAD_VAL"),
                OsString::from_vec(vec![b'a', b'/', 0xfe, b'b']),
            ),
            (OsString::from("OK_VAR"), OsString::from("value")),
        ];
        let env = build_terminal_env_from_vars(
            vars,
            Some("/tmp/vault"),
            Some("sess-1"),
            Some("/usr/bin:/bin"),
        );
        assert!(env.iter().any(|item| item == "OK_VAR=value"));
        assert!(env.iter().any(|item| item.starts_with("BAD_VAL=a/")));
        assert!(env.iter().any(|item| item == "HORTUS_VAULT=/tmp/vault"));
        assert!(env.iter().any(|item| item == "HORTUS_AGENT_SESSION=sess-1"));
        assert!(!env.iter().any(|item| item.contains("ignored")));
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_kills_stalled_process_without_blocking() {
        if !Path::new("/bin/sh").exists() {
            return;
        }
        let mut ok_cmd = Command::new("/bin/sh");
        ok_cmd.args(["-c", "printf 'ok'"]);
        let out = run_command_with_timeout(&mut ok_cmd, std::time::Duration::from_millis(800));
        assert_eq!(out.as_deref(), Some(b"ok".as_slice()));

        let started = std::time::Instant::now();
        let mut slow_cmd = Command::new("/bin/sh");
        slow_cmd.args(["-c", "sleep 5"]);
        let slow_out =
            run_command_with_timeout(&mut slow_cmd, std::time::Duration::from_millis(120));
        assert!(slow_out.is_none());
        assert!(started.elapsed() < std::time::Duration::from_millis(1000));
    }
}
