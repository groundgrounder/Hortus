# 树形 Mermaid 引擎

`hortus-diagram` 负责解析树形 flowchart / graph 和 mindmap、测量文字、按样稿 v2 分栏，并输出与平台无关的 `Scene`。不依赖 UI，不联网，不写笔记目录。

## Rust API

```rust,no_run
use hortus_diagram::{LayoutOptions, SystemFonts, layout, mermaid_block, parse_at};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let markdown = "```mermaid\nflowchart TB\nR[Root] --> A[First branch]\n```\n";
    let block = mermaid_block(markdown, 1)?;
    let tree = parse_at(block.source, block.source_range.start)?;
    let mut fonts = SystemFonts::new()?;
    let scene = layout(&tree, &LayoutOptions::default(), &mut fonts)?;
    let svg = scene.to_svg();
    Ok(())
}
```

- `mermaid_block` 按 1 起始的序号提取反引号或波浪线围栏，也识别未闭合的末尾围栏。
- `parse` 返回相对 Mermaid 文本的 UTF-8 字节范围；`parse_at` 加上调用者传入的偏移，可直接定位整篇笔记。节点范围取完整定义；只有引用时取首次引用。
- flowchart / graph 树形子集支持 `-->`、`---`、`-.->`、`-.-`、`==>`、`===` 及更长的同类变体；边标签支持 `-->|标签|` 与 `-- 标签 -->`，`A --> B & C` 会展开为多条边。双向箭头和带 `o` / `x` 端点的连线返回 `Syntax`。
- 支持 `classDef`、`class`、`:::class` 和 `style 节点 fill:...,stroke:...`；颜色接受 CSS 命名色、`#RGB` / `#RGBA` / `#RRGGBB` / `#RRGGBBAA`、`rgb()` / `rgba()`、`hsl()` / `hsla()`。无效颜色忽略，不影响排版。
- `linkStyle`、`click`、`accTitle`、`accDescr` 等只影响外观或交互的语句会忽略；`subgraph` 返回 `Unsupported`（树形图暂不支持 subgraph），其他未实现语法返回带 UTF-8 字节范围的 `Syntax`。
- `DiagramError::NotATree` 包含多父节点 id 和结构原因；其他图类型返回 `Unsupported`。
- `LayoutOptions.width` 包括左右各 24px 内边距，默认 700px；拒绝非有限数值和小于 120px 的宽度。`theme` 选择浅/深主题，`collapsed` 放折叠节点 id。
- `Scene` 包含尺寸、sRGB 图元、每段文字的字体/宽度/基线、节点命中区域和 metrics。平台直接绘制图元，`hit_test` 返回节点及源码范围。
- `FontMetrics` 可替换。运行时使用 `SystemFonts`（fontdb + rustybuzz），测试使用 `DeterministicMetrics`。西文优先系统 UI 字体，中文优先 PingFang SC，缺少时查找可用回退字体；实际字体记录在 metrics 中。
- 同一个 `SystemFonts` 对象可跨次排版复用，字体文件、字符选字和整形结果只缓存在内存中。字体不随库打包；SVG 在同一机器上使用相同字体族，系统 UI 字体使用 CSS 系统别名。

## CLI

在仓库根目录构建：

```sh
cargo build --release -p hortus-cli
target/release/hortus mermaid render note.md --out diagram.svg --metrics metrics.json
target/release/hortus mermaid render note.md --block 2 --width 1100 --theme dark --collapse A,B --out folded.svg
```

默认第 1 个 Mermaid 块、700px、浅色、不折叠。输出文件的父目录须已存在。输入笔记只读；SVG 与 metrics 必须是不同文件，且不能覆盖输入。SVG 使用 Scene 中已计算的坐标及测得的文字宽度，不依赖浏览器换行。

metrics 包含栏数、图高、最小字号、横向溢出、节点重叠数、实际字体、排版耗时和根/分支色相。CLI 的耗时包含本次排版中的整形及按需字体读取，不含解析和系统字体发现。错误退出码为非零。

## 验证与基准

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --release -p hortus-diagram --example benchmark -- note.md 700 30
```

基准只解析一次，复用字体度量对象，30 次排版包含第一次未缓存的整形；输出中位数、首轮、最小和最大耗时，解析和字体发现不计时。私人样本缺失时，对应结构和真实字体高度测试打印原因后跳过；缺少对照字体时只跳过真实高度对照。
