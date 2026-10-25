// SPDX-License-Identifier: MIT OR Apache-2.0
use pulldown_cmark::{Event, Options, Parser};
use ratex_layout::{
    FallbackGlyphMetrics, LayoutOptions, layout, layout_with_fallback_metrics, to_display_list,
    validate_layout,
};
use ratex_types::{
    display_item::{DisplayItem, DisplayList},
    math_style::MathStyle,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap, fs, hint::black_box, panic::catch_unwind, path::Path, sync::Arc,
    time::Instant,
};

fn style(s: &str) -> MathStyle {
    if s == "text" {
        MathStyle::Text
    } else {
        MathStyle::Display
    }
}

fn display(input: &str, mode: MathStyle) -> Result<DisplayList, (bool, String)> {
    let ast = ratex_parser::parse(input).map_err(|e| (false, e.to_string()))?;
    let tree = layout(&ast, &LayoutOptions::default().with_style(mode));
    validate_layout(&tree).map_err(|e| (true, e.to_string()))?;
    Ok(to_display_list(&tree))
}

fn measured(input: &str, mode: MathStyle) -> Value {
    let start = Instant::now();
    let result = catch_unwind(|| display(input, mode));
    let ns = start.elapsed().as_nanos() as u64;
    match result {
        Ok(Ok(dl)) => {
            let mut mapped = HashMap::<String, u32>::new();
            for item in &dl.items {
                if let DisplayItem::GlyphPath {
                    font, char_code, ..
                } = item
                    && let Some(id) = ratex_font::FontId::parse(font)
                {
                    mapped.insert(
                        format!("{font}:{char_code}"),
                        ratex_font::katex_ttf_glyph_char(id, *char_code) as u32,
                    );
                }
            }
            json!({"parse_ok":true,"validate_ok":true,"panic":false,"items":dl.items.len(),"width":dl.width,"height":dl.height,"depth":dl.depth,"elapsed_ns":ns,"display":dl,"ttf_char_codes":mapped,"error":null})
        }
        Ok(Err((parsed, error))) => {
            json!({"parse_ok":parsed,"validate_ok":false,"panic":false,"elapsed_ns":ns,"items":null,"width":null,"height":null,"depth":null,"error":error})
        }
        Err(e) => {
            let error = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "non-string panic".into());
            json!({"parse_ok":null,"validate_ok":false,"panic":true,"elapsed_ns":ns,"items":null,"width":null,"height":null,"depth":null,"error":error})
        }
    }
}

fn samples(path: &str) -> Vec<(String, String)> {
    let mut group = String::new();
    fs::read_to_string(path)
        .expect("samples")
        .lines()
        .filter_map(|line| {
            if let Some(header) = line.strip_prefix("# ") {
                group = header.into();
                None
            } else {
                Some((group.clone(), line.into()))
            }
        })
        .collect()
}

