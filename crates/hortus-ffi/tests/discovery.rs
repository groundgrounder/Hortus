//! Agent 发现、带登录 PATH 的探测与启动解析的 FFI 边界。
//!
//! 这个测试二进制只包含一个用例：它需要在构造 Core 前修改进程环境，
//! 与其他测试并行会互相覆盖。
use hortus_ffi::WorkspaceCore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn write_executable(path: &Path, body: &str, mode: u32) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn discovery_and_path_augmented_resolution_never_run_a_preset() {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    let cache = root.path().join("cache");
    let data = root.path().join("data");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("config.toml"),
        "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"omp\"\nargs = [\"--append-system-prompt={guide}\"]\n",
    )
    .unwrap();
    // 该测试二进制只包含这一个用例；环境变量在构造 Core 前设置。
    unsafe {
        std::env::set_var("HOME", root.path());
        std::env::set_var("HORTUS_CONFIG_DIR", &config);
        std::env::set_var("HORTUS_CACHE_DIR", &cache);
        std::env::set_var("HORTUS_DATA_DIR", &data);
        // 极简进程 PATH；本机安装的 Agent 不能影响断言。
        std::env::set_var("PATH", "/usr/bin:/bin");
    }
    let core = WorkspaceCore::new().unwrap();

    let login = root.path().join("login-bin");
    fs::create_dir(&login).unwrap();
    let marker = root.path().join("trap-marker");
    write_executable(&login.join("omp"), "#!/bin/sh\nexit 0\n", 0o755);
    write_executable(&login.join("codex"), "#!/bin/sh\nexit 0\n", 0o755);
    write_executable(
        &login.join("gemini"),
        &format!("#!/bin/sh\nprintf '%s' ran > '{}'\n", marker.display()),
        0o755,
    );
    write_executable(&login.join("aider"), "not executable\n", 0o644);
    let login_path = login.to_string_lossy().into_owned();

    let agents = core.discover_agents(Some(login_path.clone()));
    let ids: Vec<_> = agents.iter().map(|agent| agent.id.as_str()).collect();
    // 组合路径仍会包含本机常见安装目录；只断言注入目录里的条目。
    for expected in ["omp", "codex", "gemini"] {
        assert!(ids.contains(&expected), "{ids:?}");
    }
    let canonical_omp = fs::canonicalize(login.join("omp"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let omp = agents.iter().find(|agent| agent.id == "omp").unwrap();
    assert_eq!(omp.executable, canonical_omp);
    // 不可执行的同名文件不能被当作可用的 Agent。
    let aider = fs::canonicalize(login.join("aider"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(agents.iter().all(|agent| agent.executable != aider));
    assert_eq!(agents[0].display_name, "OMP");
    assert_eq!(
        agents[0].suggested_args,
        vec!["--append-system-prompt={guide}".to_string()]
    );
    assert!(agents.iter().all(|agent| {
        agent
            .suggested_args
            .iter()
            .any(|arg| arg.contains("{guide}"))
    }));
    assert!(
        !marker.exists(),
        "discovery must never execute a discovered preset"
    );

    let probe = core.probe_agent_launch_with_path(
        "omp".into(),
        vec!["--append-system-prompt={guide}".into()],
        Some(login_path.clone()),
    );
    assert!(probe.available, "{probe:?}");
    assert_eq!(probe.executable.as_deref(), Some(canonical_omp.as_str()));

    let missing = core.probe_agent_launch(
        "hortus-definitely-missing-agent".into(),
        vec!["{guide}".into()],
    );
    assert!(!missing.available);
    assert_eq!(missing.error.as_deref(), Some("agent.not_found"));

    let spec = core.agent_spec_with_path(Some(login_path.clone())).unwrap();
    assert_eq!(spec.executable, canonical_omp);
    assert_eq!(spec.args.len(), 1);
    assert!(spec.args[0].starts_with("--append-system-prompt=# Working in a Hortus"));

    // 极简进程 PATH 下仍返回包含登录目录与常见安装目录的子进程 PATH。
    let path = core.effective_agent_path(Some(login_path.clone()));
    let entries: Vec<_> = std::env::split_paths(&path).collect();
    assert!(entries.contains(&login));
    assert!(entries.contains(&Path::new("/opt/homebrew/bin").to_path_buf()));
    assert!(entries.contains(&root.path().join(".cargo/bin")));
    assert!(entries.contains(&root.path().join(".local/bin")));
    assert!(
        entries
            .iter()
            .all(|entry| entry.is_absolute() && !entry.as_os_str().is_empty())
    );
    assert!(!marker.exists());
}
