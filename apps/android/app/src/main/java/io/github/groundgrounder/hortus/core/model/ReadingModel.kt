// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.core.model

import io.github.groundgrounder.hortus.core.ReadingModel
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * 阅读模型的 Kotlin 侧视图。块与运行段偏移均为 UTF-16 单元，正好对应 Kotlin 字符串下标。
 * 同时接受打包格式 v1（64/40 字节记录）和 v2（36/20 字节紧凑记录）。
 */
internal data class ReadingRun(
    val displayStart: Int,
    val displayEnd: Int,
    val sourceStart: Int,
    val sourceEnd: Int,
    val mask: Int,
) {
    val displayLength: Int get() = displayEnd - displayStart
    val sourceLength: Int get() = sourceEnd - sourceStart
}

internal data class ReadingBlock(
    val id: Long,
    val sourceStart: Int,
    val sourceLength: Int,
    val displayStart: Int,
    val displayLength: Int,
    val runs: List<ReadingRun>,
    val diagram: Boolean,
)

internal data class DecodedReadingModel(
    val revision: Long,
    val rebuilt: Long,
    val text: String,
    val blocks: List<ReadingBlock>,
    val formatVersion: Int,
) {
    val blockCount: Int get() = blocks.size
    val runCount: Int get() = blocks.sumOf { it.runs.size }

    fun blockText(block: ReadingBlock): String {
        val start = block.displayStart.coerceIn(0, text.length)
        val end = (block.displayStart + block.displayLength).coerceIn(start, text.length)
        return text.substring(start, end)
    }
}

internal object ReadingModelDecoder {
    private const val NO_CHANGE = -2L
    private const val FULL = -1L

    private fun ByteBuffer.readHeader(): Pair<Int, Int> {
        val version = int
        val count = int
        require(version == 1 || version == 2) { "unsupported reading model version $version" }
        require(count >= 0) { "negative reading model count" }
        return version to count
    }

    private fun ByteBuffer.readRuns(version: Int, count: Int): List<ReadingRun> {
        val stride = if (version == 2) 20 else 40
        require(remaining() >= count * stride) { "truncated reading runs" }
        return List(count) {
            if (version == 2) {
                val displayStart = int
                val displayLength = int
                val sourceStart = int
                val sourceLength = int
                val mask = int
                require(displayStart >= 0 && displayLength >= 0 && sourceStart >= 0 && sourceLength >= 0)
                ReadingRun(
                    displayStart,
                    displayStart + displayLength,
                    sourceStart,
                    sourceStart + sourceLength,
                    mask,
                )
            } else {
                val displayStart = long
                val displayLength = long
                val sourceStart = long
                val sourceLength = long
                val mask = long
                require(
                    displayStart in 0..Int.MAX_VALUE.toLong() &&
                        displayLength in 0..Int.MAX_VALUE.toLong() &&
                        sourceStart in 0..Int.MAX_VALUE.toLong() &&
                        sourceLength in 0..Int.MAX_VALUE.toLong(),
                ) { "reading run does not fit Android offsets" }
                ReadingRun(
                    displayStart.toInt(),
                    (displayStart + displayLength).toInt(),
                    sourceStart.toInt(),
                    (sourceStart + sourceLength).toInt(),
                    mask.toInt(),
                )
            }
        }
    }

