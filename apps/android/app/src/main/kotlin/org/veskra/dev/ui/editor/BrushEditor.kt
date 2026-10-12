package org.veskra.dev.ui.editor

import android.graphics.Bitmap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.StringRes
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import org.veskra.dev.R
import org.veskra.dev.ui.BrushEditorState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.JsonNull
import org.veskra.kit.JsonObject
import org.veskra.kit.at
import org.veskra.kit.PointF
import org.veskra.kit.flag
import org.veskra.kit.number
import org.veskra.kit.points
import org.veskra.kit.text
import org.veskra.kit.with
import org.veskra.kit.withPoints
import kotlin.math.roundToInt

private enum class EditorTab(@param:StringRes val label: Int) {
    TIP(R.string.brush_tab_tip),
    STROKE(R.string.brush_tab_stroke),
    DYNAMICS(R.string.brush_tab_dynamics),
    TEXTURE(R.string.brush_tab_texture),
    JITTER(R.string.brush_tab_jitter),
}

/**
 * Brush editor (full screen): edits the engine's preset JSON section by section, with a stroke
 * preview that follows every save. Built-in brushes are edited as a copy (see
 * [VeskraViewModel.openBrushEditor]); the engine decides whether the result can stay an editable
 * vector brush and the header says so.
 */
@Composable
fun BrushEditor(ed: BrushEditorState, vm: VeskraViewModel) {
    val tipImage = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri -> if (uri != null) vm.importBrushImage(uri, "tip") }
    val textureImage = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri -> if (uri != null) vm.importBrushImage(uri, "texture") }
    var tab by rememberSaveable { mutableStateOf(EditorTab.TIP) }
    val p = ed.preset
    val edit: ((JsonObject) -> JsonObject) -> Unit = vm::editBrush

    Dialog(onDismissRequest = vm::closeBrushEditor, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Column(Modifier.safeDrawingPadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    IconButton(onClick = vm::closeBrushEditor) { Icon(VeskraIcons.Back, contentDescription = stringResource(R.string.action_done)) }
                    var name by remember(ed.id) { mutableStateOf(p.text("name").orEmpty()) }
                    OutlinedTextField(
                        value = name,
                        onValueChange = { v ->
                            name = v.take(64)
                            if (name.isNotBlank()) edit { it.with(name, "name") }
                        },
                        label = { Text(stringResource(R.string.brush_name)) },
                        singleLine = true,
                        modifier = Modifier.widthIn(max = 360.dp).weight(1f),
                    )
                    Text(
                        stringResource(
                            when {
                                ed.saving -> R.string.brush_editor_saving
                                ed.vectorEligible -> R.string.brush_editor_vector
                                else -> R.string.brush_editor_raster
                            },
                        ),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.widthIn(max = 260.dp),
                    )
                }
                Preview(ed, vm)
                ed.warnings.forEach { Text("• $it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error) }
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    EditorTab.entries.forEach { t -> FilterChip(selected = tab == t, onClick = { tab = t }, label = { Text(stringResource(t.label)) }) }
                }
                Column(
                    Modifier.fillMaxWidth().weight(1f).verticalScroll(rememberScrollState()).widthIn(max = 720.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    when (tab) {
                        EditorTab.TIP -> TipTab(p, edit) { tipImage.launch("image/*") }
                        EditorTab.STROKE -> StrokeTab(p, edit)
                        EditorTab.DYNAMICS -> DynamicsTab(p, edit)
                        EditorTab.TEXTURE -> TextureTab(p, edit) { textureImage.launch("image/*") }
                        EditorTab.JITTER -> JitterTab(p, edit)
                    }
                }
            }
        }
    }
}

@Composable
private fun Preview(ed: BrushEditorState, vm: VeskraViewModel) {
    var bmp by remember { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(ed.id, ed.version) { vm.editorPreview(720, 140)?.let { bmp = it } }
    Box(
        Modifier.fillMaxWidth().height(120.dp).clip(RoundedCornerShape(12.dp)).background(Color.White),
        contentAlignment = Alignment.Center,
    ) {
        bmp?.let { Image(it.asImageBitmap(), contentDescription = stringResource(R.string.brush_preview_cd), contentScale = ContentScale.Fit, modifier = Modifier.fillMaxSize()) }
    }
}

// ------------------------------------------------------------------------------------- controls

@Composable
private fun ValueSlider(
    label: String,
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    format: (Float) -> String,
    onCommit: (Float) -> Unit,
) {
    var local by remember(value) { mutableFloatStateOf(value.coerceIn(range)) }
    SliderRow(label, format(local)) {
        Slider(
            value = local,
            onValueChange = { local = it },
            onValueChangeFinished = { onCommit(local) },
            valueRange = range,
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = label },
        )
    }
}

