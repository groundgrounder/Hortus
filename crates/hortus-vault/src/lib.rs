//! 文件库：后台分批扫描、系统文件监听、原子保存与恢复稿。
pub mod note;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub parent: String,
    pub name: String,
    pub directory: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub upserted: Vec<Entry>,
    pub removed: Vec<String>,
    pub scan_complete: bool,
    pub scan_micros: u64,
    pub errors: Vec<String>,
}

/// 所有目录都由内核排好顺序；macOS 使用系统的本地化数字比较。
pub fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    #[cfg(target_os = "macos")]
    {
        use core_foundation::base::TCFType;
        use core_foundation::string::*;
        let a_cf = CFString::new(a);
        let b_cf = CFString::new(b);
        // 与 localizedStandardCompare 一样启用数字、本地化、宽度及大小写折叠。
        let flags = kCFCompareCaseInsensitive
            | kCFCompareNonliteral
            | kCFCompareNumerically
            | kCFCompareLocalized
            | kCFCompareWidthInsensitive
            | kCFCompareForcedOrdering;
        // 两个引用在调用期间有效，CFStringCompare 不持有它们。
        let result = unsafe {
            CFStringCompare(
                a_cf.as_concrete_TypeRef(),
                b_cf.as_concrete_TypeRef(),
                flags,
            )
        };
        (result as isize).cmp(&0).then_with(|| a.cmp(b))
    }
    #[cfg(not(target_os = "macos"))]
    {
        // 其他平台暂以数字段长度和值比较，避免解析任意长数字时溢出。
        let (a, b) = (a.to_lowercase(), b.to_lowercase());
        let (mut ai, mut bi) = (0, 0);
        let (aa, bb) = (a.as_bytes(), b.as_bytes());
        while ai < aa.len() && bi < bb.len() {
            if aa[ai].is_ascii_digit() && bb[bi].is_ascii_digit() {
                let (start_a, start_b) = (ai, bi);
                while ai < aa.len() && aa[ai].is_ascii_digit() {
                    ai += 1;
                }
                while bi < bb.len() && bb[bi].is_ascii_digit() {
                    bi += 1;
                }
                let x = a[start_a..ai].trim_start_matches('0');
                let y = b[start_b..bi].trim_start_matches('0');
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if !order.is_eq() {
                    return order;
                }
            } else {
                let order = aa[ai].cmp(&bb[bi]);
                if !order.is_eq() {
                    return order;
                }
                ai += 1;
                bi += 1;
            }
        }
        aa.len().cmp(&bb.len())
    }
}

pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| compare_names(&a.name, &b.name))
    });
}

fn allowed(path: &Path) -> bool {
    path.components().all(|component| match component {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            !name.starts_with('.') && name != "node_modules"
        }
        _ => false,
    })
}

/// 校验单段名称：去首尾空白后不得为空、`.`、`..`、隐藏项或 `node_modules`，
/// 也不得含路径分隔符、NUL 或任何控制字符；返回去空白后的名称。
fn validate_name(name: &str) -> io::Result<String> {
    let trimmed = name.trim();
    let invalid = trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.starts_with('.')
        || trimmed == "node_modules"
        || trimmed.contains(['/', '\\', '\0'])
        || trimmed.chars().any(|character| character.is_control());
    if invalid {
        return Err(io::Error::other("vault.invalid_name"));
    }
    Ok(trimmed.into())
}

fn has_note_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["md", "markdown", "mmd"]
                .iter()
                .any(|supported| ext.eq_ignore_ascii_case(supported))
        })
}

