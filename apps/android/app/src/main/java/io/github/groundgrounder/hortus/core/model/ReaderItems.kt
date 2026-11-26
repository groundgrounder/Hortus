// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.core.model

import io.github.groundgrounder.hortus.core.EmbedBlockInfo
import io.github.groundgrounder.hortus.core.EmbedContentInfo
import io.github.groundgrounder.hortus.core.EmbedDiagramInfo
import io.github.groundgrounder.hortus.core.EmbedRunInfo
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MediaBlock
import io.github.groundgrounder.hortus.core.ResolvedLinkInfo

internal const val MASK_STRONG = 2
internal const val MASK_EMPHASIS = 4
internal const val MASK_STRIKE = 8
internal const val MASK_INLINE_CODE = 16
internal const val MASK_CODE_BLOCK = 32
internal const val MASK_QUOTE = 64
internal const val MASK_LINK = 128
internal const val MASK_TAG = 512
internal const val MASK_HIGHLIGHT = 1024
internal const val MASK_COMMENT = 2048
internal const val MASK_CALLOUT = 4096
internal const val MASK_FOOTNOTE = 8192
internal const val MASK_FRONTMATTER = 16384
internal const val MASK_RULE = 32768
internal const val MASK_FENCED = 65536
internal const val MASK_TASK = 131072
internal const val MASK_EMBED_BIT = 1 shl 18
internal const val MASK_UNRESOLVED_BIT = 1 shl 19

internal data class CalloutInfo(val kind: String, val title: String)

internal data class TextLink(
    val start: Int,
    val end: Int,
    val target: String,
    val heading: String?,
    val alias: String?,
    val resolution: String,
    val resolvedPath: String?,
    val embed: Boolean,
)

internal data class TextTag(val start: Int, val end: Int, val tag: String)

internal sealed interface ReaderItem {
    val key: String
    val blockId: Long
    val sourceStart: Int
    val sourceLength: Int
}

internal data class TextItem(
    override val key: String,
    override val blockId: Long,
    override val sourceStart: Int,
    override val sourceLength: Int,
    val text: String,
    val displayStart: Int,
    val runs: List<ReadingRun>,
    val headingLevel: Int,
    val quote: Boolean,
    val code: Boolean,
    val fenced: Boolean,
    val rule: Boolean,
    val callout: CalloutInfo?,
    val links: List<TextLink>,
    val tags: List<TextTag>,
    val inlineMath: List<MathBlock>,
    val inlineImages: List<MediaBlock>,
    val taskLine: Boolean,
) : ReaderItem

internal data class TableItem(
    override val key: String,
    override val blockId: Long,
    override val sourceStart: Int,
    override val sourceLength: Int,
    val media: MediaBlock,
) : ReaderItem

internal data class ImageItem(
    override val key: String,
    override val blockId: Long,
    override val sourceStart: Int,
    override val sourceLength: Int,
    val media: MediaBlock,
) : ReaderItem

internal data class MathItem(
    override val key: String,
    override val blockId: Long,
    override val sourceStart: Int,
    override val sourceLength: Int,
    val formulas: List<MathBlock>,
) : ReaderItem

internal data class DiagramItem(
    override val key: String,
    override val blockId: Long,
    override val sourceStart: Int,
    override val sourceLength: Int,
    val ordinal: Int,
    val displayOrdinal: Int,
    val source: String,
    val engineText: String,
    val mmd: Boolean,
    val diagramType: String,
    val documentKey: String,
    val isEmbedded: Boolean,
) : ReaderItem

internal data class EmbedItem(
    override val key: String,
    override val blockId: Long,
    override val sourceStart: Int,
    override val sourceLength: Int,
    val target: String,
    val heading: String?,
    val alias: String?,
    val displayLength: Int,
) : ReaderItem

private val calloutPattern =
    Regex("""(?m)^\s*>\s*\[!([A-Za-z][\w-]*)\][+-]?[ \t]*(.*)$""")

internal fun parseCallout(source: String): CalloutInfo? {
    val match = calloutPattern.find(source) ?: return null
    val kind = match.groupValues[1].lowercase()
    return CalloutInfo(kind, match.groupValues[2].trim())
}

