package org.veskra.dev.ui.editor

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.BrushPreset

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

/** Brush picker (DSN-01): presets + size/opacity. Brush behavior itself is owned by the engine (BRH). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrushSheet(state: UiState, vm: VeskraViewModel) {
    ModalBottomSheet(onDismissRequest = { vm.openSheet(null) }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text(stringResource(R.string.brush_sheet_title), style = MaterialTheme.typography.titleLarge)
            LazyVerticalGrid(
                columns = GridCells.Adaptive(minSize = 160.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.heightIn(max = 360.dp),
            ) {
                items(BrushPreset.entries, key = { it.id }) { preset ->
                    val selected = preset == state.brush
                    OutlinedCard(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(min = 56.dp)
                            .selectable(selected = selected, role = Role.RadioButton, onClick = { vm.selectBrush(preset) }),
                    ) {
                        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                            Icon(
                                if (preset.isEraser) VeskraIcons.Eraser else VeskraIcons.Brush,
                                contentDescription = null,
                            )
                            Text(stringResource(brushName(preset)), modifier = Modifier.padding(start = 12.dp).weight(1f))
                            if (selected) Icon(VeskraIcons.Check, contentDescription = stringResource(R.string.brush_selected))
                        }
                    }
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                BrushSizeSlider(state.brushSize, vm::setBrushSize, Modifier.weight(1f))
                BrushOpacitySlider(state.brushOpacity, vm::setBrushOpacity, Modifier.weight(1f))
            }
        }
    }
}
