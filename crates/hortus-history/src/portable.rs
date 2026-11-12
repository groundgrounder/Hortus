//! 非 macOS 的完整复制路径：保留字节、类型、权限与修改时间。
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

pub(super) fn create_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().create(path)
}

pub(super) fn copy_file(
    source: &Path,
    destination: &Path,
    _force_copy: bool,
    _cloned: &mut u64,
    copied: &mut u64,
) -> io::Result<()> {
    fs::copy(source, destination)?;
    apply_file_metadata(source, destination)?;
    *copied += 1;
    Ok(())
}

pub(super) fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

pub(super) fn apply_directory_metadata(source: &Path, destination: &Path) -> io::Result<()> {
    apply_file_metadata(source, destination)
}

fn apply_file_metadata(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    // futimens 只需要已打开的 fd；只读文件不能先以写模式打开。
    let file = fs::OpenOptions::new().read(true).open(destination)?;
    file.set_times(
        fs::FileTimes::new()
            .set_accessed(modified_time(&metadata, true))
            .set_modified(modified_time(&metadata, false)),
    )?;
    drop(file);
    fs::set_permissions(
        destination,
        fs::Permissions::from_mode(metadata.mode() & 0o7777),
    )?;
    Ok(())
}

fn modified_time(metadata: &fs::Metadata, accessed: bool) -> std::time::SystemTime {
    use std::os::unix::fs::MetadataExt;
    let (seconds, nanos) = if accessed {
        (metadata.atime(), metadata.atime_nsec())
    } else {
        (metadata.mtime(), metadata.mtime_nsec())
    };
    if seconds >= 0 {
        UNIX_EPOCH + Duration::new(seconds as u64, nanos as u32)
    } else {
        UNIX_EPOCH - Duration::new(seconds.unsigned_abs(), nanos as u32)
    }
}
