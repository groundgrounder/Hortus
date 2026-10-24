use hortus_config::*;
use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

const LEGACY_CONFIG_ENV: &str = "MARKNOTE_CONFIG_DIR";
const LEGACY_CACHE_ENV: &str = "MARKNOTE_CACHE_DIR";

fn store(root: &TempDir) -> Store {
    Store::open(Directories {
        config: root.path().join("config"),
        cache: root.path().join("cache"),
    })
    .unwrap()
}

fn layout() -> LayoutState {
    LayoutState {
        regions: BTreeMap::from([
            (
                "left".into(),
                RegionLayout {
                    groups: vec![PanelGroup {
                        id: "1".into(),
                        tabs: vec!["files".into(), "search".into(), "tags".into()],
                        selected: "files".into(),
                    }],
                    visible: true,
                    size: 260.0,
                },
            ),
            (
                "right".into(),
                RegionLayout {
                    groups: vec![PanelGroup {
                        id: "2".into(),
                        tabs: vec!["outline".into(), "backlinks".into()],
                        selected: "outline".into(),
                    }],
                    visible: true,
                    size: 250.0,
                },
            ),
            (
                "bottom".into(),
                RegionLayout {
                    groups: vec![PanelGroup {
                        id: "3".into(),
                        tabs: vec!["terminal".into(), "changes".into()],
                        selected: "terminal".into(),
                    }],
                    visible: false,
                    size: 230.0,
                },
            ),
        ]),
    }
}

#[test]
fn settings_layout_and_session_round_trip_and_preserve_comments() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    assert_eq!(store.settings().unwrap().font_size, 15.0);
    assert!(
        fs::read_to_string(store.directories.config.join("config.toml"))
            .unwrap()
            .starts_with('#')
    );
    fs::write(
        store.directories.config.join("config.toml"),
        "# custom\nfont_size = 16.0 # inline\ncustom = true\n",
    )
    .unwrap();
    store
        .save_settings(&Settings {
            font_size: 18.0,
            default_view: ViewMode::Reading,
        })
        .unwrap();
    assert_eq!(store.settings().unwrap().font_size, 18.0);
    let text = fs::read_to_string(store.directories.config.join("config.toml")).unwrap();
    assert!(
        text.contains("# custom") && text.contains("# inline") && text.contains("custom = true")
    );
    assert!(store.layout().unwrap().is_none());
    let layout = layout();
    store.save_layout(&layout).unwrap();
    assert_eq!(store.layout().unwrap(), Some(layout));
    let vault = VaultState {
        path: "/tmp/sample".into(),
        tabs: vec!["2.md".into(), "10.md".into()],
        selected: Some("10.md".into()),
        views: BTreeMap::new(),
    };
    let expected = store.remember(vault.clone()).unwrap();
    store.remember(vault).unwrap();
    assert_eq!(store.session().unwrap(), expected);
    assert_eq!(fs::read_dir(&store.directories.config).unwrap().count(), 2);
    assert_eq!(fs::read_dir(&store.directories.cache).unwrap().count(), 1);
}

#[test]
fn directory_overrides_and_defaults_do_not_require_process_environment_mutation() {
    let root = TempDir::new().unwrap();
    let map = BTreeMap::from([
        ("HOME", root.path().join("home")),
        ("HORTUS_CONFIG_DIR", root.path().join("settings")),
        ("HORTUS_CACHE_DIR", root.path().join("sessions")),
    ]);
    let dirs = Directories::from_lookup(|key| map.get(key).cloned()).unwrap();
    assert_eq!(dirs.config, root.path().join("settings"));
    assert_eq!(dirs.cache, root.path().join("sessions"));
    let defaults =
        Directories::from_lookup(|key| (key == "HOME").then(|| root.path().to_owned())).unwrap();
    assert_eq!(defaults.config, root.path().join(".config/hortus"));
    if cfg!(target_os = "macos") {
        assert_eq!(defaults.cache, root.path().join("Library/Caches/Hortus"));
    }
    assert!(Directories::from_lookup(|_| None).is_err());
    assert!(Directories::from_lookup(|_| Some("relative".into())).is_err());
    // launchctl setenv 留下的空覆盖值视同未设置，回退到默认目录。
    let empty = Directories::from_lookup(|key| match key {
        "HOME" => Some(root.path().to_owned()),
        _ => Some(std::path::PathBuf::new()),
    })
    .unwrap();
    assert_eq!(empty.config, defaults.config);
    assert_eq!(empty.cache, defaults.cache);
}

