// SPDX-License-Identifier: AGPL-3.0-only
package io.github.groundgrounder.hortus.app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.settings.LaunchExtras
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.File

internal interface IntentDispatchTarget {
    val currentVaultPath: String?
    val savedVaultPath: String?
    val knownShortcutVaults: Set<String>
    fun openNote(path: String, heading: String? = null): Job
    fun startVault(vaultPath: String, notePath: String?, persistPreference: Boolean): Job?
    fun openPath(path: String, heading: String?): Job?
    fun openVaultAndPath(vaultPath: String, notePath: String, heading: String?, persistPreference: Boolean): Job
    fun openExternalPreview(fileName: String, text: String): Job
    fun showNotice(message: String)
    fun onPersistedVault(vaultPath: String)
}

/**
 * 集中解析 ACTION_VIEW、快捷方式与 EXTRA_OPEN_* 外部启动意图，严守存储根与 8 MiB 只读预览安全边界。
 */
internal object IntentRouter {
    private const val MAX_PREVIEW_BYTES = 8 * 1024 * 1024

    fun isStorageRoot(directory: File): Boolean {
        val canonical = runCatching { directory.canonicalFile }.getOrNull() ?: return true
        val rootPath = canonical.path
        if (rootPath == "/" || rootPath == "/storage" || rootPath == "/storage/emulated" ||
            rootPath == "/storage/emulated/0" || rootPath == "/sdcard" ||
            rootPath == "/mnt" || rootPath == "/mnt/sdcard"
        ) {
            return true
        }
        val externalRoot = runCatching {
            Environment.getExternalStorageDirectory()?.canonicalPath
        }.getOrNull()
        if (externalRoot != null && rootPath == externalRoot) return true
        if (canonical.parentFile?.path == "/storage" && canonical.name != "emulated") return true
        return false
    }

    fun isValidRelativeNotePath(note: String): Boolean {
        if (note.isBlank() || '\u0000' in note) return false
        if (note.startsWith('/') || note.startsWith('\\')) return false
        return note.split('/', '\\').none { it == ".." || it == "." }
    }

    fun isValidVaultDirectory(vault: String): Boolean {
        if (vault.isBlank() || '\u0000' in vault) return false
        if (vault.split('/', '\\').any { it == ".." }) return false
        val dir = File(vault)
        return dir.isAbsolute && dir.isDirectory && !isStorageRoot(dir)
    }

    private fun isAppPrivatePath(context: Context, file: File): Boolean {
        val canonical = runCatching { file.canonicalPath }.getOrNull() ?: return true
        val privateRoots = listOfNotNull(
            context.filesDir,
            context.cacheDir,
            context.dataDir,
        ).mapNotNull { runCatching { it.canonicalPath }.getOrNull() }
        return privateRoots.any { root ->
            canonical == root || canonical.startsWith(root + File.separator)
        }
    }

