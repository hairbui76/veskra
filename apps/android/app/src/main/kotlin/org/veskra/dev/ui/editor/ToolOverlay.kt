package org.veskra.dev.ui.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateRotation
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.isSpecified
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.Guide
import org.veskra.dev.ui.RectF
import org.veskra.dev.ui.SymmetryMode
import org.veskra.dev.ui.Tool
import org.veskra.dev.ui.TransformSession
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.kit.Affine
import org.veskra.kit.PointF
import org.veskra.kit.ViewState
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.sin

private fun Offset.toPoint(): PointF = PointF(x, y)

private fun PointF.toOffset(): Offset = Offset(x, y)

/** Two-tone outline that stays visible on any artwork: solid dark line with light dashes on top. */
private fun DrawScope.drawGuide(path: Path) {
    drawPath(path, Color.Black.copy(alpha = 0.75f), style = Stroke(1.5.dp.toPx()))
    val dash = 6.dp.toPx()
    drawPath(path, Color.White, style = Stroke(1.5.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(dash, dash))))
}

/** Thin tinted line for rulers and mirror axes, visibly different from selection outlines. */
private fun DrawScope.drawAxis(path: Path) {
    drawPath(path, Color.White.copy(alpha = 0.7f), style = Stroke(2.5.dp.toPx()))
    drawPath(path, AxisColor, style = Stroke(1.25.dp.toPx()))
}

private val AxisColor = Color(0xFF2F80C0)

private fun DrawScope.drawHandle(at: Offset) {
    drawCircle(Color.White, radius = 8.dp.toPx(), center = at)
    drawCircle(Color.Black.copy(alpha = 0.75f), radius = 8.dp.toPx(), center = at, style = Stroke(1.5.dp.toPx()))
}

private fun polygon(points: List<Offset>, close: Boolean = true): Path = Path().apply {
    points.forEachIndexed { i, p -> if (i == 0) moveTo(p.x, p.y) else lineTo(p.x, p.y) }
    if (close && points.size > 2) close()
}

private fun ViewState.screen(points: List<PointF>): List<Offset> = points.map { docToView(it).toOffset() }

private fun ellipsePoints(r: RectF, segments: Int = 72): List<PointF> {
    val c = r.center
    return List(segments) { i ->
        val t = 2.0 * Math.PI * i / segments
        PointF(c.x + r.width / 2f * cos(t).toFloat(), c.y + r.height / 2f * sin(t).toFloat())
    }
}

/**
 * Always-on canvas annotations: the selection boundary ("marching ants"), the mirror axes of the
 * active symmetry and the active ruler. Drawn in every tool, including while painting.
 */
@Composable
fun CanvasAnnotations(state: UiState, view: ViewState, modifier: Modifier = Modifier) {
    val doc = state.doc
    if (state.selectionOutline.isEmpty() && state.symmetry == SymmetryMode.OFF && state.guide == null) return
    Canvas(modifier.fillMaxSize()) {
        if (state.selectionOutline.isNotEmpty()) {
            val path = Path()
            for (contour in state.selectionOutline) path.addPath(polygon(view.screen(contour)))
            drawGuide(path)
        }
        if (doc != null && state.symmetry != SymmetryMode.OFF) {
            val w = doc.width.toFloat()
            val h = doc.height.toFloat()
            val c = PointF(w / 2f, h / 2f)
            val axes = when (state.symmetry) {
                SymmetryMode.VERTICAL -> listOf(PointF(c.x, 0f) to PointF(c.x, h))
                SymmetryMode.HORIZONTAL -> listOf(PointF(0f, c.y) to PointF(w, c.y))
                SymmetryMode.RADIAL -> {
                    val reach = maxOf(w, h)
                    List(state.symmetryCount) { i ->
                        val t = 2.0 * Math.PI * i / state.symmetryCount - Math.PI / 2
                        c to PointF(c.x + reach * cos(t).toFloat(), c.y + reach * sin(t).toFloat())
                    }
                }
                SymmetryMode.OFF -> emptyList()
            }
            for ((a, b) in axes) drawAxis(polygon(view.screen(listOf(a, b)), close = false))
        }
        when (val g = state.guide) {
            is Guide.Line -> {
                // Extend the ruler well past both points: strokes snap to the infinite line.
                val d = g.b - g.a
                val len = g.a.distanceTo(g.b).coerceAtLeast(1e-3f)
                val reach = (doc?.let { maxOf(it.width, it.height) } ?: 4096) * 2f / len
                drawAxis(polygon(view.screen(listOf(g.a - d * reach, g.b + d * reach)), close = false))
            }
            is Guide.Ellipse -> drawAxis(polygon(view.screen(ellipsePoints(g.bounds))))
            null -> Unit
        }
    }
}

