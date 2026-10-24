//! 原生文本的 UTF-16 增量镜像；全文转换、围栏扫描只由后台调用。
use std::io;
mod attachments;
pub mod markdown;
pub mod media;
pub mod outline;
pub mod reading;

pub struct Document {
    units: Vec<u16>,
    revision: u64,
    styles: Vec<markdown::Run>,
    analyzer: markdown::Analyzer,
    styled: bool,
    style_revision: u64,
    dirty_style: Option<std::ops::Range<usize>>,
    reading: reading::Cache,
    outline: Option<(u64, bool, Vec<outline::Heading>)>,
}

impl Document {
    pub fn mermaid_ranges(text: &str) -> Vec<(u32, u64, u64)> {
        hortus_diagram::mermaid_blocks(text)
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let start = text[..block.fence_range.start].encode_utf16().count() as u64;
                let length = text[block.fence_range.clone()].encode_utf16().count() as u64;
                (index as u32 + 1, start, length)
            })
            .collect()
    }
    pub fn new(text: &str) -> Self {
        Self {
            units: text.encode_utf16().collect(),
            revision: 0,
            styles: Vec::new(),
            analyzer: markdown::Analyzer::default(),
            styled: false,
            style_revision: u64::MAX,
            dirty_style: None,
            reading: reading::Cache::default(),
            outline: None,
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn outline(&mut self, mmd: bool) -> io::Result<&[outline::Heading]> {
        if !self
            .outline
            .as_ref()
            .is_some_and(|(revision, mode, _)| *revision == self.revision && *mode == mmd)
        {
            let headings = if mmd {
                Vec::new()
            } else {
                outline::parse(&self.text()?)
            };
            self.outline = Some((self.revision, mmd, headings));
        }
        Ok(&self.outline.as_ref().unwrap().2)
    }
    pub fn reading(&mut self, mmd: bool) -> io::Result<reading::Model> {
        Ok(self.reading.model(&self.text()?, mmd))
    }
    pub fn replace(&mut self, start: usize, length: usize, replacement: &str) -> io::Result<u64> {
        let end = start
            .checked_add(length)
            .filter(|&end| end <= self.units.len())
            .ok_or_else(|| io::Error::other("document.range_invalid"))?;
        let inserted = replacement.encode_utf16().count();
        let delta = inserted as isize - length as isize;
        for run in &mut self.styles {
            if run.range.start >= end {
                run.range.start = run.range.start.saturating_add_signed(delta);
            } else if run.range.start > start {
                run.range.start = start;
            }
            if run.range.end >= end {
                run.range.end = run.range.end.saturating_add_signed(delta);
            } else if run.range.end > start {
                run.range.end = start;
            }
        }
        let dirty = start..start + inserted;
        self.dirty_style = Some(match self.dirty_style.take() {
            Some(old) => old.start.min(start)..old.end.saturating_add_signed(delta).max(dirty.end),
            None => dirty,
        });
        self.units.splice(start..end, replacement.encode_utf16());
        self.revision += 1;
        Ok(self.revision)
    }
    pub fn text(&self) -> io::Result<String> {
        String::from_utf16(&self.units).map_err(io::Error::other)
    }
    pub fn reset(&mut self, text: &str) {
        self.units = text.encode_utf16().collect();
        self.revision += 1;
        self.styles.clear();
        self.styled = false;
        self.dirty_style = None;
    }
    /// 已有样式随文本变更平移；公共前后缀保持不动，仅返回需要覆盖的连续片段。
    pub fn style_changes(&mut self, since: u64, force: bool) -> io::Result<Vec<markdown::Run>> {
        let force = force || since != self.style_revision;
        let next = self.analyzer.runs(&self.text()?);
        let mut changed_range = if force || !self.styled {
            0..self.units.len()
        } else {
            self.units.len()..0
        };
        if let Some(dirty) = &self.dirty_style {
            changed_range.start = changed_range.start.min(dirty.start);
            changed_range.end = changed_range.end.max(dirty.end.min(self.units.len()));
        }
        if self.styled && !force {
            // 先跳过完全相同的前后段，通常只有编辑段落需要逐边界比较。
            let prefix = self
                .styles
                .iter()
                .zip(&next)
                .take_while(|(old, new)| old == new)
                .count();
            let suffix = self.styles[prefix..]
                .iter()
                .rev()
                .zip(next[prefix..].iter().rev())
                .take_while(|(old, new)| old == new)
                .count();
            let limit = next
                .get(next.len() - suffix)
                .map_or(self.units.len(), |run| run.range.start);
            let mut position = prefix.checked_sub(1).map_or(0, |i| next[i].range.end);
            let (mut a, mut b) = (prefix, prefix);
            while position < limit {
                while a < self.styles.len() && self.styles[a].range.end <= position {
                    a += 1;
                }
                while b < next.len() && next[b].range.end <= position {
                    b += 1;
                }
                let old = self.styles.get(a);
                let new = next.get(b);
                let old_mask = old
                    .filter(|r| r.range.contains(&position))
                    .map_or(0, |r| r.mask);
                let new_mask = new
                    .filter(|r| r.range.contains(&position))
                    .map_or(0, |r| r.mask);
                let edge = |run: Option<&markdown::Run>| {
                    run.map_or(self.units.len(), |r| {
                        if r.range.start > position {
                            r.range.start
                        } else {
                            r.range.end
                        }
                    })
                };
                let end = edge(old).min(edge(new)).min(limit);
                if old_mask != new_mask {
                    changed_range.start = changed_range.start.min(position);
                    changed_range.end = changed_range.end.max(end);
                }
                position = end;
            }
        }
        let changed = if changed_range.start <= changed_range.end {
            // 段落属性需要覆盖完整段落，普通输入仍只触及当前行。
            while changed_range.start > 0 && self.units[changed_range.start - 1] != 10 {
                changed_range.start -= 1;
            }
            while changed_range.end < self.units.len() && self.units[changed_range.end] != 10 {
                changed_range.end += 1;
            }
            if changed_range.end < self.units.len() {
                changed_range.end += 1;
            }
            let first = next.partition_point(|run| run.range.end <= changed_range.start);
            let last = next.partition_point(|run| run.range.start < changed_range.end);
            next[first..last]
                .iter()
                .filter_map(|run| {
                    let range = run.range.start.max(changed_range.start)
                        ..run.range.end.min(changed_range.end);
                    (!range.is_empty()).then_some(markdown::Run {
                        range,
                        mask: run.mask,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        self.style_revision = self.revision;
        let previous = std::mem::replace(&mut self.styles, next);
        self.analyzer.recycle(previous, &self.styles);
        self.styled = true;
        self.dirty_style = None;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_ranges_include_surrogates_and_reject_out_of_bounds() {
        let mut document = Document::new("中😀文\n");
        document.replace(1, 2, "组合").unwrap();
        assert_eq!(document.text().unwrap(), "中组合文\n");
        assert!(document.replace(99, 0, "").is_err());
        assert_eq!(document.revision(), 1);
    }
}
