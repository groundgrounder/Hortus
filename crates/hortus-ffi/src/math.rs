//! 公式一次传整张场景；版本和载荷长度属于独立于排版库的协议。
use crate::{CoreError, DocumentSession};
use hortus_math::{Engine, Error, GlyphMetrics, Style};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum MathStyle {
    Display,
    Text,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct MathBlock {
    pub block_id: u64,
    pub source_start: u64,
    pub source_length: u64,
    pub display_start: u64,
    pub display_length: u64,
    pub latex: String,
    pub style: MathStyle,
    pub block: bool,
}
#[derive(uniffi::Record)]
pub struct MathGlyphMetrics {
    pub codepoint: u32,
    pub width: f64,
    pub height: f64,
    pub depth: f64,
}
#[derive(Debug, uniffi::Error)]
pub enum MathFailure {
    Parse { message: String },
    ResourceLimit { message: String },
    Panic { message: String },
    InvalidLayout { message: String },
}
impl std::fmt::Display for MathFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { message }
            | Self::ResourceLimit { message }
            | Self::Panic { message }
            | Self::InvalidLayout { message } => f.write_str(message),
        }
    }
}
impl std::error::Error for MathFailure {}
impl From<Error> for MathFailure {
    fn from(error: Error) -> Self {
        match error {
            Error::Parse(message) => Self::Parse { message },
            Error::ResourceLimit(message) => Self::ResourceLimit { message },
            Error::Panic(message) => Self::Panic { message },
            Error::InvalidLayout(message) => Self::InvalidLayout { message },
        }
    }
}
fn guarded<T>(work: impl FnOnce() -> Result<T, MathFailure>) -> Result<T, MathFailure> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| {
        Err(MathFailure::Panic {
            message: "Formula engine panicked".into(),
        })
    })
}
#[derive(uniffi::Object, Default)]
pub struct MathEngine {
    engine: Mutex<Engine>,
}
#[uniffi::export]
impl MathEngine {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn set_metrics(&self, metrics: Vec<MathGlyphMetrics>) -> Result<(), MathFailure> {
        guarded(|| {
            let values = metrics
                .into_iter()
                .map(|m| {
                    Ok((
                        char::from_u32(m.codepoint).ok_or_else(|| MathFailure::InvalidLayout {
                            message: "Invalid metrics codepoint".into(),
                        })?,
                        GlyphMetrics {
                            width: m.width,
                            height: m.height,
                            depth: m.depth,
                        },
                    ))
                })
                .collect::<Result<Vec<_>, MathFailure>>()?;
            self.engine
                .lock()
                .map_err(|_| MathFailure::Panic {
                    message: "Formula engine lock failed".into(),
                })?
                .set_metrics(values)
                .map_err(Into::into)
        })
    }
    pub fn render(&self, source: String, style: MathStyle) -> Result<Vec<u8>, MathFailure> {
        guarded(|| {
            let scene = self
                .engine
                .lock()
                .map_err(|_| MathFailure::Panic {
                    message: "Formula engine lock failed".into(),
                })?
                .render(
                    &source,
                    match style {
                        MathStyle::Display => Style::Display,
                        MathStyle::Text => Style::Text,
                    },
                )?;
            let payload =
                serde_json::to_vec(scene.as_ref()).map_err(|e| MathFailure::InvalidLayout {
                    message: e.to_string(),
                })?;
            let length = u32::try_from(payload.len()).map_err(|_| MathFailure::ResourceLimit {
                message: "Formula scene is too large".into(),
            })?;
            let mut bytes = Vec::with_capacity(8 + payload.len());
            bytes.extend_from_slice(&1u32.to_le_bytes());
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(&payload);
            Ok(bytes)
        })
    }
}

#[uniffi::export]
impl DocumentSession {
    pub fn math_blocks(&self) -> Result<Vec<MathBlock>, CoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoreError::failure("math.lock"))?;
        let revision = state.document.revision();
        if state
            .reading_snapshot
            .as_ref()
            .is_none_or(|(cached, _)| *cached != revision)
        {
            let mmd = state.relative.to_lowercase().ends_with(".mmd");
            let model = state.document.reading(mmd)?;
            state.reading_snapshot = Some((revision, model));
        }
        let mut result = Vec::new();
        let mut display = 0;
        for block in &state.reading_snapshot.as_ref().unwrap().1.blocks {
            for item in &block.content.math {
                result.push(MathBlock {
                    block_id: block.id,
                    source_start: (block.source.start + item.source.start) as u64,
                    source_length: item.source.len() as u64,
                    display_start: (display + item.display.start) as u64,
                    display_length: item.display.len() as u64,
                    latex: item.latex.clone(),
                    style: match item.style {
                        hortus_document::math::Style::Display => MathStyle::Display,
                        hortus_document::math::Style::Text => MathStyle::Text,
                    },
                    block: item.block,
                });
            }
            display += block.content.display_len();
        }
        Ok(result)
    }
}
