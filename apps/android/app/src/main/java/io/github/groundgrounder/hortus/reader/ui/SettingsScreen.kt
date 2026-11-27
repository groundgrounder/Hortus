// SPDX-License-Identifier: AGPL-3.0-only
@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package io.github.groundgrounder.hortus.reader.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.core.content.pm.PackageInfoCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.groundgrounder.hortus.R
import io.github.groundgrounder.hortus.core.settings.MAX_READER_BODY_SP
import io.github.groundgrounder.hortus.core.settings.MIN_READER_BODY_SP
import io.github.groundgrounder.hortus.core.settings.ThemeMode
import io.github.groundgrounder.hortus.reader.state.ReaderViewModel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/**
 * 极简 Material 3 设置界面：外观、正文字号、清理缓存与关于，适配手机单栏与平板双栏。
 */
@Composable
internal fun SettingsScreen(viewModel: ReaderViewModel) {
    BackHandler { viewModel.closeSettings() }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbarHost = remember { SnackbarHostState() }
    val settings by viewModel.appSettings.collectAsStateWithLifecycle()
    val cacheClearedCount by viewModel.cacheClearedCount.collectAsStateWithLifecycle()
    val baselineClearedCount = remember { cacheClearedCount }
    val tablet = LocalConfiguration.current.screenWidthDp >= 600
    val clearedMessage = stringResource(R.string.settings_cache_cleared)
    val linkCopiedMessage = stringResource(R.string.settings_link_copied)

    LaunchedEffect(cacheClearedCount) {
        if (cacheClearedCount > baselineClearedCount) {
            snackbarHost.showSnackbar(clearedMessage)
        }
    }

    val versionLabel = remember(context) { appVersionLabel(context) }

    Surface(Modifier.fillMaxSize()) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(stringResource(R.string.settings_title)) },
                    navigationIcon = {
                        IconButton(onClick = viewModel::closeSettings) {
                            Icon(
                                Icons.Default.ArrowBack,
                                contentDescription = stringResource(R.string.reader_back),
                            )
                        }
                    },
                )
            },
            snackbarHost = { SnackbarHost(snackbarHost) },
        ) { insets ->
            Box(
                Modifier
                    .fillMaxSize()
                    .padding(insets)
                    .verticalScroll(rememberScrollState()),
                contentAlignment = Alignment.TopCenter,
            ) {
                if (tablet) {
                    Row(
                        Modifier
                            .widthIn(max = 920.dp)
                            .fillMaxWidth()
                            .padding(horizontal = 24.dp, vertical = 16.dp),
                        horizontalArrangement = Arrangement.spacedBy(32.dp),
                        verticalAlignment = Alignment.Top,
                    ) {
                        Column(
                            Modifier.weight(1f),
                            verticalArrangement = Arrangement.spacedBy(20.dp),
                        ) {
                            AppearanceSection(
                                current = settings.themeMode,
                                onSelect = viewModel::setThemeMode,
                            )
                            HorizontalDivider()
                            FontSizeSection(
                                currentSp = settings.fontSizeSp.roundToInt(),
                                onChangeSp = { viewModel.setFontSizeSp(it.toFloat()) },
                            )
                        }
                        Column(
                            Modifier.weight(1f),
                            verticalArrangement = Arrangement.spacedBy(20.dp),
                        ) {
                            CacheSection(onClear = viewModel::clearCaches)
                            HorizontalDivider()
                            AboutSection(
                                versionLabel = versionLabel,
                                onOpenUrl = { url ->
                                    openOrCopyUrl(context, url) {
                                        scope.launch { snackbarHost.showSnackbar(linkCopiedMessage) }
                                    }
                                },
                                onCopyUrl = { url ->
                                    copyUrl(context, url)
                                    scope.launch { snackbarHost.showSnackbar(linkCopiedMessage) }
                                },
                            )
                        }
                    }
                } else {
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 20.dp, vertical = 12.dp),
                        verticalArrangement = Arrangement.spacedBy(20.dp),
                    ) {
                        AppearanceSection(
                            current = settings.themeMode,
                            onSelect = viewModel::setThemeMode,
                        )
                        HorizontalDivider()
                        FontSizeSection(
                            currentSp = settings.fontSizeSp.roundToInt(),
                            onChangeSp = { viewModel.setFontSizeSp(it.toFloat()) },
                        )
                        HorizontalDivider()
                        CacheSection(onClear = viewModel::clearCaches)
                        HorizontalDivider()
                        AboutSection(
                            versionLabel = versionLabel,
                            onOpenUrl = { url ->
                                openOrCopyUrl(context, url) {
                                    scope.launch { snackbarHost.showSnackbar(linkCopiedMessage) }
                                }
                            },
                            onCopyUrl = { url ->
                                copyUrl(context, url)
                                scope.launch { snackbarHost.showSnackbar(linkCopiedMessage) }
                            },
                        )
                        Spacer(Modifier.height(16.dp))
                    }
                }
            }
        }
    }
}

