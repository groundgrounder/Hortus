// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader.state

import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.model.DiagramRenderResult
import io.github.groundgrounder.hortus.core.model.EmbeddedContent
import io.github.groundgrounder.hortus.core.model.LoadedImage
import io.github.groundgrounder.hortus.data.DiagramRenderService
import io.github.groundgrounder.hortus.data.DocumentRepository
import io.github.groundgrounder.hortus.data.EmbedRepository
import io.github.groundgrounder.hortus.data.ImageLoader
import io.github.groundgrounder.hortus.data.MathRenderService
import io.github.groundgrounder.hortus.reader.model.ItemRenderBridge
import io.github.groundgrounder.hortus.reader.render.MathRenderResult

/**
 * 将 UI 块级渲染请求转发至数据/渲染服务，隔离协程调度与仓储锁细节。
 */
internal class ReaderRenderBridge(
    private val mathService: MathRenderService,
    private val diagramService: DiagramRenderService,
    private val embedRepository: EmbedRepository,
    private val documentRepository: DocumentRepository,
) : ItemRenderBridge {
    override suspend fun prepareMath(
        formulas: List<MathBlock>,
        bodySize: Float,
    ): List<MathRenderResult> =
        mathService.prepareMath(formulas, bodySize, documentRepository.currentPath ?: "")

    override suspend fun loadImage(
        path: String,
        targetWidthPx: Int,
        requestedWidthPx: Int?,
    ): LoadedImage = ImageLoader.load(path, targetWidthPx, requestedWidthPx)

    override suspend fun renderDiagram(
        vaultPath: String,
        documentKey: String,
        engineText: String,
        mmd: Boolean,
        ordinal: Int,
        width: Double,
        dark: Boolean,
        collapsed: List<String>,
        revision: Long,
        previewContentWidthPx: Int,
        previewUnitScale: Float,
    ): DiagramRenderResult = diagramService.renderDiagram(
        vaultPath = vaultPath,
        documentKey = documentKey,
        engineText = engineText,
        mmd = mmd,
        ordinal = ordinal,
        width = width,
        dark = dark,
        collapsed = collapsed,
        revision = revision,
        previewContentWidthPx = previewContentWidthPx,
        previewUnitScale = previewUnitScale,
    )

    override suspend fun collapsedNodes(vaultPath: String, note: String, block: Int): List<String> =
        diagramService.collapsedNodes(vaultPath, note, block)

    override suspend fun resolveEmbed(
        sourcePath: String,
        target: String,
        heading: String?,
        alias: String?,
        ancestors: List<String>,
        width: Double,
        size: Double,
    ): EmbeddedContent = embedRepository.resolveEmbed(
        sourcePath = sourcePath,
        target = target,
        heading = heading,
        alias = alias,
        ancestors = ancestors,
        width = width,
        size = size,
    )
}
