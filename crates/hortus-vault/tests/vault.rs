use hortus_vault::{Batch, Entry, Vault};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;

fn wait(vault: &Vault, mut predicate: impl FnMut(&[Batch]) -> bool) -> Vec<Batch> {
    thread::scope(|scope| {
        let (done, watchdog) = mpsc::channel();
        // 失败时由独立看门狗关闭通道，避免测试永久挂住；不轮询取批次。
        scope.spawn(move || {
            if watchdog.recv_timeout(Duration::from_secs(3)).is_err() {
                vault.close();
            }
        });
        let mut all = Vec::new();
        loop {
            let batches = vault.wait_for_batches();
            assert!(!batches.is_empty(), "文件事件超时：{all:?}");
            all.extend(batches);
            if predicate(&all) {
                done.send(()).unwrap();
                return all;
            }
        }
    })
}

fn initial(vault: &Vault) -> BTreeMap<String, Entry> {
    wait(vault, |batches| batches.iter().any(|b| b.scan_complete))
        .into_iter()
        .flat_map(|b| b.upserted)
        .map(|e| (e.path.clone(), e))
        .collect()
}

#[test]
fn ignores_private_and_unrelated_files_and_sorts_naturally() {
    let dir = TempDir::new().unwrap();
    for name in [
        "10.md",
        "2.md",
        "Note10.markdown",
        "note2.markdown",
        "diagram.mmd",
        "UPPER.MD",
        "image.png",
        ".hidden.md",
    ] {
        fs::write(dir.path().join(name), "# sample").unwrap();
    }
    for name in ["folder10", "folder2", ".obsidian", ".git", "node_modules"] {
        fs::create_dir(dir.path().join(name)).unwrap();
        fs::write(dir.path().join(name).join("inside.md"), "sample").unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    let batches = wait(&vault, |b| b.iter().any(|b| b.scan_complete));
    assert!(batches.iter().all(|b| b.errors.is_empty()));
    let roots: Vec<_> = batches
        .iter()
        .flat_map(|b| &b.upserted)
        .filter(|e| e.parent.is_empty())
        .collect();
    assert_eq!(roots.len(), 8);
    assert!(roots[0].directory && roots[1].directory);
    assert_eq!(roots[0].name, "folder2");
    assert_eq!(roots[1].name, "folder10");
    let names: Vec<_> = roots.iter().map(|e| e.name.as_str()).collect();
    assert!(names.iter().position(|n| *n == "2.md") < names.iter().position(|n| *n == "10.md"));
    assert!(
        names.iter().position(|n| *n == "note2.markdown")
            < names.iter().position(|n| *n == "Note10.markdown")
    );
    assert_eq!(batches.iter().map(|b| b.upserted.len()).sum::<usize>(), 10);
}

#[test]
fn external_create_delete_rename_move_and_content_are_incremental_within_one_second() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("destination")).unwrap();
    fs::write(dir.path().join("untouched.md"), "untouched").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let mut tree = initial(&vault);
    let mut change = |action: &dyn Fn(), added: &[&str], removed: &[&str]| {
        let start = Instant::now();
        action();
        let batches = wait(&vault, |batches| {
            for batch in batches {
                for key in &batch.removed {
                    tree.remove(key);
                }
                for item in &batch.upserted {
                    tree.insert(item.path.clone(), item.clone());
                }
            }
            added.iter().all(|p| tree.contains_key(*p))
                && removed.iter().all(|p| !tree.contains_key(*p))
        });
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "变更没有在 1 秒内出现"
        );
        assert!(
            batches
                .iter()
                .all(|b| !b.scan_complete && b.errors.is_empty())
        );
        assert!(
            !batches
                .iter()
                .flat_map(|b| &b.upserted)
                .any(|e| e.path == "untouched.md")
        );
        eprintln!(
            "watch_update_ms={:.2}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    };
    change(
        &|| fs::write(dir.path().join("new.md"), "new").unwrap(),
        &["new.md"],
        &[],
    );
    change(
        &|| fs::rename(dir.path().join("new.md"), dir.path().join("renamed.md")).unwrap(),
        &["renamed.md"],
        &["new.md"],
    );
    change(
        &|| {
            fs::rename(
                dir.path().join("renamed.md"),
                dir.path().join("destination/moved.md"),
            )
            .unwrap()
        },
        &["destination/moved.md"],
        &["renamed.md"],
    );
    change(
        &|| fs::remove_file(dir.path().join("destination/moved.md")).unwrap(),
        &[],
        &["destination/moved.md"],
    );
    fs::write(dir.path().join("untouched.md"), "updated").unwrap();
    wait(&vault, |b| {
        b.iter()
            .flat_map(|b| &b.upserted)
            .any(|e| e.path == "untouched.md")
    });
    assert_eq!(vault.read_note("untouched.md").unwrap(), "updated");
}

