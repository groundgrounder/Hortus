// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.reader

import io.github.groundgrounder.hortus.MainActivity
import io.github.groundgrounder.hortus.core.mathTypographyScale
import io.github.groundgrounder.hortus.core.model.DiagramItem
import io.github.groundgrounder.hortus.core.model.EmbedItem
import io.github.groundgrounder.hortus.core.model.ImageItem
import io.github.groundgrounder.hortus.core.model.MASK_EMPHASIS
import io.github.groundgrounder.hortus.core.model.MASK_FOOTNOTE
import io.github.groundgrounder.hortus.core.model.MASK_FRONTMATTER
import io.github.groundgrounder.hortus.core.model.MASK_HIGHLIGHT
import io.github.groundgrounder.hortus.core.model.MASK_INLINE_CODE
import io.github.groundgrounder.hortus.core.model.MASK_STRIKE
import io.github.groundgrounder.hortus.core.model.MASK_STRONG
import io.github.groundgrounder.hortus.core.model.MASK_TAG
import io.github.groundgrounder.hortus.core.model.MathItem
import io.github.groundgrounder.hortus.core.model.TableItem
import io.github.groundgrounder.hortus.core.model.TextItem
import io.github.groundgrounder.hortus.data.ImageLoader
import io.github.groundgrounder.hortus.reader.render.DiagramPreviewCache
import io.github.groundgrounder.hortus.reader.render.diagramUnitScale
import io.github.groundgrounder.hortus.reader.ui.snippetDisplay
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
    suspend fun write(
        activity: MainActivity,
        name: String,
        waitMs: Long,
        expectedNote: String? = null,
        expectedVault: String? = null,
        minSequence: Long = 0L,
        openJob: kotlinx.coroutines.Job? = null,
    ) {
        runCatching {
            withContext(Dispatchers.IO) {
                val directory = checkNotNull(activity.getExternalFilesDir(null))
                val early = build(activity).put("early", true)
                File(directory, "early-$name").writeText(early.toString(2) + "\n")
                    .also { File(directory, "early-$name").setReadable(true, false) }
            }
        }
        openJob?.join()
        var waited = 0L
        while (waited < 12_000) {
            val state = activity.viewModel.readerState.value
            val current = state.content
            if (current != null &&
                !state.loading &&
                (expectedVault == null || state.vaultPath == expectedVault) &&
                (expectedNote == null || current.path == expectedNote) &&
                state.openSequence >= minSequence
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
        val viewModel = activity.viewModel
        val content = viewModel.readerState.value.content ?: return
        val renderBridge = viewModel.renderBridge
        val systemDark = (activity.resources.configuration.uiMode and
            android.content.res.Configuration.UI_MODE_NIGHT_MASK) ==
            android.content.res.Configuration.UI_MODE_NIGHT_YES
        val dark = when (viewModel.appSettings.value.themeMode) {
            io.github.groundgrounder.hortus.core.settings.ThemeMode.SYSTEM -> systemDark
            io.github.groundgrounder.hortus.core.settings.ThemeMode.LIGHT -> false
            io.github.groundgrounder.hortus.core.settings.ThemeMode.DARK -> true
        }
        val density = activity.resources.displayMetrics.density
        val unitScale = diagramUnitScale(content.bodySize, density)
        val mainLayoutWidth = (content.width / unitScale).toDouble()
        val innerWidth = (content.width - (24 * density).toInt()).coerceAtLeast(1)
        val innerBodySize = content.bodySize * 0.94f
        val innerUnitScale = diagramUnitScale(innerBodySize, density)
        val innerLayoutWidth = (innerWidth / innerUnitScale).toDouble()
        withContext(Dispatchers.IO) {
            kotlinx.coroutines.withTimeoutOrNull(30_000) {
                renderBridge.prepareMath(content.math, content.bodySize)
                renderBridge.prepareMath(
                    content.media.flatMap { media -> media.cells.flatMap { it.math } },
                    content.bodySize,
                )
                for (media in content.media.filter { it.columns.isEmpty() && it.imagePath.isNotBlank() }) {
                    ImageLoader.load(media.imagePath, content.width, media.imageWidth?.toInt())
                }
                for (item in content.diagramItems) {
                    val collapsed = renderBridge.collapsedNodes(content.vaultPath, item.documentKey, item.ordinal)
                    renderBridge.renderDiagram(
                        vaultPath = content.vaultPath,
                        documentKey = item.documentKey,
                        engineText = item.engineText,
                        mmd = item.mmd,
                        ordinal = item.ordinal,
                        width = mainLayoutWidth,
                        dark = dark,
                        collapsed = collapsed,
                        revision = content.revision,
                        previewContentWidthPx = content.width,
                        previewUnitScale = unitScale,
                    )
                }
                val ancestors = listOf(content.path)
                for (embed in content.items.filterIsInstance<EmbedItem>()) {
                    val resolved = renderBridge.resolveEmbed(
                        sourcePath = content.path,
                        target = embed.target,
                        heading = embed.heading,
                        alias = embed.alias,
                        ancestors = ancestors,
                        width = innerWidth.toDouble(),
                        size = innerBodySize.toDouble(),
                    )
                    renderBridge.prepareMath(resolved.math, innerBodySize)
                    if (resolved.info.kind == "diagram") {
                        renderBridge.renderDiagram(
                            vaultPath = content.vaultPath,
                            documentKey = resolved.info.resolvedPath ?: embed.target,
                            engineText = resolved.info.content,
                            mmd = resolved.info.isMmd,
                            ordinal = 1,
                            width = innerLayoutWidth,
                            dark = dark,
                            collapsed = emptyList(),
                            revision = content.revision,
                            previewContentWidthPx = innerWidth,
                            previewUnitScale = innerUnitScale,
                        )
                    }
                    if (resolved.info.kind == "note") {
                        for (diagram in resolved.items.filterIsInstance<DiagramItem>()) {
                            renderBridge.renderDiagram(
                                vaultPath = content.vaultPath,
                                documentKey = diagram.documentKey,
                                engineText = diagram.engineText,
                                mmd = diagram.mmd,
                                ordinal = diagram.ordinal,
                                width = innerLayoutWidth,
                                dark = dark,
                                collapsed = emptyList(),
                                revision = content.revision,
                                previewContentWidthPx = innerWidth,
                                previewUnitScale = innerUnitScale,
                            )
                        }
                    }
                }
            }
        }
    }

    fun build(activity: MainActivity): JSONObject {
        val viewModel = activity.viewModel
        val state = viewModel.readerState.value
        val searchState = viewModel.searchState.value
        val knowledgeState = viewModel.knowledgeState.value
        val editState = viewModel.editState.value
        val newNoteState = viewModel.newNoteState.value
        val appSettings = viewModel.appSettings.value
        val systemDark = (activity.resources.configuration.uiMode and
            android.content.res.Configuration.UI_MODE_NIGHT_MASK) ==
            android.content.res.Configuration.UI_MODE_NIGHT_YES
        val resolvedDark = when (appSettings.themeMode) {
            io.github.groundgrounder.hortus.core.settings.ThemeMode.SYSTEM -> systemDark
            io.github.groundgrounder.hortus.core.settings.ThemeMode.LIGHT -> false
            io.github.groundgrounder.hortus.core.settings.ThemeMode.DARK -> true
        }
        val settingsFile = File(File(activity.filesDir, "config"), "android-settings.json")
        val diagramExportsDir = File(activity.cacheDir, "diagram-exports")
        val externalPreviewDir = File(activity.cacheDir, "external-preview")
        val content = state.content
        val report = JSONObject()
            .put("capturedAt", java.time.Instant.now().toString())
            .put("debug", true)
            .put("vault", state.vaultPath)
            .put("note", content?.path)
            .put("loading", state.loading)
            .put("error", state.error)
            .put("lastAction", DebugSessionMetrics.lastAction)
            .put("settings", JSONObject()
                .put("themeMode", appSettings.themeMode.serialized)
                .put("fontSizeSp", appSettings.fontSizeSp.toDouble())
                .put("bodySizePx", viewModel.bodySizePx.value.toDouble())
                .put("systemDark", systemDark)
                .put("resolvedDark", resolvedDark)
                .put("settingsOpen", viewModel.settingsOpen.value)
                .put("settingsFileExists", settingsFile.isFile)
                .put("settingsFileText", settingsFile.takeIf { it.isFile }?.readText())
                .put("cacheDiagramExportsCount", diagramExportsDir.listFiles()?.size ?: 0)
                .put("cacheExternalPreviewCount", externalPreviewDir.listFiles()?.size ?: 0)
                .put("diagramPreviewCacheSize", DiagramPreviewCache.size()))
            .put("lastOpenTarget", DebugSessionMetrics.lastOpenTarget?.let {
                JSONObject().put("itemIndex", it.itemIndex).put("offsetPx", it.offsetPx)
            })
            .put("contentWidth", viewModel.contentWidthPx)
            .put("bodySizePx", viewModel.bodySizePx.value)
            .put("layoutMode", DebugSessionMetrics.layoutMode)
            .put("sidePaneWidthPx", DebugSessionMetrics.sidePaneWidthPx)
            .put("detailPaneWidthPx", DebugSessionMetrics.detailPaneWidthPx)
            .put("sidePaneOpen", viewModel.sidePaneOpen.value)
            .put("sideMode", viewModel.sideMode.value.name.lowercase())
            .put("externalOpenMode", DebugSessionMetrics.externalMode)
            .put("canEdit", viewModel.canEdit)
            .put("search", JSONObject()
                .put("open", searchState.open)
                .put("tab", searchState.tab.name.lowercase())
                .put("query", searchState.query)
                .put("running", searchState.running)
                .put("quickCount", searchState.quick.size)
                .put("fullCount", searchState.full.size)
                .put("quickPaths", JSONArray(searchState.quick.map { it.path }))
                .put("fullNotes", JSONArray(searchState.full.map {
                    JSONObject()
                        .put("path", it.path)
                        .put("count", it.count)
                        .put("snippets", it.matches.size)
                        .put("matches", JSONArray(it.matches.map { match ->
                            val localStart = (match.start - match.snippetStart).coerceAtLeast(0)
                            val localEnd = (localStart + match.length).coerceAtMost(match.snippet.length)
                            val display = snippetDisplay(match.snippet, localStart, localEnd)
                            JSONObject()
                                .put("start", match.start)
                                .put("length", match.length)
                                .put("snippetStart", match.snippetStart)
                                .put("snippet", match.snippet)
                                .put("display", display.text)
                                .put("highlightStart", display.start)
                                .put("highlightEnd", display.end)
                        }))
                }))
                .put("generation", searchState.generation)
                .put("cancelled", searchState.cancelled)
                .put("staleDiscarded", searchState.staleDiscarded)
                .put("summary", DebugSessionMetrics.querySummary)
                .put("lastMicros", DebugSessionMetrics.lastSearchMicros))
            .put("knowledge", JSONObject()
                .put("open", knowledgeState.open)
                .put("tab", knowledgeState.tab.name.lowercase())
                .put("loading", knowledgeState.loading)
                .put("outlineCount", knowledgeState.outline.size)
                .put("backlinkCount", knowledgeState.backlinks.size)
                .put("tagCount", knowledgeState.tags.size)
                .put("selectedTag", knowledgeState.selectedTag)
                .put("tagNoteCount", knowledgeState.tagNotes.size)
                .put("outline", JSONArray(knowledgeState.outline.map {
                    JSONObject()
                        .put("level", it.level)
                        .put("title", it.title)
                        .put("start", it.start)
                }))
                .put("backlinks", JSONArray(knowledgeState.backlinks.map {
                    JSONObject()
                        .put("source", it.source)
                        .put("line", it.line)
                        .put("inDiagram", it.inDiagram)
                        .put("embed", it.embed)
                        .put("startUtf16", it.startUtf16)
                }))
                .put("tags", JSONArray(knowledgeState.tags.map {
                    JSONObject()
                        .put("tag", it.tag)
                        .put("count", it.count)
                }))
                .put("tagNotes", JSONArray(knowledgeState.tagNotes.map { it.path })))
            .put("panelHits", JSONArray().also { array ->
                PanelRegistry.snapshot().forEach { hit ->
                    array.put(JSONObject()
                        .put("kind", hit.kind)
                        .put("label", hit.label)
                        .put("x", hit.x)
                        .put("y", hit.y)
                        .put("width", hit.width)
                        .put("height", hit.height)
                        .put("target", hit.target)
                        .put("tab", hit.tab))
                }
            })
            .put("edit", JSONObject()
                .put("active", editState.active)
                .put("dirty", editState.dirty)
                .put("saving", editState.saving)
                .put("issue", editState.issue)
                .put("copyPath", editState.copyPath)
                .put("lastSaveOk", editState.lastSaveOk)
                .put("lastSaveMicros", editState.lastSaveMicros)
                .put("lastSaveIssue", DebugSessionMetrics.lastSaveIssue)
                .put("choice", editState.choice)
                .put("textLength", editState.field.text.length)
                .put("savedTextLength", editState.savedText.length)
                .put("readOnlyReason", editState.readOnlyReason))
            .put("newNote", JSONObject()
                .put("open", newNoteState.open)
                .put("name", newNoteState.name)
                .put("parent", newNoteState.parent)
                .put("error", newNoteState.error)
                .put("creating", newNoteState.creating)
                .put("lastCreated", DebugSessionMetrics.lastCreatedNote))
            .put("shortcuts", JSONObject()
                .put("published", JSONArray(DebugSessionMetrics.shortcutsPublished))
                .put("lastLaunchNote", DebugSessionMetrics.lastShortcutLaunchNote)
                .put("lastLaunchVault", DebugSessionMetrics.lastShortcutLaunchVault)
                .put("stored", JSONArray(viewModel.shortcutNotesSync().map {
                    JSONObject()
                        .put("vault", it.vault)
                        .put("path", it.path)
                        .put("title", it.title)
                })))
            .put("firstVisibleBlockId", viewModel.lastVisibleBlockId)
            .put("firstVisibleOffsetPx", viewModel.lastVisibleOffsetPx)
            .put("openMs", if (ReaderTiming.contentInstalledAt > 0) ReaderTiming.contentInstalledAt - ReaderTiming.openStartedAt else -1)
            .put("firstFrameMs", if (ReaderTiming.firstFrameAt > 0) ReaderTiming.firstFrameAt - ReaderTiming.openStartedAt else -1)
            .put("appOpenMs", if (ReaderTiming.contentInstalledAt > 0 && ReaderTiming.activityCreatedAt > 0) ReaderTiming.contentInstalledAt - ReaderTiming.activityCreatedAt else -1)
            .put("appFirstFrameMs", if (ReaderTiming.firstFrameAt > 0 && ReaderTiming.activityCreatedAt > 0) ReaderTiming.firstFrameAt - ReaderTiming.activityCreatedAt else -1)
            .put("appFullyDrawnMs", if (ReaderTiming.fullyDrawnAt > 0 && ReaderTiming.activityCreatedAt > 0) ReaderTiming.fullyDrawnAt - ReaderTiming.activityCreatedAt else -1)
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
                .put("largestFontPx", FocusStateRegistry.largestFontPx)
                .put("sceneFontSize", FocusStateRegistry.sceneFontSize)
                .put("unitScale", FocusStateRegistry.unitScale)
                .put("drawnFontSize", FocusStateRegistry.drawnFontSize)
                .put("fitScale", FocusStateRegistry.fitScale)
                .put("sceneKey", FocusStateRegistry.sceneKey)
                .put("notePath", FocusStateRegistry.notePath)
                .put("ordinal", FocusStateRegistry.ordinal)
                .put("collapsed", JSONArray(FocusStateRegistry.collapsed))
                .put("visibleNodes", FocusStateRegistry.visibleNodes)
                .put("plusLabels", FocusStateRegistry.plusLabels)
                .put("lastTapSceneX", FocusStateRegistry.lastTapSceneX)
                .put("lastTapSceneY", FocusStateRegistry.lastTapSceneY)
                .put("lastHitLabel", FocusStateRegistry.lastHitLabel)
                .put("lastHitNodeId", FocusStateRegistry.lastHitNodeId)
                .put("lastHitCanCollapse", FocusStateRegistry.lastHitCanCollapse))
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
            .put("textLayouts", LayoutInspection.textJson())
            .put("tableLayouts", LayoutInspection.tableJson())
            .put("blockMath", LayoutInspection.blockMathJson())
            .put("itemBounds", LayoutInspection.boundsJson())
            .put("embeds", Inspection.embedsJson())
            .put("math", Inspection.mathJson())
            .put("mathMetricsCodepoints", Inspection.mathMetrics.get())
            .put("diagramCounts", DiagramInspection.total.toJson())
            .put("diagrams", DiagramInspection.snapshot())
            .put("diagramFailures", Inspection.failuresJson())
            .put("exports", Inspection.exportsJson())
            .put("shares", Inspection.sharesJson())
            .put("inlineOverlayControls", 0)
            .put("inlinePreviews", Inspection.inlinePreviewsJson())
            .put("diagramRects", Inspection.diagramRectsJson())
            .put("diagramPreview", JSONObject()
                .put("recordings", DebugPreviewInspection.recordings)
                .put("replays", DebugPreviewInspection.replays)
                .put("cached", DiagramPreviewCache.size())
                .put("counts", DebugPreviewInspection.counts.toJson()))
            .put("focusDrawCounts", FocusCounts.counts.toJson())
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
            .put("typographyScale", mathTypographyScale()))
        val failedDiagrams = Inspection.failuresJson().length()
        val failedImages = content.media.count {
            it.columns.isEmpty() && it.imagePath.isNotEmpty() && it.imageReason.isNotEmpty()
        }
        report.put("sourceFallbackCount", failed + failedDiagrams + failedImages)
        return report
    }
}