/// 解析库内相对路径（空字符串表示库根）：逐段核对可见性与符号链接，
/// 拒绝经过符号链接或越出 `root` 的目标；不存在返回 `vault.not_found`。
fn resolve_inside(root: &Path, relative: &str) -> io::Result<PathBuf> {
    if relative.is_empty() {
        return Ok(root.to_path_buf());
    }
    let path = Path::new(relative);
    if !allowed(path)
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::other("vault.invalid_path"));
    }
    let mut absolute = root.to_path_buf();
    for component in path.components() {
        absolute.push(component);
        match fs::symlink_metadata(&absolute) {
            // 不跟随符号链接：任何一段是链接都拒绝，避免越出库或进入库外目录。
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::other("vault.symlink_unsupported"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(io::Error::other("vault.not_found"));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(absolute)
}

fn entry(root: &Path, absolute: &Path) -> io::Result<Option<Entry>> {
    let relative = absolute.strip_prefix(root).map_err(io::Error::other)?;
    if relative.as_os_str().is_empty() || !allowed(relative) {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(absolute) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // 不跟随符号链接，避免越出库、循环扫描或读取库外文件。
    if metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let directory = metadata.is_dir();
    let ext = relative.extension().and_then(|s| s.to_str()).unwrap_or("");
    if !directory
        && (!metadata.is_file()
            || !["md", "markdown", "mmd"]
                .iter()
                .any(|s| ext.eq_ignore_ascii_case(s)))
    {
        return Ok(None);
    }
    let path = relative
        .to_str()
        .ok_or_else(|| io::Error::other("vault.invalid_utf8_path"))?
        .to_owned();
    Ok(Some(Entry {
        parent: relative
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .into_owned(),
        name: relative.file_name().unwrap().to_string_lossy().into_owned(),
        path,
        directory,
    }))
}

fn children(root: &Path, dir: &Path, errors: &mut Vec<String>) -> Vec<Entry> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(_) => {
            errors.push("vault.scan_failed".into());
            return vec![];
        }
    };
    let mut entries = Vec::new();
    for item in read {
        match item.and_then(|item| entry(root, &item.path())) {
            Ok(Some(value)) => entries.push(value),
            Ok(None) => {}
            Err(_) => errors.push("vault.scan_failed".into()),
        }
    }
    sort_entries(&mut entries);
    entries
}

pub struct Vault {
    root: PathBuf,
    stop: Arc<AtomicBool>,
    events: mpsc::Sender<WorkerEvent>,
    batches: Mutex<mpsc::Receiver<Batch>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

enum WorkerEvent {
    File(notify::Result<Event>),
    Stop,
}

impl Vault {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let root = fs::canonicalize(path)?;
        if !root.is_dir() {
            return Err(io::Error::other("vault.select_folder"));
        }
        let (events_tx, events_rx) = mpsc::channel();
        let watcher_tx = events_tx.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = watcher_tx.send(WorkerEvent::File(event));
        })
        .map_err(io::Error::other)?;
        // 先注册监听，再扫描，扫描期间的变更也不会遗漏。
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(io::Error::other)?;
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_root = root.clone();
        let worker = thread::Builder::new()
            .name("hortus-vault".into())
            .spawn(move || {
                run(worker_root, worker_stop, tx, events_rx, watcher);
            })?;
        Ok(Self {
            root,
            stop,
            events: events_tx,
            batches: Mutex::new(rx),
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 阻塞到有批次或关闭；空数组表示已关闭，调用方应退出等待线程。
    pub fn wait_for_batches(&self) -> Vec<Batch> {
        let batches = self.batches.lock().unwrap();
        if self.stop.load(Ordering::Acquire) {
            return vec![];
        }
        let Ok(first) = batches.recv() else {
            return vec![];
        };
        if self.stop.load(Ordering::Acquire) {
            return vec![];
        }
        std::iter::once(first).chain(batches.try_iter()).collect()
    }

    /// 返回与编辑器相同的正文投影：去除 BOM、CRLF 统一为 LF，无效 UTF-8 以替换符表示。
    pub fn read_note(&self, relative: &str) -> io::Result<String> {
        let path = Path::new(relative);
        if !allowed(path) {
            return Err(io::Error::other("vault.invalid_path"));
        }
        let absolute = self.root.join(path);
        let resolved = fs::canonicalize(&absolute)?;
        if !resolved.starts_with(&self.root)
            || !matches!(entry(&self.root, &absolute)?, Some(e) if !e.directory)
        {
            return Err(io::Error::other("vault.outside_root"));
        }
        // 搜索索引必须和 DocumentSession 看到同一份正文，否则 UTF-16 范围会偏移。
        Ok(note::Format::decode(&fs::read(resolved)?).1)
    }

    /// 库内新建笔记：无受支持扩展名时自动补 `.md`；独占创建绝不覆盖既有文件，
    /// 目标已存在（含大小写不敏感文件系统上的同名冲突）返回 `vault.already_exists`。
    pub fn create_note(&self, parent: &str, name: &str) -> io::Result<Entry> {
        let directory = resolve_inside(&self.root, parent)?;
        if !directory.is_dir() {
            return Err(io::Error::other("vault.not_found"));
        }
        let mut name = validate_name(name)?;
        if !has_note_extension(&name) {
            // 纯扩展名（如 `.md`）已被隐藏名规则拒绝；这里只补缺失的扩展名。
            name.push_str(".md");
        }
        let absolute = directory.join(&name);
        if fs::symlink_metadata(&absolute).is_ok() {
            return Err(io::Error::other("vault.already_exists"));
        }
        // create_new 保证与其他进程的并发创建不覆盖，只报 AlreadyExists。
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&absolute)
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => io::Error::other("vault.already_exists"),
                _ => error,
            })?;
        entry(&self.root, &absolute)?.ok_or_else(|| io::Error::other("vault.scan_failed"))
    }

    /// 库内新建单级文件夹：非递归创建，目标已存在返回 `vault.already_exists`。
    pub fn create_folder(&self, parent: &str, name: &str) -> io::Result<Entry> {
        let directory = resolve_inside(&self.root, parent)?;
        if !directory.is_dir() {
            return Err(io::Error::other("vault.not_found"));
        }
        let name = validate_name(name)?;
        let absolute = directory.join(&name);
        fs::create_dir(&absolute).map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => io::Error::other("vault.already_exists"),
            _ => error,
        })?;
        entry(&self.root, &absolute)?.ok_or_else(|| io::Error::other("vault.scan_failed"))
    }

    /// 库内同目录重命名（不跨目录移动）：新旧名称字节一致视为无操作；
    /// 目标指向其他文件或目录返回 `vault.already_exists`，大小写不敏感文件系统上
    /// 与源同 inode 的原地改名（如 APFS `note.md` → `Note.md`）放行。
    /// 笔记不允许改成 `hortus-vault` 不识别的扩展名，避免重命名后从文件树消失。
    pub fn rename_entry(&self, path: &str, new_name: &str) -> io::Result<Entry> {
        let new_name = validate_name(new_name)?;
        let source = resolve_inside(&self.root, path)?;
        let source_meta = fs::symlink_metadata(&source).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => io::Error::other("vault.not_found"),
            _ => error,
        })?;
        let source_entry =
            entry(&self.root, &source)?.ok_or_else(|| io::Error::other("vault.not_found"))?;
        if source_entry.name.as_bytes() == new_name.as_bytes() {
            return Ok(source_entry);
        }
        if !source_entry.directory
            && has_note_extension(&source_entry.name)
            && !has_note_extension(&new_name)
        {
            return Err(io::Error::other("vault.invalid_name"));
        }
        let parent = source
            .parent()
            .ok_or_else(|| io::Error::other("vault.invalid_path"))?;
        let target = parent.join(&new_name);
        if let Ok(target_meta) = fs::symlink_metadata(&target) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let same_inode = target_meta.dev() == source_meta.dev()
                    && target_meta.ino() == source_meta.ino();
                if !same_inode {
                    return Err(io::Error::other("vault.already_exists"));
                }
            }
            #[cfg(not(unix))]
            {
                return Err(io::Error::other("vault.already_exists"));
            }
        }
        // 同目录整棵原子重命名：非空文件夹连同子项一起移动，不丢失内容。
        fs::rename(&source, &target)?;
        entry(&self.root, &target)?.ok_or_else(|| io::Error::other("vault.scan_failed"))
    }

    /// 移到废纸篓前的安全目标解析：返回确认位于库根之内、非符号链接且存在的
    /// 绝对路径与是否目录，供平台层调用系统废纸篓 API。
    pub fn resolve_trash_target(&self, path: &str) -> io::Result<(PathBuf, bool)> {
        if path.is_empty() {
            return Err(io::Error::other("vault.invalid_path"));
        }
        let absolute = resolve_inside(&self.root, path)?;
        let metadata = fs::symlink_metadata(&absolute).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => io::Error::other("vault.not_found"),
            _ => error,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("vault.symlink_unsupported"));
        }
        Ok((absolute, metadata.is_dir()))
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::Release);
        // 文件监听器还持有发送端，必须显式唤醒正在阻塞或去抖的工作线程。
        let _ = self.events.send(WorkerEvent::Stop);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.close();
    }
}

