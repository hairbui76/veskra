package org.veskra.kit

import kotlin.math.abs
import kotlin.math.cbrt
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt

/** Hue in degrees [0, 360), saturation and brightness in [0, 1]. */
data class Hsb(val h: Float, val s: Float, val b: Float)

/** CIE L*a*b* relative to D50 (the Photoshop convention): L in [0, 100], a/b roughly [-128, 127]. */
data class Lab(val l: Float, val a: Float, val b: Float)

/** Device CMYK in [0, 1]. See [ColorMath.toCmyk] for what these numbers do and do not mean. */
data class Cmyk(val c: Float, val m: Float, val y: Float, val k: Float)

/**
 * Color-model conversions for the color picker. Everything is relative to sRGB, the only working
 * space in v1 (MASTER_PLAN §1.1). Pure Kotlin so the math is unit-tested on the host JVM.
 *
 * These conversions drive UI sliders only; the engine's color pipeline (veskra-color) stays the
 * authority for compositing and export.
 */
object ColorMath {
    fun toHsb(c: HexColor): Hsb {
        val r = c.red / 255f
        val g = c.green / 255f
        val b = c.blue / 255f
        val mx = max(r, max(g, b))
        val mn = min(r, min(g, b))
        val d = mx - mn
        val h = when {
            d == 0f -> 0f
            mx == r -> 60f * (((g - b) / d) % 6f)
            mx == g -> 60f * ((b - r) / d + 2f)
            else -> 60f * ((r - g) / d + 4f)
        }
        return Hsb(if (h < 0f) h + 360f else h, if (mx == 0f) 0f else d / mx, mx)
    }

    fun fromHsb(hsb: Hsb): HexColor = fromHsb(hsb.h, hsb.s, hsb.b)

    fun fromHsb(h: Float, s: Float, b: Float): HexColor {
        val hh = ((h % 360f) + 360f) % 360f / 60f
        val ss = s.coerceIn(0f, 1f)
        val bb = b.coerceIn(0f, 1f)
        val i = floor(hh).toInt()
        val f = hh - i
        val p = bb * (1f - ss)
        val q = bb * (1f - ss * f)
        val t = bb * (1f - ss * (1f - f))
        val (r, g, bl) = when (i % 6) {
            0 -> Triple(bb, t, p)
            1 -> Triple(q, bb, p)
            2 -> Triple(p, bb, t)
            3 -> Triple(p, q, bb)
            4 -> Triple(t, p, bb)
            else -> Triple(bb, p, q)
        }
        return HexColor.of(to8(r), to8(g), to8(bl))
    }

    /** sRGB → XYZ (D65) → Bradford → XYZ (D50) → L*a*b*. */
    fun toLab(c: HexColor): Lab {
        val r = srgbToLinear(c.red / 255.0)
        val g = srgbToLinear(c.green / 255.0)
        val b = srgbToLinear(c.blue / 255.0)
        // Linear sRGB → XYZ adapted to D50 (Bradford), the ICC PCS matrix for sRGB.
        val x = 0.4360747 * r + 0.3850649 * g + 0.1430804 * b
        val y = 0.2225045 * r + 0.7168786 * g + 0.0606169 * b
        val z = 0.0139322 * r + 0.0971045 * g + 0.7141733 * b
        val fx = labF(x / D50_X)
        val fy = labF(y / D50_Y)
        val fz = labF(z / D50_Z)
        return Lab((116.0 * fy - 16.0).toFloat(), (500.0 * (fx - fy)).toFloat(), (200.0 * (fy - fz)).toFloat())
    }

    /** Inverse of [toLab]; colors outside the sRGB gamut are clipped per channel. */
    fun fromLab(lab: Lab): HexColor {
        val fy = (lab.l + 16.0) / 116.0
        val fx = fy + lab.a / 500.0
        val fz = fy - lab.b / 200.0
        val x = D50_X * labFInv(fx)
        val y = D50_Y * labFInv(fy)
        val z = D50_Z * labFInv(fz)
        val r = 3.1338561 * x - 1.6168667 * y - 0.4906146 * z
        val g = -0.9787684 * x + 1.9161415 * y + 0.0334540 * z
        val b = 0.0719453 * x - 0.2289914 * y + 1.4052427 * z
        return HexColor.of(to8(linearToSrgb(r).toFloat()), to8(linearToSrgb(g).toFloat()), to8(linearToSrgb(b).toFloat()))
    }

