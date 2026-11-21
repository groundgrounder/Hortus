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
fn note_reads_match_the_editor_text_projection() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("bom-crlf.md"),
        b"\xef\xbb\xbf# \xe6\xa0\x87\xe9\xa2\x98\r\n\xe6\xad\xa3\xe6\x96\x87\r\n",
    )
    .unwrap();
    fs::write(dir.path().join("invalid.md"), b"# \xff\n").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    assert_eq!(vault.read_note("bom-crlf.md").unwrap(), "# 标题\n正文\n");
    // 无效 UTF-8 与 DocumentSession 一样以替换符继续读取。
    assert_eq!(vault.read_note("invalid.md").unwrap(), "# \u{fffd}\n");
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
    vault.close();
    // 生产者和阻塞消费者都必须退出，后续或重复关闭也不能再阻塞。
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    reader.join().unwrap();
    vault.close();
    assert!(vault.wait_for_batches().is_empty());
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

#[test]
fn file_operations_create_validate_and_refuse_to_clobber() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("alpha.md"), "existing").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    // 无扩展名自动补 .md；受支持扩展名原样保留。
    let note = vault.create_note("", "idea").unwrap();
    assert_eq!(
        (note.path.as_str(), note.parent.as_str(), note.directory),
        ("idea.md", "", false)
    );
    assert!(fs::read(dir.path().join("idea.md")).unwrap().is_empty());
    let markdown = vault.create_note("", "Draft.markdown").unwrap();
    assert_eq!(markdown.path, "Draft.markdown");
    // 独占创建：同名（含大小写不敏感文件系统上的变体）拒绝且不覆盖。
    for name in ["idea", "ALPHA.md", "alpha.md"] {
        let error = vault.create_note("", name).unwrap_err();
        assert_eq!(error.to_string(), "vault.already_exists", "{name}");
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("alpha.md")).unwrap(),
        "existing"
    );
    // 非法名称：空、隐藏项、node_modules、分隔符与控制字符。
    for name in [
        "",
        "   ",
        ".",
        "..",
        ".hidden",
        "node_modules",
        "a/b",
        "back\\slash",
        "nul\0",
        "ctrl\u{1}x",
    ] {
        let error = vault.create_note("", name).unwrap_err();
        assert_eq!(error.to_string(), "vault.invalid_name", "{name:?}");
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    // 文件夹：单级创建、重复拒绝、子目录内新建。
    let folder = vault.create_folder("", "Projects").unwrap();
    assert_eq!((folder.path.as_str(), folder.directory), ("Projects", true));
    assert!(dir.path().join("Projects").is_dir());
    assert_eq!(
        vault.create_folder("", "Projects").unwrap_err().to_string(),
        "vault.already_exists"
    );
    let nested = vault.create_note("Projects", "memo").unwrap();
    assert_eq!(
        (nested.path.as_str(), nested.parent.as_str()),
        ("Projects/memo.md", "Projects")
    );
    assert_eq!(
        vault.create_note("Missing", "x").unwrap_err().to_string(),
        "vault.not_found"
    );
    vault.close();
}

