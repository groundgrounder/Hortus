# Hortus macOS

需要 Apple Silicon Mac、macOS 27、Xcode 27.1（Swift 6.4）、rustup stable 和 Python 3.11+（验收脚本）。在仓库根目录运行：

```sh
./tools/build-macos.sh
open build/Hortus.app
```

构建脚本编译 Rust 静态库、生成 UniFFI Swift 绑定、构建 SwiftPM 应用并进行 ad-hoc 签名。绑定与 `.app` 均为可重建产物，不提交。应用未开启沙盒，运行时无需联网。

通过文件菜单的“打开文件夹…”（⌘O）选择笔记库。支持 `.md`、`.markdown` 的原生样式化源码编辑：正文用系统比例字体，标题放大，标记符保持可见并变淡，代码用等宽字体；字号沿用 `font_size`。支持内嵌 Mermaid 树形图和 `.mmd` 整篇图；光标进入图块时展开源码并在下方显示预览。停止输入 1 秒后自动保存，⌘S 立即保存；切换标签/库、关闭标签、窗口失焦和退出也会保存。外部修改在无本地改动时自动载入，有本地改动时提示使用磁盘、保留本地或另存冲突副本。点击有下级的行折叠或展开，折叠集合按库、笔记路径和块序号保存在会话缓存。暂不支持的图在原处显示源码和说明。面板快捷键为 ⌃⌘S、⌥⌘I、⌘J；⌘W 关闭当前笔记标签。

列表末尾回车会续写前缀，空列表项回车结束列表；Tab / Shift-Tab 调整缩进。点击任务方括号或按 ⌘⏎ 切换勾选，每次操作均可一步撤销。⌘F 打开系统查找栏，⌘G / ⇧⌘G 查找下一个／上一个，⌥⌘F 打开替换；图块内的匹配项会先展开源码，全部替换可一步撤销。粘贴按纯文本处理。无障碍标题转子提供标题级别与源码范围。

配置默认保存在 `~/.config/hortus/`，会话保存在 `~/Library/Caches/Hortus/`。测试和自检通过 `HORTUS_CONFIG_DIR`、`HORTUS_CACHE_DIR` 指定绝对路径，避免触碰个人状态。笔记库中不会产生配置、缓存或其他应用文件。

第一次启动会把旧配置与缓存目录整体改名到新目录，包含会话、折叠状态和恢复稿。新旧目录同时存在时使用新目录，不合并，旧目录原样保留并记录日志。迁移失败会停止打开状态目录并保留旧数据。指定新的环境变量覆盖目录时，不迁移个人默认目录。

```sh
./tools/test-macos.sh
python3 tools/check-macos-workspace.py
python3 tools/check-idle-and-tabs.py --output target/reports/idle-and-tabs
python3 tools/benchmark-vault.py --output target/reports/benchmark.json
```

自检参数：`--empty` 跳过恢复库；`--vault=绝对路径` 打开库；`--note=相对路径` 打开笔记（可重复）；`--light` / `--dark` 指定外观；`--snapshot=绝对路径` 捕获本应用窗口；`--metrics=绝对路径` 输出计时、内存与结构数字。截图或计量结束后自动保存状态并退出。截图使用 ScreenCaptureKit 的本进程窗口接口，无需屏幕录制授权。

空闲验收脚本分别启动空状态和示例库，等待 5 秒后用 `top` 连续采样 10 秒，保存原始输出与中位数；随后在同一进程交替打开、切换、关闭两个临时库 20 次，记录线程数，并生成单标签和溢出标签截图。库切换检查使用 `--check-vault-lifecycle=结果路径` 和至少两个 `--cycle-vault=库路径`，只在显式传参时执行。

P1-03 绘制层：`SceneRenderer` 是纯 Core Graphics / CoreText 绘制函数；`DiagramView` 负责行命中、手形指针及逐节点无障碍。`DiagramService` 在后台长期持有 Rust 图引擎，并缓存当前文档的不可变 CoreText 行。Scene 使用 UniFFI record / enum 传输，包含原始字节范围；字宽设置显式对齐 Rust 的 `wght` / `opsz` 轴，并关闭 CoreText 额外的尺寸相关字距。

正文显式连接 `DiagramContentStorage → NSTextLayoutManager → NSTextContainer → DiagramTextView`，始终使用 TextKit 2。底层 `NSTextStorage` 保留完整源码；显示元素把成功图块逐段投影成等 UTF-16 长度的不可见文本，枚举越过块内后续段落，首段 `NoteLayoutFragment` 预留 Scene 高度，原处叠加 `DiagramView`。原生选择、复制读取底层源码。可见区域由原生视口控制器排版，未强制排完整篇文档；尚未放置的离屏图保持隐藏，滚动后随片段放置。错误片段保留原生绘制，在上方预留 24 pt 显示说明。没有访问旧版布局管理器。

