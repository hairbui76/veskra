package org.veskra.dev.ui.icons

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathBuilder
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.unit.dp

/**
 * Minimal outline icon set drawn from simple original geometry (24×24 grid, 2 px stroke), so the app
 * needs no icon-pack dependency or third-party artwork. Tinted by `Icon` like any ImageVector.
 */
object VeskraIcons {
    private fun icon(name: String, block: PathBuilder.() -> Unit): ImageVector =
        ImageVector.Builder(name = name, defaultWidth = 24.dp, defaultHeight = 24.dp, viewportWidth = 24f, viewportHeight = 24f)
            .path(
                fill = null,
                stroke = SolidColor(Color.Black),
                strokeLineWidth = 2f,
                strokeLineCap = StrokeCap.Round,
                strokeLineJoin = StrokeJoin.Round,
                pathBuilder = block,
            )
            .build()

    private fun PathBuilder.circle(cx: Float, cy: Float, r: Float) {
        moveTo(cx - r, cy)
        arcTo(r, r, 0f, false, true, cx + r, cy)
        arcTo(r, r, 0f, false, true, cx - r, cy)
        close()
    }

    val Brush: ImageVector by lazy {
        icon("brush") {
            moveTo(4f, 20f); lineTo(5f, 15f); lineTo(16f, 4f); lineTo(20f, 8f); lineTo(9f, 19f); close()
            moveTo(14f, 6f); lineTo(18f, 10f)
        }
    }
    val Eraser: ImageVector by lazy {
        icon("eraser") {
            moveTo(8f, 20f); lineTo(3f, 15f); lineTo(13f, 5f); lineTo(20f, 12f); lineTo(12f, 20f); close()
            moveTo(8f, 10f); lineTo(15f, 17f)
            moveTo(12f, 20f); lineTo(21f, 20f)
        }
    }
    val Palette: ImageVector by lazy {
        icon("palette") {
            circle(12f, 12f, 9f)
            circle(12f, 12f, 3f)
        }
    }
    val Layers: ImageVector by lazy {
        icon("layers") {
            moveTo(12f, 3f); lineTo(21f, 8f); lineTo(12f, 13f); lineTo(3f, 8f); close()
            moveTo(3f, 12f); lineTo(12f, 17f); lineTo(21f, 12f)
            moveTo(3f, 16f); lineTo(12f, 21f); lineTo(21f, 16f)
        }
    }
    val Undo: ImageVector by lazy {
        icon("undo") {
            moveTo(9f, 7f); lineTo(4f, 12f); lineTo(9f, 17f)
            moveTo(4f, 12f); lineTo(14f, 12f); curveTo(17.5f, 12f, 20f, 14.5f, 20f, 18f)
        }
    }
    val Redo: ImageVector by lazy {
        icon("redo") {
            moveTo(15f, 7f); lineTo(20f, 12f); lineTo(15f, 17f)
            moveTo(20f, 12f); lineTo(10f, 12f); curveTo(6.5f, 12f, 4f, 14.5f, 4f, 18f)
        }
    }
    val Export: ImageVector by lazy {
        icon("export") {
            moveTo(12f, 15f); lineTo(12f, 3f)
            moveTo(7f, 8f); lineTo(12f, 3f); lineTo(17f, 8f)
            moveTo(5f, 13f); lineTo(5f, 20f); lineTo(19f, 20f); lineTo(19f, 13f)
        }
    }
    val Save: ImageVector by lazy {
        icon("save") {
            moveTo(4f, 4f); lineTo(16f, 4f); lineTo(20f, 8f); lineTo(20f, 20f); lineTo(4f, 20f); close()
            moveTo(8f, 4f); lineTo(8f, 9f); lineTo(15f, 9f); lineTo(15f, 4f)
            moveTo(8f, 20f); lineTo(8f, 14f); lineTo(16f, 14f); lineTo(16f, 20f)
        }
    }
    val Back: ImageVector by lazy { icon("back") { moveTo(15f, 5f); lineTo(8f, 12f); lineTo(15f, 19f) } }
    val Fit: ImageVector by lazy {
        icon("fit") {
            moveTo(4f, 9f); lineTo(4f, 4f); lineTo(9f, 4f)
            moveTo(15f, 4f); lineTo(20f, 4f); lineTo(20f, 9f)
            moveTo(20f, 15f); lineTo(20f, 20f); lineTo(15f, 20f)
            moveTo(9f, 20f); lineTo(4f, 20f); lineTo(4f, 15f)
        }
    }
    val Mirror: ImageVector by lazy {
        icon("mirror") {
            moveTo(12f, 3f); lineTo(12f, 21f)
            moveTo(9f, 7f); lineTo(4f, 17f); lineTo(9f, 17f); close()
            moveTo(15f, 7f); lineTo(20f, 17f); lineTo(15f, 17f); close()
        }
    }
    val SwapSide: ImageVector by lazy {
        icon("swap_side") {
            moveTo(4f, 8f); lineTo(20f, 8f); moveTo(16f, 4f); lineTo(20f, 8f); lineTo(16f, 12f)
            moveTo(20f, 16f); lineTo(4f, 16f); moveTo(8f, 12f); lineTo(4f, 16f); lineTo(8f, 20f)
        }
    }
    val Add: ImageVector by lazy { icon("add") { moveTo(12f, 5f); lineTo(12f, 19f); moveTo(5f, 12f); lineTo(19f, 12f) } }
    val Delete: ImageVector by lazy {
        icon("delete") {
            moveTo(5f, 7f); lineTo(19f, 7f)
            moveTo(10f, 7f); lineTo(10f, 4f); lineTo(14f, 4f); lineTo(14f, 7f)
            moveTo(7f, 7f); lineTo(8f, 20f); lineTo(16f, 20f); lineTo(17f, 7f)
        }
    }
    val Up: ImageVector by lazy { icon("up") { moveTo(6f, 15f); lineTo(12f, 9f); lineTo(18f, 15f) } }
    val Down: ImageVector by lazy { icon("down") { moveTo(6f, 9f); lineTo(12f, 15f); lineTo(18f, 9f) } }
    val Visible: ImageVector by lazy {
        icon("visible") {
            moveTo(2f, 12f); curveTo(5f, 6f, 19f, 6f, 22f, 12f); curveTo(19f, 18f, 5f, 18f, 2f, 12f); close()
            circle(12f, 12f, 3f)
        }
    }
    val Hidden: ImageVector by lazy {
        icon("hidden") {
            moveTo(2f, 12f); curveTo(5f, 6f, 19f, 6f, 22f, 12f); curveTo(19f, 18f, 5f, 18f, 2f, 12f); close()
            moveTo(4f, 4f); lineTo(20f, 20f)
        }
    }
    val Duplicate: ImageVector by lazy {
        icon("duplicate") {
            moveTo(8f, 8f); lineTo(20f, 8f); lineTo(20f, 20f); lineTo(8f, 20f); close()
            moveTo(4f, 16f); lineTo(4f, 4f); lineTo(16f, 4f)
        }
    }
    val Rename: ImageVector by lazy {
        icon("rename") {
            moveTo(4f, 20f); lineTo(20f, 20f)
            moveTo(6f, 16f); lineTo(6f, 13f); lineTo(15f, 4f); lineTo(18f, 7f); lineTo(9f, 16f); close()
        }
    }
    val Close: ImageVector by lazy { icon("close") { moveTo(6f, 6f); lineTo(18f, 18f); moveTo(18f, 6f); lineTo(6f, 18f) } }
    val Check: ImageVector by lazy { icon("check") { moveTo(5f, 12f); lineTo(10f, 17f); lineTo(19f, 7f) } }
    val Open: ImageVector by lazy {
        icon("open") { moveTo(3f, 7f); lineTo(3f, 19f); lineTo(21f, 19f); lineTo(21f, 9f); lineTo(11f, 9f); lineTo(9f, 7f); close() }
    }
    val Home: ImageVector by lazy {
        icon("home") { moveTo(3f, 11f); lineTo(12f, 3f); lineTo(21f, 11f); moveTo(6f, 9f); lineTo(6f, 20f); lineTo(18f, 20f); lineTo(18f, 9f) }
    }
    val Tools: ImageVector by lazy {
        // Drawing compass.
        icon("tools") { circle(12f, 5f, 2f); moveTo(11f, 7f); lineTo(6f, 20f); moveTo(13f, 7f); lineTo(18f, 20f); moveTo(8f, 15f); lineTo(16f, 15f) }
    }
    val More: ImageVector by lazy { icon("more") { circle(5f, 12f, 1f); circle(12f, 12f, 1f); circle(19f, 12f, 1f) } }
    val Smudge: ImageVector by lazy {
        // Fingertip dragging paint.
        icon("smudge") {
            moveTo(9f, 21f); lineTo(9f, 9f); curveTo(9f, 6f, 14f, 6f, 14f, 9f); lineTo(14f, 21f)
            moveTo(9f, 13f); lineTo(14f, 13f)
            moveTo(4f, 5f); curveTo(8f, 2f, 16f, 2f, 20f, 5f)
        }
    }
    val Fill: ImageVector by lazy {
        icon("fill") {
            moveTo(11f, 3f); lineTo(19f, 11f); lineTo(11f, 19f); lineTo(4f, 12f); close()
            moveTo(4f, 12f); lineTo(18f, 12f)
            moveTo(21f, 16f); curveTo(19f, 19f, 23f, 19f, 21f, 16f)
        }
    }
    val Eyedropper: ImageVector by lazy {
        icon("eyedropper") {
            moveTo(3f, 21f); lineTo(4f, 17f); lineTo(12f, 9f); lineTo(15f, 12f); lineTo(7f, 20f); close()
            moveTo(10f, 7f); lineTo(17f, 14f)
            circle(17.5f, 6.5f, 3.2f)
        }
    }
    val Gradient: ImageVector by lazy {
        icon("gradient") {
            moveTo(4f, 4f); lineTo(20f, 4f); lineTo(20f, 20f); lineTo(4f, 20f); close()
            moveTo(8f, 4f); lineTo(8f, 20f); moveTo(11f, 4f); lineTo(11f, 20f); moveTo(15f, 4f); lineTo(15f, 20f)
        }
    }
    val Text: ImageVector by lazy { icon("text") { moveTo(5f, 7f); lineTo(5f, 5f); lineTo(19f, 5f); lineTo(19f, 7f); moveTo(12f, 5f); lineTo(12f, 19f); moveTo(9f, 19f); lineTo(15f, 19f) } }
    val Image: ImageVector by lazy {
        icon("image") {
            moveTo(4f, 5f); lineTo(20f, 5f); lineTo(20f, 19f); lineTo(4f, 19f); close()
            moveTo(4f, 16f); lineTo(9f, 11f); lineTo(13f, 15f); lineTo(16f, 12f); lineTo(20f, 16f)
            circle(15f, 9f, 1f)
        }
    }
    val GuideLine: ImageVector by lazy { icon("guide_line") { moveTo(4f, 18f); lineTo(20f, 6f); circle(4f, 18f, 1.5f); circle(20f, 6f, 1.5f) } }
    val GuideEllipse: ImageVector by lazy {
        icon("guide_ellipse") { moveTo(4f, 12f); curveTo(4f, 5f, 20f, 5f, 20f, 12f); curveTo(20f, 19f, 4f, 19f, 4f, 12f); close(); circle(20f, 12f, 1.5f) }
    }
    val Lazy: ImageVector by lazy {
        // A pen tip trailing on a string.
        icon("lazy") { circle(17f, 7f, 3f); moveTo(15f, 9f); curveTo(11f, 13f, 12f, 17f, 6f, 19f); circle(5f, 19f, 1f) }
    }
    val ShapeLine: ImageVector by lazy { icon("shape_line") { moveTo(5f, 19f); lineTo(19f, 5f) } }
    val ShapeRect: ImageVector by lazy { icon("shape_rect") { moveTo(5f, 6f); lineTo(19f, 6f); lineTo(19f, 18f); lineTo(5f, 18f); close() } }
    val ShapeCircle: ImageVector by lazy { icon("shape_circle") { circle(12f, 12f, 8f) } }
    val ShapePath: ImageVector by lazy {
        icon("shape_path") { moveTo(4f, 18f); curveTo(8f, 4f, 12f, 20f, 20f, 6f); circle(4f, 18f, 1.5f); circle(20f, 6f, 1.5f) }
    }
    val SymVertical: ImageVector by lazy {
        icon("sym_vertical") {
            moveTo(12f, 3f); lineTo(12f, 21f)
            moveTo(9f, 8f); curveTo(4f, 8f, 4f, 16f, 9f, 16f)
            moveTo(15f, 8f); curveTo(20f, 8f, 20f, 16f, 15f, 16f)
        }
    }
    val SymHorizontal: ImageVector by lazy {
        icon("sym_horizontal") {
            moveTo(3f, 12f); lineTo(21f, 12f)
            moveTo(8f, 9f); curveTo(8f, 4f, 16f, 4f, 16f, 9f)
            moveTo(8f, 15f); curveTo(8f, 20f, 16f, 20f, 16f, 15f)
        }
    }
    val SymRadial: ImageVector by lazy {
        icon("sym_radial") {
            moveTo(12f, 3f); lineTo(12f, 21f); moveTo(3f, 12f); lineTo(21f, 12f)
            moveTo(5.6f, 5.6f); lineTo(18.4f, 18.4f); moveTo(18.4f, 5.6f); lineTo(5.6f, 18.4f)
        }
    }
    val Lock: ImageVector by lazy {
        icon("lock") {
            moveTo(6f, 11f); lineTo(18f, 11f); lineTo(18f, 20f); lineTo(6f, 20f); close()
            moveTo(8f, 11f); lineTo(8f, 8f); curveTo(8f, 3f, 16f, 3f, 16f, 8f); lineTo(16f, 11f)
        }
    }
    val SelectLasso: ImageVector by lazy {
        icon("select_lasso") {
            moveTo(4f, 10f); curveTo(4f, 3f, 20f, 3f, 20f, 10f); curveTo(20f, 16f, 9f, 17f, 6f, 13f)
            moveTo(6f, 13f); curveTo(5f, 17f, 9f, 17f, 7f, 21f)
        }
    }
    val SelectRect: ImageVector by lazy {
        icon("select_rect") {
            moveTo(4f, 5f); lineTo(8f, 5f); moveTo(11f, 5f); lineTo(13f, 5f); moveTo(16f, 5f); lineTo(20f, 5f); lineTo(20f, 8f)
            moveTo(20f, 11f); lineTo(20f, 13f); moveTo(20f, 16f); lineTo(20f, 19f); lineTo(16f, 19f)
            moveTo(13f, 19f); lineTo(11f, 19f); moveTo(8f, 19f); lineTo(4f, 19f); lineTo(4f, 16f)
            moveTo(4f, 13f); lineTo(4f, 11f); moveTo(4f, 8f); lineTo(4f, 5f)
        }
    }
    val SelectEllipse: ImageVector by lazy {
        icon("select_ellipse") {
            moveTo(4f, 12f); curveTo(4f, 9f, 5.5f, 6.5f, 8f, 5.2f)
            moveTo(11f, 4.2f); curveTo(14f, 3.8f, 17f, 5f, 18.8f, 7.5f)
            moveTo(19.8f, 10.5f); curveTo(20.3f, 13.5f, 19f, 16.5f, 16.5f, 18.5f)
            moveTo(13.5f, 19.8f); curveTo(10.5f, 20.3f, 7.5f, 19.2f, 5.6f, 16.8f)
        }
    }
    val Wand: ImageVector by lazy {
        icon("wand") {
            moveTo(4f, 20f); lineTo(14f, 10f)
            moveTo(17f, 3f); lineTo(17f, 6f); moveTo(17f, 9f); lineTo(17f, 11f); moveTo(13f, 7f); lineTo(15f, 7f); moveTo(19f, 7f); lineTo(21f, 7f)
        }
    }
    val Transform: ImageVector by lazy {
        icon("transform") {
            moveTo(12f, 3f); lineTo(12f, 21f); moveTo(3f, 12f); lineTo(21f, 12f)
            moveTo(9f, 6f); lineTo(12f, 3f); lineTo(15f, 6f); moveTo(9f, 18f); lineTo(12f, 21f); lineTo(15f, 18f)
            moveTo(6f, 9f); lineTo(3f, 12f); lineTo(6f, 15f); moveTo(18f, 9f); lineTo(21f, 12f); lineTo(18f, 15f)
        }
    }
    val Distort: ImageVector by lazy { icon("distort") { moveTo(4f, 19f); lineTo(6f, 9f); lineTo(20f, 4f); lineTo(20f, 19f); close() } }
    val Adjust: ImageVector by lazy {
        // Two drops: hue / saturation.
        icon("adjust") {
            moveTo(9f, 4f); curveTo(3f, 11f, 4f, 18f, 9f, 18f); curveTo(14f, 18f, 15f, 11f, 9f, 4f); close()
            moveTo(15f, 8f); curveTo(20f, 14f, 19f, 20f, 15f, 20f)
        }
    }
    val Curve: ImageVector by lazy {
        icon("curve") { moveTo(4f, 4f); lineTo(20f, 4f); lineTo(20f, 20f); lineTo(4f, 20f); close(); moveTo(4f, 20f); curveTo(10f, 19f, 13f, 6f, 20f, 4f) }
    }
    val Crop: ImageVector by lazy {
        icon("crop") { moveTo(7f, 3f); lineTo(7f, 17f); lineTo(21f, 17f); moveTo(3f, 7f); lineTo(17f, 7f); lineTo(17f, 21f) }
    }
    val Resize: ImageVector by lazy {
        icon("resize") {
            moveTo(4f, 4f); lineTo(20f, 4f); lineTo(20f, 20f); lineTo(4f, 20f); close()
            moveTo(9f, 15f); lineTo(15f, 9f); moveTo(11f, 9f); lineTo(15f, 9f); lineTo(15f, 13f)
        }
    }
    val Mask: ImageVector by lazy {
        icon("mask") { moveTo(4f, 5f); lineTo(20f, 5f); lineTo(20f, 19f); lineTo(4f, 19f); close(); circle(12f, 12f, 4f) }
    }
    val Invert: ImageVector by lazy { icon("invert") { circle(12f, 12f, 8f); moveTo(12f, 4f); lineTo(12f, 20f); moveTo(12f, 8f); lineTo(18.9f, 8f); moveTo(12f, 12f); lineTo(20f, 12f); moveTo(12f, 16f); lineTo(18.9f, 16f) } }
    val Settings: ImageVector by lazy {
        icon("settings") {
            circle(12f, 12f, 3f)
            moveTo(12f, 3f); lineTo(12f, 6f); moveTo(12f, 18f); lineTo(12f, 21f); moveTo(3f, 12f); lineTo(6f, 12f); moveTo(18f, 12f); lineTo(21f, 12f)
            moveTo(5.6f, 5.6f); lineTo(7.8f, 7.8f); moveTo(16.2f, 16.2f); lineTo(18.4f, 18.4f)
            moveTo(18.4f, 5.6f); lineTo(16.2f, 7.8f); moveTo(7.8f, 16.2f); lineTo(5.6f, 18.4f)
        }
    }
    val TabWheel: ImageVector by lazy { icon("tab_wheel") { circle(12f, 12f, 8f); moveTo(9f, 9f); lineTo(15f, 9f); lineTo(15f, 15f); lineTo(9f, 15f); close() } }
    val TabSliders: ImageVector by lazy {
        icon("tab_sliders") { moveTo(4f, 8f); lineTo(20f, 8f); moveTo(4f, 16f); lineTo(20f, 16f); circle(9f, 8f, 2f); circle(15f, 16f, 2f) }
    }
    val TabSwatches: ImageVector by lazy {
        icon("tab_swatches") {
            for (y in listOf(7f, 12f, 17f)) for (x in listOf(7f, 12f, 17f)) { moveTo(x - 1f, y - 1f); lineTo(x + 1f, y - 1f); lineTo(x + 1f, y + 1f); lineTo(x - 1f, y + 1f); close() }
        }
    }
    val TabTone: ImageVector by lazy { icon("tab_tone") { circle(12f, 12f, 8f); moveTo(8f, 13f); curveTo(9f, 17f, 15f, 17f, 16f, 13f) } }
    val Swap: ImageVector by lazy {
        icon("swap") { moveTo(7f, 4f); lineTo(4f, 7f); lineTo(7f, 10f); moveTo(4f, 7f); lineTo(16f, 7f); moveTo(17f, 14f); lineTo(20f, 17f); lineTo(17f, 20f); moveTo(20f, 17f); lineTo(8f, 17f) }
    }
    val Letter: ImageVector by lazy {
        // Envelope with a small heart seal.
        icon("letter") {
            moveTo(3f, 6f); lineTo(21f, 6f); lineTo(21f, 18f); lineTo(3f, 18f); close()
            moveTo(3f, 6f); lineTo(12f, 13f); lineTo(21f, 6f)
            moveTo(12f, 16.5f); curveTo(9.5f, 14.8f, 10.2f, 12.6f, 12f, 13.8f); curveTo(13.8f, 12.6f, 14.5f, 14.8f, 12f, 16.5f); close()
        }
    }
    val Update: ImageVector by lazy {
        // A circle with a download arrow: "get the new version".
        icon("update") {
            circle(12f, 12f, 9f)
            moveTo(12f, 7f); lineTo(12f, 16f)
            moveTo(8f, 12.5f); lineTo(12f, 16.5f); lineTo(16f, 12.5f)
        }
    }
    val Recover: ImageVector by lazy {
        icon("recover") {
            moveTo(20f, 12f); curveTo(20f, 16.4f, 16.4f, 20f, 12f, 20f); curveTo(7.6f, 20f, 4f, 16.4f, 4f, 12f)
            curveTo(4f, 7.6f, 7.6f, 4f, 12f, 4f); curveTo(14.8f, 4f, 17.2f, 5.4f, 18.6f, 7.6f)
            moveTo(19f, 3f); lineTo(19f, 8f); lineTo(14f, 8f)
        }
    }
}
