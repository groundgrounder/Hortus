use fontdb::{Database, Family, ID, Query, Source, Weight};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FontInfo {
    pub family: String,
    pub postscript_name: String,
    pub weight: u16,
}

#[derive(Clone, Debug)]
pub struct MeasuredRun {
    pub text: String,
    pub width: f64,
    pub font_family: String,
}

#[derive(Clone, Debug, Default)]
pub struct Measurement {
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
    pub runs: Vec<MeasuredRun>,
}

pub trait FontMetrics {
    fn measure(&mut self, text: &str, size: f64, weight: u16) -> Measurement;
    fn used_fonts(&self) -> Vec<FontInfo>;
    fn reference_fonts_available(&self) -> bool {
        false
    }
}

/// 只用于结构、几何和稳定性测试，不参与 CLI 渲染。
#[derive(Default)]
pub struct DeterministicMetrics;

impl FontMetrics for DeterministicMetrics {
    fn measure(&mut self, text: &str, size: f64, _weight: u16) -> Measurement {
        let width = text
            .graphemes(true)
            .map(|g| {
                if g == " " {
                    0.28
                } else if g.is_ascii() {
                    0.55
                } else {
                    1.0
                }
            })
            .sum::<f64>()
            * size;
        Measurement {
            width,
            ascent: size * 0.8,
            descent: size * 0.2,
            runs: vec![MeasuredRun {
                text: text.into(),
                width,
                font_family: "Deterministic".into(),
            }],
        }
    }
    fn used_fonts(&self) -> Vec<FontInfo> {
        vec![FontInfo {
            family: "Deterministic".into(),
            postscript_name: "Deterministic".into(),
            weight: 400,
        }]
    }
}

type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;

struct LoadedFace {
    data: FontData,
    index: u32,
    family: String,
    postscript: String,
}

/// 系统字体发现 + OpenType 整形。对象可复用，整形缓存只在内存中，不写笔记目录。
pub struct SystemFonts {
    db: Database,
    faces: HashMap<ID, LoadedFace>,
    files: HashMap<PathBuf, FontData>,
    selections: HashMap<(char, u16), ID>,
    cache: HashMap<(String, u64, u16), Measurement>,
    used: BTreeMap<(String, u16), FontInfo>,
    ui: String,
    cjk: String,
    reference: bool,
}

impl SystemFonts {
    pub fn new() -> Result<Self, String> {
        let mut db = Database::new();
        db.load_system_fonts();
        let find = |names: &[&str]| {
            names
                .iter()
                .find(|name| {
                    db.faces()
                        .any(|f| f.families.iter().any(|(n, _)| n == **name))
                })
                .map(|s| s.to_string())
        };
        let ui = find(&[
            ".AppleSystemUIFont",
            "System Font",
            "SF Pro",
            "SF Pro Text",
            "Segoe UI",
            "Cantarell",
            "Noto Sans",
            "DejaVu Sans",
            "Arial",
        ])
        .or_else(|| db.faces().next().map(|f| f.families[0].0.clone()))
        .ok_or("No readable system font is available")?;
        let cjk = find(&[
            "PingFang SC",
            "Noto Sans CJK SC",
            "Noto Sans SC",
            "Microsoft YaHei",
            "Hiragino Sans GB",
        ])
        .unwrap_or_else(|| ui.clone());
        let reference =
            (ui == ".AppleSystemUIFont" || ui == "System Font" || ui.starts_with("SF Pro"))
                && cjk == "PingFang SC";
        Ok(Self {
            db,
            faces: HashMap::new(),
            files: HashMap::new(),
            selections: HashMap::new(),
            cache: HashMap::new(),
            used: BTreeMap::new(),
            ui,
            cjk,
            reference,
        })
    }

    fn load(&mut self, id: ID) -> bool {
        if self.faces.contains_key(&id) {
            return true;
        }
        let Some(info) = self.db.face(id) else {
            return false;
        };
        let index = info.index;
        let family = info.families[0].0.clone();
        let postscript = info.post_script_name.clone();
        let data: FontData = match &info.source {
            Source::Binary(data) | Source::SharedFile(_, data) => data.clone(),
            Source::File(path) => {
                if let Some(data) = self.files.get(path) {
                    data.clone()
                } else {
                    let Ok(bytes) = std::fs::read(path) else {
                        return false;
                    };
                    let data: FontData = Arc::new(bytes);
                    self.files.insert(path.clone(), data.clone());
                    data
                }
            }
        };
        if rustybuzz::Face::from_slice(data.as_ref().as_ref(), index).is_none() {
            return false;
        }
        self.faces.insert(
            id,
            LoadedFace {
                data,
                index,
                family,
                postscript,
            },
        );
        true
    }

