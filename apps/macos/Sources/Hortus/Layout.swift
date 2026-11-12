import SwiftUI

/// 当前可用的静态工具。终端实例不属于这里，由 `ToolID.terminal` 表示。
enum StaticTool: String, CaseIterable, Identifiable, Codable {
    case files, search, outline
    var id: String { rawValue }

    var title: String {
        switch self {
        case .files: L10n.string("Files")
        case .search: L10n.string("Search")
        case .outline: L10n.string("Outline")
        }
    }

    var symbol: String {
        switch self {
        case .files: "folder"
        case .search: "magnifyingglass"
        case .outline: "list.bullet.indent"
        }
    }
}

/// 旧布局里已停用的视图。只用于读取磁盘上的兼容数据，永远不进入界面或 + 菜单。
enum LegacyTool: String, Codable {
    case tags, backlinks, changes
}

/// 一个工具标签：静态工具或带稳定 UUID 的终端实例。
/// 拖动、选中和持久化都只认这个标识；终端进程状态由 `TerminalController` 按 UUID 持有。
enum ToolID: Hashable, Identifiable {
    case tool(StaticTool)
    case terminal(UUID)
    case legacy(LegacyTool)

    var id: String {
        switch self {
        case .tool(let tool): "tool:" + tool.rawValue
        case .terminal(let id): "terminal:" + id.uuidString
        case .legacy(let tool): "legacy:" + tool.rawValue
        }
    }

    var isAvailable: Bool {
        if case .legacy = self { return false }
        return true
    }

    var staticTool: StaticTool? {
        if case .tool(let tool) = self { return tool }
        return nil
    }

    var terminalID: UUID? {
        if case .terminal(let id) = self { return id }
        return nil
    }

    /// 持久化与拖放载荷共用的字符串。终端实例写 UUID，落盘时再换成占位符。
    var token: String {
        switch self {
        case .tool(let tool): tool.rawValue
        case .terminal(let id): "terminal:" + id.uuidString
        case .legacy(let tool): tool.rawValue
        }
    }

    var probeName: String {
        switch self {
        case .tool(let tool): "panel-tab-" + tool.rawValue
        case .terminal(let id): "panel-tab-terminal-" + id.uuidString
        case .legacy(let tool): "panel-tab-legacy-" + tool.rawValue
        }
    }

