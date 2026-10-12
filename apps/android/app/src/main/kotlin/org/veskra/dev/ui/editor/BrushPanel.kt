package org.veskra.dev.ui.editor

import android.graphics.Bitmap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.StringRes
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.BrushFiles
import org.veskra.kit.BrushImportReport
import org.veskra.kit.BrushInfo
import kotlin.math.pow
import kotlin.math.roundToInt

@StringRes
private fun categoryName(category: String): Int? = when (category) {
    "pencil" -> R.string.brush_category_pencil
    "ink" -> R.string.brush_category_ink
    "marker" -> R.string.brush_category_marker
    "paint" -> R.string.brush_category_paint
    "airbrush" -> R.string.brush_category_airbrush
    "texture" -> R.string.brush_category_texture
    "effect" -> R.string.brush_category_effect
    "smudge" -> R.string.brush_category_smudge
    "eraser" -> R.string.brush_category_eraser
    "custom" -> R.string.brush_category_custom
    "imported" -> R.string.brush_category_imported
    else -> null
}

/** Groups for the tab row: built-in categories, then "My brushes", then one group per imported pack. */
private fun groupOf(b: BrushInfo): String = when {
    b.isImported -> "imported:" + (b.folder ?: "")
    b.source == "custom" -> "custom"
    else -> b.category
}

private fun groupOrder(group: String): Int {
    val base = group.substringBefore(':')
    val i = BrushInfo.CATEGORY_ORDER.indexOf(base)
    return if (i >= 0) i else BrushInfo.CATEGORY_ORDER.size
}

/**
 * Brush panel (DSN-01): the brush library grouped by category (built-in, the user's own and
 * imported packs) with stroke thumbnails rendered by the engine, plus size, opacity and smoothing of
 * the current brush. While erasing only erasers are listed; while smudging, the smudge settings.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun BrushPanel(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    val import = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri -> if (uri != null) vm.importBrushes(uri) }
    var exporting by remember { mutableStateOf<BrushInfo?>(null) }
    val export = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        val b = exporting
        if (uri != null && b != null) vm.exportBrush(b, uri)
        exporting = null
    }

    FloatingPanel(modifier.widthIn(max = 520.dp)) {
        Column(
            Modifier.padding(16.dp).heightIn(max = 560.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (state.smudge) {
                // Smudge moves the paint that is already there: it has a size and a strength, no preset.
                PanelLabel(stringResource(R.string.tool_smudge))
                SizeSlider(state.brushSize, vm::setBrushSize)
                FractionSlider(stringResource(R.string.smudge_strength), state.smudgeStrength, vm::setSmudgeStrength)
                Text(stringResource(R.string.smudge_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                return@Column
            }

            val erasers = state.brush.eraser
            val list = state.brushes.ifEmpty { listOf(state.brush) }.filter { it.eraser == erasers }
            val groups = list.groupBy(::groupOf).toSortedMap(compareBy<String>({ groupOrder(it) }, { it }))
            var group by rememberSaveable(erasers) { mutableStateOf(groupOf(state.brush)) }
            if (group !in groups) group = groups.keys.firstOrNull() ?: ""

            Row(verticalAlignment = Alignment.CenterVertically) {
                PanelLabel(stringResource(if (erasers) R.string.brush_group_erasers else R.string.brush_sheet_title), Modifier.weight(1f))
                TextButton(onClick = { vm.openBrushEditor(state.brush) }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Text(stringResource(R.string.brush_new))
                }
                OutlinedButton(onClick = { import.launch(arrayOf("application/zip", "application/octet-stream", "*/*")) }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Icon(VeskraIcons.Add, contentDescription = null)
                    Text(stringResource(R.string.brush_import), modifier = Modifier.padding(start = 6.dp))
                }
            }
            if (groups.size > 1) {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    groups.keys.forEach { g ->
                        val label = if (g.startsWith("imported:")) {
                            g.removePrefix("imported:").ifBlank { stringResource(R.string.brush_category_imported) }
                        } else {
                            categoryName(g)?.let { stringResource(it) } ?: g
                        }
                        FilterChip(selected = g == group, onClick = { group = g }, label = { Text(label, maxLines = 1) })
                    }
                }
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp), maxItemsInEachRow = 2) {
                groups[group].orEmpty().forEach { b ->
                    BrushCard(
                        brush = b,
                        selected = b.id == state.brush.id,
                        vm = vm,
                        onSelect = { vm.selectBrush(b) },
                        onExport = {
                            exporting = b
                            export.launch(b.name.filter { it.isLetterOrDigit() || it == ' ' || it == '-' }.trim().ifBlank { "brush" } + "." + BrushFiles.EXPORT_EXTENSION)
                        },
                        modifier = Modifier.weight(1f),
                    )
                }
            }
            Text(stringResource(R.string.brush_import_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            SizeSlider(state.brushSize, vm::setBrushSize)
            FractionSlider(stringResource(R.string.tool_brush_opacity), state.brushOpacity, vm::setBrushOpacity)
            FractionSlider(stringResource(R.string.tool_brush_smoothing), state.brushStabilization, vm::setBrushStabilization)
        }
    }
    state.brushImport?.let { ImportReportDialog(it, onDismiss = vm::dismissBrushImport) }
}

