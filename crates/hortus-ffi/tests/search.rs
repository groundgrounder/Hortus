use hortus_ffi::{SearchCancellation, WorkspaceCore};
use std::fs;

fn utf16_slice(text: &str, start: u32, length: u32) -> String {
    let units: Vec<u16> = text
        .encode_utf16()
        .skip(start as usize)
        .take(length as usize)
        .collect();
    String::from_utf16(&units).unwrap()
}

#[test]
fn search_offsets_and_titles_follow_the_document_text_projection() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let vault = root.join("vault");
    fs::create_dir(&vault).unwrap();
    let core = WorkspaceCore::in_directories(
        root.join("config").to_string_lossy().into(),
        root.join("cache").to_string_lossy().into(),
    )
    .unwrap();

    let source = "# 标题😀\n第一行\n中文😀 prefix TARGET suffix\n";
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("lf.md", source.as_bytes().to_vec()),
        ("crlf.md", source.replace('\n', "\r\n").into_bytes()),
        (
            "bom-crlf.md",
            format!("\u{feff}{}", source.replace('\n', "\r\n")).into_bytes(),
        ),
        (
            "mixed.md",
            "# 标题😀\r\n第一行\n中文😀 prefix TARGET suffix\r\n"
                .as_bytes()
                .to_vec(),
        ),
        // 无效 UTF-8 与编辑器一样以替换符继续索引，标题仍可提取。
        (
            "invalid-utf8.md",
            b"# \xe6\xa0\x87\xe9\xa2\x98\xf0\x9f\x98\x80\nfirst\n\xff TARGET suffix\n".to_vec(),
        ),
    ];
    for (name, bytes) in &cases {
        fs::write(vault.join(name), bytes).unwrap();
    }

    let root_path = vault.to_string_lossy().into_owned();
    let handle = core.open_vault(root_path.clone()).unwrap();
    let paths: Vec<String> = cases.iter().map(|(name, _)| (*name).to_owned()).collect();
    handle.update_search_index(paths, vec![]);

    let results = handle.search("TARGET".into(), 5, SearchCancellation::new());
    let quick_open = handle.quick_open("标题".into(), 50);
    assert_eq!(results.len(), cases.len());
    for result in &results {
        let session = core
            .open_document(root_path.clone(), result.path.clone())
            .unwrap();
        let text = session.snapshot().unwrap().text;
        assert_eq!(result.title, "标题😀", "{}", result.path);
        assert_eq!(result.count, 1, "{}", result.path);
        let hit = &result.matches[0];
        assert_eq!(
            utf16_slice(&text, hit.start, hit.length),
            "TARGET",
            "{}",
            result.path
        );
        assert_eq!(
            utf16_slice(
                &text,
                hit.snippet_start,
                hit.snippet.encode_utf16().count() as u32,
            ),
            hit.snippet,
            "{}",
            result.path
        );
        assert_eq!(
            session.outline().unwrap().headings[0].title,
            result.title,
            "{}",
            result.path
        );
        let quick = quick_open
            .iter()
            .find(|hit| hit.path == result.path)
            .unwrap();
        assert_eq!(quick.title, result.title, "{}", result.path);
    }
    handle.close();
}