private enum class Grab { MOVE, SCALE, ROTATE }

/** View-space geometry of the transform box for hit testing and drawing. */
private class TransformBox(session: TransformSession, view: ViewState, handleGap: Float) {
    val docCorners: List<PointF> = session.bounds.corners.map(session.matrix::apply)
    val corners: List<Offset> = view.screen(docCorners)
    val center: Offset = Offset(corners.sumOf { it.x.toDouble() }.toFloat() / 4f, corners.sumOf { it.y.toDouble() }.toFloat() / 4f)
    val rotateHandle: Offset = run {
        val topMid = (corners[0] + corners[1]) / 2f
        val out = topMid - center
        val len = out.getDistance().coerceAtLeast(1f)
        topMid + out / len * handleGap
    }
}

/** How a one-finger drag between two points is previewed. */
private enum class DragShape { LINE, RECT, ELLIPSE }

private fun dragShape(tool: Tool): DragShape? = when (tool) {
    Tool.SELECT_RECT, Tool.SHAPE_RECT -> DragShape.RECT
    Tool.SELECT_ELLIPSE, Tool.SHAPE_ELLIPSE, Tool.GUIDE_ELLIPSE -> DragShape.ELLIPSE
    Tool.SHAPE_LINE, Tool.GRADIENT, Tool.GUIDE_LINE -> DragShape.LINE
    else -> null
}

/** True when the four corners, in order, turn the same way with a clear margin (no fold, no flat corner). */
private fun isConvex(quad: List<PointF>): Boolean {
    var sign = 0f
    for (i in quad.indices) {
        val a = quad[i]
        val b = quad[(i + 1) % quad.size]
        val c = quad[(i + 2) % quad.size]
        val cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x)
        if (kotlin.math.abs(cross) < 1f) return false
        if (sign == 0f) sign = cross else if (sign * cross < 0f) return false
    }
    return true
}

/** Index of the corner of [corners] (view px) within [radius] of [p], or -1. */
private fun cornerAt(corners: List<Offset>, p: Offset, radius: Float): Int {
    val i = corners.indices.minByOrNull { (p - corners[it]).getDistance() } ?: return -1
    return if ((p - corners[i]).getDistance() <= radius) i else -1
}

/**
 * Owns the canvas while a non-paint tool is active: turns a one-finger / stylus gesture into the
 * tool's engine command and draws its preview. Two fingers still pan, zoom and rotate the view
 * (through `view.set`, since the engine's own gesture arbiter is bypassed here).
 */
