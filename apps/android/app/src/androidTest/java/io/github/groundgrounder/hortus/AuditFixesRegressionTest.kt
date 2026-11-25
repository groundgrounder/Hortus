// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Intent
import android.net.Uri
import android.os.Environment
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.groundgrounder.hortus.app.HortusServices
import io.github.groundgrounder.hortus.app.IntentDispatchTarget
import io.github.groundgrounder.hortus.app.IntentRouter
import io.github.groundgrounder.hortus.core.CoreEnvironment
import io.github.groundgrounder.hortus.core.InlineTagInfo
import io.github.groundgrounder.hortus.core.KnowledgeMarkupInfo
import io.github.groundgrounder.hortus.core.ResolvedLinkInfo
import io.github.groundgrounder.hortus.core.SceneColor
import io.github.groundgrounder.hortus.core.ScenePoint
import io.github.groundgrounder.hortus.core.SceneText
import io.github.groundgrounder.hortus.core.model.DecodedReadingModel
import io.github.groundgrounder.hortus.core.model.ReadingBlock
import io.github.groundgrounder.hortus.core.model.ReadingRun
import io.github.groundgrounder.hortus.core.model.RecentNote
import io.github.groundgrounder.hortus.core.model.TextItem
import io.github.groundgrounder.hortus.core.model.buildReaderItems
import io.github.groundgrounder.hortus.core.settings.LaunchExtras
import io.github.groundgrounder.hortus.core.settings.ShortcutStore
import io.github.groundgrounder.hortus.focus.DiagramFocusActivity
import io.github.groundgrounder.hortus.focus.FocusPayload
import io.github.groundgrounder.hortus.focus.FocusPayloads
import io.github.groundgrounder.hortus.focus.FocusViewModel
import io.github.groundgrounder.hortus.reader.render.DiagramStateBus
import io.github.groundgrounder.hortus.reader.render.SceneTextMetrics
import io.github.groundgrounder.hortus.reader.state.EditorSessionController
import io.github.groundgrounder.hortus.reader.ui.localizedRenderError
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.Locale