同一正文列复用一个文本系统，切换或关闭笔记时清空旧场景与选择，避免重复创建原生排版缓存。`NSTextStorageDelegate` 只提取变更范围，经每篇文档的串行队列交给 Rust `DocumentSession`；全文围栏扫描和图重排在后台去抖 150 ms。TextKit 2 的片段遍历限制在视口，避免为大文档创建整篇段落缓存。原生组合输入期间冻结图投影及保存；结束后统一更新。

Markdown 语义由 Rust 在同一后台队列分析，返回发生变化的段落范围。Swift 在等长显示段落中设置字体、颜色和块装饰，底层文本存储保留源码；样式不进入撤销、脏状态或保存内容。移动光标不触发语义分析，输入法组合期间冻结样式应用。系统增强对比度开启后，标记符颜色加深一级。

macOS 用系统 `itemReplacementDirectory` 在同卷暂存，Rust 写入后复制原文件的创建时间、扩展属性、权限、所有者和 ACL，完成同步并在替换前再次核对内容基线。全部暂存写入都在最后核对之前完成；已有文件仍用原子 rename，新文件用不会覆盖同名文件的硬链接发布。普通库的暂存目录必须在库外；整卷根目录作为库时，允许系统 `.TemporaryItems/folders.<uid>/TemporaryItems/NSIRD_*` 私有目录短暂暂存，成功或失败后清理。

支持 LF / CRLF、UTF-8 BOM 和原始结尾换行，不自动补换行。混合换行按多数决定，平票跟第一处换行一致；新输入也统一使用该风格。非 UTF-8 或打开时无写权限的文件只读。保存失败后继续编辑并重试，恢复稿按库和笔记路径的固定 FNV-1a 64 位哈希命名，写在系统缓存的 `recovery/`，内容继续校验库和笔记路径，重启打开时可恢复或丢弃。退出先提交后台恢复稿与保存，不依赖 terminateLater 嵌套循环中的主线程异步任务。

```sh
python3 tools/check-diagram-view.py
```

该脚本启动隔离配置与缓存的 release App，验证真实窗口、折叠重启、三类源码回退、TextKit 2 片段、无障碍树、库目录哈希/mtime、60 次宽度变化、20 次开关、3 秒退出兜底及 30 秒 CPU 时间增量。内存使用独立进程测量，避免重排计量的离屏位图抬高峰值。私人样本不存在时明确跳过私人截图，改用最大公开夹具做性能检查。公开截图和数字位于 `target/reports/diagram-view/`，私人截图只写被忽略的 `private/`。可用 `--skip-cpu` 或 `--cpu-only` 分开执行。

`--diagram-inspection=路径` 启用数字检查；`--diagram-click` 经原生鼠标事件处理器点击首个可折叠行；`--diagram-resize` / `--diagram-memory` 运行有限次验收；`--inspect-slow-shutdown` 注入 10 秒后台收尾。正常运行不启用这些循环或等待。应用退出在 3 秒后答复 AppKit，即使后台收尾尚未完成。

P1-04 的自动验收使用隔离临时库，绝不改动公开示例或私人笔记：

```sh
python3 -B tools/check-edit-and-save.py
```

它覆盖真实保存菜单、窗口失焦、标签和库切换、退出恢复、1 MiB 文档的 300 次连续按键与可见布局，以及编辑后两组各 30 秒的 CPU 时间增量。`--edit-inspection=路径` 和 `--edit-mode=` 只用于验收；`--skip-cpu` / `--cpu-only` 可拆开运行。

P1-05 的样式和性能验收：

```sh
python3 -B tools/check-styled-source.py
```

它用公开示例捕获浅色、深色及正文高对比度模拟截图，另外捕获文末区域；核对公开库内容未变，并在普通与密集 Markdown 两篇 1 MiB 文档上各测 30 次编辑到屏幕的延迟和 300 次连续按键。打开耗时从打开笔记动作计到首次可见绘制，包含文件读取和编辑器初始化。`--screenshots-only` 仅更新截图。应用参数 `--simulate-high-contrast` 只模拟正文标记颜色，不切换系统增强对比度，也不能替代系统开关的人工验收；`--snapshot-end` 将截图区域滚到文末。结果位于 `target/reports/styled-source/`。

P1-04b 的补充验收：

```sh
python3 -B tools/check-save-metadata.py
```

它检查真实原生输入、1 秒自动保存后的元数据及 6 种换行情况，再创建 APFS 临时磁盘映像验证卷根保存，以及 ACL 拒绝替换时的原文、恢复稿和暂存清理。保存前后的 `stat`、`xattr`、`ls -le` 输出和每 5 ms 的目录检查结果写入 `target/reports/save-metadata/`；映像会卸载并删除。
