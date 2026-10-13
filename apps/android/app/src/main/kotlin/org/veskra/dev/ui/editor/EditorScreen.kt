package org.veskra.dev.ui.editor

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.material3.AlertDialog
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.veskra.dev.R
import org.veskra.dev.canvas.CanvasView
import org.veskra.dev.ui.EditorSheet
import org.veskra.dev.ui.Tool
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.dev.ui.theme.VeskraTheme
import org.veskra.kit.SaveStatus
import org.veskra.kit.SelectionOp
import kotlin.math.roundToInt

/**
 * Editor (MASTER_PLAN §2.1 item 2): a full-bleed canvas with only a few floating controls so the
 * drawing keeps the screen — Home and save state top-left, Tools / Layers / More top-right, Undo and
 * Redo in a bottom corner, and a dock with the brush, the eraser and the two colors at the bottom.
 * Everything else opens as a floating panel and closes on a tap outside.
 *
 * The canvas is an Android View; Compose only draws chrome, so recomposition never touches
 * per-point painting. The chrome is always dark and neutral: it must not bias how colors on the
 * canvas are judged, whatever the system theme is.
 */
@Composable
fun EditorScreen(state: UiState, vm: VeskraViewModel) {
    val host = vm.engine ?: return
    val view by vm.view.collectAsStateWithLifecycle()
    val importImage = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri -> if (uri != null) vm.importImage(uri) }

    // Back peels one layer at a time: panel → active tool → gallery (keeping a recovery checkpoint).
    BackHandler {
        when {
            state.sheet == EditorSheet.EXPORT -> vm.closeExport()
            state.sheet != null -> vm.openSheet(null)
            state.tool.hasPreview -> vm.finishPreviewTool(keep = false)
            state.tool != Tool.PAINT -> vm.setTool(Tool.PAINT)
            else -> vm.backToGallery()
        }
    }

    VeskraTheme(dark = true) {
        Box(Modifier.fillMaxSize()) {
            AndroidView(
                factory = { ctx -> CanvasView(ctx) },
                update = { v ->
                    v.bind(host, vm.display)
                    v.brushSizeDoc = state.brushSize
                },
                modifier = Modifier.fillMaxSize(),
            )
            CanvasAnnotations(state, view)
            if (state.tool != Tool.PAINT) ToolOverlay(state, view, vm)

            val edge = Modifier.safeDrawingPadding().padding(8.dp)
            Row(edge.align(Alignment.TopStart), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ChromeButton(VeskraIcons.Home, stringResource(R.string.editor_back)) { vm.backToGallery() }
                DocumentChip(state, vm)
            }
            Row(edge.align(Alignment.TopEnd), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ChromeButton(VeskraIcons.Tools, stringResource(R.string.tool_tools), selected = state.sheet == EditorSheet.TOOLS) {
                    vm.openSheet(if (state.sheet == EditorSheet.TOOLS) null else EditorSheet.TOOLS)
                }
                ChromeButton(VeskraIcons.Layers, stringResource(R.string.tool_layers), selected = state.sheet == EditorSheet.LAYERS) {
                    vm.openSheet(if (state.sheet == EditorSheet.LAYERS) null else EditorSheet.LAYERS)
                }
                MoreMenu(state, vm)
            }
            Row(
                edge.align(if (state.railOnLeft) Alignment.BottomStart else Alignment.BottomEnd),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                ChromeButton(VeskraIcons.Undo, stringResource(R.string.tool_undo), enabled = state.doc?.canUndo ?: false) { vm.undo() }
                ChromeButton(VeskraIcons.Redo, stringResource(R.string.tool_redo), enabled = state.doc?.canRedo ?: false) { vm.redo() }
            }
            Dock(state, vm, edge.align(Alignment.BottomCenter))

            val docked = Modifier.safeDrawingPadding().padding(start = 8.dp, end = 8.dp, bottom = 72.dp).align(Alignment.BottomCenter)
            when (state.tool) {
                Tool.PAINT -> Unit
                // These two bring their own panel (sliders / curve) instead of an options bar.
                Tool.ADJUST -> AdjustPanel(state, vm, docked)
                Tool.CURVES -> CurvesPanel(state, vm, docked)
                else -> ToolOptionsBar(state, vm, Modifier.align(Alignment.TopCenter).safeDrawingPadding().padding(top = 64.dp, start = 8.dp, end = 8.dp))
            }

            val panel = state.sheet
            if (panel != null && panel != EditorSheet.EXPORT) {
                // Scrim: a tap outside closes the panel and is not passed on to the canvas.
                Box(Modifier.fillMaxSize().pointerInput(Unit) { detectTapGestures { vm.openSheet(null) } })
                val above = Modifier.safeDrawingPadding().padding(start = 8.dp, end = 8.dp, bottom = 72.dp).align(Alignment.BottomCenter)
                val below = Modifier.safeDrawingPadding().padding(top = 64.dp, start = 8.dp, end = 8.dp, bottom = 8.dp).align(Alignment.TopEnd)
                when (panel) {
                    EditorSheet.BRUSH -> BrushPanel(state, vm, above)
                    EditorSheet.COLOR -> ColorPanel(state, vm, above)
                    EditorSheet.TOOLS -> ToolsPanel(state, vm, onImportImage = { importImage.launch("image/*") }, modifier = below)
                    EditorSheet.LAYERS -> LayerPanel(state, vm, below)
                    EditorSheet.EXPORT -> Unit
                }
            }

            if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth().align(Alignment.TopCenter))
        }
        if (state.sheet == EditorSheet.EXPORT) state.export?.let { ExportSheet(it, state, vm) }
        state.brushEditor?.let { BrushEditor(it, vm) }
        if (state.textDialog) {
            TextDialog(state.doc?.height ?: 2048, onDismiss = { vm.showTextDialog(false) }, onAdd = vm::addText)
        }
        val doc = state.doc
        if (state.resizeDialog && doc != null) {
            ResizeDialog(doc.width, doc.height, doc.dpi, onDismiss = { vm.showResizeDialog(false) }, onResize = vm::resizeCanvas)
        }
    }
}