@Composable
private fun AppearanceSection(
    current: ThemeMode,
    onSelect: (ThemeMode) -> Unit,
) {
    val options = listOf(
        ThemeMode.SYSTEM to stringResource(R.string.settings_theme_system),
        ThemeMode.LIGHT to stringResource(R.string.settings_theme_light),
        ThemeMode.DARK to stringResource(R.string.settings_theme_dark),
    )
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(
            stringResource(R.string.settings_appearance),
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
            options.forEachIndexed { index, (mode, label) ->
                SegmentedButton(
                    selected = current == mode,
                    onClick = { onSelect(mode) },
                    shape = SegmentedButtonDefaults.itemShape(index = index, count = options.size),
                ) {
                    Text(label, maxLines = 1)
                }
            }
        }
    }
}

@Composable
private fun FontSizeSection(
    currentSp: Int,
    onChangeSp: (Int) -> Unit,
) {
    val minSp = MIN_READER_BODY_SP.roundToInt()
    val maxSp = MAX_READER_BODY_SP.roundToInt()
    var pendingSp by remember(currentSp) { mutableIntStateOf(currentSp.coerceIn(minSp, maxSp)) }
    val decreaseDesc = stringResource(R.string.settings_font_decrease)
    val increaseDesc = stringResource(R.string.settings_font_increase)
    LaunchedEffect(pendingSp) {
        if (pendingSp != currentSp) {
            delay(180)
            onChangeSp(pendingSp)
        }
    }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text(
                stringResource(R.string.settings_font_size),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(
                stringResource(R.string.settings_font_size_value, pendingSp),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            OutlinedButton(
                onClick = {
                    val next = (pendingSp - 1).coerceIn(minSp, maxSp)
                    if (next != pendingSp) {
                        pendingSp = next
                        onChangeSp(next)
                    }
                },
                enabled = pendingSp > minSp,
                modifier = Modifier.semantics { contentDescription = decreaseDesc },
            ) {
                Text("-")
            }
            Slider(
                value = pendingSp.toFloat(),
                onValueChange = { raw ->
                    val rounded = raw.roundToInt().coerceIn(minSp, maxSp)
                    if (rounded != pendingSp) {
                        pendingSp = rounded
                    }
                },
                onValueChangeFinished = {
                    if (pendingSp != currentSp) {
                        onChangeSp(pendingSp)
                    }
                },
                valueRange = minSp.toFloat()..maxSp.toFloat(),
                steps = (maxSp - minSp - 1).coerceAtLeast(0),
                modifier = Modifier.weight(1f),
            )
            OutlinedButton(
                onClick = {
                    val next = (pendingSp + 1).coerceIn(minSp, maxSp)
                    if (next != pendingSp) {
                        pendingSp = next
                        onChangeSp(next)
                    }
                },
                enabled = pendingSp < maxSp,
                modifier = Modifier.semantics { contentDescription = increaseDesc },
            ) {
                Text("+")
            }
        }
    }
}

@Composable
private fun CacheSection(onClear: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(
            stringResource(R.string.settings_cache),
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )
        OutlinedButton(onClick = onClear) {
            Text(stringResource(R.string.settings_clear_cache))
        }
    }
}

@Composable
private fun AboutSection(
    versionLabel: String,
    onOpenUrl: (String) -> Unit,
    onCopyUrl: (String) -> Unit,
) {
    val repoUrl = stringResource(R.string.settings_repository_url)
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(
            stringResource(R.string.settings_about),
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )
        Text(
            stringResource(R.string.app_name),
            style = MaterialTheme.typography.titleMedium,
        )
        AboutRow(
            label = stringResource(R.string.settings_version),
            value = versionLabel,
        )
        AboutRow(
            label = stringResource(R.string.settings_license),
            value = stringResource(R.string.settings_license_value),
        )
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Column(
                Modifier
                    .weight(1f)
                    .clickable { onOpenUrl(repoUrl) }
                    .padding(vertical = 4.dp),
            ) {
                Text(
                    stringResource(R.string.settings_repository),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    repoUrl,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
            TextButton(onClick = { onCopyUrl(repoUrl) }) {
                Text(stringResource(R.string.settings_copy_link))
            }
        }
    }
}

@Composable
private fun AboutRow(label: String, value: String) {
    Column(Modifier.fillMaxWidth()) {
        Text(
            label,
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            value,
            style = MaterialTheme.typography.bodyMedium,
        )
    }
}

private fun appVersionLabel(context: Context): String = try {
    val info = context.packageManager.getPackageInfo(context.packageName, 0)
    val versionName = info.versionName ?: ""
    val versionCode = PackageInfoCompat.getLongVersionCode(info)
    if (versionName.isNotBlank()) "$versionName ($versionCode)" else "$versionCode"
} catch (_: Exception) {
    ""
}

private fun copyUrl(context: Context, url: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java)
    clipboard?.setPrimaryClip(ClipData.newPlainText("Hortus", url))
}

private fun openOrCopyUrl(context: Context, url: String, onCopiedFallback: () -> Unit) {
    val launched = runCatching {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
    }.isSuccess
    if (!launched) {
        copyUrl(context, url)
        onCopiedFallback()
    }
}
