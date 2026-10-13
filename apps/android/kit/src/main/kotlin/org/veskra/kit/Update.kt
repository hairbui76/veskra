package org.veskra.kit

import kotlin.math.sign

/**
 * In-app update check (ADR-020): the website publishes `download/latest.json` describing the
 * newest release and serves its APKs next to it. This file only parses that description, compares
 * versions and picks the APK for the device; downloading and installing live in the app.
 */
object AppVersion {
    private val PATTERN = Regex("""^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$""")

    /** `v0.1.0-alpha.4` → `0.1.0-alpha.4`; null when it is not a semantic version. */
    fun normalize(tag: String): String? = PATTERN.matchEntire(tag.trim())?.value?.removePrefix("v")?.substringBefore('+')

    /**
     * Semantic Versioning 2.0 precedence: `0.1.0-alpha.4 < 0.1.0-alpha.10 < 0.1.0-beta < 0.1.0`.
     * Build metadata is ignored. Returns null when either side is not a version.
     */
    fun compare(a: String, b: String): Int? {
        val ma = PATTERN.matchEntire(a.trim()) ?: return null
        val mb = PATTERN.matchEntire(b.trim()) ?: return null
        for (i in 1..3) {
            val c = ma.groupValues[i].toBigInteger().compareTo(mb.groupValues[i].toBigInteger())
            if (c != 0) return c
        }
        val pa = ma.groupValues[4]
        val pb = mb.groupValues[4]
        if (pa.isEmpty() || pb.isEmpty()) return pb.length.coerceAtMost(1) - pa.length.coerceAtMost(1)
        val xa = pa.split('.')
        val xb = pb.split('.')
        for (i in 0 until minOf(xa.size, xb.size)) {
            val c = comparePart(xa[i], xb[i])
            if (c != 0) return c
        }
        return xa.size.compareTo(xb.size)
    }

    private fun comparePart(a: String, b: String): Int {
        val na = a.takeIf { it.all(Char::isDigit) }?.toBigInteger()
        val nb = b.takeIf { it.all(Char::isDigit) }?.toBigInteger()
        return when {
            na != null && nb != null -> na.compareTo(nb)
            na != null -> -1 // numeric identifiers sort before alphanumeric ones
            nb != null -> 1
            else -> a.compareTo(b).sign
        }
    }
}

/** One APK of a release as listed in `latest.json`. [url] is absolute. */
data class UpdateApk(val abi: String, val url: String, val name: String, val size: Long, val sha256: String)

/** `latest.json`: the newest published release (pre-releases included). */
data class ReleaseInfo(
    val tag: String,
    /** [tag] without the leading `v`. */
    val version: String,
    val pageUrl: String?,
    val published: String?,
    val prerelease: Boolean,
    /** Release notes as written on GitHub (Markdown), or null. */
    val notes: String?,
    val apks: List<UpdateApk>,
) {
    fun isNewerThan(installedVersion: String): Boolean = (AppVersion.compare(version, installedVersion) ?: 0) > 0

    /**
     * The APK for a device whose ABIs are [supportedAbis] (most preferred first, like
     * `Build.SUPPORTED_ABIS`): the first ABI with its own APK, else the universal one.
     */
    fun apkFor(supportedAbis: List<String>): UpdateApk? =
        supportedAbis.firstNotNullOfOrNull { abi -> apks.firstOrNull { it.abi == abi } }
            ?: apks.firstOrNull { it.abi == UNIVERSAL }

    /** The notes as plain lines for a dialog: Markdown markers and tables removed, blank runs collapsed. */
    fun plainNotes(maxLines: Int = 14): List<String> {
        val lines = notes.orEmpty().replace("\r", "").lines().filterNot { it.trimStart().startsWith("|") }.map { line ->
            line.trim()
                .replace(Regex("""^#{1,6}\s*"""), "")
                .replace(Regex("""^[-*+]\s+"""), "• ")
                .replace(Regex("""\*\*(.+?)\*\*"""), "$1")
                .replace(Regex("""`([^`]*)`"""), "$1")
                .replace(Regex("""\[([^\]]+)]\([^)]*\)"""), "$1")
        }
        val out = mutableListOf<String>()
        for (l in lines) if (l.isNotEmpty() || (out.isNotEmpty() && out.last().isNotEmpty())) out += l
        while (out.isNotEmpty() && out.last().isEmpty()) out.removeAt(out.lastIndex)
        return if (out.size <= maxLines) out else out.take(maxLines) + "…"
    }

    companion object {
        const val UNIVERSAL = "universal"
        private val SHA256 = Regex("^[0-9a-f]{64}$")

        /**
         * Parses `latest.json` fetched from [baseUrl] (the URL of the JSON file itself); asset
         * `file`s are resolved next to it. Assets without a valid size or SHA-256 are skipped, so a
         * download can always be verified. Returns null when the document is not a release.
         */
        fun fromJson(o: JsonObject, baseUrl: String): ReleaseInfo? {
            val tag = o.string("tag") ?: return null
            val version = AppVersion.normalize(tag) ?: return null
            val dir = baseUrl.substringBeforeLast('/') + "/"
            val apks = o.obj("assets")?.fields.orEmpty().mapNotNull { (abi, v) ->
                val a = v as? JsonObject ?: return@mapNotNull null
                val file = a.string("file")?.takeIf { it.endsWith(".apk") && '/' !in it && ".." !in it } ?: return@mapNotNull null
                val size = a.long("size")?.takeIf { it > 0 } ?: return@mapNotNull null
                val sha = a.string("sha256")?.lowercase()?.takeIf(SHA256::matches) ?: return@mapNotNull null
                UpdateApk(abi, dir + file, a.string("name") ?: file, size, sha)
            }
            return ReleaseInfo(
                tag = tag,
                version = version,
                pageUrl = o.string("url"),
                published = o.string("published"),
                prerelease = o.bool("prerelease") ?: false,
                notes = o.string("notes")?.takeIf(String::isNotBlank),
                apks = apks,
            )
        }
    }
}
