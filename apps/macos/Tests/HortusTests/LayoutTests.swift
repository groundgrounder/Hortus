import Foundation
import Testing
@testable import Hortus

@Suite @MainActor
struct LayoutTests {
    @Test func defaultLayoutHasThreeStaticToolsAndOneTerminal() {
        let layout = Layout()
        #expect(Set(layout.staticTools) == Set(StaticTool.allCases))
        #expect(layout.terminalIDs.count == 1)
        #expect(layout.availableGroups(in: .left).first?.tabs == [.tool(.files), .tool(.search)])
        #expect(layout.availableTabs(layout.availableGroups(in: .right)[0]) == [.tool(.outline)])
    }

    @Test func mergeKeepsEveryToolExactlyOnceAndTerminalIdentityStable() {
        let layout = Layout()
        let terminal = layout.terminalIDs[0]
        let target = layout.availableGroups(in: .left)[0]
        layout.move(.terminal(terminal), into: target.id)
        #expect(layout.availableGroups(in: .left)[0].selected == .terminal(terminal))
        #expect(layout.terminalIDs == [terminal])
        #expect(layout.terminalIDs.count == 1)
        #expect(Set(layout.staticTools).count == 3)
    }

    @Test func splitPreservesSelectionAndRemovesEmptySource() {
        let layout = Layout()
        let first = layout.availableGroups(in: .left)[0]
        layout.split(.tool(.files), to: .left, after: first.id)
        #expect(layout.availableGroups(in: .left).count == 2)
        #expect(layout.availableGroups(in: .left)[0].selected == .tool(.search))
        #expect(layout.availableGroups(in: .left)[1].tabs == [.tool(.files)])
        layout.moveToRegion(.tool(.files), .right)
        #expect(layout.availableGroups(in: .left).count == 1)
        #expect(layout.availableGroups(in: .right)[0].selected == .tool(.files))
    }

    @Test func movingToHiddenOrEmptyRegionRevealsIt() {
        let layout = Layout()
        #expect(!layout.isShown(.bottom))
        layout.moveToRegion(.tool(.files), .bottom)
        #expect(layout.isShown(.bottom))
        layout.moveToRegion(.tool(.outline), .left)
        #expect(layout.availableGroups(in: .right).isEmpty)
        layout.moveToRegion(.terminal(layout.terminalIDs[0]), .right)
        #expect(layout.isShown(.right))
        #expect(layout.availableGroups(in: .right)[0].tabs == [.terminal(layout.terminalIDs[0])])
    }

    @Test func staticToolIsUniqueAndCanBeClosedAndReadded() {
        let layout = Layout()
        let left = layout.availableGroups(in: .left)[0]
        #expect(layout.add(.tool(.files), to: left.id) == false)
        #expect(layout.availableGroups(in: .left)[0].selected == .tool(.files))
        #expect(layout.staticTools.filter { $0 == .files }.count == 1)

        layout.remove(.tool(.files))
        #expect(!layout.staticTools.contains(.files))
        #expect(layout.add(.tool(.files), to: layout.availableGroups(in: .right)[0].id))
        #expect(layout.staticTools.filter { $0 == .files }.count == 1)
        #expect(layout.region(containing: .tool(.files)) == .right)
    }

    @Test func terminalCommandsUseCurrentThenLastUsedTerminal() {
        let layout = Layout()
        layout.visible[.bottom] = true
        let terminal = layout.terminalIDs[0]
        #expect(layout.currentTerminalID == terminal)
        layout.select(.tool(.files), in: layout.availableGroups(in: .left)[0].id)
        #expect(layout.currentTerminalID == terminal)
        #expect(layout.terminalVisible)
        layout.setTerminalVisible(false)
        #expect(!layout.terminalVisible)
        layout.setTerminalVisible(true)
        #expect(layout.terminalVisible)
    }

    @Test func dynamicTerminalsAreIndependentAndCloseOnlyOne() {
        let layout = Layout()
        let first = layout.terminalIDs[0]
        let second = UUID()
        let third = UUID()
        layout.add(.terminal(second), to: layout.availableGroups(in: .left)[0].id)
        layout.add(.terminal(third), to: layout.availableGroups(in: .right)[0].id)
        #expect(Set(layout.terminalIDs) == Set([first, second, third]))
        layout.remove(.terminal(second))
        #expect(Set(layout.terminalIDs) == Set([first, third]))
        #expect(layout.region(containing: .terminal(third)) == .right)
        layout.remove(.terminal(first))
        layout.remove(.terminal(third))
        #expect(layout.terminalIDs.isEmpty)
        let restored = layout.state
        #expect(restored.regions.values.flatMap(\.groups).flatMap(\.tabs).filter { $0 == "terminal" }.count == 1)
    }

