package org.veskra.dev.ui.editor

import android.graphics.Bitmap
import androidx.annotation.StringRes
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.Tool
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.Cmyk
import org.veskra.kit.ColorMath
import org.veskra.kit.HexColor
import org.veskra.kit.Hsb
import org.veskra.kit.Lab
import org.veskra.kit.Palette
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt

private enum class ColorTab { WHEEL, SLIDERS, SWATCHES, TONE }

private enum class SliderModel { HSB, RGB, LAB, CMYK }

private enum class ToneMode { SHADE, TONE, TINT }

private val HueColors = listOf(0xFFFF0000, 0xFFFFFF00, 0xFF00FF00, 0xFF00FFFF, 0xFF0000FF, 0xFFFF00FF, 0xFFFF0000).map { Color(it) }

private fun HexColor.compose(): Color = Color(toArgb())

/**
 * Color picker panel: four ways to choose a color (hue wheel, numeric sliders, swatch palettes, tone
 * sphere) behind a tab bar, plus the eyedropper. Colors are sRGB (the only working space in v1).
 *
 * Dragging a control only repaints the chrome ([VeskraViewModel.previewColor]); the engine receives
 * the color when the drag ends ([VeskraViewModel.setColor]).
 */
@Composable
fun ColorPanel(state: UiState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    var tab by rememberSaveable { mutableStateOf(ColorTab.WHEEL) }
    // Hue/saturation are kept here because sRGB cannot remember them for greys and black.
    var hsb by remember { mutableStateOf(ColorMath.toHsb(state.color)) }
    LaunchedEffect(state.color) { if (ColorMath.fromHsb(hsb) != state.color) hsb = ColorMath.toHsb(state.color) }
    val opened = remember { state.color }
    val setHsb: (Hsb, Boolean) -> Unit = { v, done ->
        hsb = v
        val c = ColorMath.fromHsb(v)
        if (done) vm.setColor(c) else vm.previewColor(c)
    }

    FloatingPanel(modifier.widthIn(max = 360.dp)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                IconButton(onClick = { vm.addToUserPalette(state.color) }) {
                    Icon(VeskraIcons.Add, contentDescription = stringResource(R.string.color_add_to_palette))
                }
                ComparePreview(previous = opened, current = state.color, onRestore = { vm.setColor(opened) }, modifier = Modifier.weight(1f))
                IconButton(onClick = { vm.setTool(Tool.EYEDROPPER) }) {
                    Icon(VeskraIcons.Eyedropper, contentDescription = stringResource(R.string.tool_eyedropper))
                }
            }
            Column(
                Modifier.fillMaxWidth().weight(1f, fill = false).heightIn(max = 480.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                when (tab) {
                    ColorTab.WHEEL -> {
                        ColorWheel(hsb, setHsb, Modifier.fillMaxWidth(0.8f).aspectRatio(1f).align(Alignment.CenterHorizontally))
                        PaletteStrip(state, vm)
                    }
                    ColorTab.SLIDERS -> {
                        SlidersTab(state.color, hsb, setHsb, vm)
                        PaletteStrip(state, vm)
                    }
                    ColorTab.SWATCHES -> SwatchesTab(state, vm)
                    ColorTab.TONE -> ToneTab(hsb, setHsb, vm)
                }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                TabButton(VeskraIcons.TabWheel, stringResource(R.string.color_tab_wheel), tab == ColorTab.WHEEL) { tab = ColorTab.WHEEL }
                TabButton(VeskraIcons.TabSliders, stringResource(R.string.color_tab_sliders), tab == ColorTab.SLIDERS) { tab = ColorTab.SLIDERS }
                TabButton(VeskraIcons.TabSwatches, stringResource(R.string.color_tab_swatches), tab == ColorTab.SWATCHES) { tab = ColorTab.SWATCHES }
                TabButton(VeskraIcons.TabTone, stringResource(R.string.color_tab_tone), tab == ColorTab.TONE) { tab = ColorTab.TONE }
            }
        }
    }
}

@Composable
private fun TabButton(icon: ImageVector, label: String, selected: Boolean, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .size(width = 72.dp, height = 48.dp)
            .clip(RoundedCornerShape(24.dp))
            .background(if (selected) colors.primary else Color.Transparent)
            .clickable(role = Role.Tab, onClick = onClick)
            .semantics { this.selected = selected },
    ) {
        Icon(icon, contentDescription = label, tint = if (selected) colors.onPrimary else colors.onSurfaceVariant)
    }
}