/** One library entry: stroke thumbnail, name, and a menu (duplicate, export, delete). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun BrushCard(
    brush: BrushInfo,
    selected: Boolean,
    vm: VeskraViewModel,
    onSelect: () -> Unit,
    onExport: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    var preview by remember(brush.id, brush.version) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(brush.id, brush.version) { preview = vm.brushPreview(brush, PREVIEW_W, PREVIEW_H) }
    var menu by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(12.dp)
    Column(
        modifier
            .clip(shape)
            .background(if (selected) colors.primary.copy(alpha = 0.18f) else colors.surfaceVariant)
            .border(if (selected) 2.dp else 0.dp, if (selected) colors.primary else Color.Transparent, shape)
            .combinedClickable(role = Role.RadioButton, onClick = onSelect, onLongClick = { menu = true })
            .semantics { this.selected = selected }
            .padding(start = 10.dp, top = 8.dp, bottom = 6.dp),
    ) {
        Box(Modifier.fillMaxWidth().padding(end = 10.dp).height(44.dp).clip(RoundedCornerShape(8.dp)).background(Color.White)) {
            preview?.let { Image(it.asImageBitmap(), contentDescription = null, contentScale = ContentScale.Fit, modifier = Modifier.fillMaxWidth().height(44.dp)) }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(top = 4.dp)) {
                Text(brush.name, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (brush.vectorEligible) {
                    Text(stringResource(R.string.brush_vector_badge), style = MaterialTheme.typography.labelSmall, color = colors.onSurfaceVariant)
                }
            }
            Box {
                val more = stringResource(R.string.brush_more_cd, brush.name)
                IconButton(onClick = { menu = true }, modifier = Modifier.semantics { contentDescription = more }) {
                    Icon(VeskraIcons.More, contentDescription = null)
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(
                        text = { Text(stringResource(if (brush.editable) R.string.brush_edit else R.string.brush_edit_copy)) },
                        onClick = { menu = false; vm.openBrushEditor(brush) },
                    )
                    DropdownMenuItem(text = { Text(stringResource(R.string.brush_duplicate)) }, onClick = { menu = false; vm.duplicateBrush(brush) })
                    DropdownMenuItem(text = { Text(stringResource(R.string.brush_export)) }, onClick = { menu = false; onExport() })
                    if (brush.editable) {
                        DropdownMenuItem(text = { Text(stringResource(R.string.brush_delete)) }, onClick = { menu = false; vm.deleteBrush(brush) })
                    }
                }
            }
        }
    }
}

private const val PREVIEW_W = 360
private const val PREVIEW_H = 72

@Composable
private fun ImportReportDialog(report: BrushImportReport, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.brush_import_title)) },
        text = {
            Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.brush_import_summary, report.imported.size, report.failed.size))
                Text(stringResource(R.string.brush_import_note), style = MaterialTheme.typography.bodySmall)
                report.imported.forEach { b ->
                    Column {
                        Text(b.name, style = MaterialTheme.typography.labelLarge)
                        if (b.approximated.isNotEmpty()) {
                            Text(stringResource(R.string.brush_import_approx, b.approximated.joinToString()), style = MaterialTheme.typography.bodySmall)
                        }
                        if (b.unsupported.isNotEmpty()) {
                            Text(stringResource(R.string.brush_import_unsupported, b.unsupported.joinToString()), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                        }
                    }
                }
                report.failed.forEach { f ->
                    Text(stringResource(R.string.brush_import_failed_item, f.name, f.reason), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
                report.warnings.forEach { Text("• $it", style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.action_ok)) } },
    )
}

/**
 * Brush size 1–300 px on a curved scale (half of the travel covers 1–40 px, where most drawing
 * happens). Local state while dragging; the engine receives the value when the drag ends.
 */
@Composable
private fun SizeSlider(size: Float, onCommit: (Float) -> Unit) {
    var local by remember(size) { mutableFloatStateOf(size) }
    val label = stringResource(R.string.tool_brush_size)
    SliderRow(label, stringResource(R.string.tool_brush_size_value, local.roundToInt())) {
        Slider(
            value = sizeToFraction(local),
            onValueChange = { local = fractionToSize(it) },
            onValueChangeFinished = { onCommit(local) },
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = label },
        )
    }
}

private const val MIN_SIZE = 1f
private const val MAX_SIZE = 300f
private const val SIZE_CURVE = 2.6f

private fun fractionToSize(f: Float): Float = MIN_SIZE + (MAX_SIZE - MIN_SIZE) * f.coerceIn(0f, 1f).pow(SIZE_CURVE)

private fun sizeToFraction(s: Float): Float = ((s.coerceIn(MIN_SIZE, MAX_SIZE) - MIN_SIZE) / (MAX_SIZE - MIN_SIZE)).pow(1f / SIZE_CURVE)

/** 0–100 % slider with local drag state. */
@Composable
fun FractionSlider(label: String, value: Float, onCommit: (Float) -> Unit) {
    var local by remember(value) { mutableFloatStateOf(value) }
    SliderRow(label, stringResource(R.string.percent_value, (local * 100).roundToInt())) {
        Slider(
            value = local,
            onValueChange = { local = it },
            onValueChangeFinished = { onCommit(local) },
            valueRange = 0f..1f,
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = label },
        )
    }
}
