//! macOS 保存前复制用户元数据；所有暂存写入都在最后一次基线核对之前完成。
use std::ffi::{CString, c_char, c_int, c_ulong, c_void};
use std::fs::{self, File};
use std::io;
use std::os::darwin::fs::MetadataExt as DarwinMetadataExt;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

unsafe extern "C" {
    fn fcopyfile(from: c_int, to: c_int, state: *mut c_void, flags: u32) -> c_int;
    fn fsetattrlist(
        fd: c_int,
        attrs: *const AttrList,
        data: *const TimeSpec,
        size: usize,
        options: c_ulong,
    ) -> c_int;
    fn acl_get_fd_np(fd: c_int, kind: c_int) -> *mut c_void;
    fn acl_init(count: c_int) -> *mut c_void;
    fn acl_set_fd(fd: c_int, acl: *mut c_void) -> c_int;
    fn acl_free(acl: *mut c_void) -> c_int;
    fn fchown(fd: c_int, uid: u32, gid: u32) -> c_int;
    fn geteuid() -> u32;
    fn flistxattr(fd: c_int, names: *mut c_char, size: usize, options: c_int) -> isize;
    fn fgetxattr(
        fd: c_int,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> isize;
    fn fsetxattr(
        fd: c_int,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> c_int;
}

#[repr(C)]
struct AttrList {
    count: u16,
    reserved: u16,
    common: u32,
    volume: u32,
    directory: u32,
    file: u32,
    fork: u32,
}

#[repr(C)]
struct TimeSpec {
    seconds: i64,
    nanoseconds: i64,
}

struct Acl(*mut c_void);
impl Drop for Acl {
    fn drop(&mut self) {
        // 指针由 acl_get_fd_np 分配且只释放一次。
        unsafe {
            acl_free(self.0);
        }
    }
}

pub(super) struct StagingCleanup<'a> {
    file: &'a File,
    pub published: bool,
}
impl<'a> StagingCleanup<'a> {
    pub fn new(file: &'a File) -> Self {
        Self {
            file,
            published: false,
        }
    }
}
impl Drop for StagingCleanup<'_> {
    fn drop(&mut self) {
        if !self.published {
            // 原文件可能禁止删除。失败时先移除暂存文件的 ACL，保证 NamedTempFile 可以清理它。
            let acl = unsafe { acl_init(0) };
            if !acl.is_null() {
                let acl = Acl(acl);
                let _ = unsafe { acl_set_fd(self.file.as_raw_fd(), acl.0) };
            }
        }
    }
}