/** Left half: the color when the panel opened (tap to go back to it). Right half: the color now. */
@Composable
private fun ComparePreview(previous: HexColor, current: HexColor, onRestore: () -> Unit, modifier: Modifier = Modifier) {
    val restoreCd = stringResource(R.string.color_previous_cd, previous.toHex())
    val currentCd = stringResource(R.string.tool_current_color_cd, current.toHex())
    Row(modifier.height(40.dp).clip(RoundedCornerShape(12.dp)).border(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.5f), RoundedCornerShape(12.dp))) {
        Box(
            Modifier.weight(1f).fillMaxSize().background(previous.compose())
                .clickable(role = Role.Button, onClick = onRestore)
                .semantics { contentDescription = restoreCd },
        )
        Box(Modifier.weight(2f).fillMaxSize().background(current.compose()).semantics { contentDescription = currentCd })
    }
}

// ------------------------------------------------------------------------------------------ wheel

/** Hue ring with a saturation (x) / brightness (y) square inside. */
@Composable
private fun ColorWheel(hsb: Hsb, onChange: (Hsb, Boolean) -> Unit, modifier: Modifier = Modifier) {
    val latest by rememberUpdatedState(hsb)
    val callback by rememberUpdatedState(onChange)
    val cd = stringResource(R.string.color_wheel_cd)
    Canvas(
        modifier
            .semantics { contentDescription = cd }
            .pointerInput(Unit) {
                awaitEachGesture {
                    val g = WheelGeometry(size.width.toFloat(), size.height.toFloat())
                    val down = awaitFirstDown()
                    val onRing = (down.position - g.center).getDistance() >= g.inner - g.ring * 0.3f
                    fun update(p: Offset, done: Boolean) {
                        val cur = latest
                        val next = if (onRing) {
                            val deg = Math.toDegrees(atan2((p.y - g.center.y).toDouble(), (p.x - g.center.x).toDouble()))
                            cur.copy(h = ((deg + 360.0) % 360.0).toFloat())
                        } else {
                            cur.copy(
                                s = ((p.x - (g.center.x - g.half)) / (2 * g.half)).coerceIn(0f, 1f),
                                b = 1f - ((p.y - (g.center.y - g.half)) / (2 * g.half)).coerceIn(0f, 1f),
                            )
                        }
                        callback(next, done)
                    }
                    var last = down.position
                    update(last, false)
                    down.consume()
                    drag(down.id) { change ->
                        last = change.position
                        update(last, false)
                        change.consume()
                    }
                    update(last, true)
                }
            },
    ) {
        val g = WheelGeometry(size.width, size.height)
        drawCircle(Brush.sweepGradient(HueColors, g.center), radius = g.outer - g.ring / 2f, center = g.center, style = Stroke(g.ring))
        val topLeft = Offset(g.center.x - g.half, g.center.y - g.half)
        val square = Size(2 * g.half, 2 * g.half)
        val pure = ColorMath.fromHsb(hsb.h, 1f, 1f).compose()
        drawRect(Brush.horizontalGradient(listOf(Color.White, pure), startX = topLeft.x, endX = topLeft.x + square.width), topLeft, square)
        drawRect(Brush.verticalGradient(listOf(Color.Transparent, Color.Black), startY = topLeft.y, endY = topLeft.y + square.height), topLeft, square)

        val rad = Math.toRadians(hsb.h.toDouble())
        val mid = g.outer - g.ring / 2f
        val hueAt = Offset(g.center.x + mid * cos(rad).toFloat(), g.center.y + mid * sin(rad).toFloat())
        drawCircle(Color.White, radius = g.ring * 0.42f, center = hueAt, style = Stroke(3.dp.toPx()))
        drawCircle(Color.Black.copy(alpha = 0.4f), radius = g.ring * 0.42f + 2.dp.toPx(), center = hueAt, style = Stroke(1.dp.toPx()))

        val svAt = Offset(topLeft.x + hsb.s * square.width, topLeft.y + (1f - hsb.b) * square.height)
        drawCircle(ColorMath.fromHsb(hsb).compose(), radius = 11.dp.toPx(), center = svAt)
        drawCircle(Color.White, radius = 11.dp.toPx(), center = svAt, style = Stroke(2.5.dp.toPx()))
        drawCircle(Color.Black.copy(alpha = 0.4f), radius = 12.5.dp.toPx(), center = svAt, style = Stroke(1.dp.toPx()))
    }
}

