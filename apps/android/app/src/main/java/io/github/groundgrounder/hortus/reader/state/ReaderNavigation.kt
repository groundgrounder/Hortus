// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.state

import io.github.groundgrounder.hortus.core.model.NoteContent
import io.github.groundgrounder.hortus.reader.model.ReaderAnchor
import io.github.groundgrounder.hortus.reader.model.ScrollTarget

internal data class NavigationDestination(val path: String, val heading: String?)

/**
 * 管理笔记浏览历史栈、滚动锚点与标题/源码偏移跳转计算。
 */
internal class ReaderNavigation {
    private val stack = ArrayList<NavigationDestination>()
    private val anchors = HashMap<String, ReaderAnchor>()

    var pendingScroll: Pair<String, Int>? = null
    var lastVisibleBlockId: Long? = null
        private set
    var lastVisibleOffsetPx: Int = 0
        private set
    var lastVisiblePath: String? = null
        private set

    fun clearPending() {
        pendingScroll = null
    }

    fun resetForVault() {
        stack.clear()
        anchors.clear()
        pendingScroll = null
    }

    fun canPop(): Boolean = stack.size > 1

    fun recordVisit(path: String, heading: String?, push: Boolean) {
        val destination = NavigationDestination(path, heading)
        when {
            push || stack.isEmpty() -> stack.add(destination)
            else -> stack[stack.lastIndex] = destination
        }
    }

    fun popPrevious(currentContent: NoteContent?): NavigationDestination? {
        if (stack.size <= 1) return null
        if (currentContent != null) {
            lastVisibleBlockId?.let { anchors[currentContent.path] = ReaderAnchor(it, lastVisibleOffsetPx) }
        }
        stack.removeAt(stack.lastIndex)
        return stack.last()
    }

    fun updateAnchor(path: String?, blockId: Long, offsetPx: Int) {
        lastVisibleBlockId = blockId
        lastVisibleOffsetPx = offsetPx
        val notePath = path ?: return
        lastVisiblePath = notePath
        anchors[notePath] = ReaderAnchor(blockId, offsetPx)
    }

    fun restoreTarget(content: NoteContent): ScrollTarget? {
        if (lastVisiblePath != content.path) return null
        val anchor = anchors[content.path] ?: return null
        val index = itemIndexForBlock(content, anchor.blockId) ?: return null
        return ScrollTarget(index, anchor.offsetPx)
    }

    fun resolveOpenTarget(content: NoteContent, heading: String?, restoreAnchor: Boolean): ScrollTarget? {
        val pending = pendingScroll
        if (pending != null && pending.first == content.path) {
            pendingScroll = null
            return sourceOffsetTarget(content, pending.second)
                ?: scrollTargetFor(content, heading, restoreAnchor)
        }
        return scrollTargetFor(content, heading, restoreAnchor)
    }

    fun scrollTargetFor(content: NoteContent, heading: String?, restoreAnchor: Boolean): ScrollTarget? {
        val anchor = if (restoreAnchor) anchors[content.path] else null
        if (anchor != null) {
            val index = itemIndexForBlock(content, anchor.blockId)
            if (index != null) return ScrollTarget(index, anchor.offsetPx)
        }
        if (heading != null) {
            return headingTarget(content, heading)
        }
        return null
    }

    fun headingTarget(content: NoteContent, heading: String): ScrollTarget? {
        val normalized = heading.trim()
        if (normalized.isEmpty()) return null
        val match = content.outline.headings.firstOrNull {
            it.title.trim().equals(normalized, ignoreCase = true) ||
                slug(it.title) == slug(normalized)
        } ?: return null
        val start = match.start.toInt()
        val block = content.model.blocks.firstOrNull {
            start in it.sourceStart until (it.sourceStart + it.sourceLength)
        } ?: return null
        val index = itemIndexForBlock(content, block.id) ?: return null
        return ScrollTarget(index, 0)
    }

    fun sourceOffsetTarget(content: NoteContent, offset: Int): ScrollTarget? {
        content.items.firstOrNull {
            offset >= it.sourceStart && offset < it.sourceStart + it.sourceLength
        }?.let { item ->
            val index = content.items.indexOf(item)
            if (index >= 0) return ScrollTarget(index, 0)
        }
        val heading = content.outline.headings.lastOrNull { it.start.toInt() <= offset }
        if (heading != null) {
            val block = content.model.blocks.firstOrNull {
                heading.start.toInt() >= it.sourceStart &&
                    heading.start.toInt() < it.sourceStart + it.sourceLength
            }
            if (block != null) {
                val index = itemIndexForBlock(content, block.id)
                if (index != null) return ScrollTarget(index, 0)
            }
        }
        return null
    }

    private fun itemIndexForBlock(content: NoteContent, blockId: Long): Int? {
        val index = content.items.indexOfFirst { it.blockId == blockId }
        return index.takeIf { it >= 0 }
    }

    private fun slug(value: String): String = value.trim().lowercase().replace(' ', '-')
}
