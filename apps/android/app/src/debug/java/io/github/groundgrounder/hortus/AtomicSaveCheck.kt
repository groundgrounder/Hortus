// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Context
import android.system.ErrnoException
import android.system.Os
import io.github.groundgrounder.hortus.core.DocumentIssue
import io.github.groundgrounder.hortus.core.DocumentPersistence
import io.github.groundgrounder.hortus.core.SaveChoice
import io.github.groundgrounder.hortus.core.WorkspaceCore
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.AtomicMoveNotSupportedException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

internal object AtomicSaveCheck {
    private const val edit = "\n\nAndroid atomic save check · 原子保存。\n"

    fun run(
        context: Context,
        workspace: WorkspaceCore,
        vault: File,
        sourcePath: String,
        sourceText: String,
        report: JSONObject,
    ) {
        val cache = checkNotNull(context.externalCacheDir).canonicalFile
        val vaultDevice = Os.stat(vault.path).st_dev
        val cacheDevice = Os.stat(cache.path).st_dev
        report.put("vaultDev", vaultDevice)
        report.put("externalCacheDev", cacheDevice)
        report.put("sameDevice", vaultDevice == cacheDevice)
        report.put("externalCacheDir", cache.path)
        check(!cache.toPath().startsWith(vault.toPath())) { "External cache is inside the vault" }

        val vaultBefore = entries(vault)
        val cacheBefore = entries(cache)
        val sharedParent = checkNotNull(vault.parentFile)
        val sharedBefore = sharedStagingNames(sharedParent)
        val original = vault.resolve(sourcePath)
        val originalDigest = sha256(original.readBytes())
        report.put("sourcePath", sourcePath)
        report.put("sourceSha256", originalDigest)

        try {
            val copy = Files.createTempFile(vault.toPath(), "Hortus-save-check-", ".md").toFile()
            try {
                copy.writeText(sourceText)
                probeRestrictions(cache, copy, report)
                check(vaultDevice == cacheDevice) { "External cache and vault have different st_dev" }

                val before = Os.stat(copy.path)
                val expected = sourceText + edit
                workspace.openDocument(vault.path, copy.name).use { document ->
                    check(document.snapshot().text == sourceText) { "Test copy content changed before editing" }
                    document.replace(sourceText.length.toULong(), 0uL, edit)
                    val saved = document.save(cache.path, SaveChoice.NORMAL)
                    report.put("saveIssue", saved.issue.name)
                    report.put("saveMicros", saved.saveMicros.toLong())
                    check(saved.issue == DocumentIssue.NONE && !saved.dirty &&
                        saved.persistence == DocumentPersistence.PERSISTED) { saved.message }
                }

                val after = Os.stat(copy.path)
                report.put("inodeBefore", before.st_ino)
                report.put("inodeAfter", after.st_ino)
                val disk = copy.readBytes()
                val reopened = workspace.openDocument(vault.path, copy.name).use { it.snapshot().text }
                report.put("savedSha256", sha256(disk))
                report.put("expectedSha256", sha256(expected.encodeToByteArray()))
                val identical = disk.contentEquals(expected.encodeToByteArray()) && reopened == expected
                report.put("contentMatches", identical)
                check(identical) { "Saved bytes or reopened document differ from the edit" }
            } finally {
                Files.deleteIfExists(copy.toPath())
            }
        } finally {
            val vaultAfter = entries(vault)
            val cacheAfter = entries(cache)
            val leftovers = (vaultAfter - vaultBefore).sorted()
            val cacheLeftovers = (cacheAfter - cacheBefore).sorted()
            val sharedLeftovers = (sharedStagingNames(sharedParent) - sharedBefore).sorted()
            val sourceUnchanged = sha256(original.readBytes()) == originalDigest
            report.put("leftoverTempFiles", JSONArray(leftovers))
            report.put("externalCacheLeftovers", JSONArray(cacheLeftovers))
            report.put("sharedStagingLeftovers", JSONArray(sharedLeftovers))
            report.put("vaultEntriesUnchanged", vaultAfter == vaultBefore)
            report.put("sourceUnchanged", sourceUnchanged)
            check(vaultAfter == vaultBefore && cacheLeftovers.isEmpty() &&
                sharedLeftovers.isEmpty() && sourceUnchanged) {
                "Test copy, staging cleanup or original note verification failed"
            }
        }
    }

    private fun probeRestrictions(cache: File, copy: File, report: JSONObject) {
        val target = Files.createTempFile(copy.parentFile!!.toPath(), "Hortus-save-probe-", ".md").toFile()
        try {
            val externalError = probeAtomicMove(cache, target, report)
            report.put("externalCacheAtomicMove", externalError == null)
            if (externalError != null) report.put("externalCacheAtomicMoveError", externalError)
            val sibling = Files.createTempDirectory(copy.parentFile!!.parentFile!!.toPath(), ".hortus-probe-")
            try {
                val sharedError = probeAtomicMove(sibling.toFile(), target)
                report.put("sharedStagingAtomicMove", sharedError == null)
                if (sharedError != null) report.put("sharedStagingAtomicMoveError", sharedError)
            } finally {
                Files.delete(sibling)
            }
        } finally {
            Files.delete(target.toPath())
        }
    }

    private fun probeAtomicMove(stagingDir: File, target: File, report: JSONObject? = null): String? {
        val staging = Files.createTempFile(stagingDir.toPath(), ".tmp", "").toFile()
        try {
            FileOutputStream(staging).use { output ->
                output.write(byteArrayOf(1))
                val chmodErrno = errno { Os.fchmod(output.fd, Os.stat(target.path).st_mode and 0xFFF) }
                val syncErrno = errno { Os.fsync(output.fd) }
                report?.put("chmodErrno", chmodErrno)?.put("fsyncErrno", syncErrno)
            }
            if (report != null) {
                val path = target.parentFile!!.resolve("${target.name}.link").path
                val linkErrno = errno { Os.link(staging.path, path) }
                if (linkErrno == 0) Files.delete(File(path).toPath())
                report.put("hardLinkErrno", linkErrno)
            }
            // Os.rename 会在 Android 上用普通移动处理 EXDEV。探针要求真正的原子移动。
            Files.move(
                staging.toPath(), target.toPath(),
                StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING,
            )
            return null
        } catch (e: AtomicMoveNotSupportedException) {
            return e.reason ?: e.javaClass.simpleName
        } finally {
            Files.deleteIfExists(staging.toPath())
        }
    }

    private fun errno(operation: () -> Unit): Int = try {
        operation()
        0
    } catch (e: ErrnoException) {
        e.errno
    }

    private fun entries(dir: File): Set<String> = Files.walk(dir.toPath()).use { paths ->
        paths.iterator().asSequence().map { dir.toPath().relativize(it).toString() }.toSet()
    }

    private fun sharedStagingNames(parent: File): Set<String> =
        checkNotNull(parent.list()).filter { it.startsWith(".hortus-save-") || it.startsWith(".hortus-probe-") }.toSet()

    private fun sha256(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
}