private class WheelGeometry(width: Float, height: Float) {
    val center = Offset(width / 2f, height / 2f)
    val outer = min(width, height) / 2f
    val ring = outer * 0.18f
    val inner = outer - ring

    /** Half side of the square inscribed in the inner circle, with a small gap to the ring. */
    val half = inner * 0.66f
}

// ---------------------------------------------------------------------------------------- sliders

@Composable
private fun SlidersTab(color: HexColor, hsb: Hsb, setHsb: (Hsb, Boolean) -> Unit, vm: VeskraViewModel) {
    var model by rememberSaveable { mutableStateOf(SliderModel.HSB) }
    val apply: (HexColor, Boolean) -> Unit = { c, done -> if (done) vm.setColor(c) else vm.previewColor(c) }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
        SliderModel.entries.forEach { m -> ModelChip(m.name, selected = m == model) { model = m } }
    }
    when (model) {
        SliderModel.HSB -> {
            Channel("H", hsb.h.roundToInt().toString(), hsb.h / 360f, HueColors) { f, d -> setHsb(hsb.copy(h = (f * 360f).coerceAtMost(359.99f)), d) }
            Channel("S", pct(hsb.s), hsb.s, listOf(ColorMath.fromHsb(hsb.h, 0f, hsb.b), ColorMath.fromHsb(hsb.h, 1f, hsb.b)).map { it.compose() }) { f, d ->
                setHsb(hsb.copy(s = f), d)
            }
            Channel("B", pct(hsb.b), hsb.b, listOf(Color.Black, ColorMath.fromHsb(hsb.h, hsb.s, 1f).compose())) { f, d -> setHsb(hsb.copy(b = f), d) }
        }
        SliderModel.RGB -> {
            fun rgb(r: Int = color.red, g: Int = color.green, b: Int = color.blue) = HexColor.of(r, g, b)
            Channel("R", color.red.toString(), color.red / 255f, listOf(rgb(r = 0), rgb(r = 255)).map { it.compose() }) { f, d -> apply(rgb(r = to8(f)), d) }
            Channel("G", color.green.toString(), color.green / 255f, listOf(rgb(g = 0), rgb(g = 255)).map { it.compose() }) { f, d -> apply(rgb(g = to8(f)), d) }
            Channel("B", color.blue.toString(), color.blue / 255f, listOf(rgb(b = 0), rgb(b = 255)).map { it.compose() }) { f, d -> apply(rgb(b = to8(f)), d) }
        }
        SliderModel.LAB -> {
            // Local copy: Lab values outside the sRGB gamut would otherwise snap back while dragging.
            var lab by remember { mutableStateOf(ColorMath.toLab(color)) }
            LaunchedEffect(color) { if (ColorMath.fromLab(lab) != color) lab = ColorMath.toLab(color) }
            fun ramp(f: (Float) -> Lab) = List(7) { i -> ColorMath.fromLab(f(i / 6f)).compose() }
            val set: (Lab, Boolean) -> Unit = { v, d -> lab = v; apply(ColorMath.fromLab(v), d) }
            Channel("L", lab.l.roundToInt().toString(), lab.l / 100f, ramp { lab.copy(l = it * 100f) }) { f, d -> set(lab.copy(l = f * 100f), d) }
            Channel("a", lab.a.roundToInt().toString(), (lab.a + 128f) / 255f, ramp { lab.copy(a = it * 255f - 128f) }) { f, d -> set(lab.copy(a = f * 255f - 128f), d) }
            Channel("b", lab.b.roundToInt().toString(), (lab.b + 128f) / 255f, ramp { lab.copy(b = it * 255f - 128f) }) { f, d -> set(lab.copy(b = f * 255f - 128f), d) }
        }
        SliderModel.CMYK -> {
            var cmyk by remember { mutableStateOf(ColorMath.toCmyk(color)) }
            LaunchedEffect(color) { if (ColorMath.fromCmyk(cmyk) != color) cmyk = ColorMath.toCmyk(color) }
            val set: (Cmyk, Boolean) -> Unit = { v, d -> cmyk = v; apply(ColorMath.fromCmyk(v), d) }
            fun ends(f: (Float) -> Cmyk) = listOf(ColorMath.fromCmyk(f(0f)), ColorMath.fromCmyk(f(1f))).map { it.compose() }
            Channel("C", pct(cmyk.c), cmyk.c, ends { cmyk.copy(c = it) }) { f, d -> set(cmyk.copy(c = f), d) }
            Channel("M", pct(cmyk.m), cmyk.m, ends { cmyk.copy(m = it) }) { f, d -> set(cmyk.copy(m = f), d) }
            Channel("Y", pct(cmyk.y), cmyk.y, ends { cmyk.copy(y = it) }) { f, d -> set(cmyk.copy(y = f), d) }
            Channel("K", pct(cmyk.k), cmyk.k, ends { cmyk.copy(k = it) }) { f, d -> set(cmyk.copy(k = f), d) }
            Text(stringResource(R.string.color_cmyk_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    HexField(color, onApply = vm::setColor)
}

private fun pct(v: Float): String = (v * 100f).roundToInt().toString()

private fun to8(f: Float): Int = (f * 255f).roundToInt().coerceIn(0, 255)

@Composable
private fun ModelChip(text: String, selected: Boolean, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(12.dp))
            .clickable(role = Role.Tab, onClick = onClick)
            .semantics { this.selected = selected }
            .padding(horizontal = 14.dp),
    ) {
        Text(text, style = MaterialTheme.typography.labelLarge, color = if (selected) colors.onSurface else colors.onSurfaceVariant.copy(alpha = 0.6f))
    }
}

@Composable
private fun Channel(label: String, valueText: String, fraction: Float, colors: List<Color>, onChange: (Float, Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, style = MaterialTheme.typography.labelLarge, modifier = Modifier.width(24.dp))
        GradientSlider(fraction, colors, label, Modifier.weight(1f), onChange)
        Text(valueText, style = MaterialTheme.typography.labelLarge, modifier = Modifier.width(40.dp).padding(start = 8.dp))
    }
}

