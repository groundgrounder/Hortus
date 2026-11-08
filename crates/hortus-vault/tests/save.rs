use hortus_vault::note::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn format_roundtrip_preserves_bom_line_endings_and_eof() {
    for bom in [false, true] {
        for crlf in [false, true] {
            for newline in [false, true] {
                let text = format!("中😀\n末尾{}", if newline { "\n" } else { "" });
                let format = Format { bom, crlf };
                let bytes = format.encode(&text);
                let (decoded, actual, valid) = Format::decode(&bytes);
                assert!(valid);
                assert_eq!(actual, text);
                assert_eq!(decoded.encode(&actual), bytes);
                assert_eq!(decoded.bom, bom);
                assert_eq!(decoded.crlf, crlf);
            }
        }
    }
    assert!(!Format::decode(&[0xff, 0xfe, 0x41]).2);
}

#[test]
fn mixed_line_endings_choose_majority_then_first_and_normalize_new_input() {
    for (source, crlf) in [
        ("a\nb\nc\n", false),
        ("a\r\nb\r\nc\r\n", true),
        ("a\nb\r\nc\n", false),
        ("a\nb\r\nc\r\n", true),
        ("a\nb\r\n", false),
        ("a\r\nb\n", true),
        ("a\rb", false),
        ("", false),
    ] {
        let (format, normalized, valid) = Format::decode(source.as_bytes());
        assert!(valid);
        assert_eq!(format.crlf, crlf, "{source:?}");
        let edited = format!("{normalized}new\nwindows\r\n");
        let canonical = edited.replace("\r\n", "\n");
        let expected = if crlf {
            canonical.replace('\n', "\r\n")
        } else {
            canonical
        };
        assert_eq!(format.encode(&edited), expected.as_bytes());
    }
}

#[test]
fn atomic_replacement_rechecks_disk_retains_permissions_and_cleans_failed_staging() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempdir().unwrap();
    let vault = base.path().join("vault");
    let temporary = base.path().join("replacement");
    fs::create_dir_all(&vault).unwrap();
    fs::create_dir_all(&temporary).unwrap();
    let path = vault.join("note.md");
    fs::write(&path, "before").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let baseline = read(&path).unwrap().unwrap();
    atomic_replace(&path, b"after", Some(&baseline), &temporary, &vault, None).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    assert!(matches!(
        atomic_replace(&path, b"new", None, &temporary, &vault, None),
        Err(WriteFailure::Changed)
    ));
    assert_eq!(fs::read(&path).unwrap(), b"after");
    assert_eq!(fs::read_dir(&vault).unwrap().count(), 1);
    assert!(matches!(
        atomic_replace(&path, b"stale", Some(&baseline), &temporary, &vault, None),
        Err(WriteFailure::Changed)
    ));
    assert_eq!(fs::read(&path).unwrap(), b"after");
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    assert!(matches!(
        atomic_replace(&path, b"bad", None, &vault, &vault, None),
        Err(WriteFailure::Io(_))
    ));
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        atomic_replace(&path, b"missing", Some(&baseline), &temporary, &vault, None),
        Err(WriteFailure::Missing)
    ));
    atomic_replace(&path, b"recreate", None, &temporary, &vault, None).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"recreate");
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
}