    fun handleExternalIntent(
        context: Context,
        scope: CoroutineScope,
        intent: Intent,
        hasAccess: Boolean,
        target: IntentDispatchTarget,
    ): Job? {
        val uri = intent.data ?: return null
        val physical = resolvePhysicalPath(context, uri)
        if (physical != null) {
            val file = File(physical)
            if (isAppPrivatePath(context, file)) {
                target.showNotice(context.getString(R.string.error_permission_denied))
                return null
            }
            if (file.isFile && (hasAccess || uri.scheme != "content")) {
                if (!hasAccess) {
                    target.showNotice(context.getString(R.string.error_permission_denied))
                    return null
                }
                val canonical = runCatching { file.canonicalPath }.getOrNull() ?: return null
                val known = target.currentVaultPath ?: target.savedVaultPath
                val knownRoot = known?.let { runCatching { File(it).canonicalPath }.getOrNull() }
                val vault: String
                val note: String
                val mode: String
                val persistPreference: Boolean
                if (knownRoot != null && canonical.startsWith(knownRoot + File.separator)) {
                    vault = knownRoot
                    note = canonical.removePrefix(knownRoot + File.separator)
                    mode = "vault"
                    persistPreference = true
                } else {
                    val parent = file.parentFile ?: return null
                    if (isStorageRoot(parent)) {
                        return scope.launch {
                            val loaded = withContext(Dispatchers.IO) {
                                runCatching {
                                    if (file.length() > MAX_PREVIEW_BYTES.toLong()) {
                                        throw IllegalStateException("preview.too_large")
                                    }
                                    file.name to file.readText(Charsets.UTF_8)
                                }.getOrNull()
                            }
                            if (loaded == null) {
                                target.showNotice(context.getString(R.string.preview_error))
                            } else {
                                target.openExternalPreview(loaded.first, loaded.second)
                            }
                        }
                    }
                    vault = parent.absolutePath
                    note = file.name
                    mode = "parent_vault"
                    persistPreference = false
                }
                return if (target.currentVaultPath == vault) {
                    DebugProbes.recordExternalMode(mode)
                    target.openNote(note.replace(File.separatorChar, '/'))
                } else {
                    if (persistPreference) target.onPersistedVault(vault)
                    val job = target.startVault(
                        vault,
                        note.replace(File.separatorChar, '/'),
                        persistPreference = persistPreference,
                    )
                    DebugProbes.recordExternalMode(mode)
                    job
                }
            }
        }
        return scope.launch {
            val loaded = withContext(Dispatchers.IO) {
                runCatching {
                    val name = displayName(context, uri) ?: "Preview.md"
                    val stream = context.contentResolver.openInputStream(uri)
                        ?: throw IllegalStateException("preview.unreadable")
                    stream.use { input ->
                        val output = ByteArrayOutputStream()
                        val buffer = ByteArray(64 * 1024)
                        while (true) {
                            val read = input.read(buffer)
                            if (read < 0) break
                            output.write(buffer, 0, read)
                            if (output.size() > MAX_PREVIEW_BYTES) {
                                throw IllegalStateException("preview.too_large")
                            }
                        }
                        name to output.toByteArray().toString(Charsets.UTF_8)
                    }
                }.getOrNull()
            }
            if (loaded == null) {
                target.showNotice(
                    context.getString(
                        if (!hasAccess && uri.scheme != "content") R.string.error_permission_denied
                        else R.string.preview_error,
                    ),
                )
            } else {
                target.openExternalPreview(loaded.first, loaded.second)
            }
        }
    }

    fun handleOpenIntent(
        intent: Intent,
        lastPath: String,
        target: IntentDispatchTarget,
    ): Job? {
        val note = intent.getStringExtra(LaunchExtras.EXTRA_OPEN_NOTE)
            ?.takeIf(::isValidRelativeNotePath)
            ?: return null
        val heading = intent.getStringExtra(LaunchExtras.EXTRA_OPEN_HEADING)
        val explicitVault = intent.getStringExtra(LaunchExtras.EXTRA_OPEN_VAULT)
        if (explicitVault != null && !isValidVaultDirectory(explicitVault)) return null
        val vault = explicitVault
            ?: target.currentVaultPath
            ?: lastPath.takeIf { it.isNotBlank() }
            ?: target.savedVaultPath
            ?: return null
        if (!isValidVaultDirectory(vault)) return null
        val persistPreference = (vault == target.currentVaultPath) ||
            (vault == target.savedVaultPath) ||
            (vault in target.knownShortcutVaults)
        if (persistPreference) target.onPersistedVault(vault)
        return if (target.currentVaultPath == vault) {
            target.openPath(note, heading)
        } else {
            target.openVaultAndPath(vault, note, heading, persistPreference)
        }
    }

    private fun resolvePhysicalPath(context: Context, uri: Uri): String? {
        return try {
            when (uri.scheme) {
                "file" -> uri.path
                "content" -> when (uri.authority) {
                    "com.android.externalstorage.documents" -> {
                        val id = DocumentsContract.getDocumentId(uri)
                        val parts = id.split(':', limit = 2)
                        if (parts.size == 2) {
                            val volume = if (parts[0].equals("primary", ignoreCase = true)) {
                                "/storage/emulated/0"
                            } else {
                                "/storage/${parts[0]}"
                            }
                            "$volume/${parts[1]}"
                        } else {
                            null
                        }
                    }
                    "com.android.providers.downloads.documents" -> {
                        val id = DocumentsContract.getDocumentId(uri)
                        id.removePrefix("raw:").takeIf { it != id }
                    }
                    else -> context.contentResolver.query(
                        uri,
                        arrayOf("_data"),
                        null,
                        null,
                        null,
                    )?.use { cursor ->
                        if (cursor.moveToFirst()) cursor.getString(0) else null
                    }
                }
                else -> null
            }
        } catch (_: Exception) {
            null
        }
    }

    private fun displayName(context: Context, uri: Uri): String? = runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
    }.getOrNull()
}
