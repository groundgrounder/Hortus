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
        fs::read_to_string(resolved)
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