/** 把源码 UTF-16 范围映射到显示文本范围；标记被隐藏时可能得到空范围。 */
internal fun displayRangeForSource(block: ReadingBlock, start: Int, length: Int): IntRange? {
    val end = start + length
    var min = Int.MAX_VALUE
    var max = Int.MIN_VALUE
    for (run in block.runs) {
        val runSourceStart = block.sourceStart + run.sourceStart
        val runSourceEnd = block.sourceStart + run.sourceEnd
        if (runSourceEnd <= start || runSourceStart >= end) continue
        val localStart = maxOf(start, runSourceStart) - runSourceStart
        val localEnd = minOf(end, runSourceEnd) - runSourceStart
        val displayStart = block.displayStart + run.displayStart + localStart
        val displayEnd = block.displayStart + run.displayStart + localEnd
        if (displayEnd > displayStart) {
            min = minOf(min, displayStart)
            max = maxOf(max, displayEnd)
        }
    }
    return if (max > min) min until max else null
}

private data class Replacement(
    val start: Int,
    val end: Int,
    val build: (Int) -> ReaderItem,
)

private data class SpanEntry<T>(val value: T, val start: Int, val end: Int)

private fun <T> assignToBlocks(
    blocks: List<ReadingBlock>,
    items: List<T>,
    startOf: (T) -> Int,
    lengthOf: (T) -> Int,
): List<List<T>> {
    if (blocks.isEmpty() || items.isEmpty()) return List(blocks.size) { emptyList() }
    val sorted = items.map { item ->
        val start = startOf(item)
        SpanEntry(item, start, start + maxOf(1, lengthOf(item)))
    }.sortedBy { it.start }
    val blockStarts = IntArray(blocks.size)
    val blockEnds = IntArray(blocks.size)
    for (i in blocks.indices) {
        val block = blocks[i]
        var minStart = block.sourceStart
        var maxEnd = block.sourceStart + block.sourceLength
        for (run in block.runs) {
            minStart = minOf(minStart, block.sourceStart + run.sourceStart)
            maxEnd = maxOf(maxEnd, block.sourceStart + run.sourceEnd)
        }
        blockStarts[i] = minStart
        blockEnds[i] = maxEnd
    }
    val order = blocks.indices.sortedBy { blockStarts[it] }
    val buckets = Array<ArrayList<T>?>(blocks.size) { null }
    val active = ArrayList<SpanEntry<T>>()
    var cursor = 0
    for (blockIndex in order) {
        val bStart = blockStarts[blockIndex]
        val bEnd = blockEnds[blockIndex]
        active.removeAll { it.end <= bStart }
        while (cursor < sorted.size && sorted[cursor].start < bEnd) {
            val entry = sorted[cursor]
            cursor += 1
            if (entry.end > bStart) active.add(entry)
        }
        if (active.isNotEmpty()) {
            val list = ArrayList<T>(active.size)
            for (entry in active) {
                if (entry.start < bEnd && entry.end > bStart) list.add(entry.value)
            }
            if (list.isNotEmpty()) buckets[blockIndex] = list
        }
    }
    return List(blocks.size) { buckets[it] ?: emptyList() }
}

/**
 * 单个阅读块拆成平台条目：文本、表格、图片、块级公式、图与嵌入卡片。
 * 所有偏移先转成块内 UTF-16 下标，切分后文本块保留原始 run 以复用样式。
 */
