package org.veskra.dev.ui.editor

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.kit.HexColor

/** Default swatches: neutral ramp + basic hues (sRGB values; plain data, no third-party palette). */
private val SWATCHES: List<HexColor> = listOf(
    0x000000, 0x404040, 0x808080, 0xBFBFBF, 0xFFFFFF,
    0x7A1F1F, 0xD03A2F, 0xF08A3C, 0xF2C94C, 0x6FAF4F,
    0x2E7D5B, 0x2F80C0, 0x24477F, 0x6B4FA0, 0xC0508A, 0x8A5A3C,
).map(::HexColor)

/**
 * Color sheet (DSN-03 slice): numeric sRGB hex entry (usable without gestures, MASTER_PLAN §8.3),
 * swatches, recent colors, profile badge. Color wheel/eyedropper are follow-up tickets.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun ColorSheet(state: UiState, vm: VeskraViewModel) {
    var hex by rememberSaveable { mutableStateOf(state.color.toHex()) }
    val parsed = HexColor.parse(hex)
    ModalBottomSheet(onDismissRequest = { vm.openSheet(null) }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(stringResource(R.string.color_sheet_title), style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                Surface(shape = MaterialTheme.shapes.small, color = MaterialTheme.colorScheme.secondaryContainer) {
                    Text(stringResource(R.string.color_profile_badge), modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp))
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Swatch(parsed ?: state.color, selected = true, label = (parsed ?: state.color).toHex(), onClick = {})
                OutlinedTextField(
                    value = hex,
                    onValueChange = { hex = it.take(7) },
                    label = { Text(stringResource(R.string.color_hex_label)) },
                    isError = parsed == null,
                    supportingText = { if (parsed == null) Text(stringResource(R.string.color_hex_invalid)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { parsed?.let(vm::setColor) }),
                    modifier = Modifier.weight(1f),
                )
                Button(onClick = { parsed?.let(vm::setColor) }, enabled = parsed != null) { Text(stringResource(R.string.color_apply)) }
            }
            Text(stringResource(R.string.color_swatches), style = MaterialTheme.typography.titleSmall)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                SWATCHES.forEach { c ->
                    Swatch(c, selected = c == state.color, label = c.toHex()) {
                        hex = c.toHex()
                        vm.setColor(c)
                    }
                }
            }
            if (state.recentColors.isNotEmpty()) {
                Text(stringResource(R.string.color_recent), style = MaterialTheme.typography.titleSmall)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    state.recentColors.forEach { c ->
                        Swatch(c, selected = c == state.color, label = c.toHex()) {
                            hex = c.toHex()
                            vm.setColor(c)
                        }
                    }
                }
            }
        }
    }
}

/** 48 dp touch target; accessible name includes the hex value. */
@Composable
private fun Swatch(c: HexColor, selected: Boolean, label: String, onClick: () -> Unit) {
    val cd = stringResource(R.string.color_swatch_cd, label)
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(48.dp)
            .semantics { contentDescription = cd }
            .clickable(role = Role.Button, onClick = onClick),
    ) {
        Box(
            Modifier
                .size(36.dp)
                .background(Color(c.toArgb()), RoundedCornerShape(8.dp))
                .border(
                    width = if (selected) 3.dp else 1.dp,
                    color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
                    shape = RoundedCornerShape(8.dp),
                ),
        )
    }
}
