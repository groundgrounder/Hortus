//! 布局与路由共用的正交线段/矩形相交判断。

use crate::{Point, Rect};

/// 水平或竖直短段是否穿过矩形内部；端点贴边不算穿过。
pub(crate) fn segment_intersects(a: Point, b: Point, r: Rect) -> bool {
    if (a.x - b.x).abs() < 0.001 {
        a.x > r.x && a.x < r.x + r.width && a.y.min(b.y) < r.y + r.height && a.y.max(b.y) > r.y
    } else {
        a.y > r.y && a.y < r.y + r.height && a.x.min(b.x) < r.x + r.width && a.x.max(b.x) > r.x
    }
}
