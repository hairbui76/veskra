package org.veskra.dev.ui.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.kit.HexColor

/** Shared shapes of the floating editor chrome. */
internal val PanelShape = RoundedCornerShape(20.dp)
internal val SwatchShape = RoundedCornerShape(10.dp)

/**
 * Round button floating over the canvas (48 dp target). A translucent disc keeps the icon readable
 * on white paper as well as on the dark backdrop; [selected] marks the active tool.
 */
@Composable
fun ChromeButton(
    icon: ImageVector,
    label: String,
    modifier: Modifier = Modifier,
    selected: Boolean = false,
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Box(
        contentAlignment = Alignment.Center,
        modifier = modifier
            .size(48.dp)
            .clip(CircleShape)
            .background(if (selected) colors.primary else colors.surface.copy(alpha = 0.86f))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { this.selected = selected },
    ) {
        Icon(
            icon,
            contentDescription = label,
            tint = if (selected) colors.onPrimary else colors.onSurface,
            modifier = Modifier.alpha(if (enabled) 1f else 0.35f),
        )
    }
}

/** A floating card over the canvas: tool panels, the dock, option bars. */
@Composable
fun FloatingPanel(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Surface(modifier = modifier, shape = PanelShape, tonalElevation = 3.dp, shadowElevation = 8.dp) {
        Column(content = content)
    }
}

/** Small caption above a group of controls inside a panel. */
@Composable
fun PanelLabel(text: String, modifier: Modifier = Modifier) {
    Text(text, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = modifier)
}

/** Square color chip. [size] ≥ 40 dp keeps it comfortably tappable inside a spaced grid. */
@Composable
fun Swatch(color: HexColor, modifier: Modifier = Modifier, size: Dp = 44.dp, selected: Boolean = false, onClick: () -> Unit) {
    val cd = stringResource(R.string.color_swatch_cd, color.toHex())
    Box(
        modifier
            .size(size)
            .clip(SwatchShape)
            .background(Color(color.toArgb()))
            .border(
                if (selected) 2.dp else 1.dp,
                if (selected) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.outline.copy(alpha = 0.5f),
                SwatchShape,
            )
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = cd; this.selected = selected },
    )
}

/**
 * Slider whose track shows the result of moving it (hue rainbow, saturation ramp, …). [value] is the
 * 0..1 fraction; [onValueChange] reports `(fraction, finished)` where `finished` marks the end of a drag.
 */
@Composable
fun GradientSlider(
    value: Float,
    colors: List<Color>,
    label: String,
    modifier: Modifier = Modifier,
    onValueChange: (Float, Boolean) -> Unit,
) {
    val callback by rememberUpdatedState(onValueChange)
    val outline = MaterialTheme.colorScheme.outline
    Canvas(
        modifier
            .fillMaxWidth()
            .height(40.dp)
            .semantics {
                contentDescription = label
                progressBarRangeInfo = ProgressBarRangeInfo(value.coerceIn(0f, 1f), 0f..1f)
                setProgress { v -> callback(v.coerceIn(0f, 1f), true); true }
            }
            .pointerInput(Unit) {
                awaitEachGesture {
                    val pad = THUMB_RADIUS.toPx()
                    fun fraction(x: Float) = ((x - pad) / (size.width - 2 * pad).coerceAtLeast(1f)).coerceIn(0f, 1f)
                    val down = awaitFirstDown()
                    var last = fraction(down.position.x)
                    callback(last, false)
                    down.consume()
                    drag(down.id) { change ->
                        last = fraction(change.position.x)
                        callback(last, false)
                        change.consume()
                    }
                    callback(last, true)
                }
            },
    ) {
        val pad = THUMB_RADIUS.toPx()
        val track = 12.dp.toPx()
        val cy = size.height / 2f
        val gradient = if (colors.size >= 2) colors else listOf(colors.firstOrNull() ?: Color.Gray, colors.firstOrNull() ?: Color.Gray)
        drawRoundRect(
            brush = Brush.horizontalGradient(gradient, startX = pad, endX = size.width - pad),
            topLeft = Offset(pad - track / 2f, cy - track / 2f),
            size = Size(size.width - 2 * pad + track, track),
            cornerRadius = CornerRadius(track / 2f),
        )
        val x = pad + value.coerceIn(0f, 1f) * (size.width - 2 * pad)
        drawCircle(outline, radius = pad, center = Offset(x, cy))
        drawCircle(Color.White, radius = pad - 1.5.dp.toPx(), center = Offset(x, cy))
    }
}

private val THUMB_RADIUS = 11.dp

/** `label ……… value` header row followed by [slider]. */
@Composable
fun SliderRow(label: String, valueText: String, modifier: Modifier = Modifier, slider: @Composable () -> Unit) {
    Column(modifier) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(label, style = MaterialTheme.typography.labelLarge, modifier = Modifier.weight(1f))
            Text(valueText, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        slider()
    }
}