@RunWith(AndroidJUnit4::class)
class AuditFixesRegressionTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private fun waitForNote(scenario: ActivityScenario<MainActivity>, path: String, timeout: Long = 12_000) {
        val deadline = System.currentTimeMillis() + timeout
        while (System.currentTimeMillis() < deadline) {
            var current: String? = null
            scenario.onActivity { current = it.viewModel.readerState.value.content?.path }
            if (current == path) return
            Thread.sleep(100)
        }
        throw AssertionError("note did not open: $path")
    }

    @Test
    fun externalEntryAndVaultSwitchFlushUnsavedEdits() {
        instrumentation.uiAutomation.executeShellCommand(
            "appops set ${context.packageName} MANAGE_EXTERNAL_STORAGE allow",
        ).close()
        val docsRoot = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS)
        val stamp = System.nanoTime()
        val vaultA = File(docsRoot, "m4b-vault-a-$stamp").apply { mkdirs() }
        val vaultB = File(docsRoot, "m4b-vault-b-$stamp").apply { mkdirs() }
        try {
            val fileA = File(vaultA, "Draft.md").apply { writeText("# Draft\n\nInitial.\n") }
            File(vaultB, "Target.md").apply { writeText("# Target\n\nOther vault.\n") }
            val intent = Intent(context, MainActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                putExtra(LaunchExtras.EXTRA_OPEN_VAULT, vaultA.absolutePath)
                putExtra(LaunchExtras.EXTRA_OPEN_NOTE, "Draft.md")
            }
            val scenario = ActivityScenario.launch<MainActivity>(intent)
            try {
                waitForNote(scenario, "Draft.md")
                val editedA = "# Draft\n\nUnsaved edit before external preview.\n"
                var previewJob: Job? = null
                scenario.onActivity { activity ->
                    activity.viewModel.startEditing()
                    activity.viewModel.updateEditText(TextFieldValue(editedA, TextRange(editedA.length)))
                    assertTrue(activity.viewModel.editState.value.dirty)
                    previewJob = activity.viewModel.openExternalPreview("Outside.md", "# Outside\n\nPreview.\n")
                }
                runBlocking { previewJob?.join() }
                assertEquals("openExternalPreview must flush dirty edits", editedA, fileA.readText())

                scenario.onActivity { activity ->
                    activity.viewModel.back()
                }
                waitForNote(scenario, "Draft.md")

                val editedB = "# Draft\n\nUnsaved edit before vault switch.\n"
                var switchJob: Job? = null
                scenario.onActivity { activity ->
                    activity.viewModel.startEditing()
                    activity.viewModel.updateEditText(TextFieldValue(editedB, TextRange(editedB.length)))
                    switchJob = activity.viewModel.openVaultAndPath(vaultB.absolutePath, "Target.md")
                }
                runBlocking { switchJob?.join() }
                waitForNote(scenario, "Target.md")
                assertEquals("openVaultAndPath must flush dirty edits", editedB, fileA.readText())
            } finally {
                scenario.close()
            }
        } finally {
            vaultA.deleteRecursively()
            vaultB.deleteRecursively()
        }
    }

    @Test
    fun embeddedNoteDiagramUsesRealNoteKeyAndResolvesRelativeLinks() = runBlocking {
        CoreEnvironment.initialize(context)
        val services = HortusServices.get(context)
        val vault = File(context.cacheDir, "m4b-embed-${System.nanoTime()}").apply { mkdirs() }
        try {
            File(vault, "sub").mkdirs()
            File(vault, "sub/Target.md").writeText("# Target\n\nReached.\n")
            File(vault, "sub/Child.md").writeText(
                "# Child\n\n```mermaid\nmindmap\n  root((Root))\n    [[Target]]\n      Leaf\n```\n",
            )
            File(vault, "Host.md").writeText("# Host\n\n![[sub/Child]]\n")
            val vaultRepo = services.createVaultRepository {}
            val embedRepo = services.createEmbedRepository(vaultRepo)
            try {
                vaultRepo.openVault(vault.absolutePath)
                vaultRepo.awaitScan()
                val embedded = embedRepo.resolveEmbed(
                    sourcePath = "Host.md",
                    target = "sub/Child",
                    heading = null,
                    alias = null,
                    ancestors = listOf("Host.md"),
                    width = 600.0,
                    size = 17.0,
                )
                val diagram = embedded.items.filterIsInstance<io.github.groundgrounder.hortus.core.model.DiagramItem>().first()
                assertEquals("sub/Child.md", diagram.documentKey)

                val focusVm = FocusViewModel(context.applicationContext as android.app.Application)
                focusVm.initialize(
                    FocusPayload(
                        vaultPath = vault.absolutePath,
                        notePath = diagram.documentKey,
                        documentKey = diagram.documentKey,
                        engineText = diagram.engineText,
                        mmd = diagram.mmd,
                        ordinal = diagram.ordinal,
                        revision = 1L,
                        dark = false,
                        collapsed = emptyList(),
                        title = "Child",
                        renderWidth = 600.0,
                    ),
                    dark = false,
                )
                focusVm.toggleNode("node_1", dark = false)
                assertEquals(
                    listOf("node_1"),
                    DiagramStateBus.nodes(vault.absolutePath, "sub/Child.md", diagram.ordinal),
                )
                val resolved = focusVm.resolveVaultLink("Target")
                assertEquals("sub/Target.md", resolved)
            } finally {
                vaultRepo.close()
            }
        } finally {
            vault.deleteRecursively()
        }
    }

    @Test
    fun missingFocusPayloadFinishesCleanlyWithoutBlankScreen() {
        FocusPayloads.clear()
        val scenario = ActivityScenario.launch<DiagramFocusActivity>(
            DiagramFocusActivity.intent(context, token = 999_999),
        )
        try {
            val deadline = System.currentTimeMillis() + 5_000
            while (System.currentTimeMillis() < deadline && scenario.state != Lifecycle.State.DESTROYED) {
                Thread.sleep(50)
            }
            assertEquals(Lifecycle.State.DESTROYED, scenario.state)
        } finally {
            scenario.close()
        }
    }

    @Test
    fun storageRootPrivateFileFallbackAndBlankTitleShortcutAreSafe() = runBlocking {
        assertFalse(IntentRouter.isValidVaultDirectory("/storage/emulated/0"))
        assertFalse(IntentRouter.isValidVaultDirectory("/sdcard"))

        val privateFile = File(context.filesDir, "secret-test.md").apply {
            parentFile?.mkdirs()
            writeText("# Secret\n")
        }
        val externalDir = context.getExternalFilesDir(null)!!
        val previewFile = File(externalDir, "m4b-shared-${System.nanoTime()}.md").apply {
            writeText("# Shared\n\nFallback stream.\n")
        }
        try {
            val notices = ArrayList<String>()
            var openedPreview: Pair<String, String>? = null
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
            val target = object : IntentDispatchTarget {
                override val currentVaultPath: String? = null
                override val savedVaultPath: String? = null
                override val knownShortcutVaults: Set<String> = emptySet()
                override fun openNote(path: String, heading: String?) = Job()
                override fun startVault(vaultPath: String, notePath: String?, persistPreference: Boolean): Job? = null
                override fun openPath(path: String, heading: String?): Job? = null
                override fun openVaultAndPath(vaultPath: String, notePath: String, heading: String?, persistPreference: Boolean) = Job()
                override fun openExternalPreview(fileName: String, text: String): Job {
                    openedPreview = fileName to text
                    return Job().apply { complete() }
                }
                override fun showNotice(message: String) { notices.add(message) }
                override fun onPersistedVault(vaultPath: String) = Unit
            }
            val privateIntent = Intent(Intent.ACTION_VIEW, Uri.fromFile(privateFile))
            val privateJob = IntentRouter.handleExternalIntent(context, scope, privateIntent, hasAccess = true, target = target)
            privateJob?.join()
            assertNull("private filesDir URI must never open preview", openedPreview)
            assertTrue("private filesDir URI must emit denial notice", notices.isNotEmpty())

            val contentUri = Uri.parse("content://${context.packageName}.debugpreview/${previewFile.name}")
            val contentIntent = Intent(Intent.ACTION_VIEW, contentUri)
            val contentJob = IntentRouter.handleExternalIntent(context, scope, contentIntent, hasAccess = false, target = target)
            contentJob?.join()
            assertNotNull("content:// stream fallback must succeed even when hasAccess=false", openedPreview)
            assertTrue(openedPreview!!.second.contains("Fallback stream"))
            scope.cancel()

            val shortcuts = ShortcutStore(context)
            shortcuts.requestPin(RecentNote("/tmp/vault", "NoTitle.md", "", System.currentTimeMillis()))
            val stored = shortcuts.load().firstOrNull { it.path == "NoTitle.md" }
            assertNotNull(stored)
            assertEquals("NoTitle.md", stored!!.title)
        } finally {
            privateFile.delete()
            previewFile.delete()
        }
    }

    @Test
    fun editorImeGuardAndConcurrentTypingPreserveDirtyState() = runBlocking {
        instrumentation.uiAutomation.executeShellCommand(
            "appops set ${context.packageName} MANAGE_EXTERNAL_STORAGE allow",
        ).close()
        CoreEnvironment.initialize(context)
        val services = HortusServices.get(context)
        val docsRoot = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS)
        val vault = File(docsRoot, "m4b-edit-${System.nanoTime()}").apply { mkdirs() }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        try {
            val noteFile = File(vault, "Edit.md").apply { writeText("# Edit\n\nStart.\n") }
            val vaultRepo = services.createVaultRepository {}
            val docRepo = services.createDocumentRepository(vaultRepo) {}
            try {
                vaultRepo.openVault(vault.absolutePath)
                vaultRepo.awaitScan()
                val content = docRepo.openNote(vault.absolutePath, "Edit.md", 600, 17f)
                val loadMutex = Mutex()
                val openedCopies = ArrayList<String>()
                val controller = EditorSessionController(
                    context = context,
                    scope = scope,
                    documentRepository = docRepo,
                    loadMutex = loadMutex,
                    onNotice = {},
                    onRefreshLocked = {},
                    onOpenConflictCopyLocked = { copy -> openedCopies.add(copy) },
                )
                controller.startEditing(content, canEdit = true)
                // 1. IME composing blocks autoSaveEdit until composition commits
                controller.updateEditText(
                    TextFieldValue("# Edit\n\npinyin", TextRange(14), composition = TextRange(8, 14)),
                )
                assertNull("autoSaveEdit must skip while IME composing", controller.autoSaveEdit())
                assertEquals("# Edit\n\nStart.\n", noteFile.readText())

                // Committing composition triggers pending autosave
                controller.updateEditText(
                    TextFieldValue("# Edit\n\n中文提交", TextRange(12), composition = null),
                )
                controller.awaitSaveIdle()
                assertEquals("# Edit\n\n中文提交", noteFile.readText())

                // 2. Concurrent typing while save is in-flight preserves dirty=true and resaves latest text
                val firstDraft = "# Edit\n\nFirst draft.\n"
                val secondDraft = "# Edit\n\nFirst draft + typed during save.\n"
                controller.updateEditText(TextFieldValue(firstDraft, TextRange(firstDraft.length)))
                val saveJob = scope.launch { controller.saveEditSuspending(io.github.groundgrounder.hortus.core.SaveChoice.NORMAL) }
                delay(5)
                controller.updateEditText(TextFieldValue(secondDraft, TextRange(secondDraft.length)))
                saveJob.join()
                controller.awaitSaveIdle()
                assertEquals(secondDraft, noteFile.readText())

                // 3. Deleted original + CONFLICT_COPY switches to the created conflict copy
                val copyDraft = "# Edit\n\nCopy after delete.\n"
                controller.updateEditText(TextFieldValue(copyDraft, TextRange(copyDraft.length)))
                assertTrue(noteFile.delete())
                assertFalse(controller.saveEditSuspending(io.github.groundgrounder.hortus.core.SaveChoice.NORMAL))
                assertEquals("missing", controller.editState.value.issue)
                assertTrue(controller.saveEditSuspending(io.github.groundgrounder.hortus.core.SaveChoice.CONFLICT_COPY))
                assertEquals(1, openedCopies.size)
                assertTrue(openedCopies[0].contains("(conflict copy)"))
                assertFalse(controller.editState.value.active)
            } finally {
                docRepo.close()
                vaultRepo.close()
            }
        } finally {
            scope.cancel()
            vault.deleteRecursively()
        }
    }

    @Test
    fun renderErrorsAreLocalizedAndSceneMetricsQuantizeZoom() {
        val zhConfig = android.content.res.Configuration(context.resources.configuration).apply {
            setLocale(Locale.SIMPLIFIED_CHINESE)
        }
        val zhContext = context.createConfigurationContext(zhConfig)
        val diagMsg = localizedRenderError(zhContext, "diagram.not_a_tree: A, B")
        val imgMsg = localizedRenderError(zhContext, "image.decode_failed")
        val mathMsg = localizedRenderError(zhContext, "math.parse: bad token")
        assertFalse(diagMsg.contains("diagram."))
        assertFalse(imgMsg.contains("image."))
        assertFalse(mathMsg.contains("math."))
        assertTrue(diagMsg.contains("树形"))
        assertTrue(imgMsg.contains("图片"))
        assertTrue(mathMsg.contains("公式"))

        val span = SceneText(
            text = "ZoomQuantize",
            baseline = ScenePoint(0.0, 10.0),
            width = 80.0,
            fontSize = 15.0,
            fontWeight = 400u.toUShort(),
            fontFamily = "Roboto",
            color = SceneColor(0.0, 0.0, 0.0, 1.0),
        )
        val s1 = SceneTextMetrics.scaleX(span, 1.0011f)
        val s2 = SceneTextMetrics.scaleX(span, 1.0014f)
        assertEquals(s1, s2, 0.0001f)
        val widePaint = SceneTextMetrics.paint(span, 2.975f, android.graphics.Color.BLACK)
        val measuredWide = widePaint.measureText(span.text).toDouble() / 16.0 / 2.975
        assertEquals(span.width, measuredWide, 0.10)

        val blocks = listOf(
            ReadingBlock(1L, 0, 10, 0, 10, listOf(ReadingRun(0, 10, 0, 10, 0)), false),
            ReadingBlock(2L, 11, 15, 11, 15, listOf(ReadingRun(0, 15, 0, 15, 0)), false),
        )
        val model = DecodedReadingModel(1L, 1L, "0123456789\n012345678901234", blocks, 2)
        val knowledge = KnowledgeMarkupInfo(
            links = listOf(
                ResolvedLinkInfo(
                    target = "Target",
                    heading = null,
                    alias = null,
                    embed = false,
                    inDiagram = false,
                    attachment = false,
                    resolutionKind = "note",
                    resolvedPath = "Target.md",
                    candidates = emptyList(),
                    line = 2uL,
                    startUtf16 = 12uL,
                    lengthUtf16 = 6uL,
                ),
            ),
            tags = listOf(
                InlineTagInfo(
                    tag = "tag",
                    startUtf16 = 2uL,
                    lengthUtf16 = 4uL,
                ),
            ),
        )
        val items = buildReaderItems(model, "0123456789\n012345678901234", emptyList(), emptyList(), knowledge, emptyList(), "A.md", false)
        val first = items[0] as TextItem
        val second = items[1] as TextItem
        assertEquals(1, first.tags.size)
        assertEquals(0, first.links.size)
        assertEquals(1, second.links.size)
        assertEquals(0, second.tags.size)
    }
}
