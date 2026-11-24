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
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.view.WindowCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import io.github.groundgrounder.hortus.app.IntentDispatchTarget
import io.github.groundgrounder.hortus.app.IntentRouter
import io.github.groundgrounder.hortus.app.StoragePermissions
import io.github.groundgrounder.hortus.core.CoreEnvironment
import io.github.groundgrounder.hortus.core.settings.LaunchExtras
import io.github.groundgrounder.hortus.core.settings.ThemeMode
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel
import io.github.groundgrounder.hortus.reader.ui.ReaderApp
import io.github.groundgrounder.hortus.ui.HortusTheme
import kotlinx.coroutines.Job

/**
 * 主入口瘦宿主：持有 ViewModel 并在配置变更期间保持原生库与文档会话不丢失、不泄漏。
 */
class MainActivity : ComponentActivity() {
    companion object {
        const val EXTRA_OPEN_NOTE = LaunchExtras.EXTRA_OPEN_NOTE
        const val EXTRA_OPEN_HEADING = LaunchExtras.EXTRA_OPEN_HEADING
        const val EXTRA_OPEN_VAULT = LaunchExtras.EXTRA_OPEN_VAULT
    }

    internal val viewModel: ReaderViewModel by viewModels()
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

    private val dispatchTarget = object : IntentDispatchTarget {
        override val currentVaultPath: String? get() = viewModel.readerState.value.vaultPath
        override val savedVaultPath: String? get() = viewModel.preferencesState.vaultPath
        override val knownShortcutVaults: Set<String> get() = viewModel.knownShortcutVaults()
        override fun openNote(path: String, heading: String?): Job = viewModel.openNote(path, heading)
        override fun startVault(vaultPath: String, notePath: String?, persistPreference: Boolean): Job? =
            viewModel.start(vaultPath, notePath, persistPreference = persistPreference)
        override fun openPath(path: String, heading: String?): Job? = viewModel.openPath(path, heading)
        override fun openVaultAndPath(
            vaultPath: String,
            notePath: String,
            heading: String?,
            persistPreference: Boolean,
        ): Job = viewModel.openVaultAndPath(vaultPath, notePath, heading, persistPreference)
        override fun openExternalPreview(fileName: String, text: String): Job =
            viewModel.openExternalPreview(fileName, text)
        override fun showNotice(message: String) = viewModel.notice(message)
        override fun onPersistedVault(vaultPath: String) {
            path = vaultPath
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        CoreEnvironment.initialize(this)
        viewModel.markActivityCreated()
        enableEdgeToEdge()
        refreshPermission()
        setContent {
            val appSettings by viewModel.appSettings.collectAsStateWithLifecycle()
            val dark = when (appSettings.themeMode) {
                ThemeMode.SYSTEM -> isSystemInDarkTheme()
                ThemeMode.LIGHT -> false
                ThemeMode.DARK -> true
            }
            SideEffect {
                WindowCompat.getInsetsController(window, window.decorView).apply {
                    isAppearanceLightStatusBars = !dark
                    isAppearanceLightNavigationBars = !dark
                }
            }
            HortusTheme(darkTheme = dark) {
                ReaderApp(
                    viewModel = viewModel,
                    hasAccess = hasAccess,
                    onRequestPermission = ::requestPermission,
                )
            }
        }
        if (savedInstanceState == null) {
            val handledExternal = if (intent.action == Intent.ACTION_VIEW && intent.data != null) {
                pendingIntentJob = IntentRouter.handleExternalIntent(
                    context = this,
                    scope = lifecycleScope,
                    intent = intent,
                    hasAccess = hasAccess,
                    target = dispatchTarget,
                )
                true
            } else {
                false
            }
            val handledDebug = DebugHooks.handlesLaunch(intent)
            if (!handledExternal && !handledDebug) {
                if (intent.getStringExtra(EXTRA_OPEN_NOTE) != null) {
                    pendingIntentJob = IntentRouter.handleOpenIntent(intent, path, dispatchTarget)
                } else {
                    openSavedOrRequested()
                }
            }
            DebugHooks.onActivityCreate(this, intent)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        val handledExternal = if (intent.action == Intent.ACTION_VIEW && intent.data != null) {
            pendingIntentJob = IntentRouter.handleExternalIntent(
                context = this,
                scope = lifecycleScope,
                intent = intent,
                hasAccess = hasAccess,
                target = dispatchTarget,
            )
            true
        } else {
            false
        }
        if (!handledExternal && !DebugHooks.handlesLaunch(intent)) {
            pendingIntentJob = IntentRouter.handleOpenIntent(intent, path, dispatchTarget)
        }
        DebugHooks.onNewIntent(this, intent)
    }

    override fun onResume() {
        super.onResume()
        refreshPermission()
        viewModel.setForeground(true)
        if (viewModel.readerState.value.content != null) {
            viewModel.refresh()
        }
    }

    override fun onPause() {
        viewModel.setForeground(false)
        super.onPause()
    }

    override fun onStop() {
        viewModel.onActivityStop()
        super.onStop()
    }

    private fun openSavedOrRequested() {
        if (!hasAccess) return
        val current = viewModel.readerState.value
        if (current.vaultPath != null || current.loading) return
        val saved = viewModel.preferencesState
        val requested = intent?.getStringExtra(EXTRA_OPEN_NOTE)
        if (saved.vaultPath != null) {
            path = saved.vaultPath
            viewModel.start(saved.vaultPath, requested ?: saved.notePath)
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
