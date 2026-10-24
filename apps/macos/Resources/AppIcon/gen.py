# Hortus 图标（方案 E 叶页）：平面版、Icon Composer 分层版、预览图
# 运行：python3 gen.py；预览：qlmanage -t -s 1200 -o . preview.svg
import pathlib

OUT = pathlib.Path(__file__).parent
FOREST, FOREST2, CREAM = "#2F5D46", "#1E3F30", "#F3EFE3"
MIDRIB, VEIN = "#7FA88A", "#9DB9A3"
LEAF = "M300 724 C300 480 480 300 724 300 C724 544 544 724 300 724 Z"
# 旧式 icns 的圆角方形是 100..924；Icon Composer 的画布就是图标形状，前景按 1024/824 放大
FULL = 'transform="translate(512 512) scale(1.2427) translate(-512 -512)"'


def svg(body, size=1024):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}">{body}</svg>'


def line(x1, y1, x2, y2, w, c):
    return f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{c}" stroke-width="{w}" stroke-linecap="round"/>'


def gradient(p):
    return (f'<linearGradient id="{p}g" x1="0" y1="0" x2="0" y2="1">'
            f'<stop offset="0" stop-color="{FOREST}"/><stop offset="1" stop-color="{FOREST2}"/></linearGradient>')


def leaf():
    return f'<path d="{LEAF}" fill="{CREAM}"/>'


def veins(p):
    # 叶脉是一条主脉加三行"文字"，行只画在叶子里面，从主脉开始
    rows = "".join(line(1024 - y - 8, y, 1024 - y + w, y, 16, VEIN) for y, w in ((430, 150), (510, 170), (590, 150)))
    return (f'<defs><clipPath id="{p}leaf"><path d="{LEAF}"/></clipPath></defs>'
            f'<g clip-path="url(#{p}leaf)">{line(332, 692, 650, 374, 10, MIDRIB)}{rows}</g>')


def flat(p):  # 平面版：带圆角方形，给 icns 兜底和预览用
    return (f'<defs>{gradient(p)}</defs><rect x="100" y="100" width="824" height="824" rx="185" fill="url(#{p}g)"/>'
            + leaf() + veins(p))


layers = OUT / "layers"
layers.mkdir(exist_ok=True)
(OUT / "hortus-icon.svg").write_text(svg(flat("f")))
(layers / "1-background.svg").write_text(svg(f'<defs>{gradient("b")}</defs><rect width="1024" height="1024" fill="url(#bg)"/>'))
(layers / "2-leaf.svg").write_text(svg(f'<g {FULL}>{leaf()}</g>'))
(layers / "3-veins.svg").write_text(svg(f'<g {FULL}>{veins("v")}</g>'))
# 分层叠起来检查一遍（满画布，不加遮罩）
(layers / "check.svg").write_text(svg(
    f'<defs>{gradient("c")}</defs><rect width="1024" height="1024" fill="url(#cg)"/><g {FULL}>{leaf()}{veins("c")}</g>'))

# 预览：上半浅色、下半深色，每行 400/200/128/64/32/16
parts = ['<rect width="1200" height="600" fill="#ECECEC"/><rect y="600" width="1200" height="600" fill="#1F1F1F"/>']
for half, y0 in (("L", 0), ("D", 600)):
    x = 30
    for i, s in enumerate((400, 200, 128, 64, 32, 16)):
        parts.append(f'<svg x="{x}" y="{y0 + 300 - s / 2}" width="{s}" height="{s}" viewBox="0 0 1024 1024">{flat(half + str(i))}</svg>')
        x += s + 50
(OUT / "preview.svg").write_text(svg("".join(parts), 1200))
print("ok")