fn stats(ns: &mut [u64]) -> Value {
    ns.sort_unstable();
    let n = ns.len();
    json!({"count":n,"median_ns":ns[n/2],"p95_ns":ns[(n*95).div_ceil(100)-1],"max_ns":ns[n-1]})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("") {
        "once" => println!("{}", measured(&args[3], style(&args[2]))),
        "samples" => {
            let cases = samples(&args[2]);
            let mut output = String::new();
            for (i, (group, input)) in cases.iter().enumerate() {
                for mode in ["display", "text"] {
                    let mut row = measured(input, style(mode));
                    row["id"] = json!(i+1); row["group"] = json!(group);
                    row["input"] = json!(input); row["style"] = json!(mode);
                    output.push_str(&row.to_string()); output.push('\n');
                }
            }
            fs::write(&args[3], output)?;
        }
        "bench" => {
            let cases = samples(&args[2]);
            let mut report = json!({"rounds":50,"unit":"ns","timer":"Instant; parse + layout + validate + to_display_list; excludes JSON and IO"});
            // No JSON work or IO in a timed region. Include failures separately.
            for mode in ["display", "text"] {
                let mut all = Vec::new(); let mut success = Vec::new(); let mut failures = Vec::new();
                let mut per_case = vec![Vec::new(); cases.len()];
                for _ in 0..50 {
                    for (i, (_, input)) in cases.iter().enumerate() {
                        let start = Instant::now();
                        let result = catch_unwind(|| display(black_box(input), style(mode)));
                        let ns = start.elapsed().as_nanos() as u64;
                        let ok = matches!(&result, Ok(Ok(_)));
                        let _ = black_box(result);
                        all.push(ns); per_case[i].push(ns);
                        if ok { success.push(ns); } else { failures.push(ns); }
                    }
                }
                report[mode] = json!({"all":stats(&mut all),"success":stats(&mut success),"failures":stats(&mut failures),"per_sample":per_case.iter_mut().enumerate().map(|(i,v)|json!({"id":i+1,"stats":stats(v)})).collect::<Vec<_>>()});
            }
            let inline = ["x^2", "\\frac{a}{b}", "\\sum_{i=1}^n i", "\\alpha+\\beta", "\\sqrt{x+1}", "\\mathbf{v}", "\\text{速度}=v", "a_i", "\\sin x", "\\mathbb{R}"];
            let blocks = ["\\int_0^1 x^2\\,dx", "\\begin{pmatrix}1&0\\\\0&1\\end{pmatrix}", "\\sum_{k=0}^\\infty \\frac{x^k}{k!}", "f(x)=\\begin{cases}x^2&x>0\\\\0&x\\le0\\end{cases}", "\\begin{aligned}a&=b+c\\\\&=d\\end{aligned}"];
            let mut note = String::new();
            for i in 0..500 { note.push_str(&format!("第 {i} 项 ${}$，继续正文。\n\n", inline[i%inline.len()])); }
            for i in 0..50 { note.push_str(&format!("$$\n{}\n$$\n\n", blocks[i%blocks.len()])); }
            let mut layout_times = Vec::new(); let mut document_times = Vec::new();
            let mut items = 0;
            for _ in 0..50 {
                let start = Instant::now();
                let mut count = 0;
                for i in 0..500 { count += display(black_box(inline[i%inline.len()]), MathStyle::Text).unwrap().items.len(); }
                for i in 0..50 { count += display(black_box(blocks[i%blocks.len()]), MathStyle::Display).unwrap().items.len(); }
                layout_times.push(start.elapsed().as_nanos() as u64); items = black_box(count);
                let start = Instant::now();
                let mut math_count = 0;
                for event in Parser::new_ext(black_box(&note), Options::ENABLE_MATH) {
                    match event {
                        Event::InlineMath(s) => { black_box(display(&s, MathStyle::Text).unwrap()); math_count+=1; }
                        Event::DisplayMath(s) => { black_box(display(&s, MathStyle::Display).unwrap()); math_count+=1; }
                        _ => {}
                    }
                }
                document_times.push(start.elapsed().as_nanos() as u64); assert_eq!(math_count, 550);
            }
            report["note"] = json!({"inline":500,"block":50,"items":items,"cache":false,"layout_only":stats(&mut layout_times),"markdown_plus_layout":stats(&mut document_times)});
            fs::write(Path::new(&args[3]).with_file_name("performance-note.md"), note)?;
            fs::write(&args[3], serde_json::to_string_pretty(&report)?)?;
        }
        "markdown" => {
            let source = fs::read_to_string(&args[2])?;
            let rows: Vec<Value> = Parser::new_ext(&source, Options::ENABLE_MATH | Options::ENABLE_TABLES | Options::ENABLE_GFM).into_offset_iter().map(|(event, range)| {
                let (kind, text) = match &event { Event::InlineMath(s)=>("inline_math",Some(s.as_ref())), Event::DisplayMath(s)=>("display_math",Some(s.as_ref())), Event::Text(s)=>("text",Some(s.as_ref())), Event::Code(s)=>("code",Some(s.as_ref())), _=>("other",None) };
                json!({"event":format!("{event:?}"),"kind":kind,"text":text,"start":range.start,"end":range.end,"source":&source[range]})
            }).collect();
            fs::write(&args[3], serde_json::to_string_pretty(&rows)?)?;
        }
        "fallback" => {
            let raw: Value = serde_json::from_str(&fs::read_to_string(&args[2])?)?;
            let map = raw.as_object().unwrap().iter().map(|(ch,v)| (ch.chars().next().unwrap(), FallbackGlyphMetrics { width:v["width"].as_f64().unwrap(),height:v["height"].as_f64().unwrap(),depth:v["depth"].as_f64().unwrap() })).collect();
            let ast = ratex_parser::parse(&args[3])?;
            let tree = layout_with_fallback_metrics(&ast, &LayoutOptions::default(), Arc::new(map));
            validate_layout(&tree)?;
            let dl = to_display_list(&tree);
            println!("{}", json!({"display":dl,"input":args[3],"style":"display","fallback":"host metrics"}));
        }
        _ => return Err("usage: math-spike once <text|display> <latex> | samples <txt> <jsonl> | bench <txt> <json> | markdown <md> <json> | fallback <metrics.json> <latex>".into()),
    }
    Ok(())
}
