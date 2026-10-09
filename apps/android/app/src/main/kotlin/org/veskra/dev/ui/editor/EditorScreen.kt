package org.veskra.dev.ui.editor

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconToggleButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import org.veskra.dev.R
import org.veskra.dev.canvas.CanvasView
import org.veskra.dev.ui.EditorSheet
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.BrushPreset
import org.veskra.kit.SaveStatus
import kotlin.math.roundToInt

/**
 * Editor (MASTER_PLAN §2.1 item 2): full-bleed canvas, floating left/right tool rail, top status bar,
 * bottom quick sliders and the brush/color/layer/export sheets. The canvas is an Android View; Compose
 * only draws chrome, so recomposition never touches per-point painting.
 */
@Composable
fun EditorScreen(state: UiState, vm: VeskraViewModel) {
    val host = vm.engine ?: return

    // Back closes an open sheet first, then returns to the gallery (keeping a recovery checkpoint).
    BackHandler {
        when (state.sheet) {
            null -> vm.backToGallery()
            EditorSheet.EXPORT -> vm.closeExport()
            else -> vm.openSheet(null)
        }
    }

    Box(Modifier.fillMaxSize()) {
        AndroidView(
            factory = { ctx -> CanvasView(ctx) },
            update = { view ->
                view.bind(host, vm.display)
                view.brushSizeDoc = state.brushSize
            },
            modifier = Modifier.fillMaxSize(),
        )

        TopBar(state, vm, Modifier.align(Alignment.TopCenter))

        ToolRail(
            state = state,
            vm = vm,
            modifier = Modifier
                .align(if (state.railOnLeft) Alignment.CenterStart else Alignment.CenterEnd)
                .safeDrawingPadding()
                .padding(12.dp),
        )

        QuickSliders(state, vm, Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(12.dp))

        if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth().align(Alignment.TopCenter))
    }

    when (state.sheet) {
        EditorSheet.BRUSH -> BrushSheet(state, vm)
        EditorSheet.COLOR -> ColorSheet(state, vm)
        EditorSheet.LAYERS -> LayerSheet(state, vm)
        EditorSheet.EXPORT -> state.export?.let { ExportSheet(it, state, vm) }
        null -> Unit
    }
}

@Composable
private fun TopBar(state: UiState, vm: VeskraViewModel, modifier: Modifier) {
    Surface(
        modifier = modifier.safeDrawingPadding().padding(8.dp),
        shape = MaterialTheme.shapes.large,
        tonalElevation = 3.dp,
        shadowElevation = 2.dp,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(horizontal = 4.dp)) {
            RailButton(VeskraIcons.Back, stringResource(R.string.editor_back)) { vm.backToGallery() }
            Text(
                state.docName,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.widthIn(max = 220.dp).padding(horizontal = 8.dp),
            )
            SaveStatusIndicator(state.saveStatus, onRetry = vm::save)
            RailButton(VeskraIcons.Save, stringResource(R.string.tool_save)) { vm.save() }
            RailButton(VeskraIcons.Export, stringResource(R.string.tool_export)) { vm.openExport() }
        }
    }
}

/** Visible save/recovery state (AGENTS.md §5 UI). Failure is tappable to retry. */
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
        modifier = Modifier.heightIn(min = 48.dp).padding(vertical = 6.dp).semantics {
            contentDescription = if (detail != null) "$text. $detail" else text
        },
    ) {
        Box(contentAlignment = Alignment.Center, modifier = Modifier.padding(horizontal = 10.dp)) {
            Text(text, style = MaterialTheme.typography.labelLarge)
        }
    }
}

