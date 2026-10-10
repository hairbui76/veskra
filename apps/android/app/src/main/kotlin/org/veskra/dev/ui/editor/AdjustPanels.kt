package org.veskra.dev.ui.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.Adjustment
import org.veskra.dev.ui.CurveChannel
import org.veskra.dev.ui.Curves
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.kit.CanvasSpec
import org.veskra.kit.PointF
import kotlin.math.roundToInt

/** Cancel / Done row shared by the panels of tools with a live preview. */
@Composable
private fun PreviewActions(vm: VeskraViewModel) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
        TextButton(onClick = { vm.finishPreviewTool(keep = false) }, modifier = Modifier.heightIn(min = 48.dp)) {
            Text(stringResource(R.string.action_cancel))
        }
        Button(onClick = { vm.finishPreviewTool(keep = true) }, modifier = Modifier.heightIn(min = 48.dp).padding(start = 8.dp)) {
            Text(stringResource(R.string.action_done))
        }
    }
}

/**
 * Color adjustment of the active layer (or the selection): hue, saturation, brightness, contrast and
 * invert. The canvas shows the result when a slider is released; Done keeps it, Cancel restores.
 */
@Composable
fun AdjustPanel(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    val a = state.adjustment
    FloatingPanel(modifier.widthIn(max = 360.dp)) {
        Column(Modifier.padding(16.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(stringResource(R.string.tool_adjust), style = MaterialTheme.typography.titleMedium)
            DeltaSlider(stringResource(R.string.color_hue), a.hue, -180f..180f, { "${it.roundToInt()}°" }, { vm.previewAdjustment(a.copy(hue = it)) }) {
                vm.applyAdjustment(a.copy(hue = it))
            }
            DeltaSlider(stringResource(R.string.color_saturation), a.saturation, -1f..1f, ::signedPercent, { vm.previewAdjustment(a.copy(saturation = it)) }) {
                vm.applyAdjustment(a.copy(saturation = it))
            }
            DeltaSlider(stringResource(R.string.color_brightness), a.brightness, -1f..1f, ::signedPercent, { vm.previewAdjustment(a.copy(brightness = it)) }) {
                vm.applyAdjustment(a.copy(brightness = it))
            }
            DeltaSlider(stringResource(R.string.adjust_contrast), a.contrast, -1f..1f, ::signedPercent, { vm.previewAdjustment(a.copy(contrast = it)) }) {
                vm.applyAdjustment(a.copy(contrast = it))
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(a.invert, { vm.applyAdjustment(a.copy(invert = !a.invert)) }, { Text(stringResource(R.string.adjust_invert)) })
                TextButton(onClick = { vm.applyAdjustment(Adjustment()) }, enabled = !a.isNeutral, modifier = Modifier.heightIn(min = 48.dp)) {
                    Text(stringResource(R.string.adjust_reset))
                }
            }
            PreviewActions(vm)
        }
    }
}

private fun signedPercent(v: Float): String {
    val p = (v * 100).roundToInt()
    return if (p > 0) "+$p" else p.toString()
}

/** Slider around zero with a local value while dragging; [onDone] fires when the drag ends. */
@Composable
private fun DeltaSlider(
    label: String,
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    format: (Float) -> String,
    onChange: (Float) -> Unit,
    onDone: (Float) -> Unit,
) {
    var local by remember(value) { mutableFloatStateOf(value) }
    SliderRow(label, format(local)) {
        Slider(
            value = local,
            onValueChange = { local = it; onChange(it) },
            onValueChangeFinished = { onDone(local) },
            valueRange = range,
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = label },
        )
    }
}

/**
 * Tone curves: drag a point to bend the curve, touch an empty spot on the curve area to add a
 * point, drag a point off the left or right edge onto its neighbour to remove it. One master curve
 * (RGB) plus one per channel.
 */
@Composable
fun CurvesPanel(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    val curves = state.curves
    FloatingPanel(modifier.widthIn(max = 360.dp)) {
        Column(Modifier.padding(16.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.tool_curves), style = MaterialTheme.typography.titleMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CurveChannel.entries.forEach { ch ->
                    FilterChip(curves.channel == ch, { vm.previewCurves(curves.copy(channel = ch)) }, { Text(ch.name) })
                }
            }
            CurveEditor(curves, onChange = vm::previewCurves, onDone = vm::applyCurves, modifier = Modifier.fillMaxWidth().aspectRatio(1f))
            TextButton(onClick = { vm.applyCurves(Curves(channel = curves.channel)) }, enabled = !curves.isNeutral, modifier = Modifier.heightIn(min = 48.dp)) {
                Text(stringResource(R.string.adjust_reset))
            }
            PreviewActions(vm)
        }
    }
}

private const val MAX_CURVE_POINTS = 12

/** Monotone piecewise-linear sample of [points] at [x]; only used to draw the curve preview. */
private fun sampleCurve(points: List<PointF>, x: Float): Float {
    if (points.isEmpty()) return x
    if (x <= points.first().x) return points.first().y
    for (i in 1 until points.size) {
        val a = points[i - 1]
        val b = points[i]
        if (x <= b.x) return if (b.x - a.x < 1e-6f) b.y else a.y + (b.y - a.y) * (x - a.x) / (b.x - a.x)
    }
    return points.last().y
}

