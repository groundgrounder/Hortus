// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus

import android.content.Context
import android.system.Os
import io.github.groundgrounder.hortus.core.WorkspaceCore
import java.io.File

/**
 * 在加载 Rust 内核前配置应用私有目录，保证不向笔记文件夹写入任何配置或缓存。
 */
object CoreEnvironment {
    @Volatile
    private var initialized = false

    lateinit var homeDir: File
        private set
    lateinit var configDir: File
        private set
    lateinit var cacheDir: File
        private set
    lateinit var dataDir: File
        private set

    @Synchronized
    fun initialize(context: Context) {
        if (initialized) return
        val appContext = context.applicationContext
        homeDir = File(appContext.filesDir, "home").apply { mkdirs() }
        configDir = File(appContext.filesDir, "config").apply { mkdirs() }
        cacheDir = File(appContext.cacheDir, "hortus").apply { mkdirs() }
        dataDir = File(appContext.filesDir, "data").apply { mkdirs() }

        Os.setenv("HOME", homeDir.absolutePath, true)
        Os.setenv("HORTUS_CONFIG_DIR", configDir.absolutePath, true)
        Os.setenv("HORTUS_CACHE_DIR", cacheDir.absolutePath, true)
        Os.setenv("HORTUS_DATA_DIR", dataDir.absolutePath, true)
        initialized = true
    }

    fun createWorkspaceCore(context: Context): WorkspaceCore {
        initialize(context)
        return WorkspaceCore.inDirectories(
            config = configDir.absolutePath,
            cache = cacheDir.absolutePath,
        )
    }
}
