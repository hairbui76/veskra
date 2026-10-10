package org.veskra.dev.ui.editor

import androidx.annotation.StringRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.BlendMode
import org.veskra.kit.LayerInfo
import org.veskra.kit.LayerKind
import kotlin.math.roundToInt

/** Mirrors veskra_core::limits::MAX_NAME_BYTES. */
private const val MAX_NAME_BYTES = 255

@StringRes
private fun blendName(b: BlendMode): Int = when (b) {
    BlendMode.NORMAL -> R.string.blend_normal
    BlendMode.MULTIPLY -> R.string.blend_multiply
    BlendMode.SCREEN -> R.string.blend_screen
    BlendMode.OVERLAY -> R.string.blend_overlay
}

/**
 * Layer panel (DSN-03 slice): add/remove/duplicate/reorder/visibility/opacity/blend/alpha-lock/
 * clip/rename/mask, with the layer kind on every row (MASTER_PLAN §2.1 item 5).
 * Thumbnails and drag-reorder are follow-ups; every action has a button (no gesture-only control).
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun LayerPanel(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    var renaming by remember { mutableStateOf<LayerInfo?>(null) }
    FloatingPanel(modifier.widthIn(max = 380.dp)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.layers_title), style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(start = 4.dp))
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilledTonalButton(onClick = { vm.addLayer(LayerKind.RASTER) }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Icon(VeskraIcons.Add, contentDescription = null)
                    Text(stringResource(R.string.layers_add_raster), modifier = Modifier.padding(start = 8.dp))
                }
                FilledTonalButton(onClick = { vm.addLayer(LayerKind.VECTOR) }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Icon(VeskraIcons.Add, contentDescription = null)
                    Text(stringResource(R.string.layers_add_vector), modifier = Modifier.padding(start = 8.dp))
                }
            }
            LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.heightIn(max = 460.dp)) {
                items(state.layers, key = { it.id }) { layer ->
                    LayerRow(
                        layer = layer,
                        active = layer.id == state.doc?.activeLayerId,
                        vm = vm,
                        onRename = { renaming = layer },
                    )
                }
            }
        }
    }
    renaming?.let { layer -> RenameDialog(layer, onDismiss = { renaming = null }, onRename = { vm.renameLayer(layer.id, it); renaming = null }) }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun LayerRow(layer: LayerInfo, active: Boolean, vm: VeskraViewModel, onRename: () -> Unit) {
    val selectCd = stringResource(R.string.layers_select_cd, layer.name)
    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(start = (8 + layer.depth * 16).dp, end = 8.dp, top = 4.dp, bottom = 4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (layer.visible) {
                    RailButton(VeskraIcons.Visible, stringResource(R.string.layers_hide)) { vm.setLayerVisible(layer.id, false) }
                } else {
                    RailButton(VeskraIcons.Hidden, stringResource(R.string.layers_show)) { vm.setLayerVisible(layer.id, true) }
                }
                Column(
                    Modifier
                        .weight(1f)
                        .heightIn(min = 48.dp)
                        .semantics { contentDescription = selectCd }
                        .clickable(role = Role.Button) { vm.selectLayer(layer.id) }
                        .padding(horizontal = 8.dp),
                    verticalArrangement = Arrangement.Center,
                ) {
                    Text(layer.name, style = if (active) MaterialTheme.typography.titleMedium else MaterialTheme.typography.bodyLarge)
                    // The row names the layer's own kind; what exports as editable vectors is counted
                    // by the export sheet's preflight, where it depends on the chosen format.
                    val kind = stringResource(
                        when (layer.kind) {
                            LayerKind.GROUP -> R.string.layers_kind_group
                            LayerKind.VECTOR -> R.string.layers_kind_vector
                            LayerKind.RASTER -> R.string.layers_kind_raster
                        },
                    )
                    Text("$kind · ${stringResource(blendName(layer.blend))} · ${(layer.opacity * 100).roundToInt()}%", style = MaterialTheme.typography.bodySmall)
                }
                RailButton(VeskraIcons.Up, stringResource(R.string.layers_move_up)) { vm.moveLayer(layer.id, up = true) }
                RailButton(VeskraIcons.Down, stringResource(R.string.layers_move_down)) { vm.moveLayer(layer.id, up = false) }
            }
            if (active) {
                var opacity by remember(layer.opacity) { mutableFloatStateOf(layer.opacity) }
                val opacityLabel = stringResource(R.string.layers_opacity)
                Text(stringResource(R.string.layers_opacity_value, (opacity * 100).roundToInt()), style = MaterialTheme.typography.labelMedium)
                Slider(
                    value = opacity,
                    onValueChange = { opacity = it },
                    onValueChangeFinished = { vm.setLayerOpacity(layer.id, opacity) },
                    modifier = Modifier.semantics { contentDescription = opacityLabel },
                )
                Text(stringResource(R.string.layers_blend), style = MaterialTheme.typography.labelMedium)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    BlendMode.entries.forEach { b ->
                        FilterChip(selected = layer.blend == b, onClick = { vm.setLayerBlend(layer.id, b) }, label = { Text(stringResource(blendName(b))) })
                    }
                }
                SwitchRow(stringResource(R.string.layers_alpha_lock), layer.alphaLock) { vm.setLayerAlphaLock(layer.id, it) }
                SwitchRow(stringResource(R.string.layers_clip), layer.clip) { vm.setLayerClip(layer.id, it) }
                Row {
                    RailButton(VeskraIcons.Rename, stringResource(R.string.layers_rename), onClick = onRename)
                    RailButton(VeskraIcons.Duplicate, stringResource(R.string.layers_duplicate)) { vm.duplicateLayer(layer.id) }
                    RailButton(VeskraIcons.Delete, stringResource(R.string.layers_delete)) { vm.removeLayer(layer.id) }
                    if (layer.kind == LayerKind.RASTER) {
                        if (layer.hasMask) {
                            RailButton(VeskraIcons.Check, stringResource(R.string.layers_mask_apply)) { vm.applyLayerMask(layer.id) }
                            RailButton(VeskraIcons.Close, stringResource(R.string.layers_mask_remove)) { vm.removeLayerMask(layer.id) }
                        } else {
                            RailButton(VeskraIcons.Mask, stringResource(R.string.layers_mask_add)) { vm.maskLayerFromSelection(layer.id) }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SwitchRow(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .toggleable(value = checked, role = Role.Switch, onValueChange = onChange),
    ) {
        Text(label, modifier = Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = null)
    }
}

@Composable
private fun RenameDialog(layer: LayerInfo, onDismiss: () -> Unit, onRename: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf(layer.name) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.layers_rename)) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { if (it.toByteArray(Charsets.UTF_8).size <= MAX_NAME_BYTES) name = it },
                label = { Text(stringResource(R.string.layers_name_label)) },
                singleLine = true,
            )
        },
        confirmButton = { TextButton(onClick = { onRename(name) }, enabled = name.isNotBlank()) { Text(stringResource(R.string.action_ok)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}
