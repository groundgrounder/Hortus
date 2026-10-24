//! 笔记编码、内容基线和原子替换；临时文件由平台提供的同卷替换目录承载。
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "macos")]
#[path = "note_macos.rs"]
mod macos;

// 固定的 FNV-1a 64 位算法；恢复稿文件名不能随 Rust 工具链的 Hasher 实现改变。
fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

#[derive(Clone, Debug)]
pub struct DiskVersion {
    pub bytes: Vec<u8>,
    fingerprint: u64,
}
impl DiskVersion {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            fingerprint: fnv1a(bytes.iter().copied()),
            bytes,
        }
    }
    pub fn matches(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint && self.bytes == other.bytes
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Format {
    pub bom: bool,
    pub crlf: bool,
}
impl Format {
    pub fn decode(bytes: &[u8]) -> (Self, String, bool) {
        let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
        let content = &bytes[if bom { 3 } else { 0 }..];
        let (mut text, valid) = match std::str::from_utf8(content) {
            Ok(text) => (text.to_owned(), true),
            Err(_) => (String::from_utf8_lossy(content).into_owned(), false),
        };
        let (mut crlf_count, mut lf_count, mut first) = (0, 0, None);
        for (index, &byte) in content.iter().enumerate() {
            if byte == b'\n' {
                let crlf = index > 0 && content[index - 1] == b'\r';
                first.get_or_insert(crlf);
                if crlf { crlf_count += 1 } else { lf_count += 1 }
            }
        }
        let crlf = crlf_count > lf_count || (crlf_count == lf_count && first.unwrap_or(false));
        if crlf_count > 0 {
            text = text.replace("\r\n", "\n");
        }
        (Self { bom, crlf }, text, valid)
    }
    pub fn encode(&self, text: &str) -> Vec<u8> {
        let mut bytes = if self.bom {
            vec![0xef, 0xbb, 0xbf]
        } else {
            vec![]
        };
        let text = text.replace("\r\n", "\n");
        if self.crlf {
            bytes.extend(text.replace('\n', "\r\n").bytes());
        } else {
            bytes.extend(text.bytes());
        }
        bytes
    }
}

pub fn note_path(root: &Path, relative: &str) -> io::Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(io::Error::other("note.invalid_path"));
    }
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::other("note.symlink"));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(path)
}

pub fn read(path: &Path) -> io::Result<Option<DiskVersion>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(DiskVersion::new(bytes))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn writable(path: &Path) -> bool {
    let check = |p: &Path| {
        fs::metadata(p).is_ok_and(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o222 != 0 && access_write(p)
            }
            #[cfg(not(unix))]
            {
                !m.permissions().readonly()
            }
        })
    };
    check(path) && path.parent().is_some_and(check)
}

#[cfg(unix)]
fn access_write(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn access(path: *const std::ffi::c_char, mode: std::ffi::c_int) -> std::ffi::c_int;
    }
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // access 同时检查 ACL；指针在同步调用期间有效，W_OK 在 Unix 平台为 2。
    unsafe { access(path.as_ptr(), 2) == 0 }
}

pub fn same_version(a: Option<&DiskVersion>, b: Option<&DiskVersion>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.matches(b),
        (None, None) => true,
        _ => false,
    }
}

#[derive(Debug)]
pub enum WriteFailure {
    Changed,
    Missing,
    Io(io::Error),
}
impl From<io::Error> for WriteFailure {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// 写入完成后、替换前再核对基线。新路径用硬链接发布，避免覆盖同时创建的文件。
pub fn atomic_replace(
    path: &Path,
    bytes: &[u8],
    baseline: Option<&DiskVersion>,
    temporary: &Path,
    vault: &Path,
    permissions: Option<fs::Permissions>,
) -> Result<DiskVersion, WriteFailure> {
    let temporary = fs::canonicalize(temporary)?;
    let vault = fs::canonicalize(vault)?;
    if temporary.starts_with(&vault) && !volume_root_replacement(&temporary, &vault) {
        return Err(io::Error::other("note.temp_inside_vault").into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("note.missing_parent"))?;
    let parent_meta = fs::metadata(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if parent_meta.dev() != fs::metadata(&temporary)?.dev() {
            return Err(io::Error::other("note.different_volume").into());
        }
        if parent_meta.permissions().mode() & 0o222 == 0 || !access_write(parent) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "note.directory_not_writable",
            )
            .into());
        }
    }
    if path.exists() && !writable(path) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "note.not_writable").into());
    }
    let mut staged = tempfile::NamedTempFile::new_in(temporary)?;
    staged.write_all(bytes)?;
    #[cfg(target_os = "macos")]
    let mut cleanup = macos::StagingCleanup::new(staged.as_file());
    #[cfg(target_os = "macos")]
    let copied_metadata = if baseline.is_some() {
        match fs::File::open(path) {
            Ok(source) => {
                macos::copy_metadata(&source, staged.as_file())?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        }
    } else {
        false
    };
    if let Some(permissions) = fs::metadata(path)
        .ok()
        .map(|m| m.permissions())
        .or(permissions)
    {
        // macOS 已连同 ACL 复制当前权限；再次 chmod 可能改变 ACL 的有效权限。
        #[cfg(target_os = "macos")]
        if !copied_metadata {
            staged.as_file().set_permissions(permissions)?;
        }
        #[cfg(not(target_os = "macos"))]
        staged.as_file().set_permissions(permissions)?;
    }
    staged.as_file().sync_all()?;
    let current = read(path)?;
    if !same_version(baseline, current.as_ref()) {
        return Err(if current.is_none() {
            WriteFailure::Missing
        } else {
            WriteFailure::Changed
        });
    }
    if baseline.is_some() {
        fs::rename(staged.path(), path)?;
    } else {
        publish_new(staged.path(), path)?;
    }
    #[cfg(target_os = "macos")]
    {
        cleanup.published = true;
    }
    // 目录同步让已完成的替换尽可能经受断电；不支持目录 fsync 的平台不把成功保存降为失败。
    let _ = fs::File::open(parent).and_then(|file| file.sync_all());
    Ok(DiskVersion::new(bytes.to_vec()))
}

