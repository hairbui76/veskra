package org.veskra.dev.ui.editor

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.Guide
import org.veskra.dev.ui.SymmetryMode
import org.veskra.dev.ui.Tool
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons

/** One cell of the tools grid: an icon with a caption; [active] marks a tool or toggle that is on. */
private class ToolItem(val icon: ImageVector, @param:StringRes val label: Int, val active: Boolean = false, val onClick: () -> Unit)

private class ToolSection(@param:StringRes val title: Int, val items: List<ToolItem>)

/**
 * Tools panel: everything that is not a brush, grouped like a painter's toolbox — a Create tab
 * (things that add marks or help draw them) and an Edit tab (things that change what is there).
 */
@Composable
fun ToolsPanel(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    var editTab by rememberSaveable { mutableStateOf(false) }
    val importImage = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri -> if (uri != null) vm.importImage(uri) }
    fun tool(icon: ImageVector, @StringRes label: Int, t: Tool) = ToolItem(icon, label, active = state.tool == t) { vm.setTool(t) }
    fun symmetry(icon: ImageVector, @StringRes label: Int, mode: SymmetryMode) = ToolItem(icon, label, active = state.symmetry == mode) {
        vm.setSymmetry(if (state.symmetry == mode) SymmetryMode.OFF else mode)
    }

    val create = listOf(
        ToolSection(
            R.string.tools_section_tools,
            listOf(
                tool(VeskraIcons.Fill, R.string.tool_fill, Tool.FILL),
                tool(VeskraIcons.Gradient, R.string.tool_gradient, Tool.GRADIENT),
                ToolItem(VeskraIcons.Text, R.string.tool_text) { vm.showTextDialog(true) },
                ToolItem(VeskraIcons.Image, R.string.tool_import_image) { importImage.launch("image/*") },
                tool(VeskraIcons.Eyedropper, R.string.tool_eyedropper, Tool.EYEDROPPER),
            ),
        ),
        ToolSection(
            R.string.tools_section_guides,
            listOf(
                // A ruler that is on is switched off by its button; otherwise the button starts placing it.
                ToolItem(VeskraIcons.GuideLine, R.string.tool_guide_line, active = state.guide is Guide.Line || state.tool == Tool.GUIDE_LINE) {
                    if (state.guide is Guide.Line) vm.setGuide(null) else vm.setTool(Tool.GUIDE_LINE)
                },
                ToolItem(VeskraIcons.GuideEllipse, R.string.tool_guide_ellipse, active = state.guide is Guide.Ellipse || state.tool == Tool.GUIDE_ELLIPSE) {
                    if (state.guide is Guide.Ellipse) vm.setGuide(null) else vm.setTool(Tool.GUIDE_ELLIPSE)
                },
                ToolItem(VeskraIcons.Lazy, R.string.tool_lazy, active = state.brushStabilization > 0f) {
                    vm.setBrushStabilization(if (state.brushStabilization > 0f) 0f else LAZY_DEFAULT)
                },
            ),
        ),
        ToolSection(
            R.string.tools_section_shapes,
            listOf(
                tool(VeskraIcons.ShapeLine, R.string.tool_shape_line, Tool.SHAPE_LINE),
                tool(VeskraIcons.ShapeRect, R.string.tool_shape_rect, Tool.SHAPE_RECT),
                tool(VeskraIcons.ShapeCircle, R.string.tool_shape_circle, Tool.SHAPE_ELLIPSE),
                tool(VeskraIcons.ShapePath, R.string.tool_shape_path, Tool.SHAPE_PATH),
            ),
        ),
        ToolSection(
            R.string.tools_section_symmetry,
            listOf(
                symmetry(VeskraIcons.SymVertical, R.string.tool_sym_vertical, SymmetryMode.VERTICAL),
                symmetry(VeskraIcons.SymHorizontal, R.string.tool_sym_horizontal, SymmetryMode.HORIZONTAL),
                symmetry(VeskraIcons.SymRadial, R.string.tool_sym_radial, SymmetryMode.RADIAL),
            ),
        ),
        ToolSection(
            R.string.tools_section_camera,
            listOf(
                ToolItem(VeskraIcons.Fit, R.string.tool_fit_short) { vm.fitView() },
                ToolItem(VeskraIcons.Mirror, R.string.tool_flip, active = state.mirrored) { vm.toggleMirror() },
                ToolItem(VeskraIcons.Lock, R.string.tool_lock, active = state.viewLocked) { vm.toggleViewLock() },
            ),
        ),
    )
    val edit = listOf(
        ToolSection(
            R.string.tools_section_select,
            listOf(
                tool(VeskraIcons.SelectLasso, R.string.tool_select_lasso, Tool.SELECT_LASSO),
                tool(VeskraIcons.SelectRect, R.string.tool_select_rect, Tool.SELECT_RECT),
                tool(VeskraIcons.SelectEllipse, R.string.tool_select_ellipse, Tool.SELECT_ELLIPSE),
                tool(VeskraIcons.Wand, R.string.tool_select_wand, Tool.SELECT_WAND),
            ),
        ),
        ToolSection(
            R.string.tools_section_transform,
            listOf(
                tool(VeskraIcons.Transform, R.string.tool_transform_basic, Tool.TRANSFORM),
                tool(VeskraIcons.Distort, R.string.tool_distort, Tool.DISTORT),
            ),
        ),
        ToolSection(
            R.string.tools_section_edit,
            listOf(
                tool(VeskraIcons.Adjust, R.string.tool_adjust_short, Tool.ADJUST),
                tool(VeskraIcons.Curve, R.string.tool_curves_short, Tool.CURVES),
            ),
        ),
        ToolSection(
            R.string.tools_section_canvas,
            listOf(
                tool(VeskraIcons.Crop, R.string.tool_crop, Tool.CROP),
                ToolItem(VeskraIcons.Resize, R.string.tool_resize) { vm.showResizeDialog(true) },
            ),
        ),
    )

    FloatingPanel(modifier.widthIn(max = 380.dp)) {
        Column(Modifier.padding(12.dp)) {
            Row(Modifier.fillMaxWidth()) {
                TabTitle(stringResource(R.string.tools_tab_create), selected = !editTab, modifier = Modifier.weight(1f)) { editTab = false }
                TabTitle(stringResource(R.string.tools_tab_edit), selected = editTab, modifier = Modifier.weight(1f)) { editTab = true }
            }
            Column(
                Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()).padding(top = 8.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                (if (editTab) edit else create).forEach { section -> Section(section) }
            }
        }
    }
}

