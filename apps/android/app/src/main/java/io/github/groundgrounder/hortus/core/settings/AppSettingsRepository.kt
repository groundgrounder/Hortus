// SPDX-License-Identifier: GPL-3.0-only
package io.github.groundgrounder.hortus.core.settings

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption

internal const val DEFAULT_READER_BODY_SP = 17f
internal const val MIN_READER_BODY_SP = 12f
internal const val MAX_READER_BODY_SP = 28f

internal enum class ThemeMode(val serialized: String) {
    SYSTEM("system"),
    LIGHT("light"),
    DARK("dark");

    companion object {
        fun fromSerialized(value: String?): ThemeMode =
            entries.firstOrNull { it.serialized.equals(value, ignoreCase = true) } ?: SYSTEM
    }
}

internal data class AppSettings(
    val themeMode: ThemeMode = ThemeMode.SYSTEM,
    val fontSizeSp: Float = DEFAULT_READER_BODY_SP,
)

internal data class ReaderPreferenceState(val vaultPath: String?, val notePath: String?)

/**
 * 统一管理应用私有配置：
 * - 上次打开的库与笔记：`filesDir/config/reader-state.json`
 * - M2b 主题与字号设置：`filesDir/config/android-settings.json`
 * 构造阶段不做主线程磁盘 IO，所有文件读写均收敛至 IO 线程与原子替换。
 */
internal class AppSettingsRepository(context: Context) {
    private val directory = File(context.applicationContext.filesDir, "config")
    private val readerStateFile = File(directory, "reader-state.json")
    private val settingsFile = File(directory, "android-settings.json")
    private val ioLock = Any()

    private val _readerState = MutableStateFlow(ReaderPreferenceState(null, null))
    val readerState: StateFlow<ReaderPreferenceState> = _readerState.asStateFlow()

    private val _settings = MutableStateFlow(AppSettings())
    val settings: StateFlow<AppSettings> = _settings.asStateFlow()

    @Volatile
    private var preloaded = false

    val isPreloaded: Boolean get() = preloaded

    suspend fun preload() = withContext(Dispatchers.IO) {
        synchronized(ioLock) {
            if (preloaded) return@withContext
            _readerState.value = readReaderStateLocked()
            _settings.value = readSettingsLocked()
            preloaded = true
        }
    }

    fun currentReaderState(): ReaderPreferenceState = _readerState.value

    suspend fun loadReaderState(): ReaderPreferenceState = withContext(Dispatchers.IO) {
        synchronized(ioLock) {
            readReaderStateLocked().also {
                _readerState.value = it
                preloaded = true
            }
        }
    }

    suspend fun saveReaderState(vaultPath: String?, notePath: String?) = withContext(Dispatchers.IO) {
        synchronized(ioLock) {
            val next = ReaderPreferenceState(vaultPath, notePath)
            _readerState.value = next
            preloaded = true
            writeJsonAtomically(
                readerStateFile,
                ".reader-state.tmp",
                JSONObject()
                    .put("vault", vaultPath ?: "")
                    .put("note", notePath ?: ""),
            )
        }
    }

    suspend fun setThemeMode(mode: ThemeMode) = withContext(Dispatchers.IO) {
        synchronized(ioLock) {
            val next = _settings.value.copy(themeMode = mode)
            _settings.value = next
            preloaded = true
            writeSettingsLocked(next)
        }
    }

    suspend fun setFontSizeSp(fontSizeSp: Float) = withContext(Dispatchers.IO) {
        val clamped = fontSizeSp.coerceIn(MIN_READER_BODY_SP, MAX_READER_BODY_SP)
        synchronized(ioLock) {
            val next = _settings.value.copy(fontSizeSp = clamped)
            _settings.value = next
            preloaded = true
            writeSettingsLocked(next)
        }
    }

    private fun readReaderStateLocked(): ReaderPreferenceState = try {
        if (!readerStateFile.isFile) {
            ReaderPreferenceState(null, null)
        } else {
            val json = JSONObject(readerStateFile.readText())
            ReaderPreferenceState(
                json.optString("vault").takeIf { it.isNotBlank() },
                json.optString("note").takeIf { it.isNotBlank() },
            )
        }
    } catch (_: Exception) {
        ReaderPreferenceState(null, null)
    }

    private fun readSettingsLocked(): AppSettings = try {
        if (!settingsFile.isFile) {
            AppSettings()
        } else {
            val json = JSONObject(settingsFile.readText())
            val mode = ThemeMode.fromSerialized(json.optString("themeMode"))
            val rawSize = json.optDouble("fontSizeSp", DEFAULT_READER_BODY_SP.toDouble()).toFloat()
            val size = if (rawSize.isFinite()) {
                rawSize.coerceIn(MIN_READER_BODY_SP, MAX_READER_BODY_SP)
            } else {
                DEFAULT_READER_BODY_SP
            }
            AppSettings(themeMode = mode, fontSizeSp = size)
        }
    } catch (_: Exception) {
        AppSettings()
    }

    private fun writeSettingsLocked(value: AppSettings) {
        writeJsonAtomically(
            settingsFile,
            ".android-settings.tmp",
            JSONObject()
                .put("themeMode", value.themeMode.serialized)
                .put("fontSizeSp", value.fontSizeSp.toDouble()),
        )
    }

    private fun writeJsonAtomically(target: File, tempName: String, json: JSONObject) {
        try {
            directory.mkdirs()
            val temporary = File(directory, tempName)
            temporary.writeText(json.toString(2) + "\n")
            Files.move(
                temporary.toPath(),
                target.toPath(),
                StandardCopyOption.REPLACE_EXISTING,
            )
        } catch (_: Exception) {
            Unit
        }
    }
}
