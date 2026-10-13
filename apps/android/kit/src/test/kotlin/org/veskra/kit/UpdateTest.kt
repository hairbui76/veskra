package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class UpdateTest {
    private val sha = "2ff8dbc8213d86f947c074b4e27717e6328d06c1a71dbece9205731295293574"

    private fun latest(tag: String = "v0.1.0-alpha.5", assets: String = defaultAssets, notes: String? = null) = Json.parseObject(
        """{"tag":"$tag","url":"https://github.com/hairbui76/veskra/releases/tag/$tag","published":"2026-10-14T03:27:02Z",
        "prerelease":true,${notes?.let { "\"notes\":" + JsonString(it).encode() + "," } ?: ""}"assets":{$assets}}""",
    )

    private val defaultAssets = """
        "arm64-v8a":{"file":"veskra-arm64-v8a.apk","name":"veskra-0.1.0-alpha.5-arm64-v8a.apk","size":15632195,"sha256":"$sha"},
        "x86_64":{"file":"veskra-x86_64.apk","name":"veskra-0.1.0-alpha.5-x86_64.apk","size":16305701,"sha256":"$sha"},
        "universal":{"file":"veskra-universal.apk","name":"veskra-0.1.0-alpha.5-universal.apk","size":28577481,"sha256":"$sha"},
        "sums":{"file":"SHA256SUMS.txt","name":"veskra-0.1.0-alpha.5-SHA256SUMS.txt","size":300}
    """

    private val base = "https://veskra.hairbui76.id.vn/download/latest.json"

    @Test
    fun `versions follow semantic version precedence`() {
        val ordered = listOf("0.1.0-alpha", "0.1.0-alpha.1", "0.1.0-alpha.4", "0.1.0-alpha.10", "0.1.0-alpha.beta", "0.1.0-beta.2", "0.1.0-rc.1", "0.1.0", "0.1.1", "0.2.0", "1.0.0")
        for (i in ordered.indices) {
            for (j in ordered.indices) {
                assertEquals(i.compareTo(j), AppVersion.compare(ordered[i], ordered[j]), "${ordered[i]} vs ${ordered[j]}")
            }
        }
        assertEquals(0, AppVersion.compare("v0.1.0-alpha.4", "0.1.0-alpha.4+build.7"))
        assertNull(AppVersion.compare("0.1", "0.1.0"))
        assertNull(AppVersion.compare("nightly", "0.1.0"))
        assertEquals("0.1.0-alpha.4", AppVersion.normalize("v0.1.0-alpha.4"))
    }

    @Test
    fun `parses latest json and resolves the APK next to it`() {
        val r = ReleaseInfo.fromJson(latest(), base)!!
        assertEquals("0.1.0-alpha.5", r.version)
        assertTrue(r.prerelease)
        assertEquals(3, r.apks.size) // SHA256SUMS.txt is not an APK
        val arm = r.apkFor(listOf("arm64-v8a", "armeabi-v7a"))!!
        assertEquals("https://veskra.hairbui76.id.vn/download/veskra-arm64-v8a.apk", arm.url)
        assertEquals(15632195L, arm.size)
        assertEquals(sha, arm.sha256)
        assertEquals("x86_64", r.apkFor(listOf("x86_64", "x86"))!!.abi)
        assertEquals("universal", r.apkFor(listOf("armeabi-v7a"))!!.abi)
    }

    @Test
    fun `newer only when the published version is above the installed one`() {
        val r = ReleaseInfo.fromJson(latest(), base)!!
        assertTrue(r.isNewerThan("0.1.0-alpha.4"))
        assertFalse(r.isNewerThan("0.1.0-alpha.5"))
        assertFalse(r.isNewerThan("0.1.0"))
        assertFalse(r.isNewerThan("not-a-version"))
    }

    @Test
    fun `assets that cannot be verified or escape the folder are skipped`() {
        val bad = """
            "arm64-v8a":{"file":"../evil.apk","size":10,"sha256":"$sha"},
            "x86_64":{"file":"veskra-x86_64.apk","size":10,"sha256":"not-a-hash"},
            "universal":{"file":"veskra-universal.apk","sha256":"$sha"}
        """
        val r = ReleaseInfo.fromJson(latest(assets = bad), base)!!
        assertTrue(r.apks.isEmpty())
        assertNull(r.apkFor(listOf("arm64-v8a")))
        assertNull(ReleaseInfo.fromJson(Json.parseObject("""{"tag":"latest"}"""), base))
    }

    @Test
    fun `notes become plain lines`() {
        val notes = "## Veskra 0.1.0-alpha.5\n\n\n**Check for updates** in the `⋯` menu.\n- Downloads the [APK](https://x) for your tablet\n* Keeps your drawings\n\n| File | For |\n|---|---|\n| a.apk | tablets |\n"
        val r = ReleaseInfo.fromJson(latest(notes = notes), base)!!
        assertEquals(
            listOf("Veskra 0.1.0-alpha.5", "", "Check for updates in the ⋯ menu.", "• Downloads the APK for your tablet", "• Keeps your drawings"),
            r.plainNotes(),
        )
        assertEquals(3, r.plainNotes(maxLines = 2).size)
        assertTrue(ReleaseInfo.fromJson(latest(), base)!!.plainNotes().isEmpty())
    }
}