    @Test func invalidDropsAndMissingTargetsDoNotLoseTools() {
        let layout = Layout()
        let first = layout.availableGroups(in: .left)[0]
        let terminal = ToolID.terminal(layout.terminalIDs[0])
        #expect(!layout.canMerge(nil, into: first))
        #expect(!layout.canMerge(.tool(.files), into: first))
        #expect(layout.canMerge(terminal, into: first))
        layout.split(.tool(.files), to: .right)
        let single = layout.availableGroups(in: .right).last!
        #expect(!layout.canSplit(.tool(.files), after: single))
        #expect(layout.canSplit(terminal, after: single))
        let before = layout.state
        layout.move(.tool(.files), into: UUID())
        layout.split(terminal, to: .left, after: UUID())
        layout.split(.tool(.files), to: .right, after: single.id)
        layout.select(.tool(.files), in: UUID())
        #expect(layout.state == before)
        let missing = PanelGroup(tabs: [.tool(.search)], selected: .tool(.search))
        #expect(!layout.canMerge(.tool(.outline), into: missing))
        #expect(!layout.canSplit(.tool(.outline), after: missing))
        // 其它窗口的终端载荷不能进入本窗口。
        #expect(!layout.canMerge(.terminal(UUID()), into: first))
    }

    @Test func roundTripKeepsStaticLayoutVisibilityAndOneTerminalPlaceholder() throws {
        let original = Layout()
        original.split(.tool(.files), to: .left)
        let second = UUID()
        original.add(.terminal(second), to: original.availableGroups(in: .right).first?.id)
        original.visible[.right] = false
        original.size[.left] = 310
        let data = try JSONEncoder().encode(original.state)
        let restored = Layout()
        restored.restore(try JSONDecoder().decode(LayoutState.self, from: data))
        #expect(restored.state == original.state)
        #expect(restored.terminalIDs.count == 1)
        #expect(restored.size[.left] == 310)
        #expect(restored.visible[.right] == false)
        #expect(Set(restored.staticTools) == Set(StaticTool.allCases))

        let template = original.persistedState
        let templateRestored = Layout()
        templateRestored.restore(template)
        #expect(templateRestored.terminalIDs.count == 1)
        #expect(template.regions.values.allSatisfy { $0.visible == nil })
        #expect(template.regions.values.flatMap(\.groups).flatMap(\.tabs).allSatisfy {
            ["files", "search", "outline", "terminal"].contains($0)
        })
    }

    @Test func legacyLayoutMigratesTerminalAndPreservesHiddenCompatibilityData() throws {
        let legacy = LayoutState(regions: [
            "left": .init(groups: [.init(id: UUID(), tabs: ["tags", "files", "search"], selected: "tags")], visible: false, size: 300),
            "right": .init(groups: [
                .init(id: UUID(), tabs: ["backlinks"], selected: "backlinks"),
                .init(id: UUID(), tabs: ["outline", "changes"], selected: "changes"),
            ], visible: true, size: 240),
            "bottom": .init(groups: [.init(id: UUID(), tabs: ["terminal", "changes"], selected: "terminal")], visible: true, size: 260),
        ])
        let restored = Layout()
        restored.restore(try JSONDecoder().decode(LayoutState.self, from: JSONEncoder().encode(legacy)))
        #expect(restored.availableGroups(in: .left).map(\.tabs) == [[.tool(.files), .tool(.search)]])
        #expect(restored.availableGroups(in: .right).map(\.tabs) == [[.tool(.outline)]])
        #expect(restored.terminalIDs.count == 1)
        #expect(restored.region(containing: .terminal(restored.terminalIDs[0])) == .bottom)
        #expect(restored.visible[.left] == false && restored.visible[.bottom] == true)
        #expect(restored.size[.left] == 300)
        // 隐藏的空壳仍在兼容数据里，但不可显示、不可拖动或加入菜单。
        #expect(restored.state.regions["left"]?.groups.first?.tabs.contains("tags") == true)
        let group = restored.availableGroups(in: .left)[0]
        for hidden in [LegacyTool.tags, .backlinks, .changes] {
            #expect(!restored.canMerge(.legacy(hidden), into: group))
            #expect(!restored.canSplit(.legacy(hidden), after: group))
            restored.select(.legacy(hidden), in: group.id)
            restored.move(.legacy(hidden), into: group.id)
            restored.moveToRegion(.legacy(hidden), .left)
        }
        #expect(restored.region(containing: .legacy(.tags)) == .left)
    }

    @Test func templatesBecomeIndependentForEachNewWindow() {
        let a = Layout()
        a.moveToRegion(.tool(.outline), .left)
        let b = Layout()
        b.restore(a.persistedState)
        // 恢复时的终端是新的运行时 UUID，但静态排布一致。
        #expect(Set(a.staticTools) == Set(b.staticTools))
        #expect(a.region(containing: .tool(.outline)) == b.region(containing: .tool(.outline)))
        b.moveToRegion(.tool(.search), .right)
        #expect(a.region(containing: .tool(.search)) == .left)
        #expect(b.region(containing: .tool(.search)) == .right)
    }
}