#[test]
fn real_environment_overrides_are_tested_in_an_isolated_subprocess() {
    let root = TempDir::new().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "environment_child", "--nocapture"])
        .env("HORTUS_TEST_CHILD", "1")
        .env_remove(LEGACY_CONFIG_ENV)
        .env_remove(LEGACY_CACHE_ENV)
        .env("HOME", root.path().join("home"))
        .env("HORTUS_CONFIG_DIR", root.path().join("settings"))
        .env("HORTUS_CACHE_DIR", root.path().join("sessions"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.path().join("settings/config.toml").exists());
    assert!(!root.path().join("home").exists());
}

#[test]
fn environment_child() {
    if std::env::var_os("HORTUS_TEST_CHILD").is_none() {
        return;
    }
    let dirs = Directories::from_env().unwrap();
    if std::env::var_os("HORTUS_TEST_EXPECT_DEFAULTS").is_some() {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(dirs.config, home.join(".config/hortus"));
        Store::open(dirs).unwrap();
        return;
    }
    assert_eq!(
        dirs.config,
        std::path::PathBuf::from(std::env::var_os("HORTUS_CONFIG_DIR").unwrap())
    );
    assert_eq!(
        dirs.cache,
        std::path::PathBuf::from(std::env::var_os("HORTUS_CACHE_DIR").unwrap())
    );
    Store::open(dirs).unwrap();
}

fn isolated_environment(root: &TempDir) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "environment_child", "--nocapture"])
        .env("HORTUS_TEST_CHILD", "1")
        .env("HOME", root.path().join("home"))
        .env("HORTUS_CONFIG_DIR", root.path().join("config"))
        .env("HORTUS_CACHE_DIR", root.path().join("cache"))
        .env_remove(LEGACY_CONFIG_ENV)
        .env_remove(LEGACY_CACHE_ENV);
    command
}