fn publish_new(staged: &Path, path: &Path) -> Result<(), WriteFailure> {
    fs::hard_link(staged, path).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            WriteFailure::Changed
        } else {
            e.into()
        }
    })
}

fn volume_root_replacement(temporary: &Path, vault: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::volume_root_replacement(temporary, vault)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (temporary, vault);
        false
    }
}

#[derive(Serialize, Deserialize)]
struct RecoveryData {
    vault: String,
    note: String,
    text: String,
    #[serde(default)]
    format: Format,
}

pub struct Recovery {
    path: PathBuf,
    vault: String,
    note: String,
}
impl Recovery {
    pub fn new(cache: &Path, root: &Path, note: &str) -> Self {
        let vault = root.to_string_lossy().into_owned();
        // 路径不允许 NUL，以它分隔可避免两段路径拼接产生歧义。
        let hash = fnv1a(vault.bytes().chain([0]).chain(note.bytes()));
        Self {
            path: cache.join("recovery").join(format!("{hash:016x}.json")),
            vault,
            note: note.into(),
        }
    }
    pub fn load(&self) -> io::Result<Option<String>> {
        Ok(self.load_note()?.map(|(text, _)| text))
    }
    pub fn load_note(&self) -> io::Result<Option<(String, Format)>> {
        let Some(version) = read(&self.path)? else {
            return Ok(None);
        };
        let data: RecoveryData =
            serde_json::from_slice(&version.bytes).map_err(io::Error::other)?;
        if data.vault != self.vault || data.note != self.note {
            return Err(io::Error::other("note.recovery_path_mismatch"));
        }
        Ok(Some((data.text, data.format)))
    }
    pub fn write(&self, text: &str) -> io::Result<()> {
        self.write_note(text, &Format::default())
    }
    pub fn write_note(&self, text: &str, format: &Format) -> io::Result<()> {
        let parent = self.path.parent().unwrap();
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer(
            &mut file,
            &RecoveryData {
                vault: self.vault.clone(),
                note: self.note.clone(),
                text: text.into(),
                format: format.clone(),
            },
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn clear(&self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_fixed_vectors() {
        for (bytes, expected) in [
            (b"".as_slice(), 0xcbf29ce484222325),
            (b"a".as_slice(), 0xaf63dc4c8601ec8c),
            (b"foobar".as_slice(), 0x85944171f73967e8),
        ] {
            assert_eq!(fnv1a(bytes.iter().copied()), expected);
        }
        let cache = Path::new("/cache");
        assert_eq!(
            Recovery::new(cache, Path::new("/one"), "a.md").path,
            cache.join("recovery/c2874b21d0791fa4.json")
        );
        assert_ne!(
            Recovery::new(cache, Path::new("/ab"), "c.md").path,
            Recovery::new(cache, Path::new("/a"), "bc.md").path
        );
    }

    #[test]
    fn new_note_cannot_clobber_a_file_created_after_the_final_check() {
        let base = tempfile::tempdir().unwrap();
        let path = base.path().join("note.md");
        let staged = base.path().join("staged");
        fs::write(&staged, "mine").unwrap();
        assert!(read(&path).unwrap().is_none());
        // 模拟另一个进程在最终核对和发布之间创建同名文件。
        fs::write(&path, "theirs").unwrap();
        assert!(matches!(
            publish_new(&staged, &path),
            Err(WriteFailure::Changed)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
    }
}
