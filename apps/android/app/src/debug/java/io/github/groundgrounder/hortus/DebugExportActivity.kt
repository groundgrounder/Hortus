// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.DocumentsContract
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.lifecycle.lifecycleScope
import io.github.groundgrounder.hortus.core.AndroidFonts
import io.github.groundgrounder.hortus.core.DiagramEngine
import io.github.groundgrounder.hortus.reader.render.DiagramExporter
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** 仅用于仪器测试：走与菜单相同的 PNG 编码路径并触发系统“创建文档”。 */
class DebugExportActivity : ComponentActivity() {
    private var bytes: ByteArray? = null
    private val launcher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val uri = result.data?.data
        val value = bytes
        if (result.resultCode == Activity.RESULT_OK && uri != null && value != null) {
            lifecycleScope.launch {
                withContext(Dispatchers.IO) {
                    contentResolver.openOutputStream(uri, "w")?.use { it.write(value) }
                }
                setResult(RESULT_OK)
                finish()
            }
        } else {
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        lifecycleScope.launch {
            val scene = withContext(Dispatchers.IO) {
                val paths = AndroidFonts.discover().mapNotNull { it.file?.path }.distinct()
                val engine = DiagramEngine.withFontPaths(paths)
                try {
                    engine.render(
                        "flowchart LR\n  A[Alpha] --> B[Beta]\n  B --> C[Gamma]\n",
                        true,
                        1u,
                        640.0,
                        false,
                        emptyList(),
                    )
                } finally {
                    engine.close()
                }
            }
            bytes = withContext(Dispatchers.IO) { DiagramExporter.encodePng(scene, false) }
            val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "image/png"
                putExtra(Intent.EXTRA_TITLE, "m1-document-export.png")
                putExtra(
                    DocumentsContract.EXTRA_INITIAL_URI,
                    Uri.parse("content://com.android.externalstorage.documents/document/primary%3ADownload"),
                )
            }
            launcher.launch(intent)
        }
    }
}