/** Document name + visible save state (AGENTS.md §5 UI). */
@Composable
private fun DocumentChip(state: UiState, vm: VeskraViewModel) {
    Surface(
        shape = RoundedCornerShape(24.dp),
        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.86f),
        contentColor = MaterialTheme.colorScheme.onSurface,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 14.dp, end = 4.dp)) {
            Text(
                state.docName,
                style = MaterialTheme.typography.labelLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.widthIn(max = 160.dp).padding(end = 6.dp),
            )
            SaveStatusIndicator(state.saveStatus, onRetry = vm::save)
        }
    }
}

/** Visible save/recovery state. Failure is tappable to retry. */
@Composable
fun SaveStatusIndicator(status: SaveStatus, onRetry: () -> Unit) {
    val (text, isError) = when (status) {
        is SaveStatus.Saved -> stringResource(R.string.save_status_saved) to false
        is SaveStatus.Unsaved -> stringResource(R.string.save_status_unsaved) to false
        is SaveStatus.NeverSaved -> stringResource(R.string.save_status_never) to false
        is SaveStatus.Saving -> stringResource(R.string.save_status_saving) to false
        is SaveStatus.Failed -> stringResource(R.string.save_status_failed) to true
    }
    val detail = (status as? SaveStatus.Failed)?.let { stringResource(VeskraViewModel.saveFailureText(it.failure.reason)) }
    Surface(
        onClick = onRetry,
        enabled = isError,
        shape = MaterialTheme.shapes.small,
        color = if (isError) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.heightIn(min = 48.dp).padding(vertical = 8.dp).semantics {
            contentDescription = if (detail != null) "$text. $detail" else text
        },
    ) {
        Box(contentAlignment = Alignment.Center, modifier = Modifier.padding(horizontal = 10.dp)) {
            Text(text, style = MaterialTheme.typography.labelMedium)
        }
    }
}