fn checked(result: c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn read_attribute(mut read: impl FnMut(*mut c_void, usize) -> isize) -> io::Result<Vec<u8>> {
    let size = read(std::ptr::null_mut(), 0);
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut data = vec![0; size as usize];
    let size = read(data.as_mut_ptr().cast(), data.len());
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    data.truncate(size as usize);
    Ok(data)
}

fn verify_xattrs(source: &File, staged: &File) -> io::Result<()> {
    // copyfile 会对部分扩展属性采用尽力而为策略；逐项核验并补齐，不能静默丢掉用户数据。
    let names = read_attribute(|buffer, size| unsafe {
        flistxattr(source.as_raw_fd(), buffer.cast(), size, 0)
    })?;
    for name in names
        .split(|&byte| byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = CString::new(name)?;
        // 名字带 NUL 结尾，缓冲区在每次同步调用中有效；flags=0 包含所有可见扩展属性。
        let get = |file: &File| {
            read_attribute(|buffer, size| unsafe {
                fgetxattr(file.as_raw_fd(), name.as_ptr(), buffer, size, 0, 0)
            })
        };
        let original = get(source)?;
        if get(staged).ok().as_ref() != Some(&original) {
            checked(unsafe {
                fsetxattr(
                    staged.as_raw_fd(),
                    name.as_ptr(),
                    original.as_ptr().cast(),
                    original.len(),
                    0,
                    0,
                )
            })?;
            if get(staged)? != original {
                return Err(io::Error::other("扩展属性复制后不一致"));
            }
        }
    }
    Ok(())
}

pub(super) fn copy_metadata(source: &File, staged: &File) -> io::Result<()> {
    let metadata = source.metadata()?;
    // COPYFILE_XATTR 不指定 copy intent，复制全部扩展属性及资源叉；不复制旧的修改时间。
    // 文件描述符和空状态指针在同步调用期间有效。
    checked(unsafe {
        fcopyfile(
            source.as_raw_fd(),
            staged.as_raw_fd(),
            std::ptr::null_mut(),
            1 << 2,
        )
    })?;
    verify_xattrs(source, staged)?;
    let staged_metadata = staged.metadata()?;
    if (metadata.uid(), metadata.gid()) != (staged_metadata.uid(), staged_metadata.gid()) {
        // 所有者和组也影响权限语义；无法保留时让保存失败，避免悄悄改变文件归属。
        let result = checked(unsafe { fchown(staged.as_raw_fd(), metadata.uid(), metadata.gid()) });
        // 例外：/Users/Shared 等目录里的文件继承目录的属组（如 wheel），普通用户无法把它设回去。
        // 文件归自己、只有属组不同，且组权限与其他人权限相同时，换组不改变任何人的访问能力。
        let group_irrelevant = metadata.uid() == staged_metadata.uid()
            && (metadata.mode() >> 3) & 0o7 == metadata.mode() & 0o7;
        match result {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied && group_irrelevant => {}
            other => other?,
        }
    }
    staged.set_permissions(metadata.permissions())?;
    let attrs = AttrList {
        count: 5,
        reserved: 0,
        common: 0x200,
        volume: 0,
        directory: 0,
        file: 0,
        fork: 0,
    };
    let birth = TimeSpec {
        seconds: metadata.st_birthtime(),
        nanoseconds: metadata.st_birthtime_nsec(),
    };
    // ATTR_CMN_CRTIME 的数据只有一个 timespec，保留创建时间的纳秒精度。
    checked(unsafe {
        fsetattrlist(
            staged.as_raw_fd(),
            &attrs,
            &birth,
            std::mem::size_of_val(&birth),
            0,
        )
    })?;
    // ACL_TYPE_EXTENDED；单独检查读取和应用结果，避免 copyfile 对权限失败的尽力而为行为。
    let mut acl = unsafe { acl_get_fd_np(source.as_raw_fd(), 0x100) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
        // 没有 ACL 也要显式应用空 ACL，去掉暂存目录可能继承的条目。
        acl = unsafe { acl_init(0) };
    }
    if acl.is_null() {
        return Err(io::Error::last_os_error());
    }
    let acl = Acl(acl);
    checked(unsafe { acl_set_fd(staged.as_raw_fd(), acl.0) })
}

pub(super) fn volume_root_replacement(temporary: &Path, vault: &Path) -> bool {
    // 整卷作为库时，同卷暂存必然位于卷根之内。仅允许系统提供的私有替换目录。
    let is_root = vault == Path::new("/")
        || vault.parent().is_some_and(|parent| {
            fs::metadata(vault).is_ok_and(|root| {
                fs::metadata(parent).is_ok_and(|parent| root.dev() != parent.dev())
            })
        });
    let Ok(relative) = temporary.strip_prefix(vault) else {
        return false;
    };
    let parts: Vec<_> = relative.iter().collect();
    // geteuid 无参数且不持有资源；目录必须归当前用户所有，并且不向其他用户开放。
    let uid = unsafe { geteuid() };
    is_root
        && parts.len() == 4
        && parts[0] == ".TemporaryItems"
        && parts[1] == format!("folders.{uid}").as_str()
        && parts[2] == "TemporaryItems"
        && parts[3]
            .to_str()
            .is_some_and(|name| name.starts_with("NSIRD_"))
        && fs::metadata(temporary)
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0)
}