internal fun buildReaderItems(
    model: DecodedReadingModel,
    sourceText: String,
    media: List<MediaBlock>,
    math: List<MathBlock>,
    knowledge: io.github.groundgrounder.hortus.core.KnowledgeMarkupInfo,
    diagramBlocks: List<io.github.groundgrounder.hortus.core.DiagramBlock>,
    documentKey: String,
    documentIsMmd: Boolean,
): List<ReaderItem> {
    val mediaByBlock = media.groupBy { it.blockId.toLong() }
    val mathByBlock = math.groupBy { it.blockId.toLong() }
    val embedLinks = knowledge.links.filter { it.embed && !it.attachment && !it.inDiagram }
    val embedLinksByBlock = assignToBlocks(model.blocks, embedLinks, { it.startUtf16.toInt() }, { it.lengthUtf16.toInt() })
    val linksByBlock = assignToBlocks(model.blocks, knowledge.links, { it.startUtf16.toInt() }, { it.lengthUtf16.toInt() })
    val tagsByBlock = assignToBlocks(model.blocks, knowledge.tags, { it.startUtf16.toInt() }, { it.lengthUtf16.toInt() })
    val items = ArrayList<ReaderItem>(model.blocks.size)
    var diagramOrdinal = 0
    for ((blockIndex, block) in model.blocks.withIndex()) {
        val blockText = model.blockText(block)
        val blockMedia = mediaByBlock[block.id].orEmpty()
        val blockMath = mathByBlock[block.id].orEmpty()
        val blockEmbedLinks = embedLinksByBlock[blockIndex]
        val blockLinks = linksByBlock[blockIndex]
        val blockTags = tagsByBlock[blockIndex]
        val source = if (block.sourceStart in 0..sourceText.length &&
            block.sourceStart + block.sourceLength <= sourceText.length
        ) {
            sourceText.substring(block.sourceStart, block.sourceStart + block.sourceLength)
        } else {
            ""
        }
        if (block.diagram) {
            diagramOrdinal += 1
            val ordinal = diagramOrdinal
            val info = diagramBlocks.getOrNull(ordinal - 1)
            val type = info?.diagramType?.takeIf { it.isNotBlank() } ?: inferDiagramType(source)
            items += DiagramItem(
                key = "d${block.id}",
                blockId = block.id,
                sourceStart = block.sourceStart,
                sourceLength = block.sourceLength,
                ordinal = ordinal,
                displayOrdinal = ordinal,
                source = source,
                engineText = sourceText,
                mmd = documentIsMmd,
                diagramType = type,
                documentKey = documentKey,
                isEmbedded = false,
            )
            continue
        }
        val replacements = ArrayList<Replacement>()
        for (mediaBlock in blockMedia) {
            val localStart = (mediaBlock.displayStart.toLong() - block.displayStart).toInt()
            if (localStart !in 0 until maxOf(1, block.displayLength)) continue
            val localEnd = (localStart + mediaBlock.displayLength.toInt()).coerceAtMost(block.displayLength)
            if (mediaBlock.columns.isNotEmpty()) {
                replacements += Replacement(localStart, localEnd) { seq ->
                    TableItem(
                        "t${block.id}_$seq",
                        block.id,
                        block.sourceStart,
                        block.sourceLength,
                        mediaBlock,
                    )
                }
            } else if (mediaBlock.imageWiki && mediaBlock.imageReason.startsWith("embed.")) {
                val raw = mediaBlock.imageTarget
                val target = raw.substringBefore('#')
                val heading = raw.substringAfter('#', "").ifBlank { null }
                val alias = mediaBlock.imageAlt.takeIf { it.isNotBlank() && it != raw && it != target }
                replacements += Replacement(localStart, localEnd) { seq ->
                    EmbedItem(
                        "e${block.id}_$seq",
                        block.id,
                        block.sourceStart,
                        block.sourceLength,
                        target,
                        heading,
                        alias,
                        localEnd - localStart,
                    )
                }
            } else {
                replacements += Replacement(localStart, localEnd) { seq ->
                    ImageItem(
                        "i${block.id}_$seq",
                        block.id,
                        block.sourceStart,
                        block.sourceLength,
                        mediaBlock,
                    )
                }
            }
        }
        for (formula in blockMath) {
            if (!formula.block) continue
            val localStart = (formula.displayStart.toLong() - block.displayStart).toInt()
            if (localStart !in 0 until maxOf(1, block.displayLength)) continue
            val localEnd = (localStart + formula.displayLength.toInt()).coerceAtMost(block.displayLength)
            replacements += Replacement(localStart, localEnd) { seq ->
                MathItem(
                    "m${block.id}_$seq",
                    block.id,
                    block.sourceStart,
                    block.sourceLength,
                    listOf(formula),
                )
            }
        }
        for (link in blockEmbedLinks) {
            val range = displayRangeForSource(
                block,
                link.startUtf16.toInt(),
                link.lengthUtf16.toInt(),
            ) ?: continue
            val localStart = range.first - block.displayStart
            val localEnd = range.last + 1 - block.displayStart
            if (localStart < 0 || localEnd > block.displayLength) continue
            replacements += Replacement(localStart, localEnd) { seq ->
                EmbedItem(
                    "e${block.id}_$seq",
                    block.id,
                    block.sourceStart,
                    block.sourceLength,
                    link.target,
                    link.heading,
                    link.alias,
                    localEnd - localStart,
                )
            }
        }
        replacements.sortWith(compareBy<Replacement> { it.start }.thenByDescending { it.end })
        var cursor = 0
        var sequence = 0
        fun addText(start: Int, end: Int) {
            if (end <= start) return
            val raw = blockText.substring(start, end)
            val leading = raw.indexOfFirst { it != '\n' && it != '\r' }.let { if (it < 0) return else it }
            val trailing = raw.indexOfLast { it != '\n' && it != '\r' } + 1
            if (trailing <= leading) return
            val value = raw.substring(leading, trailing)
            if (value.isBlank()) return
            val displayStart = block.displayStart + start + leading
            val displayEnd = block.displayStart + start + trailing
            val runs = block.runs
                .filter { it.displayEnd + block.displayStart > displayStart && it.displayStart + block.displayStart < displayEnd }
                .map {
                    it.copy(
                        displayStart = it.displayStart + block.displayStart,
                        displayEnd = it.displayEnd + block.displayStart,
                    )
                }
            val heading = block.runs.firstOrNull()?.mask?.let { (it ushr 24) and 7 } ?: 0
            val mask = block.runs.fold(0) { acc, run -> acc or run.mask }
            val links = blockLinks.mapNotNull { link ->
                val range = displayRangeForSource(
                    block,
                    link.startUtf16.toInt(),
                    link.lengthUtf16.toInt(),
                ) ?: return@mapNotNull null
                val local = maxOf(range.first, displayStart) - displayStart
                val localEnd = minOf(range.last + 1, displayEnd) - displayStart
                if (localEnd <= local) null else TextLink(
                    start = local,
                    end = localEnd,
                    target = link.target,
                    heading = link.heading,
                    alias = link.alias,
                    resolution = link.resolutionKind,
                    resolvedPath = link.resolvedPath,
                    embed = link.embed,
                )
            }
            val tags = blockTags.mapNotNull { tag ->
                val range = displayRangeForSource(
                    block,
                    tag.startUtf16.toInt(),
                    tag.lengthUtf16.toInt(),
                ) ?: return@mapNotNull null
                val local = maxOf(range.first, displayStart) - displayStart
                val localEnd = minOf(range.last + 1, displayEnd) - displayStart
                if (localEnd <= local) null else TextTag(local, localEnd, tag.tag)
            }
            val inline = blockMath.filter {
                !it.block && it.displayStart.toLong() in displayStart.toLong() until displayEnd.toLong()
            }
            val images = blockMedia.filter {
                it.columns.isEmpty() &&
                    it.displayStart.toLong() in displayStart.toLong() until displayEnd.toLong()
            }
            items += TextItem(
                key = "x${block.id}_$sequence",
                blockId = block.id,
                sourceStart = block.sourceStart,
                sourceLength = block.sourceLength,
                text = value,
                displayStart = displayStart,
                runs = runs,
                headingLevel = heading,
                quote = mask and MASK_QUOTE != 0,
                code = mask and (MASK_CODE_BLOCK or MASK_FENCED) != 0,
                fenced = mask and MASK_FENCED != 0,
                rule = mask and MASK_RULE != 0 && value.replace("\u200b", "").isBlank(),
                callout = parseCallout(source),
                links = links,
                tags = tags,
                inlineMath = inline,
                inlineImages = images,
                taskLine = value.trimStart().startsWith("☐") || value.trimStart().startsWith("☑"),
            )
            sequence += 1
        }
        for (replacement in replacements) {
            if (replacement.start < cursor) continue
            addText(cursor, replacement.start)
            items += replacement.build(sequence)
            sequence += 1
            cursor = replacement.end
        }
        addText(cursor, blockText.length)
    }
    return items
}

