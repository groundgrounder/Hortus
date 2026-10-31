//! 公开符号生成器；大批量检查显式运行，统计输出可供不同提交对照。
use hortus_document::reading::{Cache, Model};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

struct Random(u64);
impl Random {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as usize
    }
    fn source(&mut self) -> String {
        let symbols = [
            ">", "> [!", "[!", "]", "%%", "<!--", "-->", "\n", "\r\n", "\r", " ", "\t", "$", "$$",
            "[^1]", "[[", "]]", "|", "- ", "1) ", "😀", "中文",
        ];
        let count = 1 + self.next() % 30;
        (0..count)
            .map(|_| symbols[self.next() % symbols.len()])
            .collect()
    }
}

fn equivalent(a: &Model, b: &Model) -> bool {
    a.blocks.len() == b.blocks.len()
        && a.blocks.iter().zip(&b.blocks).all(|(a, b)| {
            a.source == b.source && format!("{:?}", a.content) == format!("{:?}", b.content)
        })
}

#[test]
#[ignore = "百万次完整和增量构建；以 release 显式运行"]
fn million_full_and_incremental_builds_do_not_panic() {
    std::panic::set_hook(Box::new(|_| {}));
    let seed = 0x123c011_u64;
    let mut random = Random(seed);
    let mut panics = [0; 2];
    let start = Instant::now();
    for step in 0..1_000_000 {
        let before = random.source();
        let after = random.source();
        if catch_unwind(AssertUnwindSafe(|| Cache::default().model(&after, false))).is_err() {
            panics[0] += 1;
        }
        if catch_unwind(AssertUnwindSafe(|| {
            let mut cache = Cache::default();
            cache.model(&before, false);
            cache.model(&after, false);
        }))
        .is_err()
        {
            panics[1] += 1;
        }
        if (step + 1) % 250_000 == 0 {
            eprintln!("completed {}", step + 1);
        }
    }
    println!(
        "{{\"seed\":{seed},\"steps_per_mode\":1000000,\"full_panics\":{},\"incremental_panics\":{},\"seconds\":{}}}",
        panics[0],
        panics[1],
        start.elapsed().as_secs_f64()
    );
    assert_eq!(panics, [0, 0]);
}

#[test]
#[ignore = "固定四万步编辑，用相同测试源码对照基线"]
fn forty_thousand_incremental_edits_audit() {
    std::panic::set_hook(Box::new(|_| {}));
    let seed = 0x123ed17_u64;
    let mut random = Random(seed);
    let tokens = [
        "\n", "\r\n", "\r", "> ", "- ", "1) ", "# ", "---", "$$", "$", "%%", "<!--", "-->", "[^1]",
        "[[", "]]", "|", " ", "\t", "😀", "中文",
    ];
    let mut text = "# Heading\n\n> quote\n\n- item\n  $$x$$\n\nend\n".to_owned();
    let mut cache = Cache::default();
    cache.model(&text, false);
    let mut mismatches = Vec::new();
    let mut panics = Vec::new();
    let start = Instant::now();
    for step in 0..40_000 {
        let boundaries: Vec<_> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain([text.len()])
            .collect();
        let a = random.next() % boundaries.len();
        let b = (a + random.next() % 6).min(boundaries.len() - 1);
        let token = tokens[random.next() % tokens.len()];
        text.replace_range(boundaries[a]..boundaries[b], token);
        if text.len() > 2048 {
            text = "# Heading\n\n> quote\n\n- item\n  $$x$$\n\nend\n".into();
        }
        match catch_unwind(AssertUnwindSafe(|| {
            let a = cache.model(&text, false);
            let b = Cache::default().model(&text, false);
            equivalent(&a, &b)
        })) {
            Ok(false) => mismatches.push(step),
            Err(_) => {
                panics.push(step);
                cache = Cache::default();
            }
            _ => {}
        }
    }
    println!(
        "{{\"seed\":{seed},\"steps\":40000,\"mismatch_count\":{},\"mismatch_steps\":{:?},\"panic_steps\":{:?},\"seconds\":{}}}",
        mismatches.len(),
        mismatches,
        panics,
        start.elapsed().as_secs_f64()
    );
    assert!(panics.is_empty());
}
