// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.Manifest
import android.os.Build
import android.os.Bundle
import android.os.Environment
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
import io.github.groundgrounder.hortus.ui.FoundationScreen
import io.github.groundgrounder.hortus.ui.HortusTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

open class MainActivity : ComponentActivity() {
    private var hasAccess by mutableStateOf(false)
    private var path by mutableStateOf("")
    private var running by mutableStateOf(false)
    private var failed by mutableStateOf(false)
    private var result by mutableStateOf<FoundationResult?>(null)
    private var preview by mutableStateOf<NotePreview?>(null)

    private val settings = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        refreshPermission()
    }
    private val legacyPermission =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            refreshPermission()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        path = savedInstanceState?.getString("vaultPath") ?: File(
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS),
            "HortusDemoVault",
        ).path
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
                FoundationScreen(
                    hasAccess = hasAccess,
                    grantAccess = ::requestPermission,
                    path = path,
                    setPath = { path = it },
                    running = running,
                    failed = failed,
                    result = result,
                    preview = preview,
                    open = { launchChecks(path.trim(), testSave = false) },
                    testSave = { result?.let { launchChecks(it.vaultPath, testSave = true) } },
                    openNote = ::openNote,
                    closePreview = { preview = null },
                )
            }
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("vaultPath", path)
        super.onSaveInstanceState(outState)
    }

    override fun onResume() {
        super.onResume()
        refreshPermission()
    }

    private fun refreshPermission() {
        hasAccess = StoragePermissions.hasAllFilesAccess(this)
        if (!hasAccess) {
            result = null
            preview = null
        }
    }

    private fun requestPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            settings.launch(StoragePermissions.buildAllFilesAccessIntent(this))
        } else {
            legacyPermission.launch(arrayOf(
                Manifest.permission.READ_EXTERNAL_STORAGE,
                Manifest.permission.WRITE_EXTERNAL_STORAGE,
            ))
        }
    }

    internal fun launchChecks(
        vaultPath: String,
        testSave: Boolean,
        publish: (FoundationResult) -> Unit = {},
    ) {
        if (running || !hasAccess || !File(vaultPath).isAbsolute) return
        path = vaultPath
        running = true
        failed = false
        result = null
        preview = null
        lifecycleScope.launch {
            try {
                val completed = FoundationChecks.run(applicationContext, vaultPath, testSave)
                if (hasAccess) result = completed
                withContext(Dispatchers.IO) { publish(completed) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                failed = true
            } finally {
                running = false
            }
        }
    }

    private fun openNote(relativePath: String) {
        val vaultPath = result?.vaultPath ?: return
        lifecycleScope.launch {
            val excerpt = withContext(Dispatchers.IO) {
                try {
                    CoreEnvironment.createWorkspaceCore(applicationContext).use { workspace ->
                        workspace.openDocument(vaultPath, relativePath).use { document ->
                            document.snapshot().text.trim().take(900)
                        }
                    }
                } catch (e: Exception) {
                    null
                }
            }
            if (hasAccess && result?.vaultPath == vaultPath) {
                preview = NotePreview(relativePath, excerpt)
            }
        }
    }
}

internal data class NotePreview(val path: String, val excerpt: String?)