internal fun inferDiagramType(source: String): String {
    val line = source.lineSequence().map { it.trim() }.firstOrNull { it.isNotEmpty() } ?: return ""
    return line.substringBefore(' ').substringBefore('(').lowercase()
}

internal data class EmbeddedContent(
    val info: EmbedContentInfo,
    val items: List<ReaderItem>,
    val media: List<MediaBlock>,
    val math: List<MathBlock>,
)

internal fun buildEmbeddedItems(
    info: EmbedContentInfo,
    parentDocumentKey: String,
): EmbeddedContent {
    if (info.kind != "note" && info.kind != "diagram") {
        return EmbeddedContent(info, emptyList(), emptyList(), emptyList())
    }
    if (info.kind == "diagram") {
        return EmbeddedContent(
            info,
            emptyList(),
            info.media,
            info.math,
        )
    }
    val blocks = info.blocks.map { embedded ->
        ReadingBlock(
            id = embedded.id.toLong(),
            sourceStart = embedded.sourceStartUtf16.toInt(),
            sourceLength = embedded.sourceLengthUtf16.toInt(),
            displayStart = embedded.displayStart.toInt(),
            displayLength = embedded.displayLength.toInt(),
            runs = embedded.runs.map(::embeddedRun),
            diagram = embedded.diagram,
        )
    }
    val model = DecodedReadingModel(
        revision = 0,
        rebuilt = 0,
        text = info.text,
        blocks = blocks,
        formatVersion = 2,
    )
    val knowledge = io.github.groundgrounder.hortus.core.KnowledgeMarkupInfo(
        links = info.links,
        tags = info.tags,
    )
    val diagrams = info.diagrams.map { diagram ->
        io.github.groundgrounder.hortus.core.DiagramBlock(
            ordinal = diagram.ordinal,
            fenceStart = 0u,
            fenceEnd = 0u,
            sourceStart = diagram.startUtf16,
            sourceEnd = diagram.startUtf16 + diagram.lengthUtf16,
            diagramType = "",
        )
    }
    val items = buildReaderItems(
        model = model,
        sourceText = info.content,
        media = info.media,
        math = info.math,
        knowledge = knowledge,
        diagramBlocks = diagrams,
        documentKey = parentDocumentKey,
        documentIsMmd = info.isMmd,
    )
    return EmbeddedContent(info, items, info.media, info.math)
}

