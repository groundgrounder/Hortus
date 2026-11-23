// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Context
import io.github.groundgrounder.hortus.core.DiagramEngine
import io.github.groundgrounder.hortus.core.MathEngine
import io.github.groundgrounder.hortus.core.MathStyle
import io.github.groundgrounder.hortus.core.ScenePrimitive
import io.github.groundgrounder.hortus.core.TreeEntry
import io.github.groundgrounder.hortus.core.VaultHandle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder

internal data class FoundationResult(
    val vaultPath: String,
    val notes: List<TreeEntry>,
    val report: JSONObject,
)

internal fun checkResult(result: JSONObject, block: JSONObject.() -> Unit): JSONObject = result.apply {
    try {
        block()
        put("passed", true)
    } catch (e: Exception) {
        put("passed", false)
        put("error", e.message ?: e.javaClass.simpleName)
    }
}

internal object FoundationChecks {
    private val diagrams = linkedMapOf(
        "mindmap" to "mindmap\n  root((Hortus))\n    Notes\n    Diagrams",
        "flowchart" to "flowchart LR\n  A[Notes] --> B[Diagrams]",
        "sequence" to "sequenceDiagram\n  Alice->>Bob: Read note\n  Bob-->>Alice: Markdown",
        "state" to "stateDiagram-v2\n  [*] --> Reading\n  Reading --> [*]",
        "class" to "classDiagram\n  Vault --> Note\n  class Note {\n    +String title\n  }",
        "er" to "erDiagram\n  VAULT ||--o{ NOTE : contains",
    )

    suspend fun run(context: Context, path: String, testSave: Boolean): FoundationResult =
        withContext(Dispatchers.IO) {
            val vaultDir = File(path).canonicalFile
            val vaultScan = JSONObject()
            val atomicSave = JSONObject().put("attempted", testSave)
            var notes = emptyList<TreeEntry>()
            checkResult(vaultScan) {
                require(vaultDir.isDirectory) { "Vault directory is unavailable" }
                CoreEnvironment.createWorkspaceCore(context).use { workspace ->
                    check(File(workspace.dataDirectory()).canonicalFile == CoreEnvironment.dataDir.canonicalFile) {
                        "Rust data directory does not match the app's private directory"
                    }
                    workspace.openVault(vaultDir.path).use { vault ->
                        try {
                            val entries = scan(vault)
                            notes = entries.filterNot(TreeEntry::directory).sortedBy(TreeEntry::path)
                            vaultScan.put("entryCount", entries.size)
                            vaultScan.put("noteCount", notes.size)
                            vaultScan.put("notePaths", JSONArray(notes.map(TreeEntry::path)))
                            if (testSave) {
                                checkResult(atomicSave) {
                                    val source = notes.firstOrNull { it.path.endsWith(".md", ignoreCase = true) }
                                    checkNotNull(source) { "A Markdown note is needed for the save check" }
                                    AtomicSaveCheck.run(
                                        context, workspace, vaultDir, source.path,
                                        vault.readNote(source.path), atomicSave,
                                    )
                                }
                            }
                        } finally {
                            vault.stopWatching()
                        }
                    }
                }
            }
            if (testSave && !atomicSave.has("passed")) {
                atomicSave.put("passed", false).put("error", "Vault scan failed")
            }

            val fonts = JSONObject()
            val diagramChecks = JSONObject()
            checkResult(fonts) {
                val systemFonts = AndroidFonts.discover()
                fonts.put("discoveredCount", systemFonts.size)
                val paths = systemFonts.mapNotNull { it.file?.path }.distinct()
                fonts.put("injectedPaths", JSONArray(paths))
                checkResult(diagramChecks) {
                    DiagramEngine.withFontPaths(paths).use { engine ->
                        val items = JSONArray()
                        val families = sortedSetOf<String>()
                        diagramChecks.put("items", items)
                        for ((type, source) in diagrams) {
                            items.put(checkResult(JSONObject().put("type", type)) {
                                val scene = engine.render(source, true, 1u, 680.0, false, emptyList())
                                put("width", scene.width)
                                put("height", scene.height)
                                put("primitiveCount", scene.primitives.size)
                                check(scene.width > 0 && scene.height > 0 && scene.primitives.isNotEmpty())
                                scene.primitives.filterIsInstance<ScenePrimitive.Text>()
                                    .mapTo(families) { it.span.fontFamily }
                            })
                        }
                        fonts.put("diagramFontFamilies", JSONArray(families))
                        check((0 until items.length()).all { items.getJSONObject(it).getBoolean("passed") }) {
                            "One or more Mermaid scenes failed"
                        }
                    }
                }
                AndroidFonts.compareWidths(systemFonts, fonts)
            }
            if (!diagramChecks.has("passed")) {
                diagramChecks.put("passed", false).put("error", "System fonts are unavailable")
            }

            val math = checkResult(JSONObject()) {
                MathEngine().use { engine ->
                    put("displayBytes", mathScene(engine.render("""\frac{1}{2} + \sqrt{x}""", MathStyle.DISPLAY)))
                    put("textBytes", mathScene(engine.render("""x^2 + \alpha""", MathStyle.TEXT)))
                }
            }
            val checks = listOf(vaultScan, diagramChecks, math, fonts)
            val report = JSONObject()
                .put("vaultScan", vaultScan)
                .put("diagrams", diagramChecks)
                .put("math", math)
                .put("fonts", fonts)
                .put("atomicSave", atomicSave)
                .put("allPassed", checks.all { it.optBoolean("passed") } &&
                    (!testSave || atomicSave.optBoolean("passed")))
            FoundationResult(vaultDir.path, notes, report)
        }

    private fun scan(vault: VaultHandle): List<TreeEntry> {
        val entries = mutableMapOf<String, TreeEntry>()
        while (true) {
            val batches = vault.waitForBatches()
            check(batches.isNotEmpty()) { "Vault scan ended before completion" }
            for (batch in batches) {
                check(batch.errors.isEmpty()) { batch.errors.joinToString() }
                batch.removed.forEach(entries::remove)
                batch.upserted.forEach { entries[it.path] = it }
                if (batch.scanComplete) return entries.values.toList()
            }
        }
    }

    private fun mathScene(bytes: ByteArray): Int {
        check(bytes.size > 8) { "Empty formula scene" }
        val envelope = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        check(envelope.int == 1 && envelope.int == bytes.size - 8) { "Invalid formula scene header" }
        val scene = JSONObject(bytes.decodeToString(startIndex = 8))
        check(scene.getJSONArray("items").length() > 0) { "Formula scene has no drawing items" }
        return bytes.size
    }
}
