//! 只读、只执行与 `uchg` 锁定文件在 clone 与强制完整复制下都必须可快照。
use hortus_history::{HistoryStore, SessionState};
use std::collections::BTreeMap;
use std::fs;
use std::os::darwin::fs::MetadataExt as DarwinMetadataExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Expected {
    mode: u32,
    flags: u32,
    mtime: (i64, i64),
    size: u64,
    bytes: Vec<u8>,
}

fn expected(path: &Path) -> Expected {
    let metadata = fs::symlink_metadata(path).unwrap();
    Expected {
        mode: metadata.mode() & 0o7777,
        flags: metadata.st_flags(),
        mtime: (metadata.mtime(), metadata.mtime_nsec()),
        size: metadata.size(),
        bytes: fs::read(path).unwrap(),
    }
}

fn assert_fidelity(source: &Path, destination: &Path, name: &str) {
    let from = expected(source);
    let to = expected(destination);
    assert_eq!(to.bytes, from.bytes, "{name}: bytes");
    assert_eq!(to.size, from.size, "{name}: size");
    assert_eq!(to.mode, from.mode, "{name}: mode");
    assert_eq!(to.flags, from.flags, "{name}: flags");
    assert_eq!(to.mtime, from.mtime, "{name}: mtime");
}

struct FlagCleanup(PathBuf);

impl Drop for FlagCleanup {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/chflags")
            .args(["-R", "nouchg"])
            .arg(&self.0)
            .status();
    }
}

#[test]
fn read_only_and_locked_files_snapshot_on_both_copy_paths() {
    let root = tempfile::TempDir::new().unwrap();
    let _cleanup = FlagCleanup(root.path().to_path_buf());
    let vault = root.path().join("vault");
    fs::create_dir(&vault).unwrap();
    fs::write(vault.join("notes.md"), b"# Notes\n").unwrap();

    let files: [(&str, u32, &[u8]); 4] = [
        ("readonly-0444.md", 0o444, b"read only\n"),
        ("owner-0400.md", 0o400, b"owner read only\n"),
        ("readonly-exec.sh", 0o500, b"#!/bin/sh\necho hi\n"),
        ("readonly-0755.sh", 0o555, b"#!/bin/sh\necho hi\n"),
    ];
    for (name, mode, bytes) in files {
        let path = vault.join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    let locked = vault.join("locked-uchg.md");
    fs::write(&locked, b"locked\n").unwrap();
    let locked_supported = Command::new("/usr/bin/chflags")
        .args(["uchg"])
        .arg(&locked)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !locked_supported {
        eprintln!("跳过 uchg：本机文件系统不支持用户不可变标志");
    }

    let source: BTreeMap<String, Expected> = fs::read_dir(&vault)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                expected(&entry.path()),
            )
        })
        .collect();

    for (label, force_copy) in [("clonefile", false), ("copy", true)] {
        unsafe {
            if force_copy {
                std::env::set_var("HORTUS_HISTORY_COPY", "copy");
            } else {
                std::env::remove_var("HORTUS_HISTORY_COPY");
            }
        }
        let store = HistoryStore::new(root.path().join(format!("data-{label}")));
        let record = store.begin(&vault, None).unwrap();
        assert_eq!(record.state, SessionState::Ready, "{label}");
        let snapshot = store.snapshot_directory(&record.id).unwrap();
        for name in source.keys() {
            assert_fidelity(&vault.join(name), &snapshot.join(name), name);
        }
        assert_eq!(
            record.stats.as_ref().unwrap().method,
            if force_copy { "copy" } else { "clonefile" }
        );
    }
    unsafe {
        std::env::remove_var("HORTUS_HISTORY_COPY");
    }

    let after: BTreeMap<String, Expected> = fs::read_dir(&vault)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                expected(&entry.path()),
            )
        })
        .collect();
    assert_eq!(after, source, "snapshot modified the source vault");
}
