import SwiftUI

/// 窗口骨架：正文铺满整个窗口，面板作为玻璃浮在上面，滚动时内容从玻璃下面经过（macOS 26 起的系统做法）。
/// 左右侧栏贯穿全高，底部面板在两侧栏之间（与 VS Code 默认一致）
struct ShellView: View {
    @Environment(Layout.self) private var layout
    @Environment(Workspace.self) private var workspace
    @State private var toolbarWidth: CGFloat = 1320
    @State private var geometry = ToolbarGeometry()
    private let gap: CGFloat = 8

    var body: some View {
        ZStack {
            EditorView(insets: editorInsets)
            panels
            edgeDropZones
            QuickOpenOverlay()
        }
        .background(Color(nsColor: .textBackgroundColor).ignoresSafeArea())
        .toolbar { toolbarItems }
        // 不要单独的顶栏：去掉标题和工具栏底，只留红绿灯和按钮浮在内容上；全屏时整行隐藏，指针移到顶部再出现
        .toolbar(removing: .title)
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        .windowToolbarFullScreenVisibility(.onHover)
        .navigationTitle(workspace.title)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { toolbarWidth = $0 }
    }

    /// 正文让开浮在上面的面板；背景仍然铺满。顶部只留一点呼吸空间，正文可以滚到顶栏那一行下面
    private var editorInsets: EdgeInsets {
        let side = { (r: Region) in layout.isShown(r) ? (layout.size[r] ?? 0) + gap * 2 : 0 }
        let bottom = layout.isShown(.bottom) ? (layout.size[.bottom] ?? 0) + gap * 2 : 0
        return EdgeInsets(top: 8, leading: side(.left), bottom: bottom, trailing: side(.right))
    }

    private var panels: some View {
        ZStack(alignment: .top) {
            HStack(alignment: .top, spacing: gap) {
                // 左右侧栏都从顶栏那一行下面开始：红绿灯和左侧栏按钮浮在左侧栏上方，面板按钮浮在右侧栏上方，左右对称
                if layout.isShown(.left) {
                    RegionView(region: .left).frame(width: layout.size[.left])
                        .background(ContentFrameProbe(name: "left-panel"))
                        .overlay(alignment: .trailing) { ResizeHandle(region: .left, axis: .horizontal, sign: 1).offset(x: gap) }
                }
                // 中间列只承载底部面板，标签由系统工具栏统一管理。
                VStack(alignment: .leading, spacing: gap) {
                    Spacer(minLength: 0)
                    if layout.isShown(.bottom) {
                        RegionView(region: .bottom).frame(height: layout.size[.bottom])
                            .overlay(alignment: .top) { ResizeHandle(region: .bottom, axis: .vertical, sign: -1).offset(y: -gap) }
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                if layout.isShown(.right) {
                    RegionView(region: .right).frame(width: layout.size[.right])
                        .overlay(alignment: .leading) { ResizeHandle(region: .right, axis: .horizontal, sign: -1).offset(x: -gap) }
                }
            }
            .padding(.horizontal, gap)
            .padding(.bottom, gap)
        }
    }

    /// 拖动时，隐藏或空着的区域在窗口边缘显示落点
    @ViewBuilder private var edgeDropZones: some View {
        if layout.dragging?.isAvailable == true {
            ZStack {
                ForEach(Region.allCases.filter { !layout.isShown($0) }) { edgeZone($0) }
            }
            .padding(gap)
        }
    }

    private static let edgeZoneWidth: CGFloat = 220

    private func edgeZone(_ region: Region) -> some View {
        let vertical = region != .bottom
        // 底部落点只在两侧之间（底部面板本来的位置），不压住侧栏或侧栏落点
        let side = { (r: Region) in (layout.isShown(r) ? layout.size[r] ?? 0 : Self.edgeZoneWidth) + gap }
        return Text(L10n.string("Move to %@", region.title))
            .font(.callout.weight(.medium)).foregroundStyle(.tint)
            .frame(maxWidth: vertical ? Self.edgeZoneWidth : .infinity, maxHeight: vertical ? .infinity : 150)
            .modifier(DropZone(enabled: true, idleVisible: true, shape: .rect(cornerRadius: 16, style: .continuous)) { layout.split($0, to: region) })
            .padding(.leading, vertical ? 0 : side(.left))
            .padding(.trailing, vertical ? 0 : side(.right))
            // 侧栏落点停在底部面板上方：侧栏收起时底部面板会伸到窗口边，不能被侧栏落点压住
            .padding(.bottom, vertical && layout.isShown(.bottom) ? (layout.size[.bottom] ?? 0) + gap : 0)
            .frame(maxWidth: .infinity, maxHeight: .infinity,
                   alignment: region == .left ? .leading : region == .right ? .trailing : .bottom)
    }

    @ToolbarContentBuilder private var toolbarItems: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            toggle(.left, symbol: "sidebar.left", help: "Left Sidebar ⌃⌘S")
        }
        if !workspace.tabs.isEmpty {
            if layout.isShown(.left) {
                ToolbarItem(id: "editor-alignment", placement: .navigation) {
                    Color.clear.frame(width: alignmentWidth, height: 1)
                }
                .sharedBackgroundVisibility(.hidden)
            } else {
                ToolbarSpacer(.fixed, placement: .navigation)
            }
            ToolbarItem(id: "editor-tabs", placement: .navigation) {
                EditorTabs(maximumWidth: max(1, toolbarWidth - tabsLeading - trailingClearance))
                    .background(ToolbarProbe(geometry: geometry))
            }
        }
        // 去掉标题后中间没有弹性空间了，要显式撑开，面板按钮才会回到右上角
        ToolbarSpacer(.flexible)
        ToolbarItem(id: "bottom-panel", placement: .primaryAction) {
            toggle(.bottom, symbol: "rectangle.bottomthird.inset.filled", help: "Bottom Panel ⌘J")
        }
        ToolbarItem(id: "right-panel", placement: .primaryAction) {
            toggle(.right, symbol: "sidebar.right", help: "Right Sidebar ⌥⌘I")
        }
    }

