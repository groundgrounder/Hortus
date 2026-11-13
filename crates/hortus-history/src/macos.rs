//! macOS 快照复制：APFS `clonefile` 优先，其他文件系统用完整复制。
//!
//! 两条路径都逐项恢复权限、所有者、创建/修改时间、ACL 和扩展属性；
//! 只有系统管理的 `com.apple.provenance` 允许由系统转换，其余属性必须逐字一致。
use std::ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::fs;
use std::io;
use std::os::darwin::fs::MetadataExt as DarwinMetadataExt;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const COPYFILE_ACL: c_uint = 1 << 0;
const COPYFILE_STAT: c_uint = 1 << 1;
const COPYFILE_XATTR: c_uint = 1 << 2;
const COPYFILE_DATA: c_uint = 1 << 3;
const COPYFILE_ALL: c_uint = COPYFILE_ACL | COPYFILE_STAT | COPYFILE_XATTR | COPYFILE_DATA;

const O_RDONLY: c_int = 0;
const O_RDWR: c_int = 2;
const O_CREAT: c_int = 0x0200;
const O_EXCL: c_int = 0x0800;
const O_NOFOLLOW: c_int = 0x0100;
const O_DIRECTORY: c_int = 0x100000;

const AT_FDCWD: c_int = -2;
const AT_SYMLINK_NOFOLLOW: c_int = 0x0020;
const ACL_TYPE_EXTENDED: c_int = 0x100;
const ATTR_CMN_CRTIME: u32 = 0x200;
const XATTR_NOFOLLOW: c_int = 0x0001;

// 阻止修改文件自身元数据的用户/系统标志；复制阶段临时清除，最后按源恢复。
const UF_APPEND: u32 = 0x0000_0004;
const UF_COMPRESSED: u32 = 0x0000_0020;
const UF_IMMUTABLE: u32 = 0x0000_0002;
const SF_APPEND: u32 = 0x0004_0000;
const SF_IMMUTABLE: u32 = 0x0002_0000;

#[repr(C)]
struct TimeSpec {
    tv_sec: i64,
    tv_nsec: c_long,
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

unsafe extern "C" {
    fn clonefile(from: *const c_char, to: *const c_char, flags: c_int) -> c_int;
    fn fcopyfile(from: c_int, to: c_int, state: *mut c_void, flags: c_uint) -> c_int;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn fsync(fd: c_int) -> c_int;
    fn futimens(fd: c_int, times: *const TimeSpec) -> c_int;
    fn utimensat(dirfd: c_int, path: *const c_char, times: *const TimeSpec, flags: c_int) -> c_int;
    fn fchown(fd: c_int, owner: u32, group: u32) -> c_int;
    fn lchown(path: *const c_char, owner: u32, group: u32) -> c_int;
    fn fchmod(fd: c_int, mode: c_int) -> c_int;
    fn fchflags(fd: c_int, flags: c_uint) -> c_int;
    fn fsetattrlist(
        fd: c_int,
        attrs: *const AttrList,
        data: *const c_void,
        size: usize,
        options: c_ulong,
    ) -> c_int;
    fn acl_get_fd_np(fd: c_int, kind: c_int) -> *mut c_void;
    fn acl_set_fd(fd: c_int, acl: *mut c_void) -> c_int;
    fn acl_init(count: c_int) -> *mut c_void;
    fn acl_free(acl: *mut c_void) -> c_int;
    fn listxattr(path: *const c_char, names: *mut c_char, size: usize, options: c_int) -> isize;
    fn getxattr(
        path: *const c_char,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> isize;
    fn setxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> c_int;
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::other("history.invalid_path"))
}

fn checked(result: c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

struct Descriptor(RawFd);

impl Descriptor {
    fn open(path: &Path, flags: c_int, mode: c_int) -> io::Result<Self> {
        let path = c_path(path)?;
        let fd = unsafe { open(path.as_ptr(), flags, mode) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(fd))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // fd 由 Descriptor 独占；失败只会发生在进程退出阶段。
        unsafe {
            let _ = close(self.0);
        }
    }
}

pub(super) fn create_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().create(path)
}

pub(super) fn copy_file(
    source: &Path,
    destination: &Path,
    force_copy: bool,
    cloned: &mut u64,
    copied: &mut u64,
) -> io::Result<()> {
    let from = c_path(source)?;
    let to = c_path(destination)?;
    let result = if force_copy {
        -1
    } else {
        unsafe { clonefile(from.as_ptr(), to.as_ptr(), 0) }
    };
    if result == 0 {
        // clonefile 已复制数据与元数据；再核验一遍，避免文件系统差异。
        apply_file_metadata(source, destination)?;
        verify_file(source, destination)?;
        *cloned += 1;
        return Ok(());
    }
    // 跨卷或文件系统不支持 clonefile 时完整复制同一范围；任何失败都不能忽略。
    let from_fd = Descriptor::open(source, O_RDONLY | O_NOFOLLOW, 0)?;
    let to_fd = Descriptor::open(destination, O_RDWR | O_CREAT | O_EXCL, 0o600)?;
    checked(unsafe { fcopyfile(from_fd.0, to_fd.0, std::ptr::null_mut(), COPYFILE_ALL) })?;
    apply_file_metadata(source, destination)?;
    verify_file(source, destination)?;
    checked(unsafe { fsync(to_fd.0) })?;
    *copied += 1;
    Ok(())
}

pub(super) fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    let target = fs::read_link(source)?;
    std::os::unix::fs::symlink(&target, destination)?;
    let metadata = fs::symlink_metadata(source)?;
    let path = c_path(destination)?;
    let result = unsafe { lchown(path.as_ptr(), metadata.uid(), metadata.gid()) };
    if result != 0 {
        let error = io::Error::last_os_error();
        if !group_irrelevant(&metadata, &fs::symlink_metadata(destination)?) {
            return Err(error);
        }
    }
    set_times(&path, None, &metadata, true)?;
    copy_xattrs(source, destination, true)?;
    Ok(())
}