#[test]
fn moving_a_directory_reconciles_the_whole_subtree() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("old/nested")).unwrap();
    fs::write(dir.path().join("old/nested/1.md"), "sample").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let mut tree = initial(&vault);
    fs::rename(dir.path().join("old"), dir.path().join("new")).unwrap();
    wait(&vault, |batches| {
        for batch in batches {
            for key in &batch.removed {
                tree.remove(key);
            }
            for e in &batch.upserted {
                tree.insert(e.path.clone(), e.clone());
            }
        }
        tree.contains_key("new/nested/1.md") && !tree.keys().any(|key| key.starts_with("old"))
    });
    assert_eq!(tree.len(), 3);
}

fn snapshot(root: &Path) -> BTreeMap<String, (SystemTime, u64)> {
    fn visit(root: &Path, dir: &Path, result: &mut BTreeMap<String, (SystemTime, u64)>) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            result.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                (metadata.modified().unwrap(), metadata.len()),
            );
            if metadata.is_dir() {
                visit(root, &path, result);
            }
        }
    }
    let mut result = BTreeMap::new();
    let metadata = fs::metadata(root).unwrap();
    result.insert(
        String::new(),
        (metadata.modified().unwrap(), metadata.len()),
    );
    visit(root, root, &mut result);
    result
}

#[test]
fn scan_watch_read_and_close_never_write_into_the_vault() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("notes/subfolder")).unwrap();
    fs::create_dir(dir.path().join(".obsidian")).unwrap();
    fs::write(dir.path().join("notes/subfolder/sample.md"), "# sample").unwrap();
    fs::write(dir.path().join(".obsidian/config.json"), "{}").unwrap();
    let before = snapshot(dir.path());
    let vault = Vault::open(dir.path()).unwrap();
    assert_eq!(initial(&vault).len(), 3);
    assert_eq!(
        vault.read_note("notes/subfolder/sample.md").unwrap(),
        "# sample"
    );
    thread::sleep(Duration::from_millis(300));
    vault.close();
    drop(vault);
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn note_reads_cannot_escape_the_vault_or_follow_symlink_cycles() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.md"), "outside").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    assert!(initial(&vault).is_empty());
    for path in [
        "../secret.md",
        "/tmp/secret.md",
        "escape/secret.md",
        ".hidden.md",
    ] {
        assert!(vault.read_note(path).is_err());
    }
}

#[test]
fn blocking_batches_deliver_scan_and_changes_and_close_wakes_all_waiters() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("initial.md"), "initial").unwrap();
    let vault = Arc::new(Vault::open(dir.path()).unwrap());
    let (tx, rx) = mpsc::channel();
    let reader_vault = vault.clone();
    let reader = thread::spawn(move || {
        loop {
            let batches = reader_vault.wait_for_batches();
            if batches.is_empty() {
                break;
            }
            tx.send(batches).unwrap();
        }
    });
    let mut scanned = false;
    let mut entries = 0;
    while !scanned {
        for batch in rx.recv_timeout(Duration::from_secs(3)).unwrap() {
            scanned |= batch.scan_complete;
            entries += batch.upserted.len();
        }
    }
    assert_eq!(entries, 1);
    // 空闲期间没有批次；超时仅用于测试断言，不参与生产收取逻辑。
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    let start = Instant::now();
    fs::write(dir.path().join("created.md"), "created").unwrap();
    let batches = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        batches
            .iter()
            .any(|batch| batch.upserted.iter().any(|e| e.path == "created.md"))
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    let closing = Instant::now();
    vault.close();
    // 生产者和阻塞消费者都必须退出，后续或重复关闭也不能再阻塞。
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    reader.join().unwrap();
    vault.close();
    assert!(vault.wait_for_batches().is_empty());
    assert!(closing.elapsed() < Duration::from_millis(200));
}

#[test]
fn concurrent_close_during_scan_or_pending_events_always_unblocks_consumers() {
    let dir = TempDir::new().unwrap();
    for n in 0..200 {
        fs::write(dir.path().join(format!("{n}.md")), "sample").unwrap();
    }
    for _ in 0..20 {
        let vault = Arc::new(Vault::open(dir.path()).unwrap());
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let vault = vault.clone();
                thread::spawn(move || while !vault.wait_for_batches().is_empty() {})
            })
            .collect();
        fs::write(dir.path().join("0.md"), "changed").unwrap();
        thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| vault.close());
            }
        });
        for reader in readers {
            reader.join().unwrap();
        }
    }
}