    private var alignmentWidth: CGFloat {
        max(0, (layout.size[.left] ?? 0) + gap * 2 - geometry.alignedOrigin)
    }

    private var tabsLeading: CGFloat {
        layout.isShown(.left) ? (layout.size[.left] ?? 0) + gap * 2 : geometry.compactOrigin
    }

    private var trailingClearance: CGFloat {
        max(layout.isShown(.right) ? (layout.size[.right] ?? 0) + gap * 2 : 0, geometry.trailingClearance)
    }

    private func toggle(_ region: Region, symbol: String, help: String) -> some View {
        Button {
            withAnimation(.snappy) { layout.visible[region] = !(layout.visible[region] ?? true) }
        } label: {
            Label(region.title, systemImage: symbol)
        }
        .help(L10n.string(help))
        // 也是放置目标：把标签拖到按钮上，就移到这个区域
        .dropDestination(for: String.self) { items, _ in
            guard let raw = items.first, let view = ViewKind(rawValue: raw), view.isAvailable else { return }
            withAnimation(.snappy) { layout.split(view, to: region) }
            layout.dragging = nil
        }
    }
}

/// 面板之间的缝隙兼作拖动把手
struct ResizeHandle: View {
    @Environment(Layout.self) private var layout
    let region: Region
    let axis: Axis
    let sign: CGFloat
    @State private var start: CGFloat?

    var body: some View {
        Rectangle()
            .fill(.clear)
            .frame(width: axis == .horizontal ? 8 : nil, height: axis == .vertical ? 8 : nil)
            .contentShape(Rectangle())
            .pointerStyle(axis == .horizontal ? .columnResize : .rowResize)
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { value in
                        let base = start ?? layout.size[region] ?? 250
                        start = base
                        let delta = axis == .horizontal ? value.translation.width : value.translation.height
                        let limits: ClosedRange<CGFloat> = axis == .horizontal ? 180...480 : 120...520
                        layout.size[region] = min(max(base + sign * delta, limits.lowerBound), limits.upperBound)
                    }
                    .onEnded { _ in start = nil }
            )
            .accessibilityHidden(true)
    }
}
