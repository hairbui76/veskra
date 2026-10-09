package org.veskra.dev.ui.gallery

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.UiState
import org.veskra.kit.CanvasSpec
import org.veskra.kit.HexColor
import org.veskra.kit.formatBytes

private enum class Preset { SQUARE, LANDSCAPE, PORTRAIT, CUSTOM }

/** New Canvas (MASTER_PLAN §2.1 item 3): presets, custom size, DPI, sRGB badge, memory estimate. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun NewCanvasDialog(onDismiss: () -> Unit, onCreate: (CanvasSpec, HexColor?, String) -> Unit) {
    var preset by rememberSaveable { mutableStateOf(Preset.SQUARE) }
    var widthText by rememberSaveable { mutableStateOf("2048") }
    var heightText by rememberSaveable { mutableStateOf("2048") }
    var dpiText by rememberSaveable { mutableStateOf("300") }
    var name by rememberSaveable { mutableStateOf(UiState.DEFAULT_DOC_NAME) }
    var whiteBackground by rememberSaveable { mutableStateOf(true) }

    val spec: CanvasSpec = when (preset) {
        Preset.SQUARE -> CanvasSpec.PRESET_SQUARE.copy(dpi = dpiText.toIntOrNull() ?: -1)
        Preset.LANDSCAPE -> CanvasSpec.PRESET_LANDSCAPE.copy(dpi = dpiText.toIntOrNull() ?: -1)
        Preset.PORTRAIT -> CanvasSpec.PRESET_PORTRAIT.copy(dpi = dpiText.toIntOrNull() ?: -1)
        Preset.CUSTOM -> {
            val w = widthText.toIntOrNull()
            val h = heightText.toIntOrNull()
            val d = dpiText.toIntOrNull()
            if (w != null && h != null && d != null) CanvasSpec(w, h, d) else CanvasSpec(0, 0, d ?: -1)
        }
    }
    val problem = spec.validate()
    val errorText = when (problem) {
        null -> null
        CanvasSpec.Problem.NON_POSITIVE -> stringResource(R.string.new_canvas_error_non_positive)
        CanvasSpec.Problem.EDGE_TOO_LARGE -> stringResource(R.string.new_canvas_error_edge)
        CanvasSpec.Problem.TOO_MANY_PIXELS -> stringResource(R.string.new_canvas_error_pixels)
        CanvasSpec.Problem.DPI_OUT_OF_RANGE -> stringResource(R.string.new_canvas_error_dpi)
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.new_canvas_title)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it.take(80) },
                    label = { Text(stringResource(R.string.new_canvas_name)) },
                    singleLine = true,
                )
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PresetChip(preset == Preset.SQUARE, stringResource(R.string.new_canvas_preset_square)) { preset = Preset.SQUARE }
                    PresetChip(preset == Preset.LANDSCAPE, stringResource(R.string.new_canvas_preset_landscape)) { preset = Preset.LANDSCAPE }
                    PresetChip(preset == Preset.PORTRAIT, stringResource(R.string.new_canvas_preset_portrait)) { preset = Preset.PORTRAIT }
                    PresetChip(preset == Preset.CUSTOM, stringResource(R.string.new_canvas_preset_custom)) { preset = Preset.CUSTOM }
                }
                if (preset == Preset.CUSTOM) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        NumberField(widthText, { widthText = it }, stringResource(R.string.new_canvas_width), Modifier.weight(1f))
                        NumberField(heightText, { heightText = it }, stringResource(R.string.new_canvas_height), Modifier.weight(1f))
                    }
                }
                NumberField(dpiText, { dpiText = it }, stringResource(R.string.new_canvas_dpi), Modifier)
                Text(stringResource(R.string.new_canvas_background), style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PresetChip(whiteBackground, stringResource(R.string.new_canvas_bg_white)) { whiteBackground = true }
                    PresetChip(!whiteBackground, stringResource(R.string.new_canvas_bg_transparent)) { whiteBackground = false }
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.new_canvas_color_space), style = MaterialTheme.typography.labelLarge)
                    // Non-interactive badge: v1 offers sRGB only (MASTER_PLAN §1.1 Color).
                    Surface(shape = MaterialTheme.shapes.small, color = MaterialTheme.colorScheme.secondaryContainer) {
                        Text(
                            stringResource(R.string.new_canvas_color_space_srgb),
                            modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                            style = MaterialTheme.typography.labelLarge,
                        )
                    }
                }
                Text(stringResource(R.string.new_canvas_color_space_note), style = MaterialTheme.typography.bodySmall)
                if (problem == null) {
                    Text(stringResource(R.string.new_canvas_memory, formatBytes(spec.estimatedBytes())), style = MaterialTheme.typography.bodyMedium)
                }
                if (errorText != null) {
                    Text(errorText, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
                }
            }
        },
        confirmButton = {
            TextButton(
                enabled = problem == null,
                onClick = { onCreate(spec, if (whiteBackground) HexColor.WHITE else null, name.trim()) },
            ) { Text(stringResource(R.string.new_canvas_create)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

@Composable
private fun PresetChip(selected: Boolean, label: String, onClick: () -> Unit) {
    FilterChip(selected = selected, onClick = onClick, label = { Text(label) })
}

@Composable
private fun NumberField(value: String, onChange: (String) -> Unit, label: String, modifier: Modifier) {
    OutlinedTextField(
        value = value,
        onValueChange = { v -> onChange(v.filter(Char::isDigit).take(6)) },
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        modifier = modifier,
    )
}
