// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.os.Bundle
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption

class DebugSmokeActivity : MainActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val vaultPath = intent.getStringExtra("vault_path")
        val outputName = intent.getStringExtra("smoke_output")
        if (!intent.getBooleanExtra("run_smoke", false) || vaultPath == null ||
            !File(vaultPath).isAbsolute || outputName == null ||
            !outputName.matches(Regex("[A-Za-z0-9_-]{1,100}\\.json"))
        ) {
            finish()
            return
        }
        launchChecks(vaultPath, testSave = true) { completed ->
            val directory = checkNotNull(getExternalFilesDir(null)).toPath()
            val staged = Files.createTempFile(directory, ".hortus-smoke-", ".tmp")
            try {
                staged.toFile().writeText(completed.report.toString(2) + "\n")
                check(staged.toFile().setReadable(true, false)) { "Cannot make the report readable by adb" }
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
    }
}
