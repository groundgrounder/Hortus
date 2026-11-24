// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.view.WindowCompat
import androidx.lifecycle.lifecycleScope
import io.github.groundgrounder.hortus.reader.ReaderApp
import io.github.groundgrounder.hortus.reader.ReaderController
import io.github.groundgrounder.hortus.ui.HortusTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

class MainActivity : ComponentActivity() {
    companion object {
        const val EXTRA_OPEN_NOTE = "io.github.groundgrounder.hortus.OPEN_NOTE"
        const val EXTRA_OPEN_HEADING = "io.github.groundgrounder.hortus.OPEN_HEADING"
        const val EXTRA_OPEN_VAULT = "io.github.groundgrounder.hortus.OPEN_VAULT"
    }

    internal lateinit var controller: ReaderController
        private set
    internal var pendingIntentJob: Job? = null
        private set
    private var hasAccess by mutableStateOf(false)
    private var path by mutableStateOf("")

    private val settings = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        refreshPermission()
    }
    private val legacyPermission =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            refreshPermission()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        CoreEnvironment.initialize(this)
        controller = ReaderController(this)
        controller.markActivityCreated()
        enableEdgeToEdge()
        refreshPermission()
        setContent {
            val dark = isSystemInDarkTheme()
            SideEffect {
                WindowCompat.getInsetsController(window, window.decorView).apply {
                    isAppearanceLightStatusBars = !dark
                    isAppearanceLightNavigationBars = !dark
                }
            }
            HortusTheme(darkTheme = dark) {
                ReaderApp(
                    controller = controller,
                    hasAccess = hasAccess,
                    onRequestPermission = ::requestPermission,
                )
            }
        }
        val handledExternal = if (intent.action == Intent.ACTION_VIEW && intent.data != null) {
            handleExternalIntent(intent)
            true
        } else {
            false
        }
        val handledDebug = DebugHooks.handlesLaunch(intent)
        if (!handledExternal && !handledDebug) {
            if (intent.getStringExtra(EXTRA_OPEN_NOTE) != null) {
                handleOpenIntent(intent)
            } else {
                openSavedOrRequested()
            }
        }
        DebugHooks.onActivityCreate(this, intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        val handledExternal = if (intent.action == Intent.ACTION_VIEW && intent.data != null) {
            handleExternalIntent(intent)
            true
        } else {
            false
        }
        if (!handledExternal && !DebugHooks.handlesLaunch(intent)) {
            handleOpenIntent(intent)
        }
        DebugHooks.onNewIntent(this, intent)
    }

    override fun onResume() {
        super.onResume()
        refreshPermission()
        controller.setForeground(true)
        if (controller.state.content != null) {
            controller.refresh()
        }
    }

    override fun onPause() {
        controller.setForeground(false)
        super.onPause()
    }

    override fun onStop() {
        controller.onActivityStop()
        super.onStop()
    }

    override fun onDestroy() {
        controller.close()
        super.onDestroy()
    }

    private fun isStorageRoot(directory: File): Boolean {
        val canonical = runCatching { directory.canonicalFile }.getOrNull() ?: return true
        val rootPath = canonical.path
        if (rootPath == "/" || rootPath == "/storage" || rootPath == "/storage/emulated" ||
            rootPath == "/storage/emulated/0" || rootPath == "/sdcard" ||
            rootPath == "/mnt" || rootPath == "/mnt/sdcard"
        ) {
            return true
        }
        val externalRoot = runCatching {
            android.os.Environment.getExternalStorageDirectory()?.canonicalPath
        }.getOrNull()
        if (externalRoot != null && rootPath == externalRoot) return true
        if (canonical.parentFile?.path == "/storage" && canonical.name != "emulated") return true
        return false
    }

    private fun isValidRelativeNotePath(note: String): Boolean {
        if (note.isBlank() || '\u0000' in note) return false
        if (note.startsWith('/') || note.startsWith('\\')) return false
        return note.split('/', '\\').none { it == ".." || it == "." }
    }

    private fun isValidVaultDirectory(vault: String): Boolean {
        if (vault.isBlank() || '\u0000' in vault) return false
        if (vault.split('/', '\\').any { it == ".." }) return false
        val dir = File(vault)
        return dir.isAbsolute && dir.isDirectory && !isStorageRoot(dir)
    }

    private fun handleExternalIntent(intent: Intent) {
        val uri = intent.data ?: return
        val physical = resolvePhysicalPath(uri)
        if (physical != null && File(physical).isFile) {
            if (!hasAccess) {
                controller.notice(getString(R.string.reader_open_failed, "io.permission_denied"))
                return
            }
            val file = File(physical)
            val canonical = runCatching { file.canonicalPath }.getOrNull() ?: return
            val known = controller.state.vaultPath ?: controller.preferencesState.vaultPath
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
                val parent = file.parentFile ?: return
                if (isStorageRoot(parent)) {
                    pendingIntentJob = lifecycleScope.launch {
                        val loaded = withContext(Dispatchers.IO) {
                            runCatching {
                                if (file.length() > 8L * 1024 * 1024) {
                                    throw IllegalStateException("preview.too_large")
                                }
                                file.name to file.readText(Charsets.UTF_8)
                            }.getOrNull()
                        }
                        if (loaded == null) {
                            controller.notice(getString(R.string.preview_error))
                        } else {
                            controller.openExternalPreview(loaded.first, loaded.second)
                        }
                    }
                    return
                }
                vault = parent.absolutePath
                note = file.name
                mode = "parent_vault"
                persistPreference = false
            }
            if (controller.state.vaultPath == vault) {
                DebugProbes.recordExternalMode(mode)
                pendingIntentJob = controller.openNote(note.replace(File.separatorChar, '/'))
            } else {
                if (persistPreference) path = vault
                pendingIntentJob = controller.start(
                    vault,
                    note.replace(File.separatorChar, '/'),
                    persistPreference = persistPreference,
                )
                DebugProbes.recordExternalMode(mode)
            }
            return
        }
        pendingIntentJob = lifecycleScope.launch {
            val loaded = withContext(Dispatchers.IO) {
                runCatching {
                    val name = displayName(uri) ?: "Preview.md"
                    val stream = contentResolver.openInputStream(uri)
                        ?: throw IllegalStateException("preview.unreadable")
                    stream.use { input ->
                        val output = java.io.ByteArrayOutputStream()
                        val buffer = ByteArray(64 * 1024)
                        while (true) {
                            val read = input.read(buffer)
                            if (read < 0) break
                            output.write(buffer, 0, read)
                            if (output.size() > 8 * 1024 * 1024) {
                                throw IllegalStateException("preview.too_large")
                            }
                        }
                        name to output.toByteArray().toString(Charsets.UTF_8)
                    }
                }.getOrNull()
            }
            if (loaded == null) {
                controller.notice(getString(R.string.preview_error))
            } else {
                controller.openExternalPreview(loaded.first, loaded.second)
            }
        }
    }

    private fun resolvePhysicalPath(uri: Uri): String? {
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
                    else -> contentResolver.query(
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

    private fun displayName(uri: Uri): String? = runCatching {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
    }.getOrNull()

    private fun handleOpenIntent(intent: Intent) {
        val note = intent.getStringExtra(EXTRA_OPEN_NOTE)?.takeIf(::isValidRelativeNotePath) ?: return
        val heading = intent.getStringExtra(EXTRA_OPEN_HEADING)
        val explicitVault = intent.getStringExtra(EXTRA_OPEN_VAULT)
        if (explicitVault != null && !isValidVaultDirectory(explicitVault)) return
        val vault = explicitVault
            ?: controller.state.vaultPath
            ?: path.takeIf { it.isNotBlank() }
            ?: controller.preferencesState.vaultPath
            ?: return
        if (!isValidVaultDirectory(vault)) return
        val savedVault = controller.preferencesState.vaultPath
        val knownShortcut = controller.shortcutNotes().any { it.vault == vault }
        val persistPreference = (vault == controller.state.vaultPath) ||
            (vault == savedVault) ||
            knownShortcut
        if (persistPreference) path = vault
        pendingIntentJob = if (controller.state.vaultPath == vault) {
            controller.openPath(note, heading)
        } else {
            controller.openVaultAndPath(vault, note, heading, persistPreference)
        }
    }

    private fun openSavedOrRequested() {
        if (!hasAccess) return
        if (controller.state.vaultPath != null || controller.state.loading) return
        val saved = controller.preferencesState
        val requested = intent?.getStringExtra(EXTRA_OPEN_NOTE)
        if (saved.vaultPath != null) {
            path = saved.vaultPath
            controller.start(saved.vaultPath, requested ?: saved.notePath)
        }
    }

    private fun refreshPermission() {
        hasAccess = StoragePermissions.hasAllFilesAccess(this)
    }

    private fun requestPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            settings.launch(StoragePermissions.buildAllFilesAccessIntent(this))
        } else {
            legacyPermission.launch(
                arrayOf(
                    Manifest.permission.READ_EXTERNAL_STORAGE,
                    Manifest.permission.WRITE_EXTERNAL_STORAGE,
                ),
            )
        }
    }
}