private fun pctText(v: Float) = "${(v * 100).roundToInt()}%"

private fun degText(v: Float) = "${v.roundToInt()}°"

@Composable
private fun Fraction(label: Int, p: JsonObject, vararg path: String, max: Float = 1f, edit: ((JsonObject) -> JsonObject) -> Unit) {
    ValueSlider(stringResource(label), p.number(*path) ?: 0f, 0f..max, ::pctText) { v -> edit { it.with(v, *path) } }
}

@Composable
private fun Toggle(label: Int, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(stringResource(label), modifier = Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

@Composable
private fun <T> Choice(label: Int, options: List<Pair<T, Int>>, selected: T, onSelect: (T) -> Unit) {
    PanelLabel(stringResource(label), Modifier.padding(top = 6.dp))
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        options.forEach { (v, name) -> FilterChip(selected = v == selected, onClick = { onSelect(v) }, label = { Text(stringResource(name)) }) }
    }
}

// ----------------------------------------------------------------------------------------- tabs

@Composable
private fun TipTab(p: JsonObject, edit: ((JsonObject) -> JsonObject) -> Unit, pickImage: () -> Unit) {
    val shape = p.text("tip", "shape") ?: "round"
    Choice(R.string.brush_tip_shape, listOf("round" to R.string.brush_tip_round, "image" to R.string.brush_tip_image), shape) { s ->
        if (s == "image") pickImage() else edit { it.with("round", "tip", "shape").with(null as String?, "tip", "image") }
    }
    if (shape == "image") {
        OutlinedButton(onClick = pickImage, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.brush_pick_image)) }
        Text(stringResource(R.string.brush_tip_image_help), style = MaterialTheme.typography.bodySmall)
    } else {
        Fraction(R.string.brush_hardness, p, "tip", "hardness", edit = edit)
    }
    ValueSlider(stringResource(R.string.brush_angle), p.number("tip", "angle") ?: 0f, -180f..180f, ::degText) { v -> edit { it.with(v, "tip", "angle") } }
    ValueSlider(stringResource(R.string.brush_roundness), p.number("tip", "roundness") ?: 1f, 0.05f..1f, ::pctText) { v -> edit { it.with(v, "tip", "roundness") } }
    Choice(
        R.string.brush_rotation,
        listOf("fixed" to R.string.brush_rotation_fixed, "direction" to R.string.brush_rotation_direction, "pen" to R.string.brush_rotation_pen),
        p.text("tip", "rotation") ?: "fixed",
    ) { r -> edit { it.with(r, "tip", "rotation") } }
}

@Composable
private fun StrokeTab(p: JsonObject, edit: ((JsonObject) -> JsonObject) -> Unit) {
    ValueSlider(stringResource(R.string.tool_brush_size), p.number("size_px") ?: 10f, 1f..300f, { "${it.roundToInt()} px" }) { v -> edit { it.with(v, "size_px") } }
    Fraction(R.string.tool_brush_opacity, p, "opacity", edit = edit)
    ValueSlider(stringResource(R.string.brush_flow), p.number("flow") ?: 1f, 0.01f..1f, ::pctText) { v -> edit { it.with(v, "flow") } }
    ValueSlider(stringResource(R.string.brush_spacing), p.number("spacing") ?: 0.1f, 0.005f..2f, ::pctText) { v -> edit { it.with(v, "spacing") } }
    Fraction(R.string.tool_brush_smoothing, p, "stabilization", edit = edit)
    Fraction(R.string.brush_wet_edges, p, "wet_edges", edit = edit)
    Toggle(R.string.brush_build_up, p.text("accumulation") == "build_up") { on -> edit { it.with(if (on) "build_up" else "max", "accumulation") } }
}

