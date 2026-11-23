// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import io.github.groundgrounder.hortus.MainActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

private fun occurrences(text: String, needle: String): Int {
    if (needle.isEmpty()) return 0
    var count = 0
    var index = 0
    while (true) {
        val found = text.indexOf(needle, index)
        if (found < 0) break
        count += 1
        index = found + needle.length
    }
    return count
}

internal object DebugInspection {
    suspend fun write(activity: MainActivity, name: String, waitMs: Long, expectedNote: String? = null) {
        runCatching {
            withContext(Dispatchers.IO) {
                val directory = checkNotNull(activity.getExternalFilesDir(null))
                val early = build(activity).put("early", true)
                File(directory, "early-$name").writeText(early.toString(2) + "\n")
                    .also { File(directory, "early-$name").setReadable(true, false) }
            }
        }
        var waited = 0L
        while (waited < 12_000) {
            val current = activity.controller.state.content
            if (current != null && (expectedNote == null || current.path == expectedNote) &&
                !activity.controller.state.loading
            ) {
                break
            }
            delay(100)
            waited += 100
        }
        if (waitMs > 0) delay(waitMs)
        prepareAll(activity)
        val report = build(activity)
        withContext(Dispatchers.IO) {
            val directory = checkNotNull(activity.getExternalFilesDir(null))
            val file = File(directory, name)
            val temporary = File(directory, ".$name.tmp")
            temporary.writeText(report.toString(2) + "\n")
            temporary.setReadable(true, false)
            java.nio.file.Files.move(
                temporary.toPath(),
                file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING,
            )
        }
    }

    private suspend fun prepareAll(activity: MainActivity) {
        val content = activity.controller.state.content ?: return
        val repository = activity.controller.repository()
        val dark = (activity.resources.configuration.uiMode and
            android.content.res.Configuration.UI_MODE_NIGHT_MASK) ==
            android.content.res.Configuration.UI_MODE_NIGHT_YES
        withContext(Dispatchers.IO) {
            kotlinx.coroutines.withTimeoutOrNull(30_000) {
                repository.prepareMath(content.math, content.bodySize)
                repository.prepareMath(
                    content.media.flatMap { media -> media.cells.flatMap { it.math } },
                    content.bodySize,
                )
                for (media in content.media.filter { it.columns.isEmpty() && it.imagePath.isNotBlank() }) {
                    ImageLoader.load(media.imagePath, content.width, media.imageWidth?.toInt())
                }
                for (item in content.diagramItems) {
                    val collapsed = repository.collapsedNodes(item.documentKey, item.ordinal)
                    repository.renderDiagram(
                        documentKey = item.documentKey,
                        engineText = item.engineText,
                        mmd = item.mmd,
                        ordinal = item.ordinal,
                        width = content.width.toDouble(),
                        dark = dark,
                        collapsed = collapsed,
                        revision = content.revision,
                    )
                }
                val ancestors = listOf(content.path)
                for (embed in content.items.filterIsInstance<EmbedItem>()) {
                    val resolved = repository.resolveEmbed(
                        sourcePath = content.path,
                        target = embed.target,
                        heading = embed.heading,
                        alias = embed.alias,
                        ancestors = ancestors,
                        width = content.width.toDouble(),
                        size = content.bodySize.toDouble(),
                    )
                    repository.prepareMath(resolved.math, content.bodySize)
                    if (resolved.info.kind == "diagram") {
                        repository.renderDiagram(
                            documentKey = resolved.info.resolvedPath ?: embed.target,
                            engineText = resolved.info.content,
                            mmd = resolved.info.isMmd,
                            ordinal = 1,
                            width = content.width.toDouble(),
                            dark = dark,
                            collapsed = emptyList(),
                            revision = 1,
                        )
                    }
                    if (resolved.info.kind == "note") {
                        for (diagram in resolved.items.filterIsInstance<DiagramItem>()) {
                            repository.renderDiagram(
                                documentKey = diagram.documentKey,
                                engineText = diagram.engineText,
                                mmd = diagram.mmd,
                                ordinal = diagram.ordinal,
                                width = content.width.toDouble(),
                                dark = dark,
                                collapsed = emptyList(),
                                revision = 1,
                            )
                        }
                    }
                }
            }
        }
    }