    /**
     * Naive device CMYK (K = 1 − max(R, G, B)). There is no print profile behind these numbers:
     * they are a convenient way to dial a color, not a prediction of ink on paper, and the UI labels
     * them as approximate.
     */
    fun toCmyk(c: HexColor): Cmyk {
        val r = c.red / 255f
        val g = c.green / 255f
        val b = c.blue / 255f
        val k = 1f - max(r, max(g, b))
        if (k >= 1f) return Cmyk(0f, 0f, 0f, 1f)
        return Cmyk((1f - r - k) / (1f - k), (1f - g - k) / (1f - k), (1f - b - k) / (1f - k), k)
    }

    fun fromCmyk(v: Cmyk): HexColor {
        val k = v.k.coerceIn(0f, 1f)
        fun ch(x: Float) = to8((1f - x.coerceIn(0f, 1f)) * (1f - k))
        return HexColor.of(ch(v.c), ch(v.m), ch(v.y))
    }

    /** Linear blend in sRGB-encoded space, [t] = 0 → [a], 1 → [b]. Used for swatch ramps in the UI. */
    fun mix(a: HexColor, b: HexColor, t: Float): HexColor {
        val u = t.coerceIn(0f, 1f)
        fun ch(x: Int, y: Int) = (x + (y - x) * u).roundToInt().coerceIn(0, 255)
        return HexColor.of(ch(a.red, b.red), ch(a.green, b.green), ch(a.blue, b.blue))
    }

    /** True when [a] and [b] differ by at most [tolerance] per 8-bit channel. */
    fun near(a: HexColor, b: HexColor, tolerance: Int = 1): Boolean =
        abs(a.red - b.red) <= tolerance && abs(a.green - b.green) <= tolerance && abs(a.blue - b.blue) <= tolerance

    private fun to8(v: Float): Int = if (v.isNaN()) 0 else (v * 255f).roundToInt().coerceIn(0, 255)

    private fun srgbToLinear(v: Double): Double = if (v <= 0.04045) v / 12.92 else ((v + 0.055) / 1.055).pow(2.4)

    private fun linearToSrgb(v: Double): Double {
        val c = v.coerceIn(0.0, 1.0)
        return if (c <= 0.0031308) c * 12.92 else 1.055 * c.pow(1.0 / 2.4) - 0.055
    }

    private const val D50_X = 0.9642
    private const val D50_Y = 1.0
    private const val D50_Z = 0.8249
    private const val LAB_EPS = 216.0 / 24389.0
    private const val LAB_KAPPA = 24389.0 / 27.0

    private fun labF(t: Double): Double = if (t > LAB_EPS) cbrt(t) else (LAB_KAPPA * t + 16.0) / 116.0

    private fun labFInv(f: Double): Double {
        val f3 = f * f * f
        return if (f3 > LAB_EPS) f3 else (116.0 * f - 16.0) / LAB_KAPPA
    }
}

/** A named swatch set shown on the palettes tab. [builtIn] palettes cannot be edited. */
data class Palette(val id: String, val name: String, val colors: List<HexColor>, val builtIn: Boolean) {
    companion object {
        const val USER_ID: String = "user"
        const val MAX_USER_COLORS: Int = 60

        private fun hexes(vararg v: Int): List<HexColor> = v.map(::HexColor)

        /** Original general-purpose set: primaries, secondaries, earths and a neutral ramp. */
        val BASICS: Palette = Palette(
            "basics", "Basics",
            hexes(
                0xC8140A, 0xEB4650, 0xF07882, 0x82508C, 0x503764,
                0x1E50B4, 0x3CA0D2, 0x6EC3A0, 0x002D37, 0x1E6E23,
                0xA0BE28, 0xFFD228, 0xFF6E2D, 0x9B5019, 0x553228,
                0x371914, 0xB4785A, 0xFABE8C, 0xF5E1B9, 0xEBF0FA,
                0xD2D2E1, 0x646464, 0x413C3C, 0x232323,
            ),
            builtIn = true,
        )

        /** Light-to-deep skin ramp with warm and cool undertones. */
        val SKIN_TONES: Palette = Palette(
            "skin", "Skin Tones",
            hexes(
                0xFFF5F0, 0xEBDCD2, 0xFFD7DC, 0xFAE6D2, 0xE1D7D7,
                0xF5E1B4, 0xF0CDAF, 0xBEA5A5, 0xDC9664, 0xC88755,
                0xC38273, 0xD77D5A, 0xB97850, 0xAF643C, 0x9B644B,
                0x964B28, 0x874B32, 0x5F4141, 0x552D28, 0x4B2D19,
                0x3C2828, 0x412319, 0x281919, 0x321919,
            ),
            builtIn = true,
        )

        val BUILT_IN: List<Palette> = listOf(BASICS, SKIN_TONES)

        fun user(colors: List<HexColor>): Palette = Palette(USER_ID, "My colors", colors.take(MAX_USER_COLORS), builtIn = false)
    }
}