#[test]
fn rename_entries_keeps_folders_whole_and_rejects_conflicts() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("docs/deep")).unwrap();
    fs::write(dir.path().join("docs/inside.md"), "inside").unwrap();
    fs::write(dir.path().join("docs/deep/leaf.mmd"), "leaf").unwrap();
    fs::write(dir.path().join("b.md"), "note").unwrap();
    fs::write(dir.path().join("c.md"), "other").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    // 普通文件重命名 + 名称去空白。
    let renamed = vault.rename_entry("b.md", "  beta.md  ").unwrap();
    assert_eq!(
        (renamed.path.as_str(), renamed.parent.as_str()),
        ("beta.md", "")
    );
    assert!(!dir.path().join("b.md").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("beta.md")).unwrap(),
        "note"
    );
    // 新旧名称字节一致：无操作，返回当前条目。
    let same = vault.rename_entry("beta.md", "beta.md").unwrap();
    assert_eq!(same.path, "beta.md");
    // 大小写不敏感文件系统上的原地改名（同 inode）必须成功。
    let case = vault.rename_entry("beta.md", "Beta.md").unwrap();
    assert_eq!(case.path, "Beta.md");
    assert_eq!(
        fs::read_to_string(dir.path().join("Beta.md")).unwrap(),
        "note"
    );
    // 与其他不同 inode 的文件同名（大小写不敏感命中）必须拒绝。
    assert_eq!(
        vault
            .rename_entry("Beta.md", "C.md")
            .unwrap_err()
            .to_string(),
        "vault.already_exists"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("c.md")).unwrap(),
        "other"
    );
    assert!(dir.path().join("Beta.md").exists());
    // 笔记不允许改成不受识别的扩展名，避免重命名后从文件树消失。
    assert_eq!(
        vault
            .rename_entry("Beta.md", "plain.txt")
            .unwrap_err()
            .to_string(),
        "vault.invalid_name"
    );
    assert!(dir.path().join("Beta.md").exists());
    // 非空文件夹整棵原子重命名，子项不丢失。
    let folder = vault.rename_entry("docs", "papers").unwrap();
    assert_eq!(folder.path, "papers");
    assert!(!dir.path().join("docs").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("papers/inside.md")).unwrap(),
        "inside"
    );
    assert!(dir.path().join("papers/deep/leaf.mmd").is_file());
    let nested = vault.rename_entry("papers/inside.md", "intro.md").unwrap();
    assert_eq!(nested.path, "papers/intro.md");
    // 文件夹改到既有文件夹名、源缺失、父目录越界都拒绝。
    fs::create_dir(dir.path().join("papers/target")).unwrap();
    assert_eq!(
        vault
            .rename_entry("papers/deep", "target")
            .unwrap_err()
            .to_string(),
        "vault.already_exists"
    );
    assert_eq!(
        vault
            .rename_entry("missing.md", "x.md")
            .unwrap_err()
            .to_string(),
        "vault.not_found"
    );
    assert_eq!(
        vault
            .rename_entry("../outside", "x.md")
            .unwrap_err()
            .to_string(),
        "vault.invalid_path"
    );
    assert_eq!(
        vault
            .rename_entry("Beta.md", "bad/name.md")
            .unwrap_err()
            .to_string(),
        "vault.invalid_name"
    );
    vault.close();
}

#[cfg(unix)]
#[test]
fn trash_target_resolution_stays_inside_the_vault() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("folder")).unwrap();
    fs::write(dir.path().join("note.md"), "text").unwrap();
    let outside = TempDir::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("linkdir")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("far.md"), dir.path().join("link.md")).unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let (note, is_dir) = vault.resolve_trash_target("note.md").unwrap();
    assert_eq!(note, dir.path().canonicalize().unwrap().join("note.md"));
    assert!(!is_dir);
    let (folder, is_dir) = vault.resolve_trash_target("folder").unwrap();
    assert!(folder.ends_with("folder") && is_dir);
    assert_eq!(
        vault
            .resolve_trash_target("missing.md")
            .unwrap_err()
            .to_string(),
        "vault.not_found"
    );
    assert_eq!(
        vault.resolve_trash_target("").unwrap_err().to_string(),
        "vault.invalid_path"
    );
    assert_eq!(
        vault
            .resolve_trash_target("../escape.md")
            .unwrap_err()
            .to_string(),
        "vault.invalid_path"
    );
    // 符号链接（自身或父目录）一律拒绝，不能把库外目标送进废纸篓。
    assert_eq!(
        vault
            .resolve_trash_target("link.md")
            .unwrap_err()
            .to_string(),
        "vault.symlink_unsupported"
    );
    assert_eq!(
        vault
            .resolve_trash_target("linkdir/any.md")
            .unwrap_err()
            .to_string(),
        "vault.symlink_unsupported"
    );
    // 新建操作同样不进入符号链接目录。
    assert_eq!(
        vault.create_note("linkdir", "x").unwrap_err().to_string(),
        "vault.symlink_unsupported"
    );
    assert_eq!(
        vault
            .rename_entry("link.md", "renamed.md")
            .unwrap_err()
            .to_string(),
        "vault.symlink_unsupported"
    );
    vault.close();
}

#[test]
fn case_only_rename_keeps_a_single_entry() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("case.md"), "# case").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let mut tree = initial(&vault);
    assert!(tree.contains_key("case.md"));
    // 大小写不敏感文件系统上，原地改名的事件会同时携带新旧两个名字；
    // 旧名不能通过不区分大小写的 stat 复活成重复条目。
    fs::rename(dir.path().join("case.md"), dir.path().join("Case.md")).unwrap();
    wait(&vault, |batches| {
        for batch in batches {
            for key in &batch.removed {
                tree.remove(key);
            }
            for item in &batch.upserted {
                tree.insert(item.path.clone(), item.clone());
            }
        }
        tree.contains_key("Case.md") && !tree.contains_key("case.md")
    });
    vault.close();
}
