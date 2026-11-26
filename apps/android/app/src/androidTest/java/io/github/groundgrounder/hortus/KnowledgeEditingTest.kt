// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.groundgrounder.hortus.core.CoreEnvironment
import io.github.groundgrounder.hortus.core.DocumentIssue
import io.github.groundgrounder.hortus.core.SaveChoice
import io.github.groundgrounder.hortus.core.SearchCancellation
import io.github.groundgrounder.hortus.core.VaultHandle
import io.github.groundgrounder.hortus.core.WorkspaceCore
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class KnowledgeEditingTest {
    private lateinit var context: android.content.Context
    private lateinit var workspace: WorkspaceCore
    private lateinit var vault: File
    private lateinit var handle: VaultHandle

    private val startNote = "# Start here\n\nIntro with #alpha.\n\n```mermaid\nmindmap\n  root((Start))\n    [[Start here]]\n```\n"
    private val bodySource = "# Body source\n\nSee [[Start here]].\n"
    private val diagramSource = "# Diagram source\n\n```mermaid\nmindmap\n  root((Refs))\n    [[Start here]]\n```\n"
    private val chineseNote = "# 中文笔记\n\n搜索词条：山水与河流。\n"

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        CoreEnvironment.initialize(context)
        workspace = CoreEnvironment.createWorkspaceCore(context)
        vault = File(context.cacheDir, "m2a-test-${System.nanoTime()}").apply { mkdirs() }
        write("Start here.md", startNote)
        write("Source body.md", bodySource)
        write("Source diagram.md", diagramSource)
        write("中文笔记.md", chineseNote)
        handle = workspace.openVault(vault.path)
        waitForScan()
        handle.upsertNoteContent("Start here.md", startNote)
        handle.upsertNoteContent("Source body.md", bodySource)
        handle.upsertNoteContent("Source diagram.md", diagramSource)
        handle.upsertNoteContent("中文笔记.md", chineseNote)
    }

    @After
    fun tearDown() {
        runCatching { handle.close() }
        runCatching { workspace.close() }
        vault.deleteRecursively()
    }

    @Test
    fun quickOpenAndFullTextSearchMatchBothLanguages() {
        val quick = handle.quickOpen("中文", 50u)
        assertTrue("quick open should find the Chinese note", quick.any { it.path == "中文笔记.md" })
        val quickPaths = quick.first { it.path == "中文笔记.md" }
        assertTrue("quick open should highlight the title", quickPaths.titleMatches.isNotEmpty())

        val search = handle.search(
            "山水",
            8u,
            SearchCancellation(),
        )
        val hit = search.firstOrNull { it.path == "中文笔记.md" }
        assertNotNull("full text should find the Chinese query", hit)
        assertTrue("full text should report at least one match", hit!!.count > 0u)
        assertTrue("match should carry a snippet", hit.matches.isNotEmpty())

        val english = handle.search("Body source", 8u, SearchCancellation())
        assertTrue("full text should find English titles", english.any { it.path == "Source body.md" })

        val cancelled = SearchCancellation()
        cancelled.cancel()
        assertTrue(
            "a cancelled search must not return stale results",
            handle.search("Start", 8u, cancelled).isEmpty(),
        )
    }

    @Test
    fun knowledgeQueriesSeeBodyAndDiagramBacklinks() {
        val backlinks = handle.backlinks("Start here.md")
        assertTrue(
            "body and diagram sources should link back",
            backlinks.any { it.source == "Source body.md" && !it.inDiagram } &&
                backlinks.any { it.source == "Source diagram.md" && it.inDiagram },
        )
        val tags = handle.tags()
        val alpha = tags.firstOrNull { it.tag == "alpha" }
        assertNotNull("tag index should contain the inline tag", alpha)
        assertTrue("tag notes should include the source note", alpha!!.notes.contains("Start here.md"))
        assertEquals(listOf("Start here.md"), handle.notesWithTag("alpha"))

        workspace.openDocument(vault.path, "Start here.md").use { session ->
            val outline = session.outline()
            assertTrue("outline should contain the level one heading", outline.headings.isNotEmpty())
        }
    }

    @Test
    fun createEditSaveAndConflictCopyKeepFilesIntact() {
        val created = handle.createNote("", "New mobile note")
        assertEquals("New mobile note.md", created.path)
        try {
            handle.createNote("", "New mobile note")
            fail("duplicate note creation must fail")
        } catch (error: Exception) {
            assertTrue(
                "duplicate error should be localizable",
                error.message.orEmpty().contains("vault.already_exists"),
            )
        }

        val cache = context.cacheDir.path
        val edited = "# New mobile note\n\n中文与 English 混排。\n\n新 #mobile-tag 与 [[Start here]]。\n\n```mermaid\nflowchart LR\n  A --> B\n```\n"
        workspace.openDocument(vault.path, created.path).use { session ->
            val current = session.snapshot().text
            session.replace(0uL, current.length.toULong(), edited)
            val saved = session.save(cache, SaveChoice.NORMAL)
            assertEquals(DocumentIssue.NONE, saved.issue)
            assertTrue(!saved.dirty)
        }
        assertEquals(edited, File(vault, created.path).readText())
        handle.upsertNoteContent(created.path, edited)
        val indexed = handle.search("mobile-tag", 8u, SearchCancellation())
        assertTrue("saved edit must update the search index", indexed.any { it.path == created.path })

        workspace.openDocument(vault.path, created.path).use { session ->
            val current = session.snapshot().text
            session.replace(0uL, current.length.toULong(), "# mine\n")
            File(vault, created.path).writeText("# external version\n")
            val conflict = session.save(cache, SaveChoice.NORMAL)
            assertEquals(DocumentIssue.CONFLICT, conflict.issue)
            val copy = session.save(cache, SaveChoice.CONFLICT_COPY)
            assertNotNull("conflict copy path should be reported", copy.copyPath)
            assertTrue(copy.copyPath!!.contains("(conflict copy)"))
            assertEquals("# mine\n", File(vault, copy.copyPath!!).readText())
        }
        assertEquals("# external version\n", File(vault, created.path).readText())

        val recreatePath = "Recreate.md"
        handle.createNote("", "Recreate")
        workspace.openDocument(vault.path, recreatePath).use { session ->
            val current = session.snapshot().text
            session.replace(0uL, current.length.toULong(), "# recreated\n")
            assertTrue(File(vault, recreatePath).delete())
            val missing = session.save(cache, SaveChoice.NORMAL)
            assertEquals(DocumentIssue.MISSING, missing.issue)
            val recreated = session.save(cache, SaveChoice.RECREATE)
            assertEquals(DocumentIssue.NONE, recreated.issue)
        }
        assertEquals("# recreated\n", File(vault, recreatePath).readText())

        val leftovers = vault.walkTopDown().filter {
            it.isFile && (it.name.startsWith(".") || it.name.contains("tmp") ||
                it.name.contains("conflict copy") && false)
        }.map { it.name }.toList()
        assertTrue("editing should not leave temporary files in the vault: $leftovers", leftovers.isEmpty())
    }

    private fun write(relative: String, text: String) {
        val file = File(vault, relative)
        file.parentFile?.mkdirs()
        file.writeText(text)
    }

    private fun waitForScan() {
        val deadline = System.currentTimeMillis() + 20_000
        while (System.currentTimeMillis() < deadline) {
            val batches = handle.waitForBatches()
            if (batches.any { it.scanComplete }) return
        }
        throw AssertionError("vault scan did not complete")
    }
}
