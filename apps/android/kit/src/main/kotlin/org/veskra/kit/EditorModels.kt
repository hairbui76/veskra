package org.veskra.kit

import java.util.Locale

/** sRGB hex color (straight alpha not supported in hex entry; brush opacity is separate). */
@JvmInline
value class HexColor(val rgb: Int) {
    init {
        require(rgb in 0..0xFFFFFF) { "rgb out of range" }
    }

    val red: Int get() = rgb shr 16 and 0xFF
    val green: Int get() = rgb shr 8 and 0xFF
    val blue: Int get() = rgb and 0xFF

    /** Canonical `#RRGGBB`, uppercase. */
    fun toHex(): String = "#" + String.format(Locale.ROOT, "%06X", rgb)

    /** Opaque ARGB int for UI swatches. */
    fun toArgb(): Int = (0xFF shl 24) or rgb

    companion object {
        val BLACK: HexColor = HexColor(0x000000)
        val WHITE: HexColor = HexColor(0xFFFFFF)

        /** Accepts `#RGB`, `#RRGGBB`, with or without `#`, surrounding whitespace ignored. */
        fun parse(text: String): HexColor? {
            var s = text.trim()
            if (s.startsWith("#")) s = s.substring(1)
            if (s.isEmpty() || s.any { Character.digit(it, 16) < 0 }) return null
            return when (s.length) {
                3 -> HexColor(s.map { "$it$it" }.joinToString("").toInt(16))
                6 -> HexColor(s.toInt(16))
                else -> null
            }
        }

        fun of(r: Int, g: Int, b: Int): HexColor {
            require(r in 0..255 && g in 0..255 && b in 0..255)
            return HexColor((r shl 16) or (g shl 8) or b)
        }
    }
}

/** Most-recently-used colors, bounded, no duplicates (DSN-03). */
class RecentColors(private val capacity: Int = 12, initial: List<HexColor> = emptyList()) {
    private val items = ArrayDeque<HexColor>()

    init {
        require(capacity > 0)
        initial.asReversed().forEach(::push)
    }

    val colors: List<HexColor> get() = items.toList()

    fun push(c: HexColor) {
        items.remove(c)
        items.addFirst(c)
        while (items.size > capacity) items.removeLast()
    }
}

/** Brush presets the UI offers; ids are the `brush.select` wire values (BRH owns their behavior). */
enum class BrushPreset(val id: String, val isEraser: Boolean = false) {
    PENCIL_ROUGH("pencil.rough"),
    PENCIL_SMOOTH("pencil.smooth"),
    PEN_TECHNICAL("pen.technical"),
    PEN_BRUSH("pen.brush"),
    MARKER("marker"),
    ROUND_HARD("round.hard"),
    ROUND_SOFT("round.soft"),
    CHARCOAL("charcoal"),
    ERASER_HARD("eraser.hard", isEraser = true),
    ERASER_SOFT("eraser.soft", isEraser = true),
    ;

    companion object {
        val DEFAULT: BrushPreset = PENCIL_ROUGH

        fun fromId(id: String): BrushPreset? = entries.firstOrNull { it.id == id }
    }
}

/**
 * New Canvas parameters + validation mirroring `veskra_core::limits` (MAX_DOCUMENT_DIM 16384,
 * MAX_DOCUMENT_PIXELS 8192²) so the dialog rejects sizes the engine would refuse.
 */
data class CanvasSpec(val width: Int, val height: Int, val dpi: Int) {
    enum class Problem { NON_POSITIVE, EDGE_TOO_LARGE, TOO_MANY_PIXELS, DPI_OUT_OF_RANGE }

    fun validate(): Problem? = when {
        width <= 0 || height <= 0 -> Problem.NON_POSITIVE
        width > MAX_DIM || height > MAX_DIM -> Problem.EDGE_TOO_LARGE
        width.toLong() * height.toLong() > MAX_PIXELS -> Problem.TOO_MANY_PIXELS
        dpi !in MIN_DPI..MAX_DPI -> Problem.DPI_OUT_OF_RANGE
        else -> null
    }

    /**
     * Rough memory estimate shown in the dialog: authoritative RGBA16 premultiplied layer pixels
     * (8 B/px, ADR-005) for [layers] fully painted layers + one RGBA8 display copy (4 B/px).
     * Sparse tiles make real usage lower for partially painted layers; this is an upper bound.
     */
    fun estimatedBytes(layers: Int = 4): Long {
        require(layers >= 1)
        val px = width.toLong() * height.toLong()
        return px * BYTES_PER_LAYER_PX * layers + px * DISPLAY_BYTES_PER_PX
    }

    companion object {
        const val MAX_DIM: Int = 16_384
        const val MAX_PIXELS: Long = 8192L * 8192L
        const val MIN_DPI: Int = 30
        const val MAX_DPI: Int = 2400
        const val BYTES_PER_LAYER_PX: Long = 8
        const val DISPLAY_BYTES_PER_PX: Long = 4

        val PRESET_SQUARE: CanvasSpec = CanvasSpec(2048, 2048, 300)
        val PRESET_LANDSCAPE: CanvasSpec = CanvasSpec(4096, 3072, 300)
        val PRESET_PORTRAIT: CanvasSpec = CanvasSpec(3072, 4096, 300)

        /** The only working space offered in v1 (MASTER_PLAN §1.1 Color); shown as a badge. */
        const val COLOR_SPACE_BADGE: String = "sRGB"
    }
}

/** Human-readable byte size for the memory estimate (base 1024, e.g. "512 MB", "1.3 GB"); locale-independent digits. */
fun formatBytes(bytes: Long): String {
    require(bytes >= 0)
    val kb = 1024.0
    return when {
        bytes < kb -> "$bytes B"
        bytes < kb * kb -> String.format(Locale.ROOT, "%.0f KB", bytes / kb)
        bytes < kb * kb * kb -> String.format(Locale.ROOT, "%.0f MB", bytes / (kb * kb))
        else -> String.format(Locale.ROOT, "%.1f GB", bytes / (kb * kb * kb))
    }
}