private const val LAZY_DEFAULT = 0.6f

@Composable
private fun TabTitle(text: String, selected: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = modifier.heightIn(min = 48.dp).clickable(role = Role.Tab, onClick = onClick).semantics { this.selected = selected },
    ) {
        Text(
            text,
            style = MaterialTheme.typography.titleMedium,
            color = if (selected) colors.onSurface else colors.onSurfaceVariant.copy(alpha = 0.6f),
            modifier = Modifier.padding(vertical = 10.dp),
        )
        Box(Modifier.fillMaxWidth().padding(horizontal = 8.dp).heightIn(min = 2.dp).background(if (selected) colors.onSurface else colors.surface))
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Section(section: ToolSection) {
    PanelLabel(stringResource(section.title), Modifier.padding(start = 8.dp, top = 8.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        section.items.forEach { item -> Cell(item) }
    }
}

@Composable
private fun Cell(item: ToolItem) {
    val colors = MaterialTheme.colorScheme
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier
            .width(84.dp)
            .heightIn(min = 72.dp)
            .clip(MaterialTheme.shapes.medium)
            .clickable(role = Role.Button, onClick = item.onClick)
            .semantics(mergeDescendants = true) { selected = item.active }
            .padding(vertical = 6.dp),
    ) {
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier.size(40.dp).clip(CircleShape).background(if (item.active) colors.primary else colors.surface),
        ) {
            Icon(item.icon, contentDescription = null, tint = if (item.active) colors.onPrimary else colors.onSurface)
        }
        Text(
            stringResource(item.label),
            style = MaterialTheme.typography.labelMedium,
            color = colors.onSurfaceVariant,
            textAlign = TextAlign.Center,
            maxLines = 2,
        )
    }
}