#[test]
fn recovery_is_separate_per_vault_and_note_and_cleared_after_resolution() {
    let base = tempdir().unwrap();
    let a = Recovery::new(base.path(), std::path::Path::new("/one"), "a.md");
    let b = Recovery::new(base.path(), std::path::Path::new("/two"), "a.md");
    let c = Recovery::new(base.path(), std::path::Path::new("/one"), "b.md");
    a.write("甲😀").unwrap();
    b.write("乙").unwrap();
    c.write("丙").unwrap();
    for name in [
        "c2874b21d0791fa4.json",
        "c48aae46c728764a.json",
        "6ecdcb091db87553.json",
    ] {
        assert!(base.path().join("recovery").join(name).is_file());
    }
    assert_eq!(a.load().unwrap().as_deref(), Some("甲😀"));
    assert_eq!(b.load().unwrap().as_deref(), Some("乙"));
    assert_eq!(c.load().unwrap().as_deref(), Some("丙"));
    a.clear().unwrap();
    assert!(a.load().unwrap().is_none());
    assert!(b.load().unwrap().is_some());
    let path = base.path().join("recovery/c48aae46c728764a.json");
    let original = fs::read(&path).unwrap();
    for field in ["vault", "note"] {
        let mut data: serde_json::Value = serde_json::from_slice(&original).unwrap();
        data[field] = "wrong".into();
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        assert!(b.load().is_err());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn replacement_preserves_birthtime_all_xattrs_acl_and_mode() {
    use std::os::darwin::fs::MetadataExt;
    use std::process::Command;
    fn run(program: &str, args: &[&str], path: &std::path::Path) -> Vec<u8> {
        let output = Command::new(program).args(args).arg(path).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    let base = tempdir().unwrap();
    let vault = base.path().join("vault");
    let temporary = base.path().join("replacement");
    fs::create_dir(&vault).unwrap();
    fs::create_dir(&temporary).unwrap();
    let path = vault.join("note.md");
    fs::write(&path, "old").unwrap();
    run("SetFile", &["-d", "01/02/2000 03:04:05"], &path);
    run(
        "xattr",
        &[
            "-wx",
            "com.apple.metadata:_kMDItemUserTags",
            "62706c6973743030",
        ],
        &path,
    );
    run(
        "xattr",
        &[
            "-wx",
            "com.apple.metadata:kMDItemFinderComment",
            "62706c6973743030",
        ],
        &path,
    );
    run("xattr", &["-wx", "user.hortus-test", "0001ffabcd"], &path);
    run(
        "xattr",
        &["-wx", "com.apple.ResourceFork", "0102030405"],
        &path,
    );
    run("chmod", &["640"], &path);
    run(
        "chmod",
        &[
            "+a",
            "everyone allow read,readattr,readextattr,readsecurity",
        ],
        &path,
    );
    let before = fs::metadata(&path).unwrap();
    let xattrs = run("xattr", &["-lx"], &path);
    let acl = run("ls", &["-le"], &path);
    let baseline = read(&path).unwrap().unwrap();
    atomic_replace(&path, b"new", Some(&baseline), &temporary, &vault, None).unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!(before.st_birthtime(), after.st_birthtime());
    assert_eq!(before.st_birthtime_nsec(), after.st_birthtime_nsec());
    assert_eq!(before.st_mode(), after.st_mode());
    assert_eq!(run("xattr", &["-lx"], &path), xattrs);
    let after_acl = run("ls", &["-le"], &path);
    assert_eq!(
        after_acl.split(|&b| b == b'\n').skip(1).collect::<Vec<_>>(),
        acl.split(|&b| b == b'\n').skip(1).collect::<Vec<_>>()
    );
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    run("chmod", &["+a", "everyone deny delete"], &path);
    let baseline = read(&path).unwrap().unwrap();
    let result = atomic_replace(&path, b"blocked", Some(&baseline), &temporary, &vault, None);
    let clean = fs::read_dir(&temporary).unwrap().count() == 0;
    run("chmod", &["-N"], &path);
    assert!(matches!(result, Err(WriteFailure::Io(_))));
    assert!(clean, "替换被 ACL 拒绝后也必须清理暂存文件");
    assert_eq!(fs::read(&path).unwrap(), b"new");
}

#[cfg(target_os = "macos")]
#[test]
fn inherited_group_that_cannot_be_restored_only_blocks_saves_when_it_matters() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    // /private/tmp 与 /Users/Shared 一样是 root:wheel 的粘滞目录，新文件继承 wheel 组。
    let shared = tempfile::tempdir_in("/private/tmp").unwrap();
    let staging = tempdir().unwrap();
    let vault = shared.path().join("vault");
    let temporary = staging.path().join("replacement");
    fs::create_dir(&vault).unwrap();
    fs::create_dir(&temporary).unwrap();
    let path = vault.join("note.md");
    fs::write(&path, "old").unwrap();
    let gid = fs::metadata(&path).unwrap().gid();
    if gid == fs::metadata(&temporary).unwrap().gid()
        || std::process::Command::new("id")
            .args(["-G"])
            .output()
            .is_ok_and(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .any(|g| g == gid.to_string())
            })
    {
        eprintln!("跳过：当前用户属于组 {gid}，或暂存目录与笔记同组，无法复现换组失败");
        return;
    }
    // 组与其他人权限相同：属组无法保留也不改变任何人的访问能力，保存成功。
    let baseline = read(&path).unwrap().unwrap();
    atomic_replace(&path, b"new", Some(&baseline), &temporary, &vault, None).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);

    // 组权限与其他人不同：换组会改变访问能力，仍然拒绝保存并保留原文件。
    let path = vault.join("group.md");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let baseline = read(&path).unwrap().unwrap();
    let result = atomic_replace(&path, b"new", Some(&baseline), &temporary, &vault, None);
    assert!(matches!(result, Err(WriteFailure::Io(_))));
    assert_eq!(fs::read(&path).unwrap(), b"old");
    assert_eq!(fs::metadata(&path).unwrap().gid(), gid);
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn readonly_acl_is_detected_even_when_mode_bits_allow_writing() {
    let base = tempdir().unwrap();
    let path = base.path().join("note.md");
    fs::write(&path, "原文").unwrap();
    assert!(
        std::process::Command::new("chmod")
            .args(["+a", "everyone deny write"])
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let detected = !writable(&path);
    assert!(
        std::process::Command::new("chmod")
            .arg("-N")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    assert!(detected);
}

#[cfg(target_os = "macos")]
#[test]
fn replacement_accepts_transformed_provenance_and_still_copies_other_xattrs_exactly() {
    use std::process::Command;
    fn xattr(path: &std::path::Path, name: &str) -> Vec<u8> {
        let output = Command::new("xattr")
            .args(["-px", name])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).unwrap())
            .collect()
    }
    fn set_xattr(path: &std::path::Path, name: &str, value: &[u8]) {
        let hex: String = value.iter().map(|byte| format!("{byte:02x}")).collect();
        let output = Command::new("xattr")
            .args(["-wx", name, &hex])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // launchd 作业没有终端来源，可以原样写入 provenance；测试进程则会把它改写。
    fn set_provenance(path: &std::path::Path, value: &[u8]) -> bool {
        let hex: String = value.iter().map(|byte| format!("{byte:02x}")).collect();
        let label = format!("hortus-p1-47-test-{}", std::process::id());
        let submitted = Command::new("launchctl")
            .args([
                "submit",
                "-l",
                &label,
                "--",
                "/usr/bin/xattr",
                "-wx",
                "com.apple.provenance",
                &hex,
            ])
            .arg(path)
            .status()
            .is_ok_and(|status| status.success());
        for _ in 0..250 {
            if xattr(path, "com.apple.provenance") == value {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = Command::new("launchctl").args(["remove", &label]).status();
        submitted && xattr(path, "com.apple.provenance") == value
    }
    let base = tempdir().unwrap();
    let vault = base.path().join("vault");
    let temporary = base.path().join("replacement");
    fs::create_dir(&vault).unwrap();
    fs::create_dir(&temporary).unwrap();
    let path = vault.join("note.md");
    fs::write(&path, "old").unwrap();
    set_xattr(
        &path,
        "com.apple.metadata:kMDItemFinderComment",
        b"hortus provenance regression",
    );
    set_xattr(
        &path,
        "user.hortus-provenance",
        &[0x00, 0x01, 0x02, 0xff, 0xab, 0xcd, 0xef],
    );
    // 合成字节只用于触发系统转换，不来自任何用户文件。放在其他属性之后：
    // 终端上下文写普通 xattr 时也可能重写 provenance。
    let requested = [0x01, 0x02, 0x00, 0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03];
    if !set_provenance(&path, &requested) {
        eprintln!("跳过：本机无法通过 launchd 写入非默认 provenance");
        return;
    }
    let provenance_before = xattr(&path, "com.apple.provenance");
    let comment_before = xattr(&path, "com.apple.metadata:kMDItemFinderComment");
    let user_before = xattr(&path, "user.hortus-provenance");
    let baseline = read(&path).unwrap().unwrap();
    atomic_replace(&path, b"new", Some(&baseline), &temporary, &vault, None).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    // 普通用户属性和 Finder 元数据仍逐字保留。
    assert_eq!(
        xattr(&path, "com.apple.metadata:kMDItemFinderComment"),
        comment_before
    );
    assert_eq!(xattr(&path, "user.hortus-provenance"), user_before);
    // provenance 由系统管理；只要求仍在，不要求与来源逐字相等。
    let provenance_after = xattr(&path, "com.apple.provenance");
    assert!(!provenance_after.is_empty());
    if provenance_after == provenance_before {
        eprintln!("注意：本机未转换 provenance，系统转换分支未被实际覆盖");
    }
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
}
