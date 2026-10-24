//! 附件按库根共享内存表；文档首次后备查找时核对目录版本，不定时轮询。
use crate::media::{ImagePath, image_extension, percent_decode, resolve_direct};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

#[derive(Clone, Copy, Debug, Default)]
pub struct AttachmentStats {
    pub files: u64,
    pub directories: u64,
    pub build_micros: u64,
    /// 保留的路径、键和容器容量估算；不包含分配器开销和图片像素。
    pub estimated_bytes: u64,
    pub builds: u64,
}

#[derive(Default)]
struct Index {
    files: BTreeMap<String, Vec<PathBuf>>,
    directories: Vec<(PathBuf, SystemTime)>,
    stats: AttachmentStats,
}

impl Index {
    fn current(&self, root: &Path) -> bool {
        self.stats.builds > 0
            && self.directories.iter().all(|(path, modified)| {
                fs::symlink_metadata(root.join(path)).is_ok_and(|metadata| {
                    metadata.is_dir()
                        && !metadata.file_type().is_symlink()
                        && metadata.modified().ok() == Some(*modified)
                })
            })
    }

    fn rebuild(&mut self, root: &Path) {
        let started = Instant::now();
        self.files.clear();
        self.directories.clear();
        let mut pending = vec![PathBuf::new()];
        let mut count = 0;
        while let Some(relative) = pending.pop() {
            let directory = root.join(&relative);
            let Ok(metadata) = fs::symlink_metadata(&directory) else {
                continue;
            };
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                continue;
            }
            // 在读目录之前记录版本，遍历中的变更会使下一次打开重新建表。
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            self.directories.push((relative.clone(), modified));
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with('.') || name == "node_modules" {
                    continue;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = relative.join(name);
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() {
                    self.files
                        .entry(name.to_lowercase())
                        .or_default()
                        .push(path);
                    count += 1;
                }
            }
        }
        let estimated_bytes = self
            .files
            .iter()
            .map(|(name, paths)| {
                name.capacity()
                    + std::mem::size_of::<(String, Vec<PathBuf>)>()
                    + paths.capacity() * std::mem::size_of::<PathBuf>()
                    + paths
                        .iter()
                        .map(|path| path.as_os_str().len())
                        .sum::<usize>()
            })
            .sum::<usize>()
            + self.directories.capacity() * std::mem::size_of::<(PathBuf, SystemTime)>()
            + self
                .directories
                .iter()
                .map(|(path, _)| path.as_os_str().len())
                .sum::<usize>();
        self.stats = AttachmentStats {
            files: count,
            directories: self.directories.len() as u64,
            build_micros: started.elapsed().as_micros() as u64,
            estimated_bytes: estimated_bytes as u64,
            builds: self.stats.builds + 1,
        };
    }
}

type SharedIndex = Arc<Mutex<Index>>;
type RootCache = VecDeque<(PathBuf, SharedIndex)>;

#[derive(Default)]
pub struct ImageResolver {
    index: Mutex<Option<(PathBuf, SharedIndex)>>,
}

impl ImageResolver {
    fn index(&self, root: &Path) -> SharedIndex {
        let mut local = self.index.lock().unwrap();
        if let Some((cached_root, index)) = &*local
            && cached_root == root
        {
            return index.clone();
        }
        static ROOTS: OnceLock<Mutex<RootCache>> = OnceLock::new();
        let index = {
            let mut cache = ROOTS
                .get_or_init(|| Mutex::new(VecDeque::new()))
                .lock()
                .unwrap();
            let existing = cache
                .iter()
                .position(|(path, _)| path == root)
                .and_then(|position| cache.remove(position));
            let index = existing.map_or_else(
                || Arc::new(Mutex::new(Index::default())),
                |(_, index)| index,
            );
            cache.push_back((root.into(), index.clone()));
            // 不无限保留曾经打开过的库；仍打开的文档持有自己的 Arc。
            while cache.len() > 8 {
                cache.pop_front();
            }
            index
        };
        {
            let mut cached = index.lock().unwrap();
            if !cached.current(root) {
                cached.rebuild(root);
            }
        }
        *local = Some((root.into(), index.clone()));
        index
    }

    pub fn stats(&self) -> AttachmentStats {
        self.index
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, index)| index.lock().unwrap().stats)
            .unwrap_or_default()
    }

    pub fn resolve(&self, root: &Path, note: &Path, target: &str, wiki: bool) -> ImagePath {
        let direct = resolve_direct(root, note, target, wiki);
        if !matches!(
            direct,
            ImagePath::Rejected("image.missing" | "image.unsupported" | "image.not_file")
        ) {
            return direct;
        }
        let Some(decoded) = percent_decode(target) else {
            return direct;
        };
        let extension = Path::new(&decoded)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_lowercase();
        if wiki && !image_extension(&extension) {
            return ImagePath::Rejected(
                if matches!(extension.as_str(), "" | "md" | "markdown" | "mmd") {
                    "embed.note"
                } else {
                    "embed.file"
                },
            );
        }
        if direct != ImagePath::Rejected("image.missing") {
            return direct;
        }
        let Ok(root) = root.canonicalize() else {
            return direct;
        };
        let mut suffix = Vec::new();
        for component in Path::new(&decoded).components() {
            match component {
                Component::Normal(name) => suffix.push(name.to_string_lossy().to_lowercase()),
                Component::CurDir => {}
                _ => return direct,
            }
        }
        let Some(name) = suffix.last() else {
            return direct;
        };
        let shared = self.index(&root);
        let index = shared.lock().unwrap();
        let Some(paths) = index.files.get(name) else {
            return direct;
        };
        let mut candidates = paths
            .iter()
            .filter(|path| {
                let components = path
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
                    .collect::<Vec<_>>();
                components.ends_with(&suffix)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| {
            let same_folder = |path: &Path| path.parent() == note.parent();
            same_folder(b)
                .cmp(&same_folder(a))
                .then_with(|| a.as_os_str().len().cmp(&b.as_os_str().len()))
                .then_with(|| a.cmp(b))
        });
        for candidate in candidates {
            // 再次规范化并检查格式，防止建表后被替换成库外符号链接。
            // 表内是实际文件名，只有链接目标才需要百分号解码。
            let encoded = candidate.to_string_lossy().replace('%', "%25");
            let resolved = resolve_direct(&root, Path::new(""), &encoded, true);
            if resolved != ImagePath::Rejected("image.missing") {
                return resolved;
            }
        }
        direct
    }
}