pub(super) fn apply_directory_metadata(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let directory = Descriptor::open(destination, O_RDONLY | O_DIRECTORY, 0)?;
    copy_ownership(directory.0, &metadata, destination)?;
    // 扩展属性与 ACL 需要目录仍可写；最终权限最后恢复。
    copy_xattrs(source, destination, false)?;
    copy_acl(source, destination)?;
    apply_birthtime(directory.0, &metadata)?;
    set_times(&c_path(destination)?, Some(directory.0), &metadata, false)?;
    checked(unsafe { fchmod(directory.0, (metadata.mode() & 0o7777) as c_int) })?;
    checked(unsafe { fsync(directory.0) })?;
    Ok(())
}

fn apply_file_metadata(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    // clonefile 会先继承 0444 等权限；只读 fd 也允许 fchmod/fchown/futimens/fsetattrlist。
    let file = Descriptor::open(destination, O_RDONLY | O_NOFOLLOW, 0)?;
    let flags = metadata.st_flags();
    let blocking = flags & (UF_IMMUTABLE | UF_APPEND | SF_IMMUTABLE | SF_APPEND);
    if blocking != 0 {
        // 先清掉 uchg/schg，否则后续元数据写入全部 EPERM。
        checked(unsafe { fchflags(file.0, (flags & !blocking) as c_uint) })?;
    }
    if metadata.mode() & 0o200 == 0 {
        // 只读目标上的 xattr/ACL 写入要求 owner write；复制完成前再恢复只读。
        checked(unsafe { fchmod(file.0, ((metadata.mode() | 0o200) & 0o7777) as c_int) })?;
    }
    copy_ownership(file.0, &metadata, destination)?;
    copy_xattrs(source, destination, false)?;
    copy_acl(source, destination)?;
    apply_birthtime(file.0, &metadata)?;
    set_times(&c_path(destination)?, Some(file.0), &metadata, false)?;
    // 最终权限与源一致，绝不留下临时可写状态。
    checked(unsafe { fchmod(file.0, (metadata.mode() & 0o7777) as c_int) })?;
    let restorable = flags & !UF_COMPRESSED;
    if restorable != 0 {
        checked(unsafe { fchflags(file.0, restorable as c_uint) })?;
    }
    checked(unsafe { fsync(file.0) })?;
    Ok(())
}

