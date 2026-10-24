import SwiftUI

/// 可停靠的视图。所有面板（包括终端）都放在玻璃上
enum ViewKind: String, CaseIterable, Identifiable, Codable {
    case files, search, tags, outline, backlinks, terminal, changes
    var id: String { rawValue }
    var isAvailable: Bool { [.files, .search, .outline, .terminal].contains(self) }

    var title: String {
        switch self {
        case .files: L10n.string("Files")
        case .search: L10n.string("Search")
        case .tags: L10n.string("Tags")
        case .outline: L10n.string("Outline")
        case .backlinks: L10n.string("Backlinks")
        case .terminal: L10n.string("Terminal")
        case .changes: L10n.string("Changes")
        }
    }

    var symbol: String {
        switch self {
        case .files: "folder"
        case .search: "magnifyingglass"
        case .tags: "tag"
        case .outline: "list.bullet.indent"
        case .backlinks: "link"
        case .terminal: "terminal"
        case .changes: "clock.arrow.circlepath"
        }
    }
}

enum Region: String, CaseIterable, Identifiable, Codable {
    case left, right, bottom
    var id: String { rawValue }
    var title: String {
        switch self {
        case .left: L10n.string("Left Sidebar")
        case .right: L10n.string("Right Sidebar")
        case .bottom: L10n.string("Bottom Panel")
        }
    }
}

/// 区域里的一组视图：叠成标签，一次显示一个
struct PanelGroup: Identifiable, Codable {
    var id = UUID()
    var tabs: [ViewKind]
    var selected: ViewKind
}

@Observable @MainActor
final class Layout {
    var groups: [Region: [PanelGroup]] = [:] { didSet { onChange?() } }
    var visible: [Region: Bool] = [:] { didSet { onChange?() } }
    var size: [Region: CGFloat] = [:] { didSet { onChange?() } }
    @ObservationIgnored var onChange: (() -> Void)?
    /// 正在拖动的视图；不为空时显示所有可放置的位置
    var dragging: ViewKind?

    init() {
        reset()
    }

    func reset() {
        groups = [
            .left: [PanelGroup(tabs: [.files, .search, .tags], selected: .files)],
            .right: [PanelGroup(tabs: [.outline, .backlinks], selected: .outline)],
            .bottom: [PanelGroup(tabs: [.terminal, .changes], selected: .terminal)],
        ]
        visible = [.left: true, .right: true, .bottom: false]
        size = [.left: 260, .right: 250, .bottom: 230]
    }

    func groups(in region: Region) -> [PanelGroup] { groups[region] ?? [] }
    /// 只过滤显示投影；持久化仍保留旧版本的视图、组和选中状态。
    func availableGroups(in region: Region) -> [PanelGroup] {
        groups(in: region).compactMap { group in
            let tabs = group.tabs.filter(\.isAvailable)
            guard let first = tabs.first else { return nil }
            return PanelGroup(id: group.id, tabs: tabs, selected: tabs.contains(group.selected) ? group.selected : first)
        }
    }
    func isShown(_ region: Region) -> Bool { (visible[region] ?? true) && !availableGroups(in: region).isEmpty }

    func region(containing view: ViewKind) -> Region? {
        Region.allCases.first { region in groups(in: region).contains { $0.tabs.contains(view) } }
    }

    var terminalVisible: Bool {
        guard let region = region(containing: .terminal) else { return false }
        return isShown(region) && groups(in: region).contains { $0.selected == .terminal }
    }

    func setTerminalVisible(_ shown: Bool) {
        guard let region = region(containing: .terminal) else { return }
        if shown {
            visible[region] = true
            if let group = groups(in: region).first(where: { $0.tabs.contains(.terminal) }) {
                select(.terminal, in: group.id)
            }
        } else {
            visible[region] = false
        }
    }

    // MARK: 落点是否有效（无效的位置不高亮）

    /// 并入这一组：视图本来就在这组里时没有意义
    func canMerge(_ view: ViewKind?, into group: PanelGroup) -> Bool {
        guard let view, view.isAvailable, !group.tabs.filter(\.isAvailable).isEmpty else { return false }
        return locate(group.id) != nil && !group.tabs.contains(view)
    }