    fn supports(&mut self, id: ID, ch: char) -> bool {
        self.load(id)
            && rustybuzz::Face::from_slice(
                self.faces[&id].data.as_ref().as_ref(),
                self.faces[&id].index,
            )
            .is_some_and(|f| f.glyph_index(ch).is_some())
    }

    fn select(&mut self, ch: char, weight: u16) -> ID {
        if let Some(id) = self.selections.get(&(ch, weight)) {
            return *id;
        }
        let primary = if ch.is_ascii() { &self.ui } else { &self.cjk };
        let mut candidates = self
            .db
            .query(&Query {
                families: &[Family::Name(primary)],
                weight: Weight(weight),
                ..Query::default()
            })
            .into_iter()
            .collect::<Vec<_>>();
        candidates.extend(self.db.query(&Query {
            families: &[Family::Name(&self.ui)],
            weight: Weight(weight),
            ..Query::default()
        }));
        let primary_id = candidates.first().copied().expect("系统字体族已验证");
        let mut selected = candidates.iter().copied().find(|id| self.supports(*id, ch));
        if selected.is_none() {
            candidates = self
                .db
                .faces()
                .filter(|f| f.weight.0 == 400)
                .map(|f| f.id)
                .collect();
            selected = candidates.into_iter().find(|id| self.supports(*id, ch));
        }
        let id = selected.unwrap_or(primary_id);
        self.load(id);
        self.selections.insert((ch, weight), id);
        id
    }
}

impl FontMetrics for SystemFonts {
    fn measure(&mut self, text: &str, size: f64, weight: u16) -> Measurement {
        let key = (text.into(), size.to_bits(), weight);
        if let Some(measured) = self.cache.get(&key) {
            return measured.clone();
        }
        let mut groups: Vec<(ID, String)> = Vec::new();
        for grapheme in text.graphemes(true) {
            let id = self.select(grapheme.chars().next().unwrap(), weight);
            if let Some((last_id, value)) = groups.last_mut().filter(|(last, _)| *last == id) {
                let _ = last_id;
                value.push_str(grapheme);
            } else {
                groups.push((id, grapheme.into()));
            }
        }
        let mut result = Measurement::default();
        for (id, text) in groups {
            let loaded = &self.faces[&id];
            let mut face = rustybuzz::Face::from_slice(loaded.data.as_ref().as_ref(), loaded.index)
                .expect("已验证的字体");
            face.set_variations(&[
                rustybuzz::Variation {
                    tag: rustybuzz::ttf_parser::Tag::from_bytes(b"wght"),
                    value: weight as f32,
                },
                rustybuzz::Variation {
                    tag: rustybuzz::ttf_parser::Tag::from_bytes(b"opsz"),
                    value: size as f32,
                },
            ]);
            let scale = size / f64::from(face.units_per_em());
            result.ascent = result.ascent.max(f64::from(face.ascender()) * scale);
            result.descent = result.descent.max(-f64::from(face.descender()) * scale);
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(&text);
            buffer.guess_segment_properties();
            let glyphs = rustybuzz::shape(&face, &[], buffer);
            let width = glyphs
                .glyph_positions()
                .iter()
                .map(|p| f64::from(p.x_advance))
                .sum::<f64>()
                * scale;
            result.width += width;
            self.used.insert(
                (loaded.postscript.clone(), weight),
                FontInfo {
                    family: loaded.family.clone(),
                    postscript_name: loaded.postscript.clone(),
                    weight,
                },
            );
            result.runs.push(MeasuredRun {
                text,
                width,
                font_family: loaded.family.clone(),
            });
        }
        self.cache.insert(key, result.clone());
        result
    }

    fn used_fonts(&self) -> Vec<FontInfo> {
        self.used.values().cloned().collect()
    }
    fn reference_fonts_available(&self) -> bool {
        self.reference
    }
}
