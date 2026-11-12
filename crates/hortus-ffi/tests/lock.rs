//! 快照被确定性阻塞时，其它窗口的布局、会话与 Agent 状态写入不能等待全局 Store 锁。
use hortus_config::{LayoutState, PanelGroup, RegionLayout, SessionState};
use hortus_ffi::WorkspaceCore;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn wait_for(path: &Path, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "等待 {} 超时；Debug 快照闸门未生效",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn layout_json() -> String {
    let layout = LayoutState {
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
    };
    serde_json::to_string(&layout).unwrap()
}

#[test]
fn snapshot_does_not_hold_the_store_lock() {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    let cache = root.path().join("cache");
    let data = root.path().join("data");
    let vault = root.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    fs::write(vault.join("note.md"), "# Note\n").unwrap();
    unsafe {
        std::env::set_var("HOME", root.path());
        std::env::set_var("HORTUS_CONFIG_DIR", &config);
        std::env::set_var("HORTUS_CACHE_DIR", &cache);
        std::env::set_var("HORTUS_DATA_DIR", &data);
    }
    let core = WorkspaceCore::new().unwrap();
    // 闸门未 armed 时先建立一个 ready 会话，作为状态写入对象。
    let baseline = core
        .begin_agent_session(vault.to_string_lossy().into_owned())
        .unwrap();
    assert_eq!(baseline.state, "ready");

    let gate = root.path().join("gate");
    fs::create_dir(&gate).unwrap();
    unsafe {
        std::env::set_var("HORTUS_SNAPSHOT_GATE", &gate);
    }
    fs::write(gate.join("armed"), b"").unwrap();

    let handle = Arc::clone(&core);
    let gated_vault = vault.to_string_lossy().into_owned();
    let snapshot = std::thread::spawn(move || handle.begin_agent_session(gated_vault));
    wait_for(&gate.join("entered"), 10);

    let finished = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&finished);
    let writer = Arc::clone(&core);
    let id = baseline.id.clone();
    let worker = std::thread::spawn(move || {
        let started = Instant::now();
        writer.save_layout(layout_json()).unwrap();
        writer
            .save_window_session(
                serde_json::to_string(&SessionState {
                    windows: Some(Vec::new()),
                    ..SessionState::default()
                })
                .unwrap(),
            )
            .unwrap();
        writer
            .mark_agent_session(id, "running".into(), None)
            .unwrap();
        let elapsed = started.elapsed();
        flag.store(true, Ordering::Release);
        elapsed
    });

    let deadline = Instant::now() + Duration::from_millis(1200);
    while !finished.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let fast = finished.load(Ordering::Acquire);
    fs::write(gate.join("release"), b"").unwrap();
    let elapsed = worker.join().unwrap();
    let record = snapshot.join().unwrap().unwrap();
    assert_eq!(record.state, "ready");
    unsafe {
        std::env::remove_var("HORTUS_SNAPSHOT_GATE");
    }

    assert!(fast, "其它窗口的保存与状态写入被快照阻塞了 {elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert!(cache.join("session.toml").exists());
    assert!(config.join("layout.toml").exists());
    let sessions = core.list_agent_sessions(None).unwrap();
    let marked = sessions.iter().find(|item| item.id == baseline.id).unwrap();
    assert_eq!(marked.state, "running");
}
