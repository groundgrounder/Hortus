use hortus_diagram::{DeterministicMetrics, FontMetrics};
use hortus_document::{media::*, reading::Cache};
use std::path::Path;

const TABLE: &str = "| 名称 | Description | 数量 |\n| :--- | :---: | ---: |\n| **中文** | *italic* and `code` | 23 |\n| A\\|B | ~~removed~~ ==marked== [[Target\\|Alias]] | 8 |\n";

#[test]
fn table_cells_share_body_styles_and_keep_escaped_pipes() {
    let (_, table) = Table::parse(TABLE).unwrap();
    assert_eq!(table.alignments, [Align::Left, Align::Center, Align::Right]);
    assert_eq!(table.rows.len(), 3);
    assert_eq!(table.rows[1][0].text.trim(), "中文");
    assert!(table.rows[1][0].runs.iter().any(|run| run.mask & 2 != 0));
    for mask in [4, 16] {
        assert!(table.rows[1][1].runs.iter().any(|run| run.mask & mask != 0));
    }
    assert_eq!(table.rows[2][0].text.trim(), "A|B");
    for mask in [8, 1024, 128] {
        assert!(table.rows[2][1].runs.iter().any(|run| run.mask & mask != 0));
    }
    assert!(table.rows[2][1].text.contains("Alias"));
}

#[test]
fn widths_keep_the_floor_and_compress_the_widest_columns_first() {
    let source = format!(
        "| Small | Description | 中文 |\n|---|---|---|\n| 1 | {} | {} |\n",
        "long text ".repeat(15),
        "这是中文".repeat(8)
    );
    let (_, table) = Table::parse(&source).unwrap();
    let natural = table.layout(5000.0, 15.0, &mut DeterministicMetrics);
    for available in [380.0, 700.0, 1100.0] {
        let layout = table.layout(available, 15.0, &mut DeterministicMetrics);
        assert!(!layout.scrolls);
        assert!(layout.width <= available + 0.001);
        assert!(layout.widths.iter().all(|width| *width >= layout.minimum));
        assert_eq!(layout.widths[0], natural.widths[0]);
    }
    let narrow = table.layout(100.0, 15.0, &mut DeterministicMetrics);
    assert!(narrow.scrolls);
    assert!(narrow.widths.iter().all(|width| *width >= narrow.minimum));
    assert_eq!(narrow.widths, narrow.floors);
}

#[test]
fn columns_never_split_a_word_and_cells_drop_padding_spaces() {
    // 10 列长英文：压缩时每列仍放得下最长的词，放不下就整表横向滚动
    let source = format!(
        "| {} |\n|{}\n| {} |\n",
        (1..=10)
            .map(|i| format!("Column {i}"))
            .collect::<Vec<_>>()
            .join(" | "),
        "---|".repeat(10),
        ["A long description wraps within its column"; 10].join(" | ")
    );
    let (_, table) = Table::parse(&source).unwrap();
    assert!(
        table
            .rows
            .iter()
            .flatten()
            .all(|cell| cell.text == cell.text.trim())
    );
    let word = DeterministicMetrics.measure("description", 15.0, 400).width + 20.0;
    for available in [380.0, 700.0, 1100.0] {
        let layout = table.layout(available, 15.0, &mut DeterministicMetrics);
        assert!(layout.widths.iter().all(|width| *width >= word - 0.001));
        assert_eq!(layout.scrolls, layout.width > available + 0.001);
    }
    // 超长的词（网址）按 12 个汉字封顶，不把一列撑得很宽
    let (_, url) = Table::parse(&format!(
        "| a |\n|---|\n| https://example.invalid/{} |\n",
        "x".repeat(200)
    ))
    .unwrap();
    let layout = url.layout(5000.0, 15.0, &mut DeterministicMetrics);
    let cap = DeterministicMetrics
        .measure(&"汉".repeat(12), 15.0, 400)
        .width
        + 20.0;
    assert!(layout.floors[0] <= cap + 0.001);
}

