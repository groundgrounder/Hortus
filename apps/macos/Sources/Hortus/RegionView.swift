import SwiftUI

/// 一个停靠区域：若干组工具，侧栏里上下排，底部面板里左右排
struct RegionView: View {
    @Environment(Layout.self) private var layout
    let region: Region

    var body: some View {
        let groups = layout.availableGroups(in: region)
        GlassEffectContainer(spacing: 8) {
            if region == .bottom {
                HStack(spacing: 8) { ForEach(groups) { GroupView(region: region, group: $0) } }
            } else {
                VStack(spacing: 8) { ForEach(groups) { GroupView(region: region, group: $0) } }
            }
        }
    }
}

struct GroupView: View {
    @Environment(Layout.self) private var layout
    let region: Region
    let group: PanelGroup

    var body: some View {
        VStack(spacing: 0) {
            TabStrip(group: group)
                .padding(.horizontal, 6).padding(.top, 6).padding(.bottom, 2)
                // 拖到标签栏：并入这一组
                .modifier(DropZone(name: "merge:" + group.id.uuidString,
                                   enabled: layout.canMerge(layout.dragging, into: group), shape: .capsule) {
                    layout.move($0, into: group.id)
                })
            PanelContent(tool: group.selected)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .background(ContentFrameProbe(name: "group-content-" + group.id.uuidString))
                // 拖到内容区：在这一组后面新开一组。高亮只盖住新组会出现的那一半
                .modifier(DropZone(name: "split:" + group.id.uuidString,
                                   enabled: layout.canSplit(layout.dragging, after: group), shape: .rect(cornerRadius: 12),
                                   preview: region == .bottom ? .trailing : .bottom) {
                    layout.split($0, to: region, after: group.id)
                })
        }
        .glassEffect(.regular, in: .rect(cornerRadius: 16, style: .continuous))
    }
}

/// 放置目标：只有指针所在的位置高亮；窗口边缘的落点（idleVisible）拖动时一直淡淡显示，否则找不到
struct DropZone<S: Shape>: ViewModifier {
    @Environment(Layout.self) private var layout
    let name: String
    let enabled: Bool
    let idleVisible: Bool
    let shape: S
    var preview: Edge?
    let action: (ToolID) -> Void
    @State private var targeted = false

    init(name: String, enabled: Bool, idleVisible: Bool = false, shape: S, preview: Edge? = nil,
         action: @escaping (ToolID) -> Void) {
        self.name = name
        self.enabled = enabled
        self.idleVisible = idleVisible
        self.shape = shape
        self.preview = preview
        self.action = action
    }

    func body(content: Content) -> some View {
        content
            .overlay {
                if enabled && (targeted || idleVisible) {
                    GeometryReader { geo in
                        let half = preview.map { $0 == .trailing ? CGSize(width: geo.size.width / 2, height: geo.size.height)
                                                                 : CGSize(width: geo.size.width, height: geo.size.height / 2) } ?? geo.size
                        shape
                            .fill(.tint.opacity(targeted ? 0.22 : 0.07))
                            .stroke(.tint.opacity(targeted ? 0.9 : 0.35), lineWidth: targeted ? 2 : 1)
                            .frame(width: targeted ? half.width : geo.size.width, height: targeted ? half.height : geo.size.height)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: targeted && preview != nil ? (preview == .trailing ? .trailing : .bottom) : .center)
                    }
                    .padding(3)
                    .allowsHitTesting(false)
                    .animation(.snappy(duration: 0.18), value: targeted)
                    .transition(.opacity)
                }
            }
            .dropDestination(for: String.self, isEnabled: enabled) { items, _ in
                guard let raw = items.first, let tool = ToolID(dragPayload: raw), layout.locateTool(tool) != nil else {
                    #if DEBUG
                    DragInspection.recordDropRejected(zone: name)
                    #endif
                    return
                }
                #if DEBUG
                DragInspection.recordDropAccepted(zone: name, tool: tool)
                #endif
                withAnimation(.snappy) { action(tool) }
                layout.dragging = nil
            }
            .onDropSessionUpdated { session in
                switch session.phase {
                case .entering, .active: targeted = true
                default: targeted = false
                }
                #if DEBUG
                DragInspection.recordDrop(zone: name, session: session, enabled: enabled, targeted: targeted,
                                          dragging: layout.dragging)
                #endif
            }
            .animation(.snappy(duration: 0.18), value: enabled)
    }
}