fn run(
    root: PathBuf,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<Batch>,
    rx: mpsc::Receiver<WorkerEvent>,
    _watcher: notify::RecommendedWatcher,
) {
    let start = Instant::now();
    let mut known = BTreeMap::new();
    let mut queue = VecDeque::from([root.clone()]);
    while let Some(dir) = queue.pop_front() {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let mut errors = Vec::new();
        let list = children(&root, &dir, &mut errors);
        for e in &list {
            if e.directory {
                queue.push_back(root.join(&e.path));
            }
            known.insert(e.path.clone(), e.clone());
        }
        for chunk in list.chunks(128) {
            if tx
                .send(Batch {
                    upserted: chunk.to_vec(),
                    ..Batch::default()
                })
                .is_err()
            {
                return;
            }
        }
        if !errors.is_empty() {
            let _ = tx.send(Batch {
                errors,
                ..Batch::default()
            });
        }
    }
    let _ = tx.send(Batch {
        scan_complete: true,
        scan_micros: start.elapsed().as_micros() as u64,
        ..Batch::default()
    });
    while !stop.load(Ordering::Acquire) {
        let first = match rx.recv() {
            Ok(WorkerEvent::File(event)) => event,
            Ok(WorkerEvent::Stop) | Err(_) => return,
        };
        let mut pending = BTreeSet::new();
        let mut errors = Vec::new();
        collect_event(first, &root, &mut pending, &mut errors);
        if pending.is_empty() && errors.is_empty() {
            continue;
        }
        let deadline = Instant::now() + Duration::from_millis(400);
        // 安静 100 ms 后刷新；连续事件最多等待 400 ms，避免一直拖延。
        while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(WorkerEvent::File(event)) => {
                    collect_event(event, &root, &mut pending, &mut errors)
                }
                Ok(WorkerEvent::Stop) => return,
                Err(_) => break,
            }
        }
        if stop.load(Ordering::Acquire) {
            return;
        }
        let mut batch = Batch {
            errors,
            ..Batch::default()
        };
        // 大小写不敏感文件系统（APFS 默认）上的原地改名会同时报告新旧两个名字；
        // 旧名经不区分大小写的 stat 依然命中，须先辨明磁盘上真实存在的名字，
        // 把旧变体从本批与已知表中清掉，避免条目复活成重复项。
        let mut by_case: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for path in &pending {
            let lower = path.to_string_lossy().to_lowercase();
            by_case.entry(lower).or_default().push(path.clone());
        }
        for (_, group) in by_case {
            if group.len() < 2 {
                continue;
            }
            let mut live: Vec<&PathBuf> = Vec::new();
            for path in &group {
                let exact = path
                    .parent()
                    .and_then(|parent| fs::read_dir(parent).ok())
                    .is_some_and(|read| {
                        read.flatten()
                            .any(|item| item.file_name() == path.file_name().unwrap_or_default())
                    });
                if exact {
                    live.push(path);
                }
            }
            if live.is_empty() {
                continue;
            }
            for path in &group {
                if live.contains(&path) {
                    continue;
                }
                pending.remove(path);
                if let Some(key) = path
                    .strip_prefix(&root)
                    .ok()
                    .map(|relative| relative.to_string_lossy().into_owned())
                    && known.remove(&key).is_some()
                {
                    batch.removed.push(key);
                }
            }
        }
        // 重叠路径只处理最上层一次，普通文件事件只读该文件的元数据。
        let mut roots: Vec<PathBuf> = Vec::new();
        for path in &pending {
            if !roots.iter().any(|parent| path.starts_with(parent)) {
                roots.push(path.clone());
            }
        }
        for path in roots {
            refresh(&root, &path, &mut known, &mut batch, &stop);
        }
        // 父目录事件合并了子文件事件时，仍把内容变化通知给已打开的文档。
        for path in pending {
            if let Ok(relative) = path.strip_prefix(&root)
                && let Some(value) = known.get(&relative.to_string_lossy().into_owned())
                && !value.directory
                && !batch.upserted.iter().any(|e| e.path == value.path)
            {
                batch.upserted.push(value.clone());
            }
        }
        if !batch.upserted.is_empty() || !batch.removed.is_empty() || !batch.errors.is_empty() {
            let _ = tx.send(batch);
        }
    }
}

