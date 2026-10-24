use serde::Serialize;

/// 已解析的 sRGB（含透明度），平台无需解释主题或 CSS。
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub fn oklch(l: f64, c: f64, hue: f64) -> Self {
        let a = c * hue.to_radians().cos();
        let b = c * hue.to_radians().sin();
        let ll = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
        let mm = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
        let ss = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
        fn gamma(v: f64) -> f64 {
            (if v <= 0.0031308 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            })
            .clamp(0.0, 1.0)
        }
        Self {
            r: gamma(4.0767416621 * ll - 3.3077115913 * mm + 0.2309699292 * ss),
            g: gamma(-1.2684380046 * ll + 2.6097574011 * mm - 0.3413193965 * ss),
            b: gamma(-0.0041960863 * ll - 0.7034186147 * mm + 1.707614701 * ss),
            a: 1.0,
        }
    }

    pub fn parse_css(value: &str) -> Option<Self> {
        let parsed = csscolorparser::parse(value.trim()).ok()?;
        Some(Self {
            r: parsed.r as f64,
            g: parsed.g as f64,
            b: parsed.b as f64,
            a: parsed.a as f64,
        })
    }

    pub fn hue(self) -> f64 {
        let linear = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let [r, g, b] = [linear(self.r), linear(self.g), linear(self.b)];
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        let a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
        let b = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
        b.atan2(a).to_degrees().rem_euclid(360.0).round()
    }

    pub(crate) fn svg(self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            (self.r * 255.0).round() as u8,
            (self.g * 255.0).round() as u8,
            (self.b * 255.0).round() as u8
        )
    }
}
