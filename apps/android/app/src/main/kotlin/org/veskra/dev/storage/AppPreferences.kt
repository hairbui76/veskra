package org.veskra.dev.storage

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import org.veskra.kit.ExportPreferences
import org.veskra.kit.HexColor

/**
 * Small typed preferences (UI conveniences + remembered export settings). Excluded from backup by
 * data_extraction_rules.xml. Reads are cheap; writes use apply() (async disk).
 */
class AppPreferences(context: Context) {
    private val prefs: SharedPreferences = context.getSharedPreferences("veskra_prefs", Context.MODE_PRIVATE)

    var railOnLeft: Boolean
        get() = prefs.getBoolean(KEY_RAIL_LEFT, true)
        set(v) = prefs.edit { putBoolean(KEY_RAIL_LEFT, v) }

    var exportPreferences: ExportPreferences
        get() = ExportPreferences.fromJsonText(prefs.getString(KEY_EXPORT, null))
        set(v) = prefs.edit { putString(KEY_EXPORT, v.toJson().encode()) }

    var recentColors: List<HexColor>
        get() = prefs.getString(KEY_RECENT_COLORS, null).orEmpty()
            .split(',')
            .mapNotNull { HexColor.parse(it) }
        set(v) = prefs.edit { putString(KEY_RECENT_COLORS, v.joinToString(",") { it.toHex() }) }

    /** The foreground color, restored when the app starts again. */
    var primaryColor: HexColor
        get() = HexColor.parse(prefs.getString(KEY_PRIMARY_COLOR, null).orEmpty()) ?: HexColor.BLACK
        set(v) = prefs.edit { putString(KEY_PRIMARY_COLOR, v.toHex()) }

    /** Imported CMYK output profile (app-private path); null = the bundled Coated FOGRA39. */
    var printProfilePath: String?
        get() = prefs.getString(KEY_PRINT_PROFILE, null)?.takeIf { java.io.File(it).isFile }
        set(v) = prefs.edit { if (v == null) remove(KEY_PRINT_PROFILE) else putString(KEY_PRINT_PROFILE, v) }

    /** The second (background) color slot of the dock. */
    var secondaryColor: HexColor
        get() = HexColor.parse(prefs.getString(KEY_SECONDARY_COLOR, null).orEmpty()) ?: HexColor.WHITE
        set(v) = prefs.edit { putString(KEY_SECONDARY_COLOR, v.toHex()) }

    /** The user's own swatches ("My colors" palette), in insertion order. */
    var userPalette: List<HexColor>
        get() = prefs.getString(KEY_USER_PALETTE, null).orEmpty().split(',').mapNotNull { HexColor.parse(it) }
        set(v) = prefs.edit { putString(KEY_USER_PALETTE, v.joinToString(",") { it.toHex() }) }

    private companion object {
        const val KEY_PRIMARY_COLOR = "primary_color"
        const val KEY_PRINT_PROFILE = "print_profile"
        const val KEY_SECONDARY_COLOR = "secondary_color"
        const val KEY_USER_PALETTE = "user_palette"

        const val KEY_RAIL_LEFT = "rail_left"
        const val KEY_EXPORT = "export_prefs_v1"
        const val KEY_RECENT_COLORS = "recent_colors"
    }
}
