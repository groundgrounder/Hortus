// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.Manifest
import android.content.Intent
import android.os.Build
import android.os.Bundle
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
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

open class MainActivity : ComponentActivity() {
    companion object {
        const val EXTRA_OPEN_NOTE = "io.github.groundgrounder.hortus.OPEN_NOTE"
        const val EXTRA_OPEN_HEADING = "io.github.groundgrounder.hortus.OPEN_HEADING"
        const val EXTRA_OPEN_VAULT = "io.github.groundgrounder.hortus.OPEN_VAULT"
    }

    internal lateinit var controller: ReaderController
        private set
    private var hasAccess by mutableStateOf(false)
    private var path by mutableStateOf("")
    private var running by mutableStateOf(false)
    private var failed by mutableStateOf(false)
    private var result by mutableStateOf<FoundationResult?>(null)

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
        DebugHooks.onActivityCreate(this, intent)
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
        if (intent.getBooleanExtra("run_smoke", false)) {
            // DebugSmokeActivity 子类自行处理；MainActivity 不启动任何检查。
        } else if (intent.getStringExtra("debug_vault") == null) {
            if (intent.getStringExtra(EXTRA_OPEN_NOTE) != null) {
                handleOpenIntent(intent)
            } else {
                openSavedOrRequested()
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleOpenIntent(intent)
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

    override fun onDestroy() {
        if (isFinishing) controller.close()
        super.onDestroy()
    }

    private fun handleOpenIntent(intent: Intent) {
        val note = intent.getStringExtra(EXTRA_OPEN_NOTE) ?: return
        val heading = intent.getStringExtra(EXTRA_OPEN_HEADING)
        val vault = intent.getStringExtra(EXTRA_OPEN_VAULT)?.takeIf { it.isNotBlank() }
            ?: controller.state.vaultPath
            ?: path.takeIf { it.isNotBlank() }
            ?: controller.preferencesState.vaultPath
            ?: return
        path = vault
        if (controller.state.vaultPath == null) {
            controller.start(vault, null)
            lifecycleScope.launch {
                var waited = 0
                while (controller.state.vaultPath == null && waited < 5000) {
                    kotlinx.coroutines.delay(50)
                    waited += 50
                }
                controller.openPath(note, heading)
            }
        } else {
            controller.openPath(note, heading)
        }
    }

    private fun openSavedOrRequested() {
        if (!hasAccess) return
        if (intent?.getStringExtra("debug_vault") != null) return
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

    internal fun openDebugVault(vault: String, note: String?) {
        if (!hasAccess) return
        path = vault
        controller.start(vault, note)
    }

    internal fun openDebugNote(note: String, heading: String? = null) {
        if (!hasAccess) return
        if (controller.state.vaultPath == null) {
            val vault = path.ifBlank { controller.preferencesState.vaultPath }
            if (vault != null) controller.start(vault, note) else return
        } else {
            controller.openPath(note, heading)
        }
    }

    internal fun launchChecks(
        vaultPath: String,
        testSave: Boolean,
        publish: (FoundationResult) -> Unit = {},
    ) {
        if (running || !hasAccess || !java.io.File(vaultPath).isAbsolute) return
        path = vaultPath
        running = true
        failed = false
        result = null
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
}
