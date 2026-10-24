import Foundation
import Testing
@testable import Hortus

@Suite @MainActor
struct LayoutTests {
    @Test func mergeKeepsEveryViewExactlyOnce() {
        let layout = Layout()
        let target = layout.groups(in: .left)[0]
        layout.move(.terminal, into: target.id)
        #expect(layout.groups(in: .left)[0].selected == .terminal)
        #expect(layout.groups(in: .left)[0].tabs.contains(.terminal))
        #expect(allViews(layout).count == 4)
        #expect(Set(allViews(layout)).count == 4)
    }

    @Test func splitPreservesSelectionAndRemovesEmptySource() {
        let layout = Layout()
        let first = layout.groups(in: .left)[0]
        layout.split(.files, to: .left, after: first.id)
        #expect(layout.groups(in: .left).count == 2)
        #expect(layout.groups(in: .left)[0].selected == .search)
        #expect(layout.groups(in: .left)[1].tabs == [.files])
        layout.moveToRegion(.files, .right)
        #expect(layout.groups(in: .left).count == 1)
        #expect(layout.groups(in: .right)[0].selected == .files)
    }

    @Test func movingToHiddenOrEmptyRegionRevealsIt() {
        let layout = Layout()
        #expect(!layout.isShown(.bottom))
        layout.moveToRegion(.files, .bottom)
        #expect(layout.isShown(.bottom))
        layout.moveToRegion(.outline, .left)
        #expect(layout.availableGroups(in: .right).isEmpty)
        layout.moveToRegion(.terminal, .right)
        #expect(layout.isShown(.right))
        #expect(layout.availableGroups(in: .right)[0].tabs == [.terminal])
    }

    @Test func invalidDropsAndMissingTargetsDoNotLoseViews() {
        let layout = Layout()
        let first = layout.groups(in: .left)[0]
        #expect(!layout.canMerge(nil, into: first))
        #expect(!layout.canMerge(.files, into: first))
        #expect(layout.canMerge(.terminal, into: first))
        layout.split(.files, to: .right)
        let single = layout.groups(in: .right).last!
        #expect(!layout.canSplit(.files, after: single))
        #expect(layout.canSplit(.terminal, after: single))
        let before = layout.state
        layout.move(.files, into: UUID())
        layout.split(.terminal, to: .left, after: UUID())
        layout.split(.files, to: .right, after: single.id)
        layout.select(.terminal, in: first.id)
        #expect(layout.state == before)
        let missing = PanelGroup(tabs: [.search], selected: .search)
        #expect(!layout.canMerge(.terminal, into: missing))
        #expect(!layout.canSplit(.terminal, after: missing))
    }

    @Test func layoutRoundTripRestoresGroupsSelectionVisibilityAndSizes() throws {
        let original = Layout()
        original.split(.terminal, to: .left)
        original.visible[.right] = false
        original.size[.left] = 310
        let data = try JSONEncoder().encode(original.state)
        let restored = Layout()
        restored.restore(try JSONDecoder().decode(LayoutState.self, from: data))
        #expect(restored.state == original.state)
        #expect(Set(allViews(restored)).count == 4)
    }

    @Test func legacyLayoutRetainsHiddenViewsButProjectsOnlyAvailableGroups() throws {
        let layout = Layout()
        layout.groups[.left] = [PanelGroup(tabs: [.tags, .files, .search], selected: .tags)]
        layout.groups[.right] = [PanelGroup(tabs: [.backlinks], selected: .backlinks),
                                PanelGroup(tabs: [.outline, .changes], selected: .changes)]
        layout.groups[.bottom] = [PanelGroup(tabs: [.changes], selected: .changes)]
        layout.visible[.bottom] = true
        let legacy = layout.state
        let restored = Layout()
        restored.restore(try JSONDecoder().decode(LayoutState.self, from: JSONEncoder().encode(legacy)))
        #expect(restored.state == legacy)
        #expect(restored.availableGroups(in: .left).map(\.tabs) == [[.files, .search]])
        #expect(restored.availableGroups(in: .left).first?.selected == .files)
        #expect(restored.availableGroups(in: .right).map(\.tabs) == [[.outline]])
        #expect(restored.availableGroups(in: .right).first?.selected == .outline)
        #expect(!restored.isShown(.bottom))
        let group = restored.availableGroups(in: .left)[0]
        for hidden in [ViewKind.tags, .backlinks, .changes] {
            #expect(!restored.canMerge(hidden, into: group))
            #expect(!restored.canSplit(hidden, after: group))
            restored.select(hidden, in: group.id)
            restored.move(hidden, into: group.id)
            restored.moveToRegion(hidden, .left)
            restored.split(hidden, to: .right)
        }
        #expect(restored.state == legacy)
        restored.moveToRegion(.terminal, .bottom)
        #expect(restored.isShown(.bottom))
        #expect(restored.availableGroups(in: .bottom).first?.tabs == [.terminal])
        restored.reset()
        #expect(restored.availableGroups(in: .left).map(\.tabs) == [[.files, .search]])
        #expect(restored.availableGroups(in: .right).map(\.tabs) == [[.outline]])
        #expect(restored.availableGroups(in: .bottom).map(\.tabs) == [[.terminal]])
        #expect(!restored.isShown(.bottom))
    }

    private func allViews(_ layout: Layout) -> [ViewKind] {
        Region.allCases.flatMap { layout.availableGroups(in: $0).flatMap(\.tabs) }
    }
}
