//! 全库快照：只读遍历、逐条目复制、起止清单与逐条目复核。
//!
//! 不跟随符号链接；任意层级的 `.git` 条目都跳过；socket、设备、FIFO
//! 等无法安全保真的条目让整次快照失败。复制完成后再次遍历源库，任何
//! 新增、删除、重命名、内容或元数据变化都使本次快照作废。
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(not(target_os = "macos"))]
#[path = "portable.rs"]
mod platform;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    /// `file`、`directory` 或 `symlink`；空目录与符号链接可据此辨认。
    pub kind: String,
    pub size: u64,
    pub mode: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    Symlink,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    relative: PathBuf,
    kind: Kind,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    mtime_seconds: i64,
    mtime_nanos: i64,
    inode: u64,
    device: u64,
    link: Option<PathBuf>,
}

impl Entry {
    fn from_metadata(relative: PathBuf, metadata: &fs::Metadata) -> io::Result<Self> {
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            Kind::Symlink
        } else if file_type.is_dir() {
            Kind::Directory
        } else if file_type.is_file() {
            Kind::File
        } else {
            // 特殊条目无法安全保真：整次快照失败，不能跳过。
            return Err(io::Error::other("history.unsupported_entry"));
        };
        Ok(Self {
            relative,
            kind,
            size: metadata.size(),
            mode: metadata.mode() & 0o7777,
            uid: metadata.uid(),
            gid: metadata.gid(),
            mtime_seconds: metadata.mtime(),
            mtime_nanos: metadata.mtime_nsec(),
            inode: metadata.ino(),
            device: metadata.dev(),
            link: None,
        })
    }

    fn manifest(&self) -> ManifestEntry {
        ManifestEntry {
            path: self.relative.to_string_lossy().into_owned(),
            kind: self.kind.name().into(),
            size: self.size,
            mode: self.mode,
            modified_ms: modified_ms(self.mtime_seconds, self.mtime_nanos),
            link: self
                .link
                .as_ref()
                .map(|link| link.to_string_lossy().into_owned()),
        }
    }
}

fn modified_ms(seconds: i64, nanos: i64) -> Option<i64> {
    Some(seconds.saturating_mul(1000) + nanos / 1_000_000)
}

#[cfg(test)]
pub(crate) static TEST_AFTER_ENTRY: std::sync::Mutex<Option<fn(&Path)>> =
    std::sync::Mutex::new(None);

fn ensure_readable(path: &Path) -> io::Result<()> {
    match fs::File::open(path) {
        Ok(file) => {
            drop(file);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err(io::Error::other("history.source_unreadable"))
        }
        Err(error) => Err(error),
    }
}

fn check_cancel(cancel: Option<&AtomicBool>) -> io::Result<()> {
    match cancel {
        Some(flag) if flag.load(Ordering::Acquire) => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "history.cancelled",
        )),
        _ => Ok(()),
    }
}

