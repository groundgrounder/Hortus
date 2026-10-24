use hortus_document::{media::*, reading::Cache};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Vault(PathBuf);
impl Vault {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hortus-attachments-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, path: &str) -> PathBuf {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, []).unwrap();
        path.canonicalize().unwrap()
    }
}
impl Drop for Vault {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn wiki_and_markdown_find_nested_unicode_and_encoded_names() {
    let vault = Vault::new();
    let file = vault.write("attachments/deep/中文 image 100%.PNG");
    let resolver = ImageResolver::default();
    for wiki in [false, true] {
        for name in [
            "中文%20image%20100%25.PNG",
            "%E4%B8%AD%E6%96%87%20IMAGE%20100%25.png",
        ] {
            assert_eq!(
                resolver.resolve(&vault.0, Path::new("notes/a.md"), name, wiki),
                ImagePath::Local(file.clone())
            );
        }
    }
    assert_eq!(resolver.stats().builds, 1);
}

#[test]
fn direct_paths_keep_root_and_note_relative_priority_without_an_index() {
    let vault = Vault::new();
    let root = vault.write("a.png");
    let same_folder = vault.write("notes/a.png");
    vault.write("attachments/a.png");
    let resolver = ImageResolver::default();
    let note = Path::new("notes/n.md");
    assert_eq!(
        resolver.resolve(&vault.0, note, "a.png", true),
        ImagePath::Local(root)
    );
    assert_eq!(
        resolver.resolve(&vault.0, note, "a.png", false),
        ImagePath::Local(same_folder)
    );
    assert_eq!(resolver.stats().builds, 0);
}

#[test]
fn duplicate_names_choose_same_folder_then_shortest_path_then_lexical_order() {
    let vault = Vault::new();
    let near = vault.write("notes/A.PNG");
    vault.write("a/a.png");
    let resolver = ImageResolver::default();
    assert_eq!(
        resolver.resolve(&vault.0, Path::new("notes/n.md"), "a.png", false),
        ImagePath::Local(near)
    );

    let short = vault.write("x/b.png");
    vault.write("long/path/b.png");
    let resolver = ImageResolver::default();
    assert_eq!(
        resolver.resolve(&vault.0, Path::new("notes/n.md"), "b.png", true),
        ImagePath::Local(short)
    );

    vault.write("z/c.png");
    let first = vault.write("a/c.png");
    let resolver = ImageResolver::default();
    for _ in 0..4 {
        assert_eq!(
            resolver.resolve(&vault.0, Path::new("notes/n.md"), "c.png", true),
            ImagePath::Local(first.clone())
        );
    }
}

#[test]
fn suffixes_match_whole_case_insensitive_components_and_skip_hidden_and_symlinks() {
    let vault = Vault::new();
    let file = vault.write("assets/img/a.png");
    vault.write("xyimg/a.png");
    for hidden in [
        ".obsidian",
        ".trash",
        ".git",
        "node_modules",
        "assets/.hidden",
    ] {
        vault.write(&format!("{hidden}/hidden.png"));
    }
    let resolver = ImageResolver::default();
    let note = Path::new("notes/n.md");
    assert_eq!(
        resolver.resolve(&vault.0, note, "IMG/A.PNG", true),
        ImagePath::Local(file)
    );
    assert_eq!(
        resolver.resolve(&vault.0, note, "hidden.png", true),
        ImagePath::Rejected("image.missing")
    );
    fs::remove_file(vault.0.join("assets/img/a.png")).unwrap();
    let resolver = ImageResolver::default();
    assert_eq!(
        resolver.resolve(&vault.0, note, "img/a.png", true),
        ImagePath::Rejected("image.missing")
    );
    #[cfg(unix)]
    {
        let outside = Vault::new();
        let target = outside.write("outside.png");
        std::os::unix::fs::symlink(&outside.0, vault.0.join("linked")).unwrap();
        std::os::unix::fs::symlink(&target, vault.0.join("symlink.png")).unwrap();
        let resolver = ImageResolver::default();
        assert_eq!(
            resolver.resolve(&vault.0, note, "outside.png", true),
            ImagePath::Rejected("image.missing")
        );
        assert_eq!(
            resolver.resolve(&vault.0, note, "symlink.png", true),
            ImagePath::Rejected("image.outside")
        );
    }
}

#[test]
fn unsafe_paths_never_fall_back_and_wiki_non_images_have_specific_reasons() {
    let vault = Vault::new();
    vault.write("attachments/a.png");
    vault.write("attachments/file.pdf");
    let resolver = ImageResolver::default();
    for target in [
        "../../a.png",
        "%2e%2e/%2e%2e/a.png",
        "file:///a.png",
        "/a.png",
    ] {
        assert_eq!(
            resolver.resolve(&vault.0, Path::new("notes/n.md"), target, true),
            ImagePath::Rejected("image.outside")
        );
    }
    assert_eq!(
        resolver.resolve(
            &vault.0,
            Path::new("n.md"),
            "https://example.invalid/a.png",
            true
        ),
        ImagePath::Rejected("image.remote")
    );
    for target in ["某篇笔记", "missing.md"] {
        assert_eq!(
            resolver.resolve(&vault.0, Path::new("n.md"), target, true),
            ImagePath::Rejected("embed.note")
        );
    }
    assert_eq!(
        resolver.resolve(&vault.0, Path::new("n.md"), "file.pdf", true),
        ImagePath::Rejected("embed.file")
    );
    assert_eq!(resolver.stats().builds, 0);
    assert_eq!(
        resolver.resolve(&vault.0, Path::new("n.md"), "file.pdf", false),
        ImagePath::Rejected("image.unsupported")
    );
}

#[test]
fn reopened_resolvers_reuse_or_rebuild_after_create_rename_delete_without_writes() {
    let vault = Vault::new();
    let note = Path::new("note.md");
    vault.write("assets/old.png");
    let resolve = |resolver: &ImageResolver, name| resolver.resolve(&vault.0, note, name, true);
    let first = ImageResolver::default();
    assert_eq!(
        resolve(&first, "new.png"),
        ImagePath::Rejected("image.missing")
    );
    let second = ImageResolver::default();
    assert_eq!(
        resolve(&second, "old.png"),
        ImagePath::Local(vault.0.join("assets/old.png"))
    );
    assert_eq!(second.stats().builds, 1);
    let created = vault.write("assets/new.png");
    let third = ImageResolver::default();
    assert_eq!(
        resolve(&third, "new.png"),
        ImagePath::Local(created.clone())
    );
    assert_eq!(third.stats().builds, 2);
    fs::rename(created, vault.0.join("assets/renamed.png")).unwrap();
    let fourth = ImageResolver::default();
    assert_eq!(
        resolve(&fourth, "new.png"),
        ImagePath::Rejected("image.missing")
    );
    assert_eq!(
        resolve(&fourth, "renamed.png"),
        ImagePath::Local(vault.0.join("assets/renamed.png"))
    );
    fs::remove_file(vault.0.join("assets/renamed.png")).unwrap();
    let fifth = ImageResolver::default();
    assert_eq!(
        resolve(&fifth, "renamed.png"),
        ImagePath::Rejected("image.missing")
    );
    let files = fs::read_dir(vault.0.join("assets")).unwrap().count();
    assert_eq!(files, 1);
    assert_eq!(fs::read_dir(&vault.0).unwrap().count(), 1);
}

#[test]
fn only_numeric_wiki_aliases_set_a_positive_width() {
    let source = "![[a.png|300]]\n\n![[a.png|300x200]]\n\n![[a.png|caption]]\n\n![[a.png|0]]\n\n![[a.png|300x0]]\n\n![[a.png|300x200x100]]\n\n![300](a.png)\n";
    let model = Cache::default().model(source, false);
    let images = model
        .blocks
        .iter()
        .flat_map(|block| &block.content.media)
        .filter_map(|media| {
            if let Kind::Image(image) = &media.kind {
                Some(image)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(images.len(), 7);
    assert_eq!(
        images.iter().map(|image| image.width).collect::<Vec<_>>(),
        [Some(300), Some(300), None, None, None, None, None]
    );
    assert_eq!(images[0].alt, "a.png");
    assert_eq!(images[2].alt, "caption");
    assert_eq!(images[6].alt, "300");
}