private fun embeddedRun(run: EmbedRunInfo): ReadingRun = ReadingRun(
    displayStart = run.start.toInt(),
    displayEnd = (run.start + run.length).toInt(),
    sourceStart = run.sourceStartUtf16.toInt(),
    sourceEnd = (run.sourceStartUtf16 + run.sourceLengthUtf16).toInt(),
    mask = run.mask.toInt(),
)

internal fun embedBlockSource(
    info: EmbedContentInfo,
    block: EmbedBlockInfo,
): String {
    if (block.diagram) {
        return info.content
    }
    val start = block.sourceStartUtf16.toInt()
    val end = start + block.sourceLengthUtf16.toInt()
    return if (start in 0..info.content.length && end <= info.content.length) {
        info.content.substring(start, end)
    } else {
        ""
    }
}

internal fun embedDiagramType(info: EmbedContentInfo, diagram: EmbedDiagramInfo?): String {
    if (diagram == null) return if (info.isMmd) inferDiagramType(info.content) else ""
    val start = diagram.startUtf16.toInt()
    val end = start + diagram.lengthUtf16.toInt()
    val source = if (start in 0..info.content.length && end <= info.content.length) {
        info.content.substring(start, end)
    } else {
        info.content
    }
    return inferDiagramType(source)
}

@Suppress("unused")
internal fun ResolvedLinkInfo.isBroken(): Boolean = resolvedPath == null && resolutionKind != "attachment"