    init?(dragPayload: String) {
        if dragPayload.hasPrefix("terminal:") {
            guard let id = UUID(uuidString: String(dragPayload.dropFirst("terminal:".count))) else { return nil }
            self = .terminal(id)
        } else if let tool = StaticTool(rawValue: dragPayload) {
            self = .tool(tool)
        } else {
            return nil
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

/// 区域里的一组工具：叠成标签，一次显示一个。
struct PanelGroup: Identifiable {
    var id = UUID()
    var tabs: [ToolID]
    var selected: ToolID
}

/// 终端在布局里的最后位置；终端全部关闭后仍能记录重启时应创建 Shell 的位置。
private struct TerminalAnchor {
    var region: Region
    var groupID: UUID?
    var index: Int
}

@Observable @MainActor
final class Layout {
    /// 每个窗口独占自己的工具组、顺序和选中状态。
    var groups: [Region: [PanelGroup]] { didSet { onChange?() } }
    var visible: [Region: Bool] = [:] { didSet { onChange?() } }
    var size: [Region: CGFloat] = [:] { didSet { onSizeChange?(); onChange?() } }
    @ObservationIgnored var onChange: (() -> Void)?
    @ObservationIgnored var onSizeChange: (() -> Void)?
    /// 正在拖动的工具；不为空时显示所有可放置的位置。
    var dragging: ToolID?
    /// 最近选择过的工具与终端；只影响命令落到哪个终端，不替代组内的选中状态。
    private(set) var lastSelectedTool: ToolID?
    private(set) var lastTerminalID: UUID?
    private var terminalAnchor: TerminalAnchor?

    init() {
        let terminalID = UUID()
        groups = [
            .left: [PanelGroup(tabs: [.tool(.files), .tool(.search)], selected: .tool(.files))],
            .right: [PanelGroup(tabs: [.tool(.outline)], selected: .tool(.outline))],
            .bottom: [PanelGroup(tabs: [.terminal(terminalID)], selected: .terminal(terminalID))],
        ]
        visible = [.left: true, .right: true, .bottom: false]
        size = [.left: 260, .right: 250, .bottom: 230]
        lastSelectedTool = .tool(.files)
        lastTerminalID = terminalID
        terminalAnchor = TerminalAnchor(region: .bottom, groupID: groups[.bottom]?.first?.id, index: 0)
    }

    func reset() {
        let terminalID = UUID()
        groups = [
            .left: [PanelGroup(tabs: [.tool(.files), .tool(.search)], selected: .tool(.files))],
            .right: [PanelGroup(tabs: [.tool(.outline)], selected: .tool(.outline))],
            .bottom: [PanelGroup(tabs: [.terminal(terminalID)], selected: .terminal(terminalID))],
        ]
        visible = [.left: true, .right: true, .bottom: false]
        size = [.left: 260, .right: 250, .bottom: 230]
        dragging = nil
        lastSelectedTool = .tool(.files)
        lastTerminalID = terminalID
        terminalAnchor = TerminalAnchor(region: .bottom, groupID: groups[.bottom]?.first?.id, index: 0)
    }

    func groups(in region: Region) -> [PanelGroup] { groups[region] ?? [] }

    func availableTabs(_ group: PanelGroup) -> [ToolID] { group.tabs.filter(\.isAvailable) }

    /// 只过滤显示投影；持久化仍保留旧版本的隐藏视图。
    func availableGroups(in region: Region) -> [PanelGroup] {
        groups(in: region).compactMap { group in
            let tabs = availableTabs(group)
            guard let first = tabs.first else { return nil }
            return PanelGroup(id: group.id, tabs: tabs, selected: tabs.contains(group.selected) ? group.selected : first)
        }
    }

    func isShown(_ region: Region) -> Bool { (visible[region] ?? true) && !availableGroups(in: region).isEmpty }

    func region(containing tool: ToolID) -> Region? {
        Region.allCases.first { region in self.groups(in: region).contains { $0.tabs.contains(tool) } }
    }

    func group(containing tool: ToolID) -> PanelGroup? {
        Region.allCases.lazy.compactMap { region in
            self.groups(in: region).first { $0.tabs.contains(tool) }
        }.first
    }

    func locateTool(_ tool: ToolID) -> (Region, Int)? {
        for region in Region.allCases {
            if let index = groups(in: region).firstIndex(where: { $0.tabs.contains(tool) }) { return (region, index) }
        }
        return nil
    }

    /// 窗口内全部终端实例，按区域和标签顺序排列。
    var terminalIDs: [UUID] {
        Region.allCases.flatMap { region in
            groups(in: region).flatMap { $0.tabs.compactMap(\.terminalID) }
        }
    }

    var staticTools: [StaticTool] {
        Region.allCases.flatMap { region in
            groups(in: region).flatMap { $0.tabs.compactMap(\.staticTool) }
        }
    }

    /// 最近选中的终端；静态工具选中时回落到上次使用的终端。
    var currentTerminalID: UUID? {
        if case .terminal(let id) = lastSelectedTool, terminalIDs.contains(id) { return id }
        return lastTerminalID
    }

    var terminalVisible: Bool {
        guard let id = currentTerminalID ?? terminalIDs.first,
              let region = region(containing: .terminal(id)), isShown(region) else { return false }
        return groups(in: region).contains { $0.tabs.contains(.terminal(id)) && $0.selected == .terminal(id) }
    }

    /// 加号或命令新建工具时的默认组：优先当前终端所在组，否则底部最后一组。
    var defaultTerminalGroupID: UUID? {
        if let id = currentTerminalID ?? terminalIDs.first, let group = group(containing: .terminal(id)) { return group.id }
        return groups(in: .bottom).last?.id
    }

    // MARK: 选中与显示

    func select(_ tool: ToolID, in groupID: UUID) {
        guard tool.isAvailable, let (region, index) = locate(groupID: groupID),
              groups(in: region)[index].tabs.contains(tool) else { return }
        groups[region]![index].selected = tool
        noteSelection(tool)
    }

    /// 显示并激活窗口内的工具；找不到时返回 false。
    @discardableResult
    func reveal(_ tool: ToolID) -> Bool {
        guard tool.isAvailable, let (region, index) = locateTool(tool) else { return false }
        groups[region]![index].selected = tool
        visible[region] = true
        noteSelection(tool)
        return true
    }

    func setTerminalVisible(_ shown: Bool) {
        guard let id = currentTerminalID ?? terminalIDs.first else { return }
        if shown {
            reveal(.terminal(id))
        } else if let region = region(containing: .terminal(id)) {
            visible[region] = false
        }
    }

    /// 显示区域；区域为空时补一个默认工具，保证 + 始终可达。
    @discardableResult
    func ensureGroup(in region: Region) -> ToolID? {
        visible[region] = true
        guard availableGroups(in: region).isEmpty else { return nil }
        let preferred: [StaticTool] = switch region {
        case .left: [.files, .search, .outline]
        case .right: [.outline, .files, .search]
        case .bottom: []
        }
        let present = Set(staticTools)
        let tool = preferred.first { !present.contains($0) }
            .map(ToolID.tool) ?? .terminal(UUID())
        if let last = groups(in: region).last {
            add(tool, to: last.id)
        } else {
            add(tool, in: region)
        }
        return tool
    }

    // MARK: 落点是否有效（无效的位置不高亮）

    func canMerge(_ tool: ToolID?, into group: PanelGroup) -> Bool {
        guard let tool, tool.isAvailable, locateTool(tool) != nil, locate(groupID: group.id) != nil else { return false }
        return !availableTabs(group).isEmpty && !group.tabs.contains(tool)
    }

    /// 在这一组后面新开一组：把一组里唯一的工具拆到它自己后面，没有意义。
    func canSplit(_ tool: ToolID?, after group: PanelGroup) -> Bool {
        guard let tool, tool.isAvailable, locateTool(tool) != nil, locate(groupID: group.id) != nil else { return false }
        return !availableTabs(group).isEmpty && availableTabs(group) != [tool]
    }

    // MARK: 操作

    /// 添加工具：静态工具在窗口内唯一，重复添加只显示已有项。
    @discardableResult
    func add(_ tool: ToolID, to groupID: UUID? = nil) -> Bool {
        guard tool.isAvailable else { return false }
        if let staticTool = tool.staticTool, let existing = staticTools.first(where: { $0 == staticTool }) {
            reveal(.tool(existing))
            return false
        }
        guard let located = groupID.flatMap({ locate(groupID: $0) })
            ?? groups(in: .bottom).indices.last.map({ (.bottom, $0) }) else {
            groups[.bottom] = [PanelGroup(tabs: [tool], selected: tool)]
            visible[.bottom] = true
            noteSelection(tool)
            return true
        }
        let (region, index) = located
        if !groups[region]![index].tabs.contains(tool) {
            groups[region]![index].tabs.append(tool)
        }
        groups[region]![index].selected = tool
        visible[region] = true
        noteSelection(tool)
        return true
    }

    @discardableResult
    func add(_ tool: ToolID, in region: Region) -> Bool {
        guard tool.isAvailable else { return false }
        var list = groups(in: region)
        list.append(PanelGroup(tabs: [tool], selected: tool))
        groups[region] = list
        visible[region] = true
        noteSelection(tool)
        return true
    }

    /// 拖到某组的标签栏：并入该组。
    func move(_ tool: ToolID, into groupID: UUID) {
        guard tool.isAvailable, locateTool(tool) != nil, locate(groupID: groupID) != nil else { return }
        if let (region, index) = locate(groupID: groupID), groups(in: region)[index].tabs.contains(tool) {
            groups[region]![index].selected = tool
            noteSelection(tool)
            return
        }
        detach(tool)
        guard let (region, index) = locate(groupID: groupID) else { return }
        groups[region]![index].tabs.append(tool)
        groups[region]![index].selected = tool
        noteSelection(tool)
    }

    /// 拖到某组的内容区或窗口边缘：在该区域新开一组（侧栏里上下分开，底部左右分开）。
    func split(_ tool: ToolID, to region: Region, after groupID: UUID? = nil) {
        guard tool.isAvailable, locateTool(tool) != nil else { return }
        if let id = groupID {
            guard let (targetRegion, index) = locate(groupID: id), targetRegion == region,
                  availableTabs(groups(in: targetRegion)[index]) != [tool] else { return }
        }
        detach(tool)
        var list = groups(in: region)
        let at = groupID.flatMap { id in list.firstIndex { $0.id == id }.map { $0 + 1 } } ?? list.count
        list.insert(PanelGroup(tabs: [tool], selected: tool), at: min(at, list.count))
        groups[region] = list
        visible[region] = true
        noteSelection(tool)
    }

    /// "移动到…"菜单：并入目标区域最后一组；区域为空时新开一组。
    func moveToRegion(_ tool: ToolID, _ region: Region) {
        guard tool.isAvailable, locateTool(tool) != nil else { return }
        if let last = availableGroups(in: region).last { move(tool, into: last.id) } else { split(tool, to: region) }
        visible[region] = true
    }

    /// 关闭工具；组空后移除，区域因此隐藏时仍可由工具栏的 + 路径重新建立。
    func remove(_ tool: ToolID) {
        guard let (region, index) = locateTool(tool) else { return }
        rememberTerminalPosition()
        var group = groups(in: region)[index]
        let position = group.tabs.firstIndex(of: tool) ?? 0
        group.tabs.removeAll { $0 == tool }
        if group.tabs.isEmpty {
            groups[region] = groups(in: region).filter { $0.id != group.id }
        } else {
            if group.selected == tool { group.selected = group.tabs[min(position, group.tabs.count - 1)] }
            groups[region]![index] = group
        }
        if lastSelectedTool == tool { lastSelectedTool = nil }
        if let id = tool.terminalID, lastTerminalID == id { lastTerminalID = terminalIDs.first }
    }

    /// 换库或复位时只保留一个终端工具。
    func keepOnlyTerminal(_ id: UUID) {
        for terminal in terminalIDs where terminal != id {
            removeTerminalTool(terminal)
        }
        rememberTerminalPosition()
    }

    func removeAllTerminalTools() {
        rememberTerminalPosition()
        for id in terminalIDs { removeTerminalTool(id) }
        lastTerminalID = nil
    }

    /// 在默认位置加入一个新终端工具；用于布局复位或模板实例化。
    func insertTerminal(_ id: UUID) {
        if let anchor = terminalAnchor, let located = anchor.groupID.flatMap({ locate(groupID: $0) }) {
            add(.terminal(id), to: groups(in: located.0)[located.1].id)
        } else {
            add(.terminal(id))
        }
        lastTerminalID = id
    }

    private func removeTerminalTool(_ id: UUID) {
        remove(.terminal(id))
    }

    private func detach(_ tool: ToolID) {
        rememberTerminalPosition()
        for region in Region.allCases {
            groups[region] = groups(in: region).compactMap { group in
                var updated = group
                updated.tabs.removeAll { $0 == tool }
                if updated.tabs.isEmpty { return nil }
                if updated.selected == tool { updated.selected = updated.tabs[0] }
                return updated
            }
        }
    }

    private func locate(groupID: UUID) -> (Region, Int)? {
        for region in Region.allCases {
            if let index = groups(in: region).firstIndex(where: { $0.id == groupID }) { return (region, index) }
        }
        return nil
    }

    private func noteSelection(_ tool: ToolID) {
        lastSelectedTool = tool
        if case .terminal(let id) = tool { lastTerminalID = id }
        rememberTerminalPosition()
    }

    private func rememberTerminalPosition() {
        guard let id = currentTerminalID ?? terminalIDs.first,
              let (region, index) = locateTool(.terminal(id)) else { return }
        let group = groups(in: region)[index]
        terminalAnchor = TerminalAnchor(region: region, groupID: group.id,
                                        index: group.tabs.firstIndex(of: .terminal(id)) ?? group.tabs.count)
    }
}

/// 使用字符串字典，保证 TOML/JSON 与其他平台共享同一布局格式。
/// 终端实例只以 `"terminal"` 占位记录位置，UUID 不进入配置或会话。
struct LayoutState: Codable, Equatable {
    struct Group: Codable, Equatable {
        var id: UUID
        var tabs: [String]
        var selected: String
    }
    struct RegionState: Codable, Equatable {
        var groups: [Group]
        var visible: Bool?
        var size: Double
    }
    var regions: [String: RegionState]
}

extension Layout {
    /// 写入 layout.toml 的默认模板：只有静态工具和默认终端位置。
    var persistedState: LayoutState { encodedState(includeLegacy: false, includeVisible: false) }

    /// 会话里的每窗口布局：保留静态工具、隐藏兼容项、可见性和尺寸。
    var state: LayoutState { encodedState(includeLegacy: true, includeVisible: true) }

    private func encodedState(includeLegacy: Bool, includeVisible: Bool) -> LayoutState {
        var encoded: [String: LayoutState.RegionState] = [:]
        var terminalWritten = false
        for region in Region.allCases {
            var list: [LayoutState.Group] = []
            for group in groups(in: region) {
                var tabs: [String] = []
                for tool in group.tabs {
                    switch tool {
                    case .tool(let staticTool):
                        tabs.append(staticTool.rawValue)
                    case .terminal:
                        if !terminalWritten {
                            terminalWritten = true
                            tabs.append("terminal")
                        }
                    case .legacy(let legacy):
                        if includeLegacy { tabs.append(legacy.rawValue) }
                    }
                }
                guard !tabs.isEmpty, let selected = encodedSelection(group.selected, in: tabs, tabs: group.tabs) else { continue }
                list.append(LayoutState.Group(id: group.id, tabs: tabs, selected: selected))
            }
            encoded[region.rawValue] = LayoutState.RegionState(
                groups: list, visible: includeVisible ? (visible[region] ?? false) : nil,
                size: Double(size[region] ?? 250))
        }
        if !terminalWritten {
            insertTerminalPlaceholder(into: &encoded, includeVisible: includeVisible)
        }
        return LayoutState(regions: encoded)
    }

    private func encodedSelection(_ selected: ToolID, in tabs: [String], tabs tools: [ToolID]) -> String? {
        if case .terminal = selected, tabs.contains("terminal") { return "terminal" }
        if case .legacy(let legacy) = selected, tabs.contains(legacy.rawValue) { return legacy.rawValue }
        if let staticTool = selected.staticTool, tabs.contains(staticTool.rawValue) { return staticTool.rawValue }
        return tabs.first
    }

    /// 终端全部关闭时，用最后位置或底部最后一组记录重启 Shell 的位置。
    private func insertTerminalPlaceholder(into encoded: inout [String: LayoutState.RegionState], includeVisible: Bool) {
        let anchorRegion = terminalAnchor?.region ?? .bottom
        var region = anchorRegion
        if encoded[region.rawValue]?.groups.isEmpty != false {
            region = .bottom
        }
        var groups = encoded[region.rawValue]?.groups ?? []
        var groupIndex = groups.firstIndex { $0.id == terminalAnchor?.groupID }
        if groupIndex == nil { groupIndex = groups.indices.last }
        if let index = groupIndex {
            let at = min(max(terminalAnchor?.index ?? 0, 0), groups[index].tabs.count)
            groups[index].tabs.insert("terminal", at: at)
            groups[index].selected = "terminal"
        } else {
            groups.append(LayoutState.Group(id: UUID(), tabs: ["terminal"], selected: "terminal"))
        }
        let old = encoded[region.rawValue]
        encoded[region.rawValue] = LayoutState.RegionState(
            groups: groups,
            visible: includeVisible ? old?.visible : nil,
            size: old?.size ?? Double(size[region] ?? 250))
    }

    func restore(_ state: LayoutState) {
        var restored: [Region: [PanelGroup]] = [:]
        var usedTerminal = false
        var seenStatic = Set<StaticTool>()
        var seenLegacy = Set<LegacyTool>()
        for region in Region.allCases {
            guard let value = state.regions[region.rawValue] else { continue }
            var list: [PanelGroup] = []
            for group in value.groups {
                var tabs: [ToolID] = []
                for raw in group.tabs {
                    if let tool = StaticTool(rawValue: raw) {
                        if seenStatic.insert(tool).inserted { tabs.append(.tool(tool)) }
                    } else if raw == "terminal" {
                        if !usedTerminal {
                            usedTerminal = true
                            tabs.append(.terminal(UUID()))
                        }
                    } else if let legacy = LegacyTool(rawValue: raw) {
                        if seenLegacy.insert(legacy).inserted { tabs.append(.legacy(legacy)) }
                    }
                }
                guard !tabs.isEmpty else { continue }
                let selected = tabs.first { persistedName($0) == group.selected }
                    ?? tabs.first(where: \.isAvailable) ?? tabs[0]
                list.append(PanelGroup(id: group.id, tabs: tabs, selected: selected))
            }
            if !list.isEmpty { restored[region] = list }
            if let shown = value.visible { visible[region] = shown }
            size[region] = CGFloat(value.size)
        }
        groups = restored
        lastTerminalID = terminalIDs.first
        lastSelectedTool = groups(in: .left).first?.selected
        terminalAnchor = nil
        rememberTerminalPosition()
    }

    private func persistedName(_ tool: ToolID) -> String {
        switch tool {
        case .terminal: "terminal"
        case .tool(let staticTool): staticTool.rawValue
        case .legacy(let legacy): legacy.rawValue
        }
    }
}
