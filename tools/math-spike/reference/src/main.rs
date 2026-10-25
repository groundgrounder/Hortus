// SPDX-License-Identifier: MIT OR Apache-2.0
use ratex_render::{RenderOptions, render_to_png};
use ratex_types::{color::Color, display_item::DisplayList};
use serde_json::Value;
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let mut row: Value = serde_json::from_slice(&fs::read(&args[2])?)?;
    let dark = args[4] == "dark";
    if dark {
        for item in row["display"]["items"].as_array_mut().unwrap() {
            let c = &mut item["color"];
            if c["r"] == 0.0 && c["g"] == 0.0 && c["b"] == 0.0 {
                c["r"] = 0.90.into();
                c["g"] = 0.91.into();
                c["b"] = 0.93.into();
            }
        }
    }
    let dl: DisplayList = serde_json::from_value(row["display"].clone())?;
    let png = render_to_png(
        &dl,
        &RenderOptions {
            font_size: 15.0,
            padding: 16.0,
            device_pixel_ratio: 3.0,
            background_color: if dark {
                Color::rgb(0.09, 0.09, 0.09)
            } else {
                Color::WHITE
            },
            font_dir: args[1].clone(),
        },
    )
    .map_err(std::io::Error::other)?;
    fs::write(&args[3], png)?;
    Ok(())
}