private val DYNAMIC_TARGETS = listOf(
    "size" to R.string.tool_brush_size,
    "opacity" to R.string.tool_brush_opacity,
    "flow" to R.string.brush_flow,
    "texture" to R.string.brush_texture_depth,
    "scatter" to R.string.brush_scatter,
)

@Composable
private fun DynamicsTab(p: JsonObject, edit: ((JsonObject) -> JsonObject) -> Unit) {
    var source by rememberSaveable { mutableStateOf("pressure") }
    Choice(
        R.string.brush_dynamics_input,
        listOf("pressure" to R.string.brush_input_pressure, "tilt" to R.string.brush_input_tilt, "velocity" to R.string.brush_input_velocity),
        source,
    ) { source = it }
    Text(
        stringResource(
            when (source) {
                "pressure" -> R.string.brush_input_pressure_help
                "tilt" -> R.string.brush_input_tilt_help
                else -> R.string.brush_input_velocity_help
            },
        ),
        style = MaterialTheme.typography.bodySmall,
    )
    DYNAMIC_TARGETS.forEach { (target, label) ->
        val base = arrayOf("dynamics", source, target)
        val enabled = p.flag(*base, "enabled") ?: false
        Toggle(label, enabled) { on -> edit { it.with(on, *base, "enabled") } }
        if (enabled) {
            val min = p.number(*base, "min") ?: 0f
            val max = p.number(*base, "max") ?: 1f
            ValueSlider(stringResource(R.string.brush_dynamics_min), min, 0f..1f, ::pctText) { v -> edit { it.with(v, *base, "min") } }
            ValueSlider(stringResource(R.string.brush_dynamics_max), max, 0f..1f, ::pctText) { v -> edit { it.with(v, *base, "max") } }
            ResponseCurveEditor(
                points = p.points(*base, "curve") ?: listOf(PointF(0f, 0f), PointF(1f, 1f)),
                onDone = { pts -> edit { it.withPoints(pts, *base, "curve") } },
                modifier = Modifier.size(width = 240.dp, height = 160.dp),
            )
        }
    }
}

@Composable
private fun TextureTab(p: JsonObject, edit: ((JsonObject) -> JsonObject) -> Unit, pickImage: () -> Unit) {
    val hasTexture = p.at("texture") is JsonObject
    val source = if (hasTexture) p.text("texture", "source") ?: "procedural" else "none"
    Choice(
        R.string.brush_texture,
        listOf("none" to R.string.brush_texture_none, "procedural" to R.string.brush_texture_grain, "image" to R.string.brush_tip_image),
        source,
    ) { s ->
        when (s) {
            "none" -> edit { it.with(JsonNull, "texture") }
            "procedural" -> edit { withTextureDefaults(it).with("procedural", "texture", "source").with(null as String?, "texture", "image") }
            "image" -> pickImage()
        }
    }
    if (hasTexture) {
        if (source == "image") OutlinedButton(onClick = pickImage, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.brush_pick_image)) }
        val scaleMax = if (source == "image") 64f else 256f
        ValueSlider(stringResource(R.string.brush_texture_scale), p.number("texture", "scale") ?: 2f, 0.05f..scaleMax, { "%.1f px".format(it) }) { v -> edit { it.with(v, "texture", "scale") } }
        Fraction(R.string.brush_texture_depth, p, "texture", "depth", edit = edit)
        Toggle(R.string.brush_texture_invert, p.flag("texture", "invert") ?: false) { on -> edit { it.with(on, "texture", "invert") } }
        Choice(
            R.string.brush_texture_mode,
            listOf("fixed" to R.string.brush_texture_fixed, "follow" to R.string.brush_texture_follow),
            p.text("texture", "mode") ?: "fixed",
        ) { m -> edit { it.with(m, "texture", "mode") } }
    }
}

/** A new texture starts from the engine's defaults, written out so the sliders show them. */
private fun withTextureDefaults(p: JsonObject): JsonObject =
    if (p.at("texture") is JsonObject) {
        p
    } else {
        p.with(2f, "texture", "scale").with(0.5f, "texture", "depth").with(false, "texture", "invert").with("fixed", "texture", "mode")
    }