@Composable
fun ToolOverlay(state: UiState, view: ViewState, vm: VeskraViewModel, modifier: Modifier = Modifier) {
    val tool = state.tool
    var start by remember(tool) { mutableStateOf<Offset?>(null) }
    var current by remember(tool) { mutableStateOf<Offset?>(null) }
    val lasso = remember(tool) { mutableStateListOf<Offset>() }
    val latestView by rememberUpdatedState(view)
    val latestState by rememberUpdatedState(state)
    val cd = stringResource(R.string.tool_overlay_cd)

    Canvas(
        modifier
            .fillMaxSize()
            .semantics { contentDescription = cd }
            .pointerInput(tool) {
                val handleRadius = 28.dp.toPx()
                val handleGap = 40.dp.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown()
                    down.consume()
                    fun doc(p: Offset): PointF? = latestView.viewToDoc(p.toPoint())
                    val downDoc = doc(down.position)

                    // Transform: decide what the finger grabbed, relative to the box as it is now.
                    val session = latestState.transform
                    val box = session?.let { TransformBox(it, latestView, handleGap) }
                    val grab = when {
                        box == null -> null
                        (down.position - box.rotateHandle).getDistance() <= handleRadius -> Grab.ROTATE
                        cornerAt(box.corners, down.position, handleRadius) >= 0 -> Grab.SCALE
                        else -> Grab.MOVE
                    }
                    val grabbedCorner = box?.let { cornerAt(it.corners, down.position, handleRadius) } ?: -1

                    fun transformFor(p: Offset): Affine? {
                        val s = session ?: return null
                        val b = box ?: return null
                        val from = downDoc ?: return null
                        val to = doc(p) ?: return null
                        return when (grab) {
                            Grab.MOVE -> s.matrix.then(Affine.translate((to.x - from.x).toDouble(), (to.y - from.y).toDouble()))
                            Grab.SCALE -> {
                                val anchor = b.docCorners[(grabbedCorner + 2) % 4]
                                val before = from.distanceTo(anchor)
                                if (before < 1e-3f) return null
                                val k = (to.distanceTo(anchor) / before).coerceIn(0.01f, 100f).toDouble()
                                s.matrix.then(Affine.scaleAbout(k, k, anchor.x.toDouble(), anchor.y.toDouble()))
                            }
                            Grab.ROTATE -> {
                                val c = s.matrix.apply(s.bounds.center)
                                val a0 = atan2((from.y - c.y).toDouble(), (from.x - c.x).toDouble())
                                val a1 = atan2((to.y - c.y).toDouble(), (to.x - c.x).toDouble())
                                s.matrix.then(Affine.rotateAbout(a1 - a0, c.x.toDouble(), c.y.toDouble()))
                            }
                            null -> null
                        }
                    }

                    // Distort: a corner handle moves that corner; anywhere else moves the whole quad.
                    val quad0 = latestState.distort?.quad
                    val quadCorner = quad0?.let { cornerAt(latestView.screen(it), down.position, handleRadius) } ?: -1
                    fun quadFor(p: Offset): List<PointF>? {
                        val q = quad0 ?: return null
                        val from = downDoc ?: return null
                        val to = doc(p) ?: return null
                        val d = to - from
                        // The engine only maps onto convex quads: a corner stops at the edge of validity.
                        return q.mapIndexed { i, c -> if (quadCorner < 0 || i == quadCorner) c + d else c }.takeIf(::isConvex)
                    }

                    // Crop: a corner handle resizes the frame; anywhere else moves it. Kept on the canvas.
                    val crop0 = latestState.crop
                    val cropCorner = crop0?.let { cornerAt(latestView.screen(it.corners), down.position, handleRadius) } ?: -1
                    fun cropFor(p: Offset): RectF? {
                        val r = crop0 ?: return null
                        val from = downDoc ?: return null
                        val to = doc(p) ?: return null
                        val info = latestState.doc ?: return null
                        val w = info.width.toFloat()
                        val h = info.height.toFloat()
                        if (cropCorner < 0) {
                            val dx = (to.x - from.x).coerceIn(-r.left, w - r.right)
                            val dy = (to.y - from.y).coerceIn(-r.top, h - r.bottom)
                            return RectF(r.left + dx, r.top + dy, r.right + dx, r.bottom + dy)
                        }
                        val fixed = r.corners[(cropCorner + 2) % 4]
                        val moved = PointF(to.x.coerceIn(0f, w), to.y.coerceIn(0f, h))
                        return RectF.of(fixed, moved).takeIf { it.width >= 1f && it.height >= 1f }
                    }

                    var last = down.position
                    var navigating = false
                    start = last
                    current = last
                    lasso.clear()
                    lasso.add(last)
                    if (tool == Tool.EYEDROPPER) downDoc?.let { vm.pickColor(it, commit = false) }

                    while (true) {
                        val event = awaitPointerEvent()
                        val pressed = event.changes.filter { it.pressed }
                        if (pressed.size >= 2) {
                            if (!navigating) {
                                // A second finger turns the gesture into navigation: drop the tool preview.
                                navigating = true
                                start = null
                                current = null
                                lasso.clear()
                                session?.let { vm.previewTransform(it.matrix) }
                                quad0?.let(vm::previewDistort)
                                crop0?.let(vm::setCrop)
                            }
                            val centroid = event.calculateCentroid(useCurrent = true)
                            if (centroid.isSpecified) {
                                vm.navigate(
                                    centroid.toPoint(),
                                    event.calculatePan().toPoint(),
                                    event.calculateZoom(),
                                    Math.toRadians(event.calculateRotation().toDouble()).toFloat(),
                                )
                            }
                        } else if (pressed.size == 1 && !navigating) {
                            last = pressed[0].position
                            current = last
                            when (tool) {
                                Tool.SELECT_LASSO -> if ((last - lasso.last()).getDistance() >= LASSO_STEP_PX) lasso.add(last)
                                Tool.EYEDROPPER -> doc(last)?.let { vm.pickColor(it, commit = false) }
                                Tool.TRANSFORM -> transformFor(last)?.let(vm::previewTransform)
                                Tool.DISTORT -> quadFor(last)?.let(vm::previewDistort)
                                Tool.CROP -> cropFor(last)?.let(vm::setCrop)
                                else -> Unit
                            }
                        }
                        event.changes.forEach { it.consume() }
                        if (pressed.isEmpty()) break
                    }

                    if (navigating) {
                        vm.endNavigation()
                    } else {
                        val b = doc(last)
                        when (tool) {
                            Tool.FILL -> b?.let(vm::fillAt)
                            Tool.SELECT_WAND -> b?.let(vm::wandAt)
                            Tool.SHAPE_PATH -> b?.let(vm::addPathPoint)
                            Tool.EYEDROPPER -> b?.let { vm.pickColor(it, commit = true) }
                            Tool.SELECT_LASSO -> vm.selectLasso(decimate(lasso.mapNotNull(::doc), MAX_LASSO_POINTS))
                            Tool.TRANSFORM -> transformFor(last)?.let(vm::applyTransform)
                            Tool.DISTORT -> quadFor(last)?.let(vm::applyDistort)
                            Tool.CROP -> cropFor(last)?.let(vm::setCrop)
                            else -> if (dragShape(tool) != null && downDoc != null && b != null) vm.dragTool(downDoc, b)
                        }
                    }
                    start = null
                    current = null
                    lasso.clear()
                }
            },
    ) {
        val a = start
        val b = current
        val shape = dragShape(tool)
        when {
            shape != null -> if (a != null && b != null) {
                val da = view.viewToDoc(a.toPoint())
                val db = view.viewToDoc(b.toPoint())
                if (da != null && db != null) {
                    val r = RectF.of(da, db)
                    when (shape) {
                        DragShape.LINE -> drawGuide(polygon(listOf(a, b), close = false))
                        DragShape.RECT -> drawGuide(polygon(view.screen(r.corners)))
                        DragShape.ELLIPSE -> drawGuide(polygon(view.screen(ellipsePoints(r))))
                    }
                }
            }
            tool == Tool.SELECT_LASSO -> if (lasso.size >= 2) drawGuide(polygon(lasso.toList(), close = false))
            tool == Tool.SHAPE_PATH -> {
                val pts = view.screen(state.pathPoints)
                if (pts.size >= 2) drawGuide(polygon(pts, close = state.shape.closed))
                pts.forEach { drawHandle(it) }
            }
            tool == Tool.EYEDROPPER -> if (b != null) {
                val ring = 30.dp.toPx()
                val at = b - Offset(0f, ring * 2f)
                drawCircle(Color(state.color.toArgb()), radius = ring, center = at)
                drawCircle(Color.White, radius = ring, center = at, style = Stroke(3.dp.toPx()))
                drawCircle(Color.Black.copy(alpha = 0.5f), radius = ring + 2.dp.toPx(), center = at, style = Stroke(1.dp.toPx()))
                drawCircle(Color.White, radius = 4.dp.toPx(), center = b, style = Stroke(1.5.dp.toPx()))
            }
            tool == Tool.TRANSFORM -> state.transform?.let { session ->
                val box = TransformBox(session, view, 40.dp.toPx())
                drawGuide(polygon(box.corners))
                drawGuide(polygon(listOf((box.corners[0] + box.corners[1]) / 2f, box.rotateHandle), close = false))
                (box.corners + box.rotateHandle).forEach { drawHandle(it) }
            }
            tool == Tool.DISTORT -> state.distort?.let { d ->
                val corners = view.screen(d.quad)
                drawGuide(polygon(corners))
                corners.forEach { drawHandle(it) }
            }
            tool == Tool.CROP -> state.crop?.let { r ->
                val corners = view.screen(r.corners)
                val frame = polygon(corners)
                // Dim what will be cut away: the whole view minus the crop frame.
                val outside = Path().apply {
                    addRect(Rect(0f, 0f, size.width, size.height))
                    addPath(frame)
                    fillType = PathFillType.EvenOdd
                }
                drawPath(outside, Color.Black.copy(alpha = 0.45f))
                drawGuide(frame)
                corners.forEach { drawHandle(it) }
            }
        }
    }
}

private const val LASSO_STEP_PX = 3f
private const val MAX_LASSO_POINTS = 1500

/** Evenly thins [points] to at most [max] (the engine does not need sub-pixel lasso detail). */
private fun decimate(points: List<PointF>, max: Int): List<PointF> {
    if (points.size <= max) return points
    val step = points.size.toDouble() / max
    return List(max) { i -> points[(i * step).toInt()] }
}