#[cfg(unix)]
#[test]
fn missing_or_empty_home_environment_does_not_prevent_startup() {
    for home in [None, Some("")] {
        let root = TempDir::new().unwrap();
        let mut command = isolated_environment(&root);
        command.env_remove("HOME");
        if let Some(home) = home {
            command.env("HOME", home);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(root.path().join("config/config.toml").exists());
        assert!(root.path().join("cache").is_dir());
        assert!(!root.path().join("home").exists());
    }
}

#[test]
fn empty_override_environment_falls_back_to_default_directories() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    let output = isolated_environment(&root)
        .env("HOME", &home)
        .env("HORTUS_CONFIG_DIR", "")
        .env("HORTUS_CACHE_DIR", "")
        .env_remove("XDG_CACHE_HOME")
        .env("HORTUS_TEST_EXPECT_DEFAULTS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.join(".config/hortus/config.toml").exists());
    assert!(!root.path().join("config").exists());
}

#[test]
fn simultaneous_directories_use_new_state_and_leave_old_bytes_unchanged() {
    let root = TempDir::new().unwrap();
    let current = store(&root);
    current
        .save_settings(&Settings {
            font_size: 20.0,
            default_view: ViewMode::Reading,
        })
        .unwrap();
    for name in ["old-config", "old-cache"] {
        fs::create_dir(root.path().join(name)).unwrap();
        fs::write(root.path().join(name).join("sentinel"), b"keep old bytes").unwrap();
    }
    let output = isolated_environment(&root)
        .env(LEGACY_CONFIG_ENV, root.path().join("old-config"))
        .env(LEGACY_CACHE_ENV, root.path().join("old-cache"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(current.settings().unwrap().font_size, 20.0);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("不合并")
            .count(),
        2
    );
    for name in ["old-config", "old-cache"] {
        assert_eq!(
            fs::read(root.path().join(name).join("sentinel")).unwrap(),
            b"keep old bytes"
        );
    }
    assert!(!root.path().join("config/sentinel").exists());
    assert!(!root.path().join("cache/sentinel").exists());
}

#[test]
fn failed_directory_rename_preserves_recovery_and_can_retry() {
    let root = TempDir::new().unwrap();
    let old = root.path().join("old-cache");
    fs::create_dir_all(old.join("recovery")).unwrap();
    fs::write(old.join("recovery/draft.json"), b"unsaved draft").unwrap();
    // 目标父路径是文件，确定性模拟无法改名，避免依赖测试进程的权限。
    fs::write(root.path().join("blocked"), b"keep parent").unwrap();
    let output = isolated_environment(&root)
        .env(LEGACY_CACHE_ENV, &old)
        .env("HORTUS_CACHE_DIR", root.path().join("blocked/cache"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read(old.join("recovery/draft.json")).unwrap(),
        b"unsaved draft"
    );
    assert_eq!(
        fs::read(root.path().join("blocked")).unwrap(),
        b"keep parent"
    );
    let retry = isolated_environment(&root)
        .env(LEGACY_CACHE_ENV, &old)
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(!old.exists());
    assert_eq!(
        fs::read(root.path().join("cache/recovery/draft.json")).unwrap(),
        b"unsaved draft"
    );
}

#[test]
fn new_overrides_never_migrate_personal_defaults() {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    let legacy_config = home.join(".config/marknote");
    let legacy_cache = if cfg!(target_os = "macos") {
        home.join("Library/Caches/MarkNote")
    } else {
        home.join(".cache/marknote")
    };
    for old in [&legacy_config, &legacy_cache] {
        fs::create_dir_all(old).unwrap();
        fs::write(old.join("sentinel"), b"personal state").unwrap();
    }
    let output = isolated_environment(&root).output().unwrap();
    assert!(output.status.success());
    assert!(legacy_config.join("sentinel").exists());
    assert!(legacy_cache.join("sentinel").exists());
    assert!(!root.path().join("config/sentinel").exists());
    assert!(!root.path().join("cache/sentinel").exists());

    // 没有新覆盖目录时，默认路径也必须完成两套目录的整体迁移。
    let dirs = Directories::from_lookup(|key| (key == "HOME").then(|| home.clone())).unwrap();
    assert!(dirs.config.join("sentinel").exists());
    assert!(dirs.cache.join("sentinel").exists());
    assert!(!legacy_config.exists());
    assert!(!legacy_cache.exists());
}

#[test]
fn invalid_layout_or_settings_never_overwrite_good_state() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let good = layout();
    store.save_layout(&good).unwrap();
    let mut bad = good.clone();
    bad.regions.get_mut("left").unwrap().groups[0]
        .tabs
        .push("terminal".into());
    assert!(store.save_layout(&bad).is_err());
    assert_eq!(store.layout().unwrap(), Some(good));
    assert!(
        store
            .save_settings(&Settings {
                default_view: ViewMode::Reading,
                font_size: f64::NAN
            })
            .is_err()
    );
    assert_eq!(store.settings().unwrap().font_size, 15.0);
    assert!(store.validate_vault(root.path()).is_err());
    let notes = root.path().join("notes");
    fs::create_dir(&notes).unwrap();
    assert!(store.validate_vault(&notes).is_ok());
}

#[test]
fn readers_only_observe_complete_atomic_replacements() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let path = store.directories.config.join("config.toml");
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            for i in 0..40 {
                store
                    .save_settings(&Settings {
                        default_view: ViewMode::Reading,
                        font_size: 15.0 + f64::from(i % 10),
                    })
                    .unwrap();
            }
        });
        while !writer.is_finished() {
            let text = fs::read_to_string(&path).unwrap();
            let parsed: Settings = toml_edit::de::from_str(&text).unwrap();
            assert!((15.0..=24.0).contains(&parsed.font_size));
        }
        writer.join().unwrap();
    });
    assert_eq!(fs::read_dir(store.directories.config).unwrap().count(), 1);
}

#[test]
fn diagram_state_survives_session_saves_and_is_scoped_to_vault_note_and_block() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    for (vault, note, block) in [
        ("v1", "n1", 1),
        ("v1", "n1", 2),
        ("v1", "n2", 1),
        ("v2", "n1", 1),
    ] {
        store
            .remember_diagram(DiagramState {
                vault: vault.into(),
                note: note.into(),
                block,
                collapsed: vec!["A".into()],
            })
            .unwrap();
    }
    store
        .remember(VaultState {
            path: "v1".into(),
            ..VaultState::default()
        })
        .unwrap();
    assert_eq!(store.session().unwrap().diagrams.len(), 4);
    store
        .remember_diagram(DiagramState {
            vault: "v1".into(),
            note: "n1".into(),
            block: 1,
            collapsed: vec![],
        })
        .unwrap();
    assert_eq!(store.session().unwrap().diagrams.len(), 3);
    assert_eq!(fs::read_dir(&store.directories.cache).unwrap().count(), 1);
}