@Composable
private fun JitterTab(p: JsonObject, edit: ((JsonObject) -> JsonObject) -> Unit) {
    PanelLabel(stringResource(R.string.brush_jitter_per_dab))
    Fraction(R.string.tool_brush_size, p, "jitter", "size", edit = edit)
    Fraction(R.string.brush_angle, p, "jitter", "angle", edit = edit)
    Fraction(R.string.tool_brush_opacity, p, "jitter", "opacity", edit = edit)
    Fraction(R.string.brush_flow, p, "jitter", "flow", edit = edit)
    Fraction(R.string.brush_scatter, p, "jitter", "scatter", max = 4f, edit = edit)
    PanelLabel(stringResource(R.string.brush_jitter_color_dab), Modifier.padding(top = 8.dp))
    Fraction(R.string.color_hue, p, "jitter", "hue", edit = edit)
    Fraction(R.string.color_saturation, p, "jitter", "saturation", edit = edit)
    Fraction(R.string.color_brightness, p, "jitter", "brightness", edit = edit)
    PanelLabel(stringResource(R.string.brush_jitter_color_stroke), Modifier.padding(top = 8.dp))
    Fraction(R.string.color_hue, p, "jitter", "stroke_hue", edit = edit)
    Fraction(R.string.color_saturation, p, "jitter", "stroke_saturation", edit = edit)
    Fraction(R.string.color_brightness, p, "jitter", "stroke_brightness", edit = edit)
}

/**
 * Input → effect curve in the unit square: drag a point, touch an empty spot to add one (end
 * points keep x = 0 and x = 1). Curves may fall as well as rise ("faster = thinner").
 */
@Composable
private fun ResponseCurveEditor(points: List<PointF>, onDone: (List<PointF>) -> Unit, modifier: Modifier = Modifier) {
    var local by remember(points) { mutableStateOf(points) }
    val latest by rememberUpdatedState(local)
    val done by rememberUpdatedState(onDone)
    val line = MaterialTheme.colorScheme.primary
    val grid = MaterialTheme.colorScheme.outline.copy(alpha = 0.35f)
    val cd = stringResource(R.string.curves_editor_cd)
    Canvas(
        modifier
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .semantics { contentDescription = cd }
            .pointerInput(Unit) {
                awaitEachGesture {
                    val grab = 28.dp.toPx()
                    fun toUnit(o: Offset) = PointF((o.x / size.width).coerceIn(0f, 1f), (1f - o.y / size.height).coerceIn(0f, 1f))
                    fun toView(q: PointF) = Offset(q.x * size.width, (1f - q.y) * size.height)
                    val down = awaitFirstDown()
                    down.consume()
                    var pts = latest
                    var i = pts.indices.minByOrNull { (toView(pts[it]) - down.position).getDistance() } ?: 0
                    if ((toView(pts[i]) - down.position).getDistance() > grab && pts.size < MAX_POINTS) {
                        val added = toUnit(down.position)
                        pts = (pts + added).sortedBy { it.x }
                        i = pts.indexOf(added)
                    }
                    fun moved(o: Offset): List<PointF> {
                        val q = toUnit(o)
                        val x = when (i) {
                            0 -> 0f
                            pts.lastIndex -> 1f
                            else -> q.x.coerceIn(pts[i - 1].x + 0.02f, pts[i + 1].x - 0.02f)
                        }
                        return pts.toMutableList().also { it[i] = PointF(x, q.y) }
                    }
                    var last = down.position
                    local = moved(last)
                    drag(down.id) { c ->
                        last = c.position
                        local = moved(last)
                        c.consume()
                    }
                    local = moved(last)
                    done(local)
                }
            },
    ) {
        for (k in 1..3) {
            val t = k / 4f
            drawLine(grid, Offset(t * size.width, 0f), Offset(t * size.width, size.height))
            drawLine(grid, Offset(0f, t * size.height), Offset(size.width, t * size.height))
        }
        val path = Path()
        local.forEachIndexed { k, q ->
            val o = Offset(q.x * size.width, (1f - q.y) * size.height)
            if (k == 0) path.moveTo(o.x, o.y) else path.lineTo(o.x, o.y)
        }
        drawPath(path, line, style = Stroke(2.dp.toPx()))
        local.forEach { q -> drawCircle(line, radius = 6.dp.toPx(), center = Offset(q.x * size.width, (1f - q.y) * size.height)) }
    }
}

private const val MAX_POINTS = 8
