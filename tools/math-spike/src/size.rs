// SPDX-License-Identifier: MIT OR Apache-2.0
use ratex_layout::{LayoutOptions, layout, to_display_list, validate_layout};

fn main() {
    let input = std::env::args().nth(1).unwrap_or_else(|| "x^2".into());
    let ast = ratex_parser::parse(&input).expect("parse");
    let tree = layout(&ast, &LayoutOptions::default());
    validate_layout(&tree).expect("validate");
    let display: ratex_types::display_item::DisplayList = to_display_list(&tree);
    std::hint::black_box(ratex_font::get_global_metrics(0));
    println!("{} {}", display.items.len(), display.width);
}