@Composable
private fun HexField(color: HexColor, onApply: (HexColor) -> Unit) {
    var text by remember(color) { mutableStateOf(color.toHex()) }
    val parsed = HexColor.parse(text)
    OutlinedTextField(
        value = text,
        onValueChange = { text = it.take(7) },
        label = { Text(stringResource(R.string.color_hex_label)) },
        singleLine = true,
        isError = parsed == null,
        supportingText = { if (parsed == null) Text(stringResource(R.string.color_hex_invalid)) },
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { parsed?.let(onApply) }),
        modifier = Modifier.fillMaxWidth(),
    )
}

// --------------------------------------------------------------------------------------- swatches

private fun palettes(state: UiState): List<Palette> = Palette.BUILT_IN + Palette.user(state.userPalette)

@StringRes
private fun paletteName(p: Palette): Int = when (p.id) {
    Palette.BASICS.id -> R.string.palette_basics
    Palette.SKIN_TONES.id -> R.string.palette_skin_tones
    else -> R.string.palette_user
}

/** Compact one-palette strip under the wheel and the sliders, with previous/next palette buttons. */
@Composable
private fun PaletteStrip(state: UiState, vm: VeskraViewModel) {
    val all = palettes(state).filter { it.colors.isNotEmpty() }
    var index by rememberSaveable { mutableIntStateOf(0) }
    val palette = all[index.coerceIn(0, all.lastIndex)]
    Row(verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = { index = (index - 1 + all.size) % all.size }) {
            Icon(VeskraIcons.Back, contentDescription = stringResource(R.string.palette_previous))
        }
        Text(stringResource(paletteName(palette)), style = MaterialTheme.typography.labelLarge, modifier = Modifier.weight(1f), textAlign = TextAlign.Center)
        IconButton(onClick = { index = (index + 1) % all.size }) {
            Icon(VeskraIcons.Back, contentDescription = stringResource(R.string.palette_next), modifier = Modifier.rotate(180f))
        }
    }
    SwatchGrid(palette.colors, state.color, onPick = vm::setColor)
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SwatchGrid(colors: List<HexColor>, current: HexColor, onPick: (HexColor) -> Unit, onLongPress: ((HexColor) -> Unit)? = null) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        colors.forEach { c ->
            if (onLongPress == null) {
                Swatch(c, selected = c == current, onClick = { onPick(c) })
            } else {
                RemovableSwatch(c, selected = c == current, onClick = { onPick(c) }, onLongPress = { onLongPress(c) })
            }
        }
    }
}

