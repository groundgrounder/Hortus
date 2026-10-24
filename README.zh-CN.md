# Hortus

[English](README.md)

![同一张 81 节点 Mermaid 树形图放进 700 px：Mermaid 默认布局与 Hortus 的清晰分栏](docs/images/mermaid-comparison.png)

Hortus 是原生 Markdown 知识库阅读器，让大幅 Mermaid 树形图保持清晰，并在 agent 写入笔记时原地刷新。

[![下载 macOS 版 · Apple 芯片](docs/images/download-macos.svg)](https://github.com/groundgrounder/Hortus/releases)

## 主要特点

- **大图看得清。** 树形图超出笔记宽度时自动分栏，文字保持可读大小。上面的两幅图来自同一份 81 节点源码，每幅宽 700 px。
- **原生 Mac 应用。** 使用原生文本控件与绘图，没有 WebView，也不是 Electron。运行时不联网、不采集遥测。
- **阅读为主，agent 写入。** 在终端里让 agent 修改 Markdown 文件，Hortus 原地刷新，便于边读边整理知识库。需要自己编辑时按 ⌘E 切换。
- **笔记就是普通文件。** 直接打开 Markdown 文件夹，也能打开已有的 Obsidian 库。支持双链、嵌入、任务列表、callout 和 frontmatter；应用状态保存在笔记文件夹之外。

可以先打开原创的 [CC0 演示库](fixtures/demo-vault/README.md)。在 Apple M3 MacBook Air（macOS 27.0.1）上，Hortus 0.1.0 安装占用 9.5 MiB，启动后约 0.4 秒显示笔记，打开演示大图时只有一个进程、内存约 130 MiB。每项取五次测量的中位数，测法见 `tools/compare-obsidian.py`。

## 安装

需要 **macOS 27 或更高版本、Apple 芯片 Mac**。0.1.0 使用 ad-hoc 签名，尚未公证。

1. 从 [Releases](https://github.com/groundgrounder/Hortus/releases) 下载 `Hortus-0.1.0-arm64.dmg`，打开后把 Hortus 拖到“应用程序”。
2. 打开 Hortus。macOS 可能因无法验证开发者而阻止首次启动，可用下面任一种方式放行这个副本：

**系统设置：** 尝试打开被阻止后，点 **完成**，进入 **系统设置 → 隐私与安全**（有的版本叫“隐私与安全性”），找到 Hortus 的提示，点 **仍要打开**。弹出“打开‘Hortus’？”时再点 **仍要打开**，然后按提示验证身份；如有需要，再次启动 Hortus。

**终端：** 将应用复制到“应用程序”后运行：

```sh
xattr -dr com.apple.quarantine /Applications/Hortus.app
open /Applications/Hortus.app
```

两种方式均已在 macOS 27.0.1 上验证。其他系统版本上的提示文字可能不同。

## 已知问题

- 0.1.0 下载包采用 ad-hoc 签名。重新下载或更新应用后，可能需要再次放行首次启动。
- 终端访问“文稿”等受保护目录时，macOS 可能弹出授权提示。应用更新后可能需要在 **隐私与安全 → 文件与文件夹** 中重新授权。
- Mermaid 当前重点支持 mindmap 和树形 flowchart，暂不支持的图仍显示源码。

## 路线图

- **0.1.1：** 公证，评估支持 macOS 26。
- 更完善的链接与反链。
- 审阅、撤销 agent 的改动。
- Linux。

## 许可证

- `crates/` 和 `tools/` 可任选 MIT（[LICENSE-MIT](LICENSE-MIT)）或 Apache License 2.0（[LICENSE-APACHE](LICENSE-APACHE)）。
- `apps/` 使用 GPL-3.0-only（[apps/LICENSE](apps/LICENSE)）。
- `fixtures/vault-sample/` 中示例笔记原有的 CC0 声明继续有效。原创[演示库](fixtures/demo-vault/LICENSE.txt)同样采用 CC0。
- 应用里包含的开源软件及其许可证列在 **关于 Hortus → 第三方许可证**（`Hortus.app/Contents/Resources/ThirdPartyNotices.txt`，由 `tools/third-party-notices.py` 生成）。

贡献按所修改目录的许可证接收，无需签署 CLA。

觉得 Hortus 有用，请点个 [star](https://github.com/groundgrounder/Hortus)。