@Composable
private fun MoreMenu(state: UiState, vm: VeskraViewModel) {
    var open by remember { mutableStateOf(false) }
    var letter by remember { mutableStateOf(false) }
    if (letter) LetterDialog(onDismiss = { letter = false })
    Box {
        ChromeButton(VeskraIcons.More, stringResource(R.string.tool_more), selected = open) { open = true }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text(stringResource(R.string.tool_save)) },
                leadingIcon = { Icon(VeskraIcons.Save, contentDescription = null) },
                onClick = { open = false; vm.save() },
            )
            DropdownMenuItem(
                text = { Text(stringResource(R.string.tool_export)) },
                leadingIcon = { Icon(VeskraIcons.Export, contentDescription = null) },
                onClick = { open = false; vm.openExport() },
            )
            DropdownMenuItem(
                text = { Text(stringResource(R.string.tool_switch_side)) },
                leadingIcon = { Icon(VeskraIcons.SwapSide, contentDescription = null) },
                onClick = { open = false; vm.toggleRailSide() },
            )
            DropdownMenuItem(
                text = { Text(stringResource(R.string.letter_menu)) },
                leadingIcon = { Icon(VeskraIcons.Letter, contentDescription = null) },
                onClick = { open = false; letter = true },
            )
        }
    }
}

/** The letter this app was made to carry. */
@Composable
private fun LetterDialog(onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.letter_title)) },
        text = {
            Text(
                stringResource(R.string.letter_body),
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier.verticalScroll(rememberScrollState()),
            )
        },
        confirmButton = {
            TextButton(onClick = onDismiss, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.letter_close)) }
        },
    )
}

/**
 * Bottom dock: brush, smudge, eraser, current color, secondary color. Tapping the tool that is already
 * active opens its panel; tapping the secondary color swaps the two colors.
 */
@Composable
private fun Dock(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    val painting = state.tool == Tool.PAINT
    val brushActive = painting && !state.brush.eraser && !state.smudge
    val smudgeActive = painting && state.smudge
    val eraserActive = painting && state.brush.eraser && !state.smudge
    Surface(modifier = modifier, shape = RoundedCornerShape(32.dp), tonalElevation = 3.dp, shadowElevation = 6.dp) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.padding(6.dp)) {
            DockButton(VeskraIcons.Brush, stringResource(R.string.tool_brush), brushActive) {
                if (brushActive) vm.openSheet(EditorSheet.BRUSH) else vm.usePaintBrush()
            }
            DockButton(VeskraIcons.Smudge, stringResource(R.string.tool_smudge), smudgeActive) {
                if (smudgeActive) vm.openSheet(EditorSheet.BRUSH) else vm.useSmudge()
            }
            DockButton(VeskraIcons.Eraser, stringResource(R.string.tool_eraser), eraserActive) {
                if (eraserActive) vm.openSheet(EditorSheet.BRUSH) else vm.useEraser()
            }
            Box(Modifier.padding(horizontal = 4.dp).size(width = 1.dp, height = 28.dp).background(MaterialTheme.colorScheme.outline.copy(alpha = 0.4f)))
            ColorDot(state.color, stringResource(R.string.tool_current_color_cd, state.color.toHex()), large = true) { vm.openSheet(EditorSheet.COLOR) }
            ColorDot(state.secondaryColor, stringResource(R.string.tool_secondary_color_cd, state.secondaryColor.toHex()), large = false) { vm.swapColors() }
        }
    }
}

@Composable
private fun DockButton(icon: ImageVector, label: String, active: Boolean, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(48.dp)
            .clip(CircleShape)
            .background(if (active) colors.primary else colors.surface.copy(alpha = 0f))
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { selected = active },
    ) {
        Icon(icon, contentDescription = label, tint = if (active) colors.onPrimary else colors.onSurfaceVariant)
    }
}

