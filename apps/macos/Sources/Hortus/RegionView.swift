import SwiftUI

/// 一个停靠区域：若干组视图，侧栏里上下排，底部面板里左右排
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
                .modifier(DropZone(enabled: layout.canMerge(layout.dragging, into: group), shape: .capsule) {
                    layout.move($0, into: group.id)
                })
            PanelContent(kind: group.selected)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                // 拖到内容区：在这一组后面新开一组。高亮只盖住新组会出现的那一半
                .modifier(DropZone(enabled: layout.canSplit(layout.dragging, after: group), shape: .rect(cornerRadius: 12),
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
    let enabled: Bool
    let idleVisible: Bool
    let shape: S
    var preview: Edge?
    let action: (ViewKind) -> Void
    @State private var targeted = false

    init(enabled: Bool, idleVisible: Bool = false, shape: S, preview: Edge? = nil, action: @escaping (ViewKind) -> Void) {
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
                guard let raw = items.first, let view = ViewKind(rawValue: raw), view.isAvailable else { return }
                withAnimation(.snappy) { action(view) }
                layout.dragging = nil
            }
            .onDropSessionUpdated { session in
                switch session.phase {
                case .entering, .active: targeted = true
                default: targeted = false
                }
            }
            .animation(.snappy(duration: 0.18), value: enabled)
    }
}

struct TabStrip: View {
    @Environment(Layout.self) private var layout
    let group: PanelGroup

    var body: some View {
        HStack(spacing: 2) {
            ForEach(group.tabs) { kind in
                let selected = kind == group.selected
                Label(kind.title, systemImage: kind.symbol)
                    .labelStyle(.titleAndIcon)
                    .font(.callout.weight(selected ? .semibold : .regular))
                    .foregroundStyle(selected ? .primary : .secondary)
                    .padding(.horizontal, 9).padding(.vertical, 5)
                    .background(selected ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .capsule)
                    .contentShape(.capsule)
                    .onTapGesture { layout.select(kind, in: group.id) }
                    .draggable(kind.rawValue)
                    // 拖动开始时亮出所有落点，结束（放下或取消）时收起
                    .onDragSessionUpdated { session in
                        switch session.phase {
                        case .initial, .active: layout.dragging = kind
                        default: layout.dragging = nil
                        }
                    }
                    .contextMenu { menu(for: kind) }
                    .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
                    .accessibilityAction { layout.select(kind, in: group.id) }
            }
            Spacer(minLength: 0)
        }
        .lineLimit(1)
    }

    /// 不拖动也能移动视图
    @ViewBuilder private func menu(for kind: ViewKind) -> some View {
        Menu("Move to") {
            ForEach(Region.allCases) { r in
                Button(r.title) { withAnimation(.snappy) { layout.moveToRegion(kind, r) } }
            }
        }
        Menu("Split into a new group") {
            ForEach(Region.allCases) { r in
                Button(r.title) { withAnimation(.snappy) { layout.split(kind, to: r) } }
            }
        }
    }
}
