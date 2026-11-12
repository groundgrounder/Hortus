use hortus_config::*;
use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

const LEGACY_CONFIG_ENV: &str = "MARKNOTE_CONFIG_DIR";
const LEGACY_CACHE_ENV: &str = "MARKNOTE_CACHE_DIR";

#[test]
fn ordered_windows_survive_session_round_trip() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let text = r#"last_vault = '/vault/b'
[[recent]]
path = '/vault/a'
tabs = ['same.md']
selected = 'same.md'
[recent.visible]
left = false
right = true
bottom = true
[[windows]]
id = 'first'
path = '/vault/a'
tabs = ['same.md']
selected = 'same.md'
[windows.views]
'same.md' = 'editing'
[[windows]]
id = 'second'
path = '/vault/b'
tabs = ['same.md', 'other.md']
selected = 'other.md'
"#;
    fs::write(store.directories.cache.join("session.toml"), text).unwrap();
    let session = store.session().unwrap();
    let encoded = toml_edit::ser::to_string_pretty(&session).unwrap();
    assert!(
        encoded.contains("[[windows]]"),
        "ordered windows were discarded: {encoded}"
    );
    assert!(
        encoded.contains("[recent.visible]"),
        "vault visibility was discarded"
    );
    let decoded: SessionState = toml_edit::de::from_str(&encoded).unwrap();
    assert_eq!(decoded, session);
}

fn store(root: &TempDir) -> Store {
    Store::open(Directories {
        config: root.path().join("config"),
        cache: root.path().join("cache"),
        data: root.path().join("data"),
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
                    visible: None,
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
                    visible: None,
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
                    visible: None,
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
        visible: None,
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

#[test]
fn legacy_session_migrates_once_and_full_snapshot_preserves_diagrams() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let path = store.directories.cache.join("session.toml");
    fs::write(&path, "last_vault = '/a'\n[[recent]]\npath = '/a'\ntabs = ['same.md']\nselected = 'same.md'\n[recent.views]\n'same.md' = 'editing'\n[[diagrams]]\nvault = '/a'\nnote = 'same.md'\nblock = 0\ncollapsed = ['root']\n").unwrap();
    let mut session = store.session().unwrap();
    let windows = session.windows.as_ref().unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].views["same.md"], ViewMode::Editing);
    session.windows.as_mut().unwrap().push(WindowState {
        id: "second".into(),
        visible: Some(BTreeMap::from([("left".into(), true)])),
        path: "/b".into(),
        tabs: vec!["same.md".into()],
        selected: Some("same.md".into()),
        views: BTreeMap::new(),
        frame: Some(WindowFrame {
            x: -10000.0,
            y: 10.0,
            width: 1320.0,
            height: 840.0,
            screen: Some("2".into()),
        }),
    });
    session.default_visible = BTreeMap::from([("left".into(), false)]);
    session.recent[0].visible = Some(session.default_visible.clone());
    store.save_windows(session.clone()).unwrap();
    let restored = store.session().unwrap();
    assert_eq!(restored, session);
    store
        .remember_diagram(DiagramState {
            vault: "/b".into(),
            note: "same.md".into(),
            block: 1,
            collapsed: vec!["node".into()],
        })
        .unwrap();
    session.windows = Some(vec![]);
    session.diagrams.clear();
    store.save_windows(session).unwrap();
    let empty = store.session().unwrap();
    assert!(empty.windows.unwrap().is_empty());
    assert_eq!(empty.diagrams.len(), 2);
    assert!(!empty.recent[0].visible.as_ref().unwrap()["left"]);
}

#[test]
fn legacy_layout_visible_is_accepted_but_new_snapshot_omits_it() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let mut state = layout();
    state.regions.get_mut("left").unwrap().visible = Some(false);
    store.save_layout(&state).unwrap();
    assert_eq!(
        store.layout().unwrap().unwrap().regions["left"].visible,
        Some(false)
    );
    for region in state.regions.values_mut() {
        region.visible = None;
    }
    store.save_layout(&state).unwrap();
    assert!(
        !fs::read_to_string(store.directories.config.join("layout.toml"))
            .unwrap()
            .contains("visible")
    );
}

#[test]
fn data_root_is_persistent_and_independent_of_cache() {
    let home = TempDir::new().unwrap();
    let lookup = |key: &str| match key {
        "HOME" => Some(home.path().to_path_buf()),
        _ => None,
    };
    let directories = Directories::resolve_from_lookup(lookup).unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!(
            directories.data,
            home.path().join("Library/Application Support/Hortus")
        );
    } else {
        assert_eq!(directories.data, home.path().join(".local/share/hortus"));
    }
    assert_ne!(directories.data, directories.cache);

    let custom = home.path().join("custom-data");
    let overridden = Directories::resolve_from_lookup(|key| match key {
        "HOME" => Some(home.path().to_path_buf()),
        "HORTUS_DATA_DIR" => Some(custom.clone()),
        _ => None,
    })
    .unwrap();
    assert_eq!(overridden.data, custom);

    let relative = Directories::resolve_from_lookup(|key| match key {
        "HOME" => Some(home.path().to_path_buf()),
        "HORTUS_DATA_DIR" => Some("relative/data".into()),
        _ => None,
    });
    assert_eq!(
        relative.unwrap_err().to_string(),
        "config.path_not_absolute"
    );
}

