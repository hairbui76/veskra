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
    val Recover: ImageVector by lazy {
        icon("recover") {
            moveTo(20f, 12f); curveTo(20f, 16.4f, 16.4f, 20f, 12f, 20f); curveTo(7.6f, 20f, 4f, 16.4f, 4f, 12f)
            curveTo(4f, 7.6f, 7.6f, 4f, 12f, 4f); curveTo(14.8f, 4f, 17.2f, 5.4f, 18.6f, 7.6f)
            moveTo(19f, 3f); lineTo(19f, 8f); lineTo(14f, 8f)
        }
    }
}
