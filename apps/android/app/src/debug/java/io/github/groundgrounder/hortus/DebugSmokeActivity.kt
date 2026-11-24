// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.lifecycleScope
import io.github.groundgrounder.hortus.ui.HortusTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/** Debug 专用烟测入口：独立于 MainActivity，直接校验存储权限并运行 M0 内核检查。 */
class DebugSmokeActivity : ComponentActivity() {
    private var statusMessage by mutableStateOf("Running foundation checks...")

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            HortusTheme {
                Scaffold { insets ->
                    Box(
                        Modifier.fillMaxSize().padding(insets),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(statusMessage)
                    }
                }
            }
        }
        val vaultPath = intent.getStringExtra("vault_path")
        val outputName = intent.getStringExtra("smoke_output")
        if (!intent.getBooleanExtra("run_smoke", false) ||
            vaultPath == null ||
            !File(vaultPath).isAbsolute ||
            outputName == null ||
            !outputName.matches(Regex("[A-Za-z0-9_-]{1,100}\\.json"))
        ) {
            finish()
            return
        }
        if (!StoragePermissions.hasAllFilesAccess(this)) {
            statusMessage = getString(R.string.permission_title)
            finish()
            return
        }
        lifecycleScope.launch {
            try {
                val completed = FoundationChecks.run(applicationContext, vaultPath, testSave = true)
                withContext(Dispatchers.IO) {
                    val directory = checkNotNull(getExternalFilesDir(null)).toPath()
                    val staged = Files.createTempFile(directory, ".hortus-smoke-", ".tmp")
                    try {
                        staged.toFile().writeText(completed.report.toString(2) + "\n")
                        check(staged.toFile().setReadable(true, false)) {
                            "Cannot make the report readable by adb"
                        }
                        Files.move(
                            staged,
                            directory.resolve(outputName),
                            StandardCopyOption.ATOMIC_MOVE,
                            StandardCopyOption.REPLACE_EXISTING,
                        )
                    } finally {
                        Files.deleteIfExists(staged)
                    }
                }
                statusMessage = if (completed.report.optBoolean("allPassed")) {
                    "Foundation checks passed"
                } else {
                    "Foundation checks failed"
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                statusMessage = e.message ?: "Foundation checks failed"
            }
        }
    }
}