#[test]
fn explicit_alignment_keeps_unicode_boundaries_alignment_and_line_endings() {
    let source = format!("前言😀\r\n\r\n{}\r\n后文", TABLE.replace('\n', "\r\n"));
    let caret = source[..source.find("中文").unwrap()]
        .encode_utf16()
        .count();
    let patch = align_table(&source, caret).unwrap();
    let units: Vec<_> = source.encode_utf16().collect();
    assert!(String::from_utf16_lossy(&units[..patch.source.start]).starts_with("前言😀"));
    assert!(String::from_utf16_lossy(&units[patch.source.end..]).contains("后文"));
    let boundaries: Vec<_> = patch
        .replacement
        .lines()
        .map(|line| {
            let mut escaped = false;
            line.char_indices()
                .filter_map(|(i, c)| {
                    let pipe = c == '|' && !escaped;
                    escaped = c == '\\' && !escaped;
                    pipe.then(|| source_width(&line[..i]))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(boundaries.iter().all(|row| row == &boundaries[0]));
    assert!(patch.replacement.contains("\r\n"));
    let mut aligned = String::from_utf16_lossy(&units[..patch.source.start]);
    aligned.push_str(&patch.replacement);
    aligned.push_str(&String::from_utf16_lossy(&units[patch.source.end..]));
    assert!(align_table(&aligned, caret).is_none());
    assert_eq!(
        Table::parse(&patch.replacement).unwrap().1.alignments,
        [Align::Left, Align::Center, Align::Right]
    );
    let indented = TABLE
        .lines()
        .map(|line| format!("  {line}\n"))
        .collect::<String>();
    let patch = align_table(&indented, 4).unwrap();
    let units: Vec<_> = indented.encode_utf16().collect();
    let aligned = String::from_utf16_lossy(&units[..patch.source.start])
        + &patch.replacement
        + &String::from_utf16_lossy(&units[patch.source.end..]);
    assert!(aligned.lines().all(|line| line.starts_with("  |")));
    assert!(align_table(&aligned, 4).is_none());
}

#[test]
fn image_syntax_has_exact_utf16_source_ranges_and_comments_leave_no_blank_line() {
    let source = "前文😀\n\n![说明](images/a.png)\n\n![[图片.png|别名]]\n\n![远程](https://example.invalid/a.png)\n\n%%注释%%\n\n```text\n![literal](a.png)\n```\n";
    let model = Cache::default().model(source, false);
    let units: Vec<_> = source.encode_utf16().collect();
    let mut images = Vec::new();
    for block in &model.blocks {
        for media in &block.content.media {
            if let Kind::Image(image) = &media.kind {
                let range =
                    block.source.start + media.source.start..block.source.start + media.source.end;
                let raw = String::from_utf16_lossy(&units[range]);
                assert!(raw.starts_with("!["));
                assert!(raw.ends_with(')') || raw.ends_with("]]"));
                assert_eq!(
                    block.content.text.encode_utf16().nth(media.display.start),
                    Some(0xfffc)
                );
                images.push(image);
            }
        }
    }
    assert_eq!(images.len(), 3);
    assert_eq!(images[1].target, "图片.png");
    assert_eq!(images[1].alt, "别名");
    assert!(images[1].wiki);
    let with_comment: String = Cache::default()
        .model("%%comment%%\n\n```text\ncode\n```\n", false)
        .blocks
        .iter()
        .map(|b| b.content.text.as_str())
        .collect();
    let without: String = Cache::default()
        .model("```text\ncode\n```\n", false)
        .blocks
        .iter()
        .map(|b| b.content.text.as_str())
        .collect();
    assert_eq!(with_comment, without);
}

#[test]
fn paths_stay_inside_the_vault_including_missing_symlink_targets() {
    let base = std::env::temp_dir().join(format!("hortus-media-{}", std::process::id()));
    let root = base.join("vault");
    std::fs::create_dir_all(root.join("notes/images")).unwrap();
    std::fs::write(root.join("notes/images/a b.png"), []).unwrap();
    std::fs::write(root.join("wiki.png"), []).unwrap();
    std::fs::write(base.join("outside.png"), []).unwrap();
    let note = Path::new("notes/note.md");
    assert_eq!(
        resolve_image(&root, note, "images/a%20b.png", false),
        ImagePath::Local(root.join("notes/images/a b.png").canonicalize().unwrap())
    );
    assert_eq!(
        resolve_image(&root, note, "wiki.png", true),
        ImagePath::Local(root.join("wiki.png").canonicalize().unwrap())
    );
    for target in [
        "../../outside.png",
        "/tmp/outside.png",
        "file:///tmp/outside.png",
    ] {
        assert_eq!(
            resolve_image(&root, note, target, false),
            ImagePath::Rejected("image.outside")
        );
    }
    assert_eq!(
        resolve_image(&root, note, "missing.png", false),
        ImagePath::Rejected("image.missing")
    );
    assert_eq!(
        resolve_image(&root, note, "https://example.invalid/a.png", false),
        ImagePath::Rejected("image.remote")
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&base, root.join("escape")).unwrap();
        for target in ["escape/outside.png", "escape/missing.png"] {
            assert_eq!(
                resolve_image(&root, note, target, true),
                ImagePath::Rejected("image.outside")
            );
        }
    }
    std::fs::remove_dir_all(base).unwrap();
}
