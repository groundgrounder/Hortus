//! 图元轮廓与正交端口共用同一份几何；平台不再推导节点形状。
use crate::{Point, Rect, Shape};

pub(crate) fn outline(shape: Shape, r: Rect) -> Vec<Point> {
    let (x, y, w, h) = (r.x, r.y, r.width, r.height);
    let point = |a: f64, b: f64| Point {
        x: x + a * w,
        y: y + b * h,
    };
    match shape {
        Shape::Circle | Shape::DoubleCircle => ellipse(r, 0.0, std::f64::consts::TAU, 96),
        Shape::Rounded | Shape::Stadium => {
            let radius = if shape == Shape::Stadium {
                h.min(w) / 2.0
            } else {
                10.0_f64.min(h / 2.0)
            };
            let mut points = Vec::new();
            for (cx, cy, angle) in [
                (x + w - radius, y + radius, -90.0_f64),
                (x + w - radius, y + h - radius, 0.0),
                (x + radius, y + h - radius, 90.0),
                (x + radius, y + radius, 180.0),
            ] {
                for step in 0..=12 {
                    let theta = (angle + step as f64 * 7.5).to_radians();
                    points.push(Point {
                        x: cx + radius * theta.cos(),
                        y: cy + radius * theta.sin(),
                    });
                }
            }
            points
        }
        Shape::Cylinder => {
            let cap = (h * 0.18).min(10.0);
            let mut points = ellipse(
                Rect {
                    x,
                    y,
                    width: w,
                    height: cap * 2.0,
                },
                std::f64::consts::PI,
                std::f64::consts::PI,
                48,
            );
            points.extend(ellipse(
                Rect {
                    x,
                    y: y + h - cap * 2.0,
                    width: w,
                    height: cap * 2.0,
                },
                0.0,
                std::f64::consts::PI,
                48,
            ));
            points
        }
        Shape::Diamond => vec![
            point(0.5, 0.0),
            point(1.0, 0.5),
            point(0.5, 1.0),
            point(0.0, 0.5),
        ],
        Shape::Hexagon => vec![
            point(0.16, 0.0),
            point(0.84, 0.0),
            point(1.0, 0.5),
            point(0.84, 1.0),
            point(0.16, 1.0),
            point(0.0, 0.5),
        ],
        Shape::Asymmetric => vec![
            point(0.0, 0.0),
            point(1.0, 0.0),
            point(1.0, 1.0),
            point(0.0, 1.0),
            point(0.16, 0.5),
        ],
        Shape::Parallelogram => vec![
            point(0.16, 0.0),
            point(1.0, 0.0),
            point(0.84, 1.0),
            point(0.0, 1.0),
        ],
        Shape::ParallelogramReverse => vec![
            point(0.0, 0.0),
            point(0.84, 0.0),
            point(1.0, 1.0),
            point(0.16, 1.0),
        ],
        Shape::Rectangle | Shape::Subroutine => vec![
            point(0.0, 0.0),
            point(1.0, 0.0),
            point(1.0, 1.0),
            point(0.0, 1.0),
        ],
    }
}

pub(crate) fn details(shape: Shape, r: Rect) -> Vec<Vec<Point>> {
    match shape {
        Shape::Cylinder => vec![ellipse(
            Rect {
                height: (r.height * 0.36).min(20.0),
                ..r
            },
            0.0,
            std::f64::consts::PI,
            48,
        )],
        Shape::DoubleCircle => {
            let mut inner = outline(
                Shape::Circle,
                Rect {
                    x: r.x + 5.0,
                    y: r.y + 5.0,
                    width: r.width - 10.0,
                    height: r.height - 10.0,
                },
            );
            inner.push(inner[0]);
            vec![inner]
        }
        Shape::Subroutine => [r.x + 7.0, r.x + r.width - 7.0]
            .into_iter()
            .map(|x| {
                vec![
                    Point { x, y: r.y },
                    Point {
                        x,
                        y: r.y + r.height,
                    },
                ]
            })
            .collect(),
        _ => vec![],
    }
}

fn ellipse(r: Rect, start: f64, sweep: f64, steps: usize) -> Vec<Point> {
    (0..=steps)
        .map(|step| {
            let theta = start + sweep * step as f64 / steps as f64;
            Point {
                x: r.x + r.width / 2.0 * (1.0 + theta.cos()),
                y: r.y + r.height / 2.0 * (1.0 + theta.sin()),
            }
        })
        .collect()
}

// fixed 是与末段垂直的轴坐标；positive 选择轮廓的右侧或下侧。
pub(crate) fn port(shape: Shape, r: Rect, vertical: bool, fixed: f64, positive: bool) -> Point {
    let points = outline(shape, r);
    let mut values = Vec::new();
    for (a, b) in points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
    {
        let (a_fixed, b_fixed, a_value, b_value) = if vertical {
            (a.x, b.x, a.y, b.y)
        } else {
            (a.y, b.y, a.x, b.x)
        };
        if (b_fixed - a_fixed).abs() < 1e-9 {
            if (a_fixed - fixed).abs() < 1e-6 {
                values.extend([a_value, b_value]);
            }
        } else {
            let t = (fixed - a_fixed) / (b_fixed - a_fixed);
            if (-1e-6..=1.0 + 1e-6).contains(&t) {
                values.push(a_value + (b_value - a_value) * t);
            }
        }
    }
    let value = if positive {
        values.into_iter().fold(f64::NEG_INFINITY, f64::max)
    } else {
        values.into_iter().fold(f64::INFINITY, f64::min)
    };
    if vertical {
        Point { x: fixed, y: value }
    } else {
        Point { x: value, y: fixed }
    }
}