@Composable
private fun CurveEditor(curves: Curves, onChange: (Curves) -> Unit, onDone: (Curves) -> Unit, modifier: Modifier = Modifier) {
    val latest by rememberUpdatedState(curves)
    val change by rememberUpdatedState(onChange)
    val done by rememberUpdatedState(onDone)
    val cd = stringResource(R.string.curves_editor_cd)
    val grid = MaterialTheme.colorScheme.outline.copy(alpha = 0.35f)
    val line = when (curves.channel) {
        CurveChannel.RGB -> MaterialTheme.colorScheme.onSurface
        CurveChannel.R -> Color(0xFFE5564B)
        CurveChannel.G -> Color(0xFF5FBF58)
        CurveChannel.B -> Color(0xFF4F8FE8)
    }
    Canvas(
        modifier
            .semantics { contentDescription = cd }
            .pointerInput(Unit) {
                awaitEachGesture {
                    val grabRadius = 28.dp.toPx()
                    fun toCurve(p: Offset) = PointF((p.x / size.width).coerceIn(0f, 1f), (1f - p.y / size.height).coerceIn(0f, 1f))
                    fun toView(p: PointF) = Offset(p.x * size.width, (1f - p.y) * size.height)
                    val down = awaitFirstDown()
                    down.consume()
                    var points = latest.active
                    var index = points.indices.minByOrNull { (toView(points[it]) - down.position).getDistance() } ?: 0
                    if ((toView(points[index]) - down.position).getDistance() > grabRadius) {
                        if (points.size >= MAX_CURVE_POINTS) return@awaitEachGesture
                        val added = toCurve(down.position)
                        points = (points + added).sortedBy { it.x }
                        index = points.indexOf(added)
                    }
                    fun moved(p: Offset): List<PointF> {
                        val c = toCurve(p)
                        // End points keep their input (0 or 1); inner points stay between their neighbours.
                        val x = when (index) {
                            0 -> 0f
                            points.lastIndex -> 1f
                            else -> c.x.coerceIn(points[index - 1].x + 0.01f, points[index + 1].x - 0.01f)
                        }
                        return points.toMutableList().also { it[index] = PointF(x, c.y) }
                    }
                    var last = down.position
                    change(latest.withActive(moved(last)))
                    drag(down.id) { ch ->
                        last = ch.position
                        change(latest.withActive(moved(last)))
                        ch.consume()
                    }
                    done(latest.withActive(moved(last)))
                }
            },
    ) {
        for (i in 0..4) {
            val t = i / 4f
            drawLine(grid, Offset(t * size.width, 0f), Offset(t * size.width, size.height))
            drawLine(grid, Offset(0f, t * size.height), Offset(size.width, t * size.height))
        }
        drawLine(grid, Offset(0f, size.height), Offset(size.width, 0f))
        val pts = curves.active
        val path = Path()
        val steps = 64
        for (i in 0..steps) {
            val x = i / steps.toFloat()
            val y = sampleCurve(pts, x)
            if (i == 0) path.moveTo(0f, (1f - y) * size.height) else path.lineTo(x * size.width, (1f - y) * size.height)
        }
        drawPath(path, line, style = Stroke(2.dp.toPx()))
        for (p in pts) {
            val at = Offset(p.x * size.width, (1f - p.y) * size.height)
            drawCircle(Color.White, radius = 7.dp.toPx(), center = at)
            drawCircle(line, radius = 7.dp.toPx(), center = at, style = Stroke(2.dp.toPx()))
        }
    }
}

/** Canvas size dialog: new pixel size, and whether the artwork is scaled with it. */
@Composable
fun ResizeDialog(width: Int, height: Int, dpi: Int, onDismiss: () -> Unit, onResize: (Int, Int, Boolean) -> Unit) {
    var w by rememberSaveable { mutableStateOf(width.toString()) }
    var h by rememberSaveable { mutableStateOf(height.toString()) }
    var resample by rememberSaveable { mutableStateOf(true) }
    val spec = CanvasSpec(w.toIntOrNull() ?: 0, h.toIntOrNull() ?: 0, dpi)
    val problem = spec.validate()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.tool_resize)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    OutlinedTextField(
                        value = w,
                        onValueChange = { w = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(R.string.new_canvas_width)) },
                        singleLine = true,
                        isError = problem != null,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.weight(1f),
                    )
                    OutlinedTextField(
                        value = h,
                        onValueChange = { h = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(R.string.new_canvas_height)) },
                        singleLine = true,
                        isError = problem != null,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.weight(1f),
                    )
                }
                FilterChip(resample, { resample = !resample }, { Text(stringResource(R.string.resize_scale_artwork)) })
                Text(
                    stringResource(if (resample) R.string.resize_scale_note else R.string.resize_extent_note),
                    style = MaterialTheme.typography.bodySmall,
                )
                if (problem != null) {
                    Text(stringResource(R.string.resize_invalid), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
            }
        },
        confirmButton = {
            TextButton(onClick = { onResize(spec.width, spec.height, resample) }, enabled = problem == null, modifier = Modifier.heightIn(min = 48.dp)) {
                Text(stringResource(R.string.tool_resize))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.action_cancel)) } },
    )
}
