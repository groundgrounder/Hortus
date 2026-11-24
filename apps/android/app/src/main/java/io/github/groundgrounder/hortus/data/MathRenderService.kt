// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.data

import android.content.Context
import io.github.groundgrounder.hortus.DebugProbes
import io.github.groundgrounder.hortus.core.MathBlock
import io.github.groundgrounder.hortus.core.MathEngine
import io.github.groundgrounder.hortus.core.mathTypographyScale
import io.github.groundgrounder.hortus.reader.render.MathRenderResult
import io.github.groundgrounder.hortus.reader.render.MathSceneRenderer
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * 进程共享公式后台排版与缓存服务，拥有独立互斥锁，不与文档/图表仓储争抢。
 */
internal class MathRenderService(context: Context) : AutoCloseable {
    private val appContext = context.applicationContext
    private val mutex = Mutex()
    private var renderer: MathSceneRenderer? = null

    private fun ensureRenderer(): MathSceneRenderer {
        renderer?.let { return it }
        val engine = DebugProbes.traceMainThread("mathRender") {
            MathEngine()
        }
        return MathSceneRenderer(appContext.assets, engine).also { renderer = it }
    }

    suspend fun prepareMath(
        formulas: List<MathBlock>,
        bodySize: Float,
        notePath: String,
    ): List<MathRenderResult> = withContext(Dispatchers.IO) {
        if (formulas.isEmpty()) return@withContext emptyList()
        mutex.withLock {
            val active = ensureRenderer()
            val em = (bodySize * mathTypographyScale()).toFloat()
            formulas.map { formula ->
                val result = if (formula.latex.isBlank()) {
                    MathRenderResult(null, null, 0)
                } else {
                    active.render(formula.latex, formula.style, em)
                }
                DebugProbes.recordMath(notePath, formula, result, bodySize)
                result
            }
        }
    }

    fun clearCache() {
        renderer?.clearCache()
    }

    override fun close() {
        try {
            renderer?.close()
        } catch (_: Exception) {
            Unit
        }
        renderer = null
    }
}