/** User-palette chip: tap picks, long-press removes (also offered as an accessibility action). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun RemovableSwatch(color: HexColor, selected: Boolean, onClick: () -> Unit, onLongPress: () -> Unit) {
    val cd = stringResource(R.string.color_swatch_cd, color.toHex())
    val removeLabel = stringResource(R.string.palette_remove_color)
    Box(
        Modifier
            .size(44.dp)
            .clip(SwatchShape)
            .background(color.compose())
            .border(
                if (selected) 2.dp else 1.dp,
                if (selected) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.outline.copy(alpha = 0.5f),
                SwatchShape,
            )
            .combinedClickable(role = Role.Button, onLongClickLabel = removeLabel, onLongClick = onLongPress, onClick = onClick)
            .semantics { contentDescription = cd; this.selected = selected },
    )
}

@Composable
private fun ColumnScope.SwatchesTab(state: UiState, vm: VeskraViewModel) {
    if (state.recentColors.isNotEmpty()) {
        PanelLabel(stringResource(R.string.color_recent))
        SwatchGrid(state.recentColors, state.color, onPick = vm::setColor)
    }
    Palette.BUILT_IN.forEach { p ->
        PanelLabel(stringResource(paletteName(p)), Modifier.padding(top = 8.dp))
        SwatchGrid(p.colors, state.color, onPick = vm::setColor)
    }
    PanelLabel(stringResource(R.string.palette_user), Modifier.padding(top = 8.dp))
    if (state.userPalette.isEmpty()) {
        Text(stringResource(R.string.palette_user_empty), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    } else {
        SwatchGrid(state.userPalette, state.color, onPick = vm::setColor, onLongPress = vm::removeFromUserPalette)
    }
    val addLabel = stringResource(R.string.color_add_to_palette)
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier
            .padding(top = 4.dp)
            .fillMaxWidth(0.5f)
            .height(48.dp)
            .align(Alignment.CenterHorizontally)
            .clip(RoundedCornerShape(24.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(role = Role.Button) { vm.addToUserPalette(state.color) }
            .semantics { contentDescription = addLabel },
    ) {
        Icon(VeskraIcons.Add, contentDescription = null)
    }
}

// ------------------------------------------------------------------------------------------- tone

/**
 * Tone sphere: a lit ball of the base color. Touching it picks that shading of the color — towards
 * black in the shadow, towards white in the highlight — which is a quick way to stay in one color
 * family while painting light and shade. The sliders below set the base color.
 */
@Composable
private fun ColumnScope.ToneTab(hsb: Hsb, setHsb: (Hsb, Boolean) -> Unit, vm: VeskraViewModel) {
    var mode by rememberSaveable { mutableStateOf(ToneMode.TONE) }
    // The base stays put while the sphere is being sampled; the sliders move it.
    var base by remember { mutableStateOf(hsb) }
    val baseColor = ColorMath.fromHsb(base)
    val bitmap = remember(baseColor, mode) { toneSphere(baseColor, mode, SPHERE_PX).asImageBitmap() }
    val cd = stringResource(R.string.color_tone_sphere_cd)
    Box(
        Modifier.fillMaxWidth(0.72f).aspectRatio(1f).align(Alignment.CenterHorizontally)
            .semantics { contentDescription = cd }
            .pointerInput(baseColor, mode) {
                awaitEachGesture {
                    fun pick(p: Offset, done: Boolean) {
                        val x = (p.x / size.width) * 2f - 1f
                        val y = (p.y / size.height) * 2f - 1f
                        val c = toneAt(baseColor, mode, x, y) ?: return
                        if (done) vm.setColor(c) else vm.previewColor(c)
                    }
                    val down = awaitFirstDown()
                    var last = down.position
                    pick(last, false)
                    down.consume()
                    drag(down.id) { change ->
                        last = change.position
                        pick(last, false)
                        change.consume()
                    }
                    pick(last, true)
                }
            },
    ) {
        Image(bitmap, contentDescription = null, modifier = Modifier.fillMaxSize())
    }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
        ModelChip(stringResource(R.string.color_tone_shade), mode == ToneMode.SHADE) { mode = ToneMode.SHADE }
        ModelChip(stringResource(R.string.color_tone_tone), mode == ToneMode.TONE) { mode = ToneMode.TONE }
        ModelChip(stringResource(R.string.color_tone_tint), mode == ToneMode.TINT) { mode = ToneMode.TINT }
    }
    val moveBase: (Hsb, Boolean) -> Unit = { v, d -> base = v; setHsb(v, d) }
    GradientSlider(base.h / 360f, HueColors, stringResource(R.string.color_hue)) { f, d -> moveBase(base.copy(h = (f * 360f).coerceAtMost(359.99f)), d) }
    GradientSlider(
        base.s,
        listOf(ColorMath.fromHsb(base.h, 0f, base.b), ColorMath.fromHsb(base.h, 1f, base.b)).map { it.compose() },
        stringResource(R.string.color_saturation),
    ) { f, d -> moveBase(base.copy(s = f), d) }
    GradientSlider(base.b, listOf(Color.Black, ColorMath.fromHsb(base.h, base.s, 1f).compose()), stringResource(R.string.color_brightness)) { f, d ->
        moveBase(base.copy(b = f), d)
    }
}