#[test]
fn data_root_inside_vault_is_rejected() {
    let root = TempDir::new().unwrap();
    let vault = root.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    let store = Store::open(Directories {
        config: root.path().join("config"),
        cache: root.path().join("cache"),
        data: vault.join("app-data"),
    })
    .unwrap();
    fs::create_dir_all(store.directories.data.clone()).unwrap();
    assert_eq!(
        store.validate_vault(&vault).unwrap_err().to_string(),
        "config.vault_contains_state"
    );
}

fn write_config(store: &Store, text: &str) {
    fs::write(store.directories.config.join("config.toml"), text).unwrap();
}

#[test]
fn agent_template_validates_structure_and_preserves_each_argument() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    write_config(
        &store,
        "font_size = 15.0\ndefault_view = \"reading\"\n\n# keep this comment\nunknown_key = 7\n\n[agent]\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"printf '%s' {guide}\"]\n",
    );
    let launch = store.agent_launch().unwrap();
    assert_eq!(launch.executable, "/bin/sh");
    assert_eq!(launch.args.len(), 2);
    assert!(launch.args[1].starts_with("printf '%s' # Working in a Hortus knowledge base\n"));
    assert!(launch.args[1].contains("[[Note name]]"));
    assert!(launch.args[1].contains(AGENT_GUIDE));
    // 保存设置仍保留注释、未知键与 [agent]。
    store
        .save_settings(&Settings {
            font_size: 16.0,
            default_view: ViewMode::Editing,
        })
        .unwrap();
    let saved = fs::read_to_string(store.directories.config.join("config.toml")).unwrap();
    assert!(saved.contains("# keep this comment"), "{saved}");
    assert!(saved.contains("unknown_key = 7"), "{saved}");
    assert!(saved.contains("[agent]"), "{saved}");
    assert!(saved.contains("printf '%s' {guide}"), "{saved}");
    assert!(saved.contains("font_size = 16.0"), "{saved}");
}

#[test]
fn agent_template_errors_are_stable_keys() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let cases = [
        (
            "font_size = 15.0\ndefault_view = \"reading\"\n",
            "agent.not_configured",
        ),
        (
            "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\nargs = [\"{guide}\"]\n",
            "agent.command_missing",
        ),
        (
            "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"/bin/sh\"\nargs = [\"-l\"]\n",
            "agent.guide_missing",
        ),
        (
            "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"relative/tool\"\nargs = [\"{guide}\"]\n",
            "agent.command_not_absolute",
        ),
        (
            "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"/definitely/missing\"\nargs = [\"{guide}\"]\n",
            "agent.not_found",
        ),
        (
            "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"/bin/sh\"\nargs = [3]\n",
            "agent.config_invalid",
        ),
        (
            "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"/bin/sh\"\nargs = [\"{guide}\", \"a\\u0000b\"]\n",
            "agent.invalid_argument",
        ),
    ];
    for (text, expected) in cases {
        write_config(&store, text);
        assert_eq!(store.agent_launch().unwrap_err().to_string(), expected);
    }
}

#[test]
fn agent_guide_override_replaces_the_whole_guide() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    write_config(
        &store,
        "font_size = 15.0\ndefault_view = \"reading\"\n[agent]\ncommand = \"/bin/sh\"\nargs = [\"{guide}\"]\n",
    );
    let guide = "  Spaces \"quotes\" $(echo nope); 中文\nline two\n";
    fs::write(store.directories.config.join("agent-guide.md"), guide).unwrap();
    let launch = store.agent_launch().unwrap();
    assert_eq!(launch.args, vec![guide.to_string()]);
    assert!(!launch.args[0].contains("Working in a Hortus"));

    fs::write(store.directories.config.join("agent-guide.md"), b"  \n\t\n").unwrap();
    assert_eq!(store.agent_launch().unwrap_err().to_string(), "guide.empty");
    fs::write(
        store.directories.config.join("agent-guide.md"),
        b"\xff\xfeinvalid",
    )
    .unwrap();
    assert_eq!(
        store.agent_launch().unwrap_err().to_string(),
        "guide.invalid_utf8"
    );
    fs::remove_file(store.directories.config.join("agent-guide.md")).unwrap();
    fs::create_dir(store.directories.config.join("agent-guide.md")).unwrap();
    assert_eq!(
        store.agent_launch().unwrap_err().to_string(),
        "guide.read_failed"
    );
}

#[test]
fn resolving_the_guide_does_not_create_the_config_directory() {
    let root = TempDir::new().unwrap();
    let config = root.path().join("missing-config");
    let text = effective_guide(&config).unwrap();
    assert_eq!(text, AGENT_GUIDE);
    assert!(!config.exists());
}
