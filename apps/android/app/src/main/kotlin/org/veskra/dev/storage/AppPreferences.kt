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

    private companion object {
        const val KEY_RAIL_LEFT = "rail_left"
        const val KEY_EXPORT = "export_prefs_v1"
        const val KEY_RECENT_COLORS = "recent_colors"
    }
}