@StringRes
private fun toolName(tool: Tool): Int = when (tool) {
    Tool.PAINT -> R.string.tool_brush
    Tool.FILL -> R.string.tool_fill
    Tool.EYEDROPPER -> R.string.tool_eyedropper
    Tool.SELECT_RECT -> R.string.tool_select_rect
    Tool.SELECT_ELLIPSE -> R.string.tool_select_ellipse
    Tool.SELECT_LASSO -> R.string.tool_select_lasso
    Tool.SELECT_WAND -> R.string.tool_select_wand
    Tool.TRANSFORM -> R.string.tool_transform
    Tool.DISTORT -> R.string.tool_distort
    Tool.SHAPE_LINE -> R.string.tool_shape_line
    Tool.SHAPE_RECT -> R.string.tool_shape_rect
    Tool.SHAPE_ELLIPSE -> R.string.tool_shape_circle
    Tool.SHAPE_PATH -> R.string.tool_shape_path
    Tool.GRADIENT -> R.string.tool_gradient
    Tool.CROP -> R.string.tool_crop
    Tool.GUIDE_LINE -> R.string.tool_guide_line
    Tool.GUIDE_ELLIPSE -> R.string.tool_guide_ellipse
    Tool.ADJUST -> R.string.tool_adjust
    Tool.CURVES -> R.string.tool_curves
}