private const val SPHERE_PX = 192

/** Light from the upper right, towards the viewer. */
private val LIGHT = floatArrayOf(0.45f, -0.55f, 0.70f)

/** Color at sphere point ([x], [y]) in [-1, 1]²; null outside the disc. */
private fun toneAt(base: HexColor, mode: ToneMode, x: Float, y: Float): HexColor? {
    val r2 = x * x + y * y
    if (r2 > 1f) return null
    val z = sqrt(1f - r2)
    val lit = ((x * LIGHT[0] + y * LIGHT[1] + z * LIGHT[2]) / LIGHT_LEN + 1f) / 2f // 0 = shadow, 1 = highlight
    return when (mode) {
        ToneMode.SHADE -> ColorMath.mix(base, HexColor.BLACK, (1f - lit) * 0.96f)
        ToneMode.TINT -> ColorMath.mix(base, HexColor.WHITE, lit * 0.96f)
        ToneMode.TONE -> if (lit >= 0.5f) {
            ColorMath.mix(base, HexColor.WHITE, (lit - 0.5f) * 2f * 0.9f)
        } else {
            ColorMath.mix(base, HexColor.BLACK, (0.5f - lit) * 2f * 0.96f)
        }
    }
}

private val LIGHT_LEN = sqrt(LIGHT[0] * LIGHT[0] + LIGHT[1] * LIGHT[1] + LIGHT[2] * LIGHT[2])

private fun toneSphere(base: HexColor, mode: ToneMode, px: Int): Bitmap {
    val pixels = IntArray(px * px)
    for (j in 0 until px) {
        val y = (j + 0.5f) / px * 2f - 1f
        for (i in 0 until px) {
            val x = (i + 0.5f) / px * 2f - 1f
            // One-pixel feather so the disc edge is not jagged.
            val edge = ((1f - sqrt(x * x + y * y)) * px / 2f).coerceIn(0f, 1f)
            val c = toneAt(base, mode, x * 0.999f, y * 0.999f)
            pixels[j * px + i] = if (c == null || edge <= 0f) 0 else ((edge * 255f).roundToInt() shl 24) or c.rgb
        }
    }
    // setPixels takes straight-alpha colors and premultiplies them for the (premultiplied) bitmap.
    return Bitmap.createBitmap(px, px, Bitmap.Config.ARGB_8888).apply { setPixels(pixels, 0, px, 0, 0, px, px) }
}

/** Filled dot of the dock: the current color (large) or the secondary color (small). */
@Composable
fun ColorDot(color: HexColor, label: String, large: Boolean, onClick: () -> Unit) {
    Box(contentAlignment = Alignment.Center, modifier = Modifier.size(48.dp).clip(CircleShape).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = label }) {
        Box(
            Modifier
                .size(if (large) 34.dp else 24.dp)
                .background(color.compose(), CircleShape)
                .border(2.dp, MaterialTheme.colorScheme.onSurface.copy(alpha = 0.8f), CircleShape),
        )
    }
}