fn collect_event(
    event: notify::Result<Event>,
    root: &Path,
    pending: &mut BTreeSet<PathBuf>,
    errors: &mut Vec<String>,
) {
    match event {
        Ok(event) if event.need_rescan() => {
            pending.insert(root.to_owned());
        }
        Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
            for path in event.paths {
                if path == root || path.strip_prefix(root).is_ok_and(allowed) {
                    pending.insert(path);
                }
            }
        }
        Ok(_) => {}
        Err(error) => {
            let _ = error;
            errors.push("vault.scan_failed".into());
            pending.insert(root.to_owned());
        }
    }
}

fn refresh(
    root: &Path,
    path: &Path,
    known: &mut BTreeMap<String, Entry>,
    batch: &mut Batch,
    stop: &AtomicBool,
) {
    let mut found = BTreeMap::new();
    let mut queue = VecDeque::new();
    if path == root {
        queue.push_back(path.to_owned());
    } else {
        match entry(root, path) {
            Ok(Some(e)) => {
                if e.directory {
                    queue.push_back(path.to_owned());
                }
                found.insert(e.path.clone(), e);
            }
            Ok(None) => {}
            Err(error) => {
                let _ = error;
                batch.errors.push("vault.scan_failed".into());
                return;
            }
        }
    }
    while let Some(dir) = queue.pop_front() {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let before_errors = batch.errors.len();
        let list = children(root, &dir, &mut batch.errors);
        // 无法读取目录时保留已有子树，不能误报删除。
        if batch.errors.len() != before_errors {
            for (key, value) in known
                .iter()
                .filter(|(key, _)| root.join(key).starts_with(&dir))
            {
                found.insert(key.clone(), value.clone());
            }
        }
        for e in list {
            if e.directory {
                queue.push_back(root.join(&e.path));
            }
            found.insert(e.path.clone(), e);
        }
    }
    let removed: Vec<_> = known
        .keys()
        .filter(|key| root.join(key).starts_with(path) && !found.contains_key(*key))
        .cloned()
        .collect();
    for key in removed {
        known.remove(&key);
        batch.removed.push(key);
    }
    for (key, value) in found {
        if known.get(&key) != Some(&value) {
            batch.upserted.push(value.clone());
            known.insert(key, value);
        } else if root.join(&key) == path && !value.directory {
            // 内容改动也推给界面，以便刷新已经打开的只读标签。
            batch.upserted.push(value);
        }
    }
    batch.upserted.sort_by(|a, b| {
        a.parent
            .cmp(&b.parent)
            .then_with(|| b.directory.cmp(&a.directory))
            .then_with(|| compare_names(&a.name, &b.name))
    });
}
