package org.veskra.kit

/** `profile.inspect` result. */
data class ProfileInfo(
    val description: String,
    val copyright: String?,
    val colorSpace: String,
    val profileClass: String,
    val bytes: Long,
    val bundled: Boolean,
    val usableForCmykExport: Boolean,
    val reason: String?,
) {
    companion object {
        fun fromJson(o: JsonObject): ProfileInfo? = ProfileInfo(
            description = o.string("description") ?: return null,
            copyright = o.string("copyright"),
            colorSpace = o.string("colorSpace") ?: "",
            profileClass = o.string("class") ?: "",
            bytes = o.long("bytes") ?: 0L,
            bundled = o.bool("bundled") ?: false,
            usableForCmykExport = o.bool("usableForCmykExport") ?: false,
            reason = o.string("reason"),
        )
    }
}

/** `color.convert` to CMYK: ink amounts in [0, 1] and whether the color can be printed as it is. */
data class PrintColor(val cmyk: Cmyk, val inGamut: Boolean, val profile: String) {
    /** Total ink in percent (C+M+Y+K), what printers limit (e.g. 300 % for coated offset). */
    val totalInkPercent: Float get() = (cmyk.c + cmyk.m + cmyk.y + cmyk.k) * 100f

    companion object {
        fun fromJson(o: JsonObject): PrintColor? {
            val v = o.array("cmyk")?.items?.map { (it as? JsonNumber)?.asDouble?.toFloat() ?: return null } ?: return null
            if (v.size != 4) return null
            return PrintColor(Cmyk(v[0], v[1], v[2], v[3]), o.bool("inGamut") ?: true, o.string("profile") ?: "")
        }
    }
}