    /// 在这一组后面新开一组：把一组里唯一的视图拆到它自己后面，没有意义
    func canSplit(_ view: ViewKind?, after group: PanelGroup) -> Bool {
        guard let view, view.isAvailable, !group.tabs.filter(\.isAvailable).isEmpty else { return false }
        return locate(group.id) != nil && group.tabs.filter(\.isAvailable) != [view]
    }

    // MARK: 操作

    func select(_ view: ViewKind, in groupID: UUID) {
        guard view.isAvailable else { return }
        if let (r, i) = locate(groupID), groups[r]![i].tabs.contains(view) { groups[r]![i].selected = view }
    }

    /// 拖到某组的标签栏：并入该组
    func move(_ view: ViewKind, into groupID: UUID) {
        guard view.isAvailable, locate(groupID) != nil else { return }
        if let (r, i) = locate(groupID), groups(in: r)[i].tabs.contains(view) {
            groups[r]![i].selected = view
            return
        }
        detach(view)
        guard let (r, i) = locate(groupID) else { return }
        groups[r]![i].tabs.append(view)
        groups[r]![i].selected = view
    }

    /// 拖到某组的内容区或窗口边缘：在该区域新开一组（侧栏里上下分开，底部左右分开）
    func split(_ view: ViewKind, to region: Region, after groupID: UUID? = nil) {
        guard view.isAvailable else { return }
        if let id = groupID {
            guard let (r, i) = locate(id), r == region, groups(in: r)[i].tabs.filter(\.isAvailable) != [view] else { return }
        }
        detach(view)
        var list = groups(in: region)
        let at = groupID.flatMap { id in list.firstIndex { $0.id == id }.map { $0 + 1 } } ?? list.count
        list.insert(PanelGroup(tabs: [view], selected: view), at: min(at, list.count))
        groups[region] = list
        visible[region] = true
    }

    /// "移动到…"菜单：并入目标区域最后一组；区域为空时新开一组
    func moveToRegion(_ view: ViewKind, _ region: Region) {
        guard view.isAvailable else { return }
        if let last = availableGroups(in: region).last { move(view, into: last.id) } else { split(view, to: region) }
        visible[region] = true
    }

    private func detach(_ view: ViewKind) {
        for region in Region.allCases {
            groups[region] = groups(in: region).compactMap { group in
                var g = group
                g.tabs.removeAll { $0 == view }
                if g.tabs.isEmpty { return nil }
                if g.selected == view { g.selected = g.tabs[0] }
                return g
            }
        }
    }

    private func locate(_ id: UUID) -> (Region, Int)? {
        for r in Region.allCases {
            if let i = groups(in: r).firstIndex(where: { $0.id == id }) { return (r, i) }
        }
        return nil
    }
}

/// 使用字符串字典，保证 TOML/JSON 与其他平台共享同一布局格式。
struct LayoutState: Codable, Equatable {
    struct Group: Codable, Equatable {
        var id: UUID
        var tabs: [ViewKind]
        var selected: ViewKind
    }
    struct RegionState: Codable, Equatable {
        var groups: [Group]
        var visible: Bool
        var size: Double
    }
    var regions: [String: RegionState]
}

extension Layout {
    var state: LayoutState {
        LayoutState(regions: Dictionary(uniqueKeysWithValues: Region.allCases.map { region in
            (region.rawValue, LayoutState.RegionState(
                groups: groups(in: region).map { .init(id: $0.id, tabs: $0.tabs, selected: $0.selected) },
                visible: visible[region] ?? false, size: Double(size[region] ?? 250)))
        }))
    }

    func restore(_ state: LayoutState) {
        for region in Region.allCases {
            guard let value = state.regions[region.rawValue] else { continue }
            groups[region] = value.groups.map { PanelGroup(id: $0.id, tabs: $0.tabs, selected: $0.selected) }
            visible[region] = value.visible
            size[region] = CGFloat(value.size)
        }
    }
}