    fun build(activity: MainActivity): JSONObject {
        val controller = activity.controller
        val state = controller.state
        val content = state.content
        val report = JSONObject()
            .put("capturedAt", java.time.Instant.now().toString())
            .put("debug", true)
            .put("vault", state.vaultPath)
            .put("note", content?.path)
            .put("loading", state.loading)
            .put("error", state.error)
            .put("lastAction", controller.lastAction)
            .put("lastOpenTarget", controller.lastOpenTarget?.let {
                JSONObject().put("itemIndex", it.itemIndex).put("offsetPx", it.offsetPx)
            })
            .put("contentWidth", controller.contentWidthPx)
            .put("bodySizePx", controller.bodySizePx)
            .put("firstVisibleBlockId", controller.lastVisibleBlockId)
            .put("firstVisibleOffsetPx", controller.lastVisibleOffsetPx)
            .put("openMs", if (ReaderTiming.contentInstalledAt > 0) ReaderTiming.contentInstalledAt - ReaderTiming.openStartedAt else -1)
            .put("firstFrameMs", if (ReaderTiming.firstFrameAt > 0) ReaderTiming.firstFrameAt - ReaderTiming.openStartedAt else -1)
            .put("openMaxFrameMs", OpenFrameMonitor.maxFrameMs.get())
            .put("rawOpenMaxFrameMs", OpenFrameMonitor.rawMaxFrameMs.get())
            .put("appOpenMs", if (ReaderTiming.contentInstalledAt > 0 && ReaderTiming.activityCreatedAt > 0) ReaderTiming.contentInstalledAt - ReaderTiming.activityCreatedAt else -1)
            .put("appFirstFrameMs", if (ReaderTiming.firstFrameAt > 0 && ReaderTiming.activityCreatedAt > 0) ReaderTiming.firstFrameAt - ReaderTiming.activityCreatedAt else -1)
            .put("mainThread", JSONObject(MainThreadCounters.snapshot() as Map<*, *>))
            .put("composedDiagramCanvases", Inspection.composedDiagramCanvases.get())
            .put("maxComposedDiagramCanvases", Inspection.maxComposedDiagramCanvases.get())
            .put("visibleDiagramCanvases", Inspection.visibleDiagramCanvases.get())
            .put("maxVisibleDiagramCanvases", Inspection.maxVisibleDiagramCanvases.get())
            .put("focus", JSONObject()
                .put("scale", FocusStateRegistry.lastScale)
                .put("offsetX", FocusStateRegistry.lastOffsetX)
                .put("offsetY", FocusStateRegistry.lastOffsetY)
                .put("sceneWidth", FocusStateRegistry.sceneWidth)
                .put("sceneHeight", FocusStateRegistry.sceneHeight)
                .put("largestFontPx", FocusStateRegistry.largestFontPx))
            .put("interactive", JSONArray().also { array ->
                HitRegistry.snapshot().forEach { hit ->
                    array.put(JSONObject()
                        .put("kind", hit.kind)
                        .put("label", hit.label)
                        .put("x", hit.x)
                        .put("y", hit.y)
                        .put("width", hit.width)
                        .put("height", hit.height)
                        .put("target", hit.target)
                        .put("heading", hit.heading)
                        .put("canCollapse", hit.canCollapse)
                        .put("nodeId", hit.nodeId))
                }
            })
            .put("embeds", Inspection.embedsJson())
            .put("math", Inspection.mathJson())
            .put("mathMetricsCodepoints", Inspection.mathMetrics.get())
            .put("diagramCounts", DiagramInspection.total.toJson())
            .put("diagrams", DiagramInspection.snapshot())
            .put("diagramFailures", Inspection.failuresJson())
            .put("exports", Inspection.exportsJson())
            .put("images", JSONArray().also { array ->
                ImageInspection.snapshot().forEach { (path, image) ->
                    array.put(JSONObject()
                        .put("path", path)
                        .put("reason", image.reason)
                        .put("decoded", image.bitmap != null)
                        .put("width", image.widthPx)
                        .put("height", image.heightPx)
                        .put("intrinsicWidth", image.intrinsicWidth)
                        .put("intrinsicHeight", image.intrinsicHeight))
                }
            })
        val screen = activity.resources.displayMetrics
        report.put("screen", JSONObject()
            .put("width", screen.widthPixels)
            .put("height", screen.heightPixels)
            .put("density", screen.density)
            .put("dark", (activity.resources.configuration.uiMode and
                android.content.res.Configuration.UI_MODE_NIGHT_MASK) ==
                android.content.res.Configuration.UI_MODE_NIGHT_YES))
        if (content == null) return report
        val text = content.model.text
        val markers = JSONObject()
            .put("doubleAsterisk", occurrences(text, "**"))
            .put("wikiLink", occurrences(text, "[["))
            .put("doubleDollar", occurrences(text, "$$"))
            .put("codeFence", occurrences(text, "```"))
            .put("calloutMarker", occurrences(text, "> [!"))
        val headings = JSONObject()
        var strong = 0
        var emphasis = 0
        var strike = 0
        var highlight = 0
        var inlineCode = 0
        var codeBlock = 0
        var quote = 0
        var task = 0
        var rule = 0
        var link = 0
        var unresolved = 0
        var tag = 0
        var footnote = 0
        var frontmatter = 0
        for (item in content.items) {
            if (item is TextItem) {
                if (item.headingLevel > 0) {
                    val key = item.headingLevel.toString()
                    headings.put(key, headings.optInt(key) + 1)
                }
                if (item.quote) quote += 1
                if (item.code) codeBlock += 1
                if (item.rule) rule += 1
                if (item.taskLine) task += 1
                for (run in item.runs) {
                    val mask = run.mask
                    if (mask and MASK_STRONG != 0) { strong += 1 }
                    if (mask and MASK_EMPHASIS != 0) { emphasis += 1 }
                    if (mask and MASK_STRIKE != 0) { strike += 1 }
                    if (mask and MASK_HIGHLIGHT != 0) { highlight += 1 }
                    if (mask and MASK_INLINE_CODE != 0) { inlineCode += 1 }
                    if (mask and MASK_FOOTNOTE != 0) { footnote += 1 }
                    if (mask and MASK_FRONTMATTER != 0) { frontmatter += 1 }
                    if (mask and MASK_TAG != 0) { tag += 1 }
                }
                if (item.runs.any { it.mask and MASK_FRONTMATTER != 0 } &&
                    item.text.contains("tags:", ignoreCase = true)
                ) {
                    tag += 1
                }
                for (value in item.links) {
                    link += 1
                    if (value.resolution != "note" && value.resolution != "attachment") unresolved += 1
                }
            }
        }
        val elements = JSONObject()
            .put("headings", headings)
            .put("strong", strong)
            .put("emphasis", emphasis)
            .put("strike", strike)
            .put("highlight", highlight)
            .put("inlineCode", inlineCode)
            .put("codeBlock", codeBlock)
            .put("quote", quote)
            .put("task", task)
            .put("rule", rule)
            .put("link", link)
            .put("unresolvedLink", unresolved)
            .put("tag", tag)
            .put("footnote", footnote)
            .put("frontmatter", frontmatter)
            .put("callouts", JSONArray().also { array ->
                content.items.filterIsInstance<TextItem>().mapNotNull { it.callout }
                    .distinctBy { it.kind to it.title }
                    .forEach { callout ->
                        array.put(JSONObject().put("kind", callout.kind).put("title", callout.title))
                    }
            })
        val itemTypes = JSONObject()
        for (item in content.items) {
            val key = when (item) {
                is TextItem -> if (item.code) "code" else "text"
                is TableItem -> "table"
                is ImageItem -> "image"
                is MathItem -> "math"
                is DiagramItem -> "diagram"
                is EmbedItem -> "embed"
            }
            itemTypes.put(key, itemTypes.optInt(key) + 1)
        }
        val maskHistogram = JSONObject()
        for (item in content.items) {
            if (item is TextItem) {
                for (run in item.runs) {
                    val key = run.mask.toString()
                    maskHistogram.put(key, maskHistogram.optInt(key) + 1)
                }
            }
        }
        report
            .put("maskHistogram", maskHistogram)
            .put("knowledgeLinks", content.knowledge.links.size)
            .put("knowledgeLinksSample", JSONArray().also { array ->
                content.knowledge.links.take(5).forEach { link ->
                    array.put(JSONObject().put("target", link.target)
                        .put("start", link.startUtf16)
                        .put("length", link.lengthUtf16)
                        .put("kind", link.resolutionKind))
                }
            })
            .put("textRuns", content.items.filterIsInstance<TextItem>().sumOf { it.runs.size })
            .put("model", JSONObject()
                .put("revision", content.revision)
                .put("formatVersion", content.model.formatVersion)
                .put("blocks", content.model.blockCount)
                .put("runs", content.model.runCount)
                .put("textLength", text.length))
            .put("isMmd", content.isMmd)
            .put("readOnlyReason", content.readOnlyReason)
            .put("title", content.title)
            .put("displayText", text)
            .put("displayMarkers", markers)
            .put("elements", elements)
            .put("itemTypes", itemTypes)
            .put("items", JSONArray().also { array ->
                content.items.forEach { item ->
                    array.put(JSONObject()
                        .put("key", item.key)
                        .put("type", when (item) {
                            is TextItem -> "text"
                            is TableItem -> "table"
                            is ImageItem -> "image"
                            is MathItem -> "math"
                            is DiagramItem -> "diagram"
                            is EmbedItem -> "embed"
                        })
                        .put("blockId", item.blockId)
                        .put("sourceStart", item.sourceStart)
                        .put("sourceLength", item.sourceLength))
                }
            })
            .put("tables", JSONArray().also { array ->
                content.media.filter { it.columns.isNotEmpty() }.forEach { table ->
                    array.put(JSONObject()
                        .put("columns", JSONArray(table.columns))
                        .put("alignments", JSONArray(table.alignments.map { it.toInt() }))
                        .put("rows", table.rows)
                        .put("cells", JSONArray().also { cells ->
                            table.cells.forEach { cell ->
                                cells.put(JSONObject()
                                    .put("text", cell.text)
                                    .put("math", JSONArray(cell.math.map { it.latex })))
                            }
                        }))
                }
            })
            .put("media", JSONObject()
                .put("tables", content.media.count { it.columns.isNotEmpty() })
                .put("images", content.media.count { it.columns.isEmpty() })
                .put("imageReasons", JSONArray(content.media.filter { it.columns.isEmpty() && it.imageReason.isNotEmpty() }
                    .map { JSONObject().put("target", it.imageTarget).put("reason", it.imageReason) })))
            .put("outline", JSONArray().also { array ->
                content.outline.headings.forEach { heading ->
                    array.put(JSONObject()
                        .put("level", heading.level.toInt())
                        .put("title", heading.title)
                        .put("start", heading.start))
                }
            })
            .put("diagramItems", JSONArray().also { array ->
                content.diagramItems.forEach { item ->
                    array.put(JSONObject()
                        .put("ordinal", item.ordinal)
                        .put("type", item.diagramType)
                        .put("documentKey", item.documentKey)
                        .put("sourceLength", item.sourceLength)
                        .put("mmd", item.mmd))
                }
            })
        val formulaStatuses = Inspection.mathJson()
        var rendered = 0
        var failed = 0
        var empty = 0
        for (index in 0 until formulaStatuses.length()) {
            val formula = formulaStatuses.getJSONObject(index)
            if (formula.optString("note") != content.path) continue
            if (formula.optBoolean("rendered")) rendered += 1
            else if (formula.optString("reason").isNotEmpty()) failed += 1
            else empty += 1
        }
        report.put("mathStatus", JSONObject()
            .put("rendered", rendered)
            .put("failed", failed)
            .put("empty", empty)
            .put("typographyScale", io.github.groundgrounder.hortus.core.mathTypographyScale()))
        val failedDiagrams = Inspection.failuresJson().length()
        val failedImages = content.media.count {
            it.columns.isEmpty() && it.imagePath.isNotEmpty() && it.imageReason.isNotEmpty()
        }
        report.put("sourceFallbackCount", failed + failedDiagrams + failedImages)
        return report
    }
}