@Composable
private fun ToolRail(state: UiState, vm: VeskraViewModel, modifier: Modifier) {
    Surface(modifier = modifier, shape = MaterialTheme.shapes.extraLarge, tonalElevation = 3.dp, shadowElevation = 4.dp) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.padding(4.dp)) {
            RailToggle(VeskraIcons.Brush, stringResource(R.string.tool_brush), checked = !state.brush.isEraser && state.sheet == null) {
                vm.openSheet(EditorSheet.BRUSH)
            }
            RailToggle(VeskraIcons.Eraser, stringResource(R.string.tool_eraser), checked = state.brush.isEraser) {
                vm.selectBrush(if (state.brush.isEraser) BrushPreset.DEFAULT else BrushPreset.ERASER_HARD)
            }
            ColorButton(state, onClick = { vm.openSheet(EditorSheet.COLOR) })
            RailButton(VeskraIcons.Layers, stringResource(R.string.tool_layers)) { vm.openSheet(EditorSheet.LAYERS) }
            HorizontalDivider(Modifier.padding(vertical = 4.dp).size(width = 32.dp, height = 1.dp))
            RailButton(VeskraIcons.Undo, stringResource(R.string.tool_undo), enabled = state.doc?.canUndo ?: false) { vm.undo() }
            RailButton(VeskraIcons.Redo, stringResource(R.string.tool_redo), enabled = state.doc?.canRedo ?: false) { vm.redo() }
            HorizontalDivider(Modifier.padding(vertical = 4.dp).size(width = 32.dp, height = 1.dp))
            RailButton(VeskraIcons.Fit, stringResource(R.string.tool_fit)) { vm.fitView() }
            RailToggle(VeskraIcons.Mirror, stringResource(R.string.tool_mirror), checked = state.mirrored) { vm.toggleMirror() }
            RailButton(VeskraIcons.SwapSide, stringResource(R.string.tool_switch_side)) { vm.toggleRailSide() }
        }
    }
}

/** IconButton = 48 dp minimum touch target; the content description is the accessible tool name. */
@Composable
fun RailButton(icon: ImageVector, label: String, enabled: Boolean = true, onClick: () -> Unit) {
    IconButton(onClick = onClick, enabled = enabled) { Icon(icon, contentDescription = label) }
}

@Composable
private fun RailToggle(icon: ImageVector, label: String, checked: Boolean, onClick: () -> Unit) {
    IconToggleButton(checked = checked, onCheckedChange = { onClick() }) { Icon(icon, contentDescription = label) }
}

@Composable
private fun ColorButton(state: UiState, onClick: () -> Unit) {
    val cd = stringResource(R.string.tool_current_color_cd, state.color.toHex())
    IconButton(onClick = onClick, modifier = Modifier.semantics { contentDescription = cd }) {
        Box(
            Modifier
                .size(28.dp)
                .background(Color(state.color.toArgb()), CircleShape)
                .border(2.dp, MaterialTheme.colorScheme.onSurface, CircleShape),
        )
    }
}

@Composable
private fun QuickSliders(state: UiState, vm: VeskraViewModel, modifier: Modifier) {
    Surface(modifier = modifier.widthIn(max = 560.dp), shape = MaterialTheme.shapes.large, tonalElevation = 3.dp) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.padding(horizontal = 16.dp),
        ) {
            BrushSizeSlider(state.brushSize, vm::setBrushSize, Modifier.weight(1f))
            Spacer(Modifier.size(4.dp))
            BrushOpacitySlider(state.brushOpacity, vm::setBrushOpacity, Modifier.weight(1f))
        }
    }
}

/** Size slider with local drag state; the engine receives the value when the drag ends. */
@Composable
fun BrushSizeSlider(size: Float, onCommit: (Float) -> Unit, modifier: Modifier = Modifier) {
    var local by remember(size) { mutableFloatStateOf(size) }
    val label = stringResource(R.string.tool_brush_size)
    Column(modifier) {
        Text(stringResource(R.string.tool_brush_size_value, local.roundToInt()), style = MaterialTheme.typography.labelMedium)
        Slider(
            value = local,
            onValueChange = { local = it },
            onValueChangeFinished = { onCommit(local) },
            valueRange = 1f..300f,
            modifier = Modifier.semantics { contentDescription = label },
        )
    }
}

@Composable
fun BrushOpacitySlider(opacity: Float, onCommit: (Float) -> Unit, modifier: Modifier = Modifier) {
    var local by remember(opacity) { mutableFloatStateOf(opacity) }
    val label = stringResource(R.string.tool_brush_opacity)
    Column(modifier) {
        Text(stringResource(R.string.tool_brush_opacity_value, (local * 100).roundToInt()), style = MaterialTheme.typography.labelMedium)
        Slider(
            value = local,
            onValueChange = { local = it },
            onValueChangeFinished = { onCommit(local) },
            valueRange = 0f..1f,
            modifier = Modifier.semantics { contentDescription = label },
        )
    }
}