/** Options of the active tool, plus the way back to painting. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ToolOptionsBar(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    val tool = state.tool
    FloatingPanel(modifier) {
        FlowRow(
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalArrangement = Arrangement.Center,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        ) {
            val center = Modifier.align(Alignment.CenterVertically)
            Text(stringResource(toolName(tool)), style = MaterialTheme.typography.titleSmall, modifier = center)
            @Composable
            fun hint(@StringRes id: Int) = Text(stringResource(id), style = MaterialTheme.typography.bodySmall, modifier = center)
            if (tool == Tool.SELECT_WAND) ToleranceSlider(state, vm, center)
            when {
                tool == Tool.FILL -> {
                    var tolerance by remember(state.fill.tolerance) { mutableFloatStateOf(state.fill.tolerance) }
                    val label = stringResource(R.string.fill_tolerance, (tolerance * 100).roundToInt())
                    Text(label, style = MaterialTheme.typography.labelMedium, modifier = center)
                    Slider(
                        value = tolerance,
                        onValueChange = { tolerance = it },
                        onValueChangeFinished = { vm.setFillOptions(state.fill.copy(tolerance = tolerance)) },
                        modifier = center.width(140.dp).semantics { contentDescription = label },
                    )
                    FilterChip(state.fill.contiguous, { vm.setFillOptions(state.fill.copy(contiguous = !state.fill.contiguous)) }, { Text(stringResource(R.string.fill_contiguous)) }, center)
                    FilterChip(state.fill.sampleMerged, { vm.setFillOptions(state.fill.copy(sampleMerged = !state.fill.sampleMerged)) }, { Text(stringResource(R.string.fill_all_layers)) }, center)
                }
                tool == Tool.EYEDROPPER -> Text(stringResource(R.string.eyedropper_hint), style = MaterialTheme.typography.bodySmall, modifier = center)
                tool.isSelection -> {
                    for ((op, name) in SELECTION_OPS) {
                        FilterChip(state.selectionOp == op, { vm.setSelectionOp(op) }, { Text(stringResource(name)) }, center)
                    }
                    TextButton(onClick = vm::selectAll, modifier = center) { Text(stringResource(R.string.select_all)) }
                    val has = state.doc?.hasSelection == true
                    TextButton(onClick = vm::invertSelection, enabled = has, modifier = center) { Text(stringResource(R.string.select_invert)) }
                    TextButton(onClick = vm::clearSelection, enabled = has, modifier = center) { Text(stringResource(R.string.select_clear)) }
                    TextButton(onClick = vm::fillSelection, enabled = has, modifier = center) { Text(stringResource(R.string.select_fill)) }
                    TextButton(onClick = vm::deleteSelection, enabled = has, modifier = center) { Text(stringResource(R.string.select_delete)) }
                }
                tool == Tool.TRANSFORM || tool == Tool.DISTORT -> {
                    hint(if (tool == Tool.TRANSFORM) R.string.transform_hint else R.string.distort_hint)
                    TextButton(onClick = { vm.finishPreviewTool(keep = false) }, modifier = center) { Text(stringResource(R.string.action_cancel)) }
                }
                tool == Tool.SHAPE_PATH -> {
                    FilterChip(state.shape.smooth, { vm.setShapeOptions(state.shape.copy(smooth = !state.shape.smooth)) }, { Text(stringResource(R.string.shape_smooth)) }, center)
                    FilterChip(state.shape.closed, { vm.setShapeOptions(state.shape.copy(closed = !state.shape.closed)) }, { Text(stringResource(R.string.shape_closed)) }, center)
                    if (state.shape.closed) {
                        FilterChip(state.shape.fill, { vm.setShapeOptions(state.shape.copy(fill = !state.shape.fill)) }, { Text(stringResource(R.string.shape_fill)) }, center)
                    }
                    TextButton(onClick = vm::removeLastPathPoint, enabled = state.pathPoints.isNotEmpty(), modifier = center) {
                        Text(stringResource(R.string.path_remove_point))
                    }
                    TextButton(onClick = vm::commitPath, enabled = state.pathPoints.size >= 2, modifier = center) { Text(stringResource(R.string.path_draw)) }
                }
                tool == Tool.SHAPE_RECT || tool == Tool.SHAPE_ELLIPSE -> {
                    FilterChip(state.shape.fill, { vm.setShapeOptions(state.shape.copy(fill = !state.shape.fill)) }, { Text(stringResource(R.string.shape_fill)) }, center)
                }
                tool == Tool.SHAPE_LINE -> hint(R.string.shape_hint)
                tool == Tool.GRADIENT -> {
                    FilterChip(!state.gradientRadial, { vm.setGradientRadial(false) }, { Text(stringResource(R.string.gradient_linear)) }, center)
                    FilterChip(state.gradientRadial, { vm.setGradientRadial(true) }, { Text(stringResource(R.string.gradient_radial)) }, center)
                    hint(R.string.gradient_hint)
                }
                tool == Tool.GUIDE_LINE || tool == Tool.GUIDE_ELLIPSE -> hint(R.string.guide_hint)
                tool == Tool.CROP -> {
                    hint(R.string.crop_hint)
                    TextButton(onClick = { vm.setTool(Tool.PAINT) }, modifier = center) { Text(stringResource(R.string.action_cancel)) }
                }
            }
            Button(
                onClick = {
                    when {
                        tool.hasPreview -> vm.finishPreviewTool(keep = true)
                        tool == Tool.CROP -> vm.applyCrop()
                        tool == Tool.SHAPE_PATH -> {
                            vm.commitPath()
                            vm.setTool(Tool.PAINT)
                        }
                        else -> vm.setTool(Tool.PAINT)
                    }
                },
                modifier = center.heightIn(min = 48.dp),
            ) { Text(stringResource(if (tool == Tool.CROP) R.string.tool_crop else R.string.action_done)) }
        }
    }
}

/** Color-similarity tolerance shared by the bucket fill and the magic wand. */
@Composable
private fun ToleranceSlider(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    var tolerance by remember(state.fill.tolerance) { mutableFloatStateOf(state.fill.tolerance) }
    val label = stringResource(R.string.fill_tolerance, (tolerance * 100).roundToInt())
    Text(label, style = MaterialTheme.typography.labelMedium, modifier = modifier)
    Slider(
        value = tolerance,
        onValueChange = { tolerance = it },
        onValueChangeFinished = { vm.setFillOptions(state.fill.copy(tolerance = tolerance)) },
        modifier = modifier.width(140.dp).semantics { contentDescription = label },
    )
}

private val SELECTION_OPS = listOf(
    SelectionOp.REPLACE to R.string.select_op_new,
    SelectionOp.ADD to R.string.select_op_add,
    SelectionOp.SUBTRACT to R.string.select_op_subtract,
    SelectionOp.INTERSECT to R.string.select_op_intersect,
)

/** IconButton = 48 dp minimum touch target; the content description is the accessible name. */
@Composable
fun RailButton(icon: ImageVector, label: String, enabled: Boolean = true, onClick: () -> Unit) {
    IconButton(onClick = onClick, enabled = enabled) { Icon(icon, contentDescription = label) }
}