/// 自检用闸门：快照复制开始前等待外部放行，用来证明期间不持有全局 Store 锁。
/// 只在 Debug 构建存在；Release 不包含这个开关。
#[cfg(debug_assertions)]
fn test_gate() {
    let Some(directory) = std::env::var_os("HORTUS_SNAPSHOT_GATE") else {
        return;
    };
    let directory = PathBuf::from(directory);
    if !directory.join("armed").exists() {
        return;
    }
    let _ = fs::write(directory.join("entered"), b"");
    let release = directory.join("release");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !release.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 建立完整快照；失败时目标目录可能含未完成条目，由调用方以失败元数据隔离。
pub fn create(
    source: &Path,
    destination: &Path,
    manifest: &Path,
    cancel: Option<&AtomicBool>,
) -> io::Result<(super::SnapshotStats, Vec<ManifestEntry>)> {
    let root = fs::symlink_metadata(source)?;
    if !root.is_dir() {
        return Err(io::Error::other("history.source_not_directory"));
    }
    let first = scan(source, cancel)?;
    check_cancel(cancel)?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(destination)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                io::Error::other("history.destination_exists")
            } else {
                error
            }
        })?;

    // 检查用开关：强制走完整复制，覆盖不支持 clonefile 的文件系统。
    #[cfg(debug_assertions)]
    test_gate();
    let force_copy = std::env::var_os("HORTUS_HISTORY_COPY").is_some_and(|value| value == "copy");
    let mut stats = super::SnapshotStats::default();
    for entry in &first {
        check_cancel(cancel)?;
        let source_path = source.join(&entry.relative);
        let destination_path = destination.join(&entry.relative);
        match entry.kind {
            Kind::Directory => platform::create_directory(&destination_path)?,
            Kind::File => {
                // clonefile 只复制元数据与数据块，不读取内容；先显式验证源文件可读，
                // 让 clone 与完整复制在“源条目读取失败”时行为一致。
                ensure_readable(&source_path)?;
                let (mut cloned, mut copied) = (0, 0);
                platform::copy_file(
                    &source_path,
                    &destination_path,
                    force_copy,
                    &mut cloned,
                    &mut copied,
                )?;
                stats.cloned += cloned;
                stats.copied += copied;
            }
            Kind::Symlink => platform::copy_symlink(&source_path, &destination_path)?,
        }
        // 逐条目复核：复制期间源条目变化立即失败，即使随后又改回相同大小。
        let after = fs::symlink_metadata(&source_path)?;
        let mut after = Entry::from_metadata(entry.relative.clone(), &after)?;
        if entry.kind == Kind::Symlink {
            after.link = Some(fs::read_link(&source_path)?);
        }
        if after != *entry {
            return Err(io::Error::other("history.source_changed"));
        }
        #[cfg(test)]
        if let Some(hook) = *TEST_AFTER_ENTRY.lock().unwrap() {
            hook(source);
        }
        count_kind(entry, &mut stats);
        stats.entries += 1;
        stats.bytes += if entry.kind == Kind::File {
            entry.size
        } else {
            0
        };
    }

    // 子项写完后再恢复目录自身的时间与权限，避免被创建动作改写。
    for entry in first.iter().rev() {
        if entry.kind == Kind::Directory {
            platform::apply_directory_metadata(
                &source.join(&entry.relative),
                &destination.join(&entry.relative),
            )?;
        }
    }
    platform::apply_directory_metadata(source, destination)?;

    let second = scan(source, cancel)?;
    if first != second {
        return Err(io::Error::other("history.source_changed"));
    }
    let entries = first.iter().map(Entry::manifest).collect();
    let data = serde_json::to_vec_pretty(&entries).map_err(io::Error::other)?;
    super::atomic_write(manifest, &data)?;
    stats.method = if stats.copied == 0 {
        "clonefile".into()
    } else if stats.cloned == 0 {
        "copy".into()
    } else {
        "mixed".into()
    };
    Ok((stats, entries))
}

fn count_kind(entry: &Entry, stats: &mut super::SnapshotStats) {
    match entry.kind {
        Kind::File => stats.files += 1,
        Kind::Directory => stats.directories += 1,
        Kind::Symlink => stats.symlinks += 1,
    }
}

fn scan(root: &Path, cancel: Option<&AtomicBool>) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    // 根目录本身不写入清单；它的元数据由目录复制单独处理。
    walk(root, Path::new(""), &mut entries, cancel)?;
    entries.sort_by(|a, b| {
        a.relative
            .as_os_str()
            .as_bytes()
            .cmp(b.relative.as_os_str().as_bytes())
    });
    Ok(entries)
}

fn walk(
    root: &Path,
    relative: &Path,
    entries: &mut Vec<Entry>,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    let mut names = Vec::new();
    for item in fs::read_dir(root.join(relative))? {
        let item = item?;
        let name = item.file_name();
        // 任意层级的 .git 都是版本库，不属于用户内容。
        if name == ".git" {
            continue;
        }
        names.push(name);
    }
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    for name in names {
        check_cancel(cancel)?;
        let child = relative.join(&name);
        let metadata = fs::symlink_metadata(root.join(&child))?;
        let mut entry = Entry::from_metadata(child.clone(), &metadata)?;
        if entry.kind == Kind::Symlink {
            // 只记录链接目标文本，绝不跟随或复制库外目标。
            entry.link = Some(fs::read_link(root.join(&child))?);
            entries.push(entry);
        } else if entry.kind == Kind::Directory {
            entries.push(entry);
            walk(root, &child, entries, cancel)?;
        } else {
            entries.push(entry);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_time_keeps_millisecond_precision() {
        assert_eq!(modified_ms(10, 999_999_999), Some(10_999));
        assert_eq!(modified_ms(0, 0), Some(0));
    }

    #[test]
    fn source_change_during_copy_fails_the_snapshot() {
        let root = tempfile::TempDir::new().unwrap();
        let vault = root.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("a.md"), b"a").unwrap();
        fs::write(vault.join("b.md"), b"b").unwrap();
        *TEST_AFTER_ENTRY.lock().unwrap() = Some(|vault: &Path| {
            fs::write(vault.join("b.md"), b"changed").unwrap();
        });
        let result = create(
            &vault,
            &root.path().join("snapshot"),
            &root.path().join("manifest.json"),
            None,
        );
        *TEST_AFTER_ENTRY.lock().unwrap() = None;
        assert_eq!(result.unwrap_err().to_string(), "history.source_changed");
    }
}
