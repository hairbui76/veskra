package org.veskra.dev.ui.editor

import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.BrushPreset
import kotlin.math.pow
import kotlin.math.roundToInt

@StringRes
fun brushName(p: BrushPreset): Int = when (p) {
    BrushPreset.PENCIL_ROUGH -> R.string.brush_pencil_rough
    BrushPreset.PENCIL_SMOOTH -> R.string.brush_pencil_smooth
    BrushPreset.PEN_TECHNICAL -> R.string.brush_pen_technical
    BrushPreset.PEN_BRUSH -> R.string.brush_pen_brush
    BrushPreset.MARKER -> R.string.brush_marker
    BrushPreset.ROUND_HARD -> R.string.brush_round_hard
    BrushPreset.ROUND_SOFT -> R.string.brush_round_soft
    BrushPreset.CHARCOAL -> R.string.brush_charcoal
    BrushPreset.ERASER_HARD -> R.string.brush_eraser_hard
    BrushPreset.ERASER_SOFT -> R.string.brush_eraser_soft
}

/**
 * Brush panel (DSN-01): the presets of the active kind (paint brushes or erasers) and the size,
 * opacity and smoothing of the current brush. Brush behavior itself is owned by the engine (BRH).
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun BrushPanel(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    val erasers = state.brush.isEraser
    FloatingPanel(modifier.widthIn(max = 360.dp)) {
        Column(
            Modifier.padding(16.dp).heightIn(max = 480.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            PanelLabel(stringResource(if (erasers) R.string.brush_group_erasers else R.string.brush_sheet_title))
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp), maxItemsInEachRow = 2) {
                BrushPreset.entries.filter { it.isEraser == erasers }.forEach { preset ->
                    PresetChip(preset, selected = preset == state.brush, modifier = Modifier.weight(1f)) { vm.selectBrush(preset) }
                }
            }
            SizeSlider(state.brushSize, vm::setBrushSize)
            FractionSlider(stringResource(R.string.tool_brush_opacity), state.brushOpacity, vm::setBrushOpacity)
            FractionSlider(stringResource(R.string.tool_brush_smoothing), state.brushStabilization, vm::setBrushStabilization)
        }
    }
}

@Composable
private fun PresetChip(preset: BrushPreset, selected: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(if (selected) colors.primary else colors.surfaceVariant)
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .padding(horizontal = 12.dp),
    ) {
        val tint = if (selected) colors.onPrimary else colors.onSurfaceVariant
        Icon(if (preset.isEraser) VeskraIcons.Eraser else VeskraIcons.Brush, contentDescription = null, tint = tint)
        Text(stringResource(brushName(preset)), color = tint, style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(start = 8.dp))
    }
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
