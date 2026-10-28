use std::{collections::BTreeSet, env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let fonts = root.join("apps/macos/Resources/Fonts/KaTeX");
    let output = env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("src/katex_cmap.rs")
    });
    let mut files: Vec<_> = fs::read_dir(&fonts)
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ttf"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 20);

    let mut generated = String::from("// Generated from the bundled KaTeX TTF cmap tables.\n");
    for file in &files {
        let name = file
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .replace("KaTeX_", "");
        let identifier = name.replace('-', "_").to_ascii_uppercase();
        let data = fs::read(file).unwrap();
        let face = ttf_parser::Face::parse(&data, 0).unwrap();
        let mut codepoints = BTreeSet::new();
        for subtable in face.tables().cmap.unwrap().subtables {
            subtable.codepoints(|codepoint| {
                if char::from_u32(codepoint)
                    .and_then(|character| face.glyph_index(character))
                    .is_some_and(|glyph| glyph.0 != 0)
                {
                    codepoints.insert(codepoint);
                }
            });
        }
        let mut ranges = Vec::new();
        for codepoint in codepoints {
            if let Some((_, end)) = ranges.last_mut().filter(|(_, end)| *end + 1 == codepoint) {
                *end = codepoint;
            } else {
                ranges.push((codepoint, codepoint));
            }
        }
        generated.push_str(&format!("static {identifier}: &[(u32, u32)] = &[\n"));
        for (start, end) in ranges {
            generated.push_str(&format!("    (0x{start:x}, 0x{end:x}),\n"));
        }
        generated.push_str("];\n");
    }
    generated.push_str(
        "\nfn ranges(font: &str) -> Option<&'static [(u32, u32)]> {\n    Some(match font {\n",
    );
    for file in &files {
        let name = file
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .replace("KaTeX_", "");
        let identifier = name.replace('-', "_").to_ascii_uppercase();
        generated.push_str(&format!("        \"{name}\" => {identifier},\n"));
    }
    generated.push_str("        _ => return None,\n    })\n}\n");
    generated.push_str(
        "\npub(super) fn contains(font: &str, codepoint: u32) -> bool {\n    ranges(font).is_some_and(|ranges| ranges.binary_search_by(|(start, end)| {\n        if codepoint < *start { std::cmp::Ordering::Greater } else if codepoint > *end { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal }\n    }).is_ok())\n}\n",
    );
    generated.push_str("\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use std::collections::BTreeSet;\n\n    #[test]\n    fn generated_ranges_match_every_bundled_font_cmap() {\n");
    for file in &files {
        let name = file
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .replace("KaTeX_", "");
        generated.push_str(&format!(
            "        assert_font_cmap(\"{name}\", include_bytes!(\"../../../apps/macos/Resources/Fonts/KaTeX/{}\"));\n",
            file.file_name().unwrap().to_string_lossy()
        ));
    }
    generated.push_str(
        "    }\n\n    fn assert_font_cmap(name: &str, data: &[u8]) {\n        let face = ttf_parser::Face::parse(data, 0).unwrap();\n        let mut actual = BTreeSet::new();\n        for subtable in face.tables().cmap.unwrap().subtables {\n            subtable.codepoints(|codepoint| {\n                if char::from_u32(codepoint).and_then(|character| face.glyph_index(character)).is_some_and(|glyph| glyph.0 != 0) { actual.insert(codepoint); }\n            });\n        }\n        let expected: BTreeSet<_> = ranges(name).unwrap().iter().flat_map(|(start, end)| *start..=*end).collect();\n        assert_eq!(actual, expected, \"{name}\");\n        assert!(!contains(name, 0xffff));\n    }\n}\n",
    );
    fs::write(output, generated).unwrap();
}