fn copy_ownership(fd: RawFd, metadata: &fs::Metadata, destination: &Path) -> io::Result<()> {
    if unsafe { fchown(fd, metadata.uid(), metadata.gid()) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // 与保存路径相同：归自己且组权限等同其他人权限时，换组不改变访问能力。
    if group_irrelevant(metadata, &fs::symlink_metadata(destination)?) {
        return Ok(());
    }
    Err(error)
}

fn group_irrelevant(source: &fs::Metadata, destination: &fs::Metadata) -> bool {
    source.uid() == destination.uid()
        && (source.mode() >> 3) & 0o7 == source.mode() & 0o7
        && destination.gid() != source.gid()
}

fn set_times(
    path: &CString,
    fd: Option<RawFd>,
    metadata: &fs::Metadata,
    symlink: bool,
) -> io::Result<()> {
    let times = [
        TimeSpec {
            tv_sec: metadata.atime(),
            tv_nsec: metadata.atime_nsec(),
        },
        TimeSpec {
            tv_sec: metadata.mtime(),
            tv_nsec: metadata.mtime_nsec(),
        },
    ];
    let result = if let Some(fd) = fd {
        unsafe { futimens(fd, times.as_ptr()) }
    } else {
        let flags = if symlink { AT_SYMLINK_NOFOLLOW } else { 0 };
        unsafe { utimensat(AT_FDCWD, path.as_ptr(), times.as_ptr(), flags) }
    };
    checked(result)
}

fn apply_birthtime(fd: RawFd, metadata: &fs::Metadata) -> io::Result<()> {
    let birth = TimeSpec {
        tv_sec: metadata.st_birthtime(),
        tv_nsec: metadata.st_birthtime_nsec(),
    };
    let attrs = AttrList {
        count: 5,
        reserved: 0,
        common: ATTR_CMN_CRTIME,
        volume: 0,
        directory: 0,
        file: 0,
        fork: 0,
    };
    checked(unsafe {
        fsetattrlist(
            fd,
            &attrs,
            (&birth as *const TimeSpec).cast(),
            std::mem::size_of_val(&birth),
            0,
        )
    })
}

fn copy_acl(source: &Path, destination: &Path) -> io::Result<()> {
    let from = Descriptor::open(source, O_RDONLY | O_NOFOLLOW, 0)?;
    let to = Descriptor::open(destination, O_RDONLY, 0)?;
    let mut acl = unsafe { acl_get_fd_np(from.0, ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
        acl = unsafe { acl_init(0) };
    }
    if acl.is_null() {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { acl_set_fd(to.0, acl) };
    unsafe {
        let _ = acl_free(acl);
    }
    checked(result)
}

/// 复制全部扩展属性并逐项核验；`com.apple.provenance` 由系统管理，跳过逐字校验。
fn copy_xattrs(source: &Path, destination: &Path, symlink: bool) -> io::Result<()> {
    let from = c_path(source)?;
    let to = c_path(destination)?;
    // macOS 用同一个调用加 XATTR_NOFOLLOW 读取链接自身，而不是库外目标。
    let options = if symlink { XATTR_NOFOLLOW } else { 0 };
    let list =
        |names: *mut c_char, size: usize| unsafe { listxattr(from.as_ptr(), names, size, options) };
    let size = list(std::ptr::null_mut(), 0);
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    if size == 0 {
        return Ok(());
    }
    let mut names = vec![0u8; size as usize];
    let read = list(names.as_mut_ptr().cast(), names.len());
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    names.truncate(read as usize);
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = CString::new(name)?;
        if name.as_bytes() == b"com.apple.provenance" {
            continue;
        }
        let get = |path: &CString| -> io::Result<Vec<u8>> {
            let size = unsafe {
                getxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    options,
                )
            };
            if size < 0 {
                let error = io::Error::last_os_error();
                return Err(error);
            }
            let mut value = vec![0; size as usize];
            let read = unsafe {
                getxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    0,
                    options,
                )
            };
            if read < 0 {
                return Err(io::Error::last_os_error());
            }
            value.truncate(read as usize);
            Ok(value)
        };
        let value = get(&from)?;
        if get(&to).ok().as_ref() == Some(&value) {
            continue;
        }
        let result = unsafe {
            setxattr(
                to.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                options,
            )
        };
        checked(result)?;
        if get(&to)? != value {
            return Err(io::Error::other("history.xattr_mismatch"));
        }
    }
    Ok(())
}

fn verify_file(source: &Path, destination: &Path) -> io::Result<()> {
    let source = fs::symlink_metadata(source)?;
    let destination = fs::symlink_metadata(destination)?;
    if source.size() != destination.size() {
        return Err(io::Error::other("history.size_mismatch"));
    }
    if source.mode() & 0o7777 != destination.mode() & 0o7777 {
        return Err(io::Error::other("history.mode_mismatch"));
    }
    if (source.mtime(), source.mtime_nsec()) != (destination.mtime(), destination.mtime_nsec()) {
        return Err(io::Error::other("history.mtime_mismatch"));
    }
    if destination.uid() != source.uid() {
        return Err(io::Error::other("history.owner_mismatch"));
    }
    // 压缩是文件系统内部状态，允许由数据复制方式决定；其余标志必须逐字一致。
    let mask = !UF_COMPRESSED;
    if source.st_flags() & mask != destination.st_flags() & mask {
        return Err(io::Error::other("history.flags_mismatch"));
    }
    Ok(())
}