struct TabStrip: View {
    @Environment(Layout.self) private var layout
    @Environment(Workspace.self) private var workspace
    let group: PanelGroup

    var body: some View {
        HStack(spacing: 2) {
            ForEach(group.tabs) { tool in
                tab(tool, selected: tool == group.selected)
            }
            Spacer(minLength: 2)
            addMenu
        }
        .lineLimit(1)
        .background(ContentFrameProbe(name: "group-tabs-" + group.id.uuidString))
    }

    private func tab(_ tool: ToolID, selected: Bool) -> some View {
        // 文本只有放得下完整标题时才出现；放不下就整段隐藏，不截断、不省略。
        ViewThatFits(in: .horizontal) {
            Label {
                Text(title(tool))
                    .background(ContentFrameProbe(name: tool.probeName + "-title"))
            } icon: {
                Image(systemName: symbol(tool))
            }
            .labelStyle(.titleAndIcon)
            .fixedSize(horizontal: true, vertical: false)
            Image(systemName: symbol(tool))
        }
        .font(.callout.weight(selected ? .semibold : .regular))
        .foregroundStyle(selected ? .primary : .secondary)
        .padding(.horizontal, 9).padding(.vertical, 5)
        .background(selected ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .capsule)
        .contentShape(.capsule)
        .onTapGesture { layout.select(tool, in: group.id) }
        .draggable(tool.token)
        // 拖动开始时亮出所有落点，结束（放下或取消）时收起
        .onDragSessionUpdated { session in
            #if DEBUG
            let before = layout.dragging
            #endif
            switch session.phase {
            case .initial, .active: layout.dragging = tool
            default: layout.dragging = nil
            }
            #if DEBUG
            DragInspection.recordDrag(session, tool: tool, groupID: group.id,
                                      before: before, after: layout.dragging)
            #endif
        }
        .contextMenu { menu(for: tool) }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(title(tool))
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
        .accessibilityAction { layout.select(tool, in: group.id) }
        .accessibilityAction(named: L10n.string("Close %@", title(tool))) { workspace.closeTool(tool) }
        .help(title(tool))
        .background(ContentFrameProbe(name: tool.probeName))
    }

    private var addMenu: some View {
        Menu {
            ForEach(StaticTool.allCases) { tool in
                Button(tool.title) { workspace.addStaticTool(tool, to: group.id) }
            }
            Divider()
            Button(L10n.string("New Shell")) { workspace.addShell(to: group.id) }
                .disabled(workspace.root == nil)
            Button(L10n.string("New Agent")) { workspace.addAgent(to: group.id) }
                .disabled(workspace.root == nil)
        } label: {
            Image(systemName: "plus")
                .font(.callout)
                .frame(width: 22, height: 22)
                .contentShape(.rect)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .help(L10n.string("Add Tool"))
        .accessibilityLabel(L10n.string("Add Tool"))
        .background(ContentFrameProbe(name: "group-add-" + group.id.uuidString))
    }

    private func title(_ tool: ToolID) -> String {
        if let staticTool = tool.staticTool { return staticTool.title }
        if let id = tool.terminalID {
            return workspace.terminals.tab(id)?.title ?? L10n.string("Terminal")
        }
        return ""
    }

    private func symbol(_ tool: ToolID) -> String {
        if let staticTool = tool.staticTool { return staticTool.symbol }
        if let id = tool.terminalID {
            return workspace.terminals.tab(id)?.kind == .agent ? "sparkles" : "terminal"
        }
        return "questionmark"
    }

    /// 不拖动也能移动或关闭工具
    @ViewBuilder private func menu(for tool: ToolID) -> some View {
        Menu(L10n.string("Move to")) {
            ForEach(Region.allCases) { region in
                Button(region.title) { withAnimation(.snappy) { layout.moveToRegion(tool, region) } }
            }
        }
        Menu(L10n.string("Split into a new group")) {
            ForEach(Region.allCases) { region in
                Button(region.title) { withAnimation(.snappy) { layout.split(tool, to: region) } }
            }
        }
        Divider()
        Button(L10n.string("Close %@", title(tool))) { workspace.closeTool(tool) }
    }
}