    fun decodeFull(wire: ReadingModel): DecodedReadingModel {
        val blockBuffer = ByteBuffer.wrap(wire.packedBlocks).order(ByteOrder.LITTLE_ENDIAN)
        val (blockVersion, blockCount) = blockBuffer.readHeader()
        val blockStride = if (blockVersion == 2) 36 else 64
        require(blockBuffer.remaining() >= blockCount * blockStride) { "truncated reading blocks" }
        val runBuffer = ByteBuffer.wrap(wire.packedRuns).order(ByteOrder.LITTLE_ENDIAN)
        val (runVersion, runCount) = runBuffer.readHeader()
        require(runVersion == blockVersion) { "reading block/run versions differ" }
        val allRuns = runBuffer.readRuns(runVersion, runCount)
        require(runBuffer.remaining() == 0) { "trailing reading run bytes" }
        val blocks = ArrayList<ReadingBlock>(blockCount)
        var runOffset = 0
        repeat(blockCount) {
            val id: Long
            val sourceStart: Int
            val sourceLength: Int
            val displayStart: Int
            val displayLength: Int
            val count: Int
            val diagram: Boolean
            if (blockVersion == 2) {
                id = blockBuffer.long
                sourceStart = blockBuffer.int
                sourceLength = blockBuffer.int
                displayStart = blockBuffer.int
                displayLength = blockBuffer.int
                val offset = blockBuffer.int
                count = blockBuffer.int
                diagram = blockBuffer.int != 0
                require(offset == runOffset) { "reading run offsets are not contiguous" }
            } else {
                id = blockBuffer.long
                sourceStart = longToInt(blockBuffer.long, "block source start")
                sourceLength = longToInt(blockBuffer.long, "block source length")
                displayStart = longToInt(blockBuffer.long, "block display start")
                displayLength = longToInt(blockBuffer.long, "block display length")
                val offset = longToInt(blockBuffer.long, "block run offset")
                count = longToInt(blockBuffer.long, "block run count")
                diagram = blockBuffer.long != 0L
                require(offset == runOffset) { "reading run offsets are not contiguous" }
            }
            require(sourceStart >= 0 && sourceLength >= 0 && displayStart >= 0 && displayLength >= 0)
            require(count >= 0 && runOffset + count <= allRuns.size) { "reading run range is invalid" }
            blocks += ReadingBlock(
                id = id,
                sourceStart = sourceStart,
                sourceLength = sourceLength,
                displayStart = displayStart,
                displayLength = displayLength,
                runs = allRuns.subList(runOffset, runOffset + count).toList(),
                diagram = diagram,
            )
            runOffset += count
        }
        require(runOffset == allRuns.size) { "unused reading runs" }
        require(blockBuffer.remaining() == 0) { "trailing reading block bytes" }
        require(wire.text.length >= blocks.lastOrNull()?.let { it.displayStart + it.displayLength } ?: 0) {
            "reading model text is shorter than its blocks"
        }
        return DecodedReadingModel(
            revision = wire.revision.toLong(),
            rebuilt = wire.rebuilt.toLong(),
            text = wire.text,
            blocks = blocks,
            formatVersion = blockVersion,
        )
    }

    fun isFull(wire: ReadingModel): Boolean = wire.deltaIndex.toLong() == FULL

    fun isEmptyDelta(wire: ReadingModel): Boolean = wire.deltaIndex.toLong() == NO_CHANGE

    fun applyDelta(base: DecodedReadingModel, wire: ReadingModel): DecodedReadingModel {
        val index = wire.deltaIndex.toLong()
        require(index in 0..Int.MAX_VALUE.toLong()) { "invalid reading delta index" }
        val at = index.toInt()
        require(at in base.blocks.indices) { "reading delta index is out of range" }
        val old = base.blocks[at]
        val runBuffer = ByteBuffer.wrap(wire.packedRuns).order(ByteOrder.LITTLE_ENDIAN)
        val (version, count) = runBuffer.readHeader()
        val runs = runBuffer.readRuns(version, count)
        require(runBuffer.remaining() == 0) { "trailing delta run bytes" }
        val newDisplayStart = wire.displayStart.toInt()
        val newDisplayLength = wire.displayLength.toInt()
        require(newDisplayStart >= 0 && newDisplayLength >= 0)
        val block = ReadingBlock(
            id = wire.blockId.toLong(),
            sourceStart = wire.sourceStart.toInt(),
            sourceLength = wire.sourceLength.toInt(),
            displayStart = newDisplayStart,
            displayLength = newDisplayLength,
            runs = runs,
            diagram = wire.diagram,
        )
        require(block.sourceStart >= 0 && block.sourceLength >= 0)
        val oldEnd = (old.displayStart + old.displayLength).coerceAtMost(base.text.length)
        val prefix = base.text.substring(0, old.displayStart.coerceIn(0, oldEnd))
        val suffix = base.text.substring(oldEnd)
        val text = prefix + wire.text + suffix
        val oldSourceEnd = old.sourceStart + old.sourceLength
        val newSourceEnd = block.sourceStart + block.sourceLength
        val sourceShift = newSourceEnd - oldSourceEnd
        val displayShift = newDisplayLength - old.displayLength
        val blocks = ArrayList<ReadingBlock>(base.blocks.size)
        for ((position, value) in base.blocks.withIndex()) {
            when {
                position < at -> blocks += value
                position == at -> blocks += block
                else -> blocks += value.copy(
                    displayStart = value.displayStart + displayShift,
                    sourceStart = (value.sourceStart + sourceShift).coerceAtLeast(0),
                )
            }
        }
        return DecodedReadingModel(
            revision = wire.revision.toLong(),
            rebuilt = wire.rebuilt.toLong(),
            text = text,
            blocks = blocks,
            formatVersion = base.formatVersion,
        )
    }

    private fun longToInt(value: Long, label: String): Int {
        require(value in 0..Int.MAX_VALUE.toLong()) { "$label does not fit Android offsets" }
        return value.toInt()
    }
}
