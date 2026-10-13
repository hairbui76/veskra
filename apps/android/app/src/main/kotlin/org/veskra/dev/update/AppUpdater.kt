package org.veskra.dev.update

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.os.Build
import android.provider.Settings
import androidx.core.net.toUri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.veskra.dev.BuildConfig
import org.veskra.dev.MainActivity
import org.veskra.kit.Json
import org.veskra.kit.ReleaseInfo
import org.veskra.kit.UpdateApk
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import kotlin.coroutines.coroutineContext

/**
 * In-app update (ADR-020). Only runs when the user taps "Check for updates": fetches the website's
 * `download/latest.json`, downloads the APK for this device, checks its size, SHA-256, package
 * name and version code, and hands it to the system installer. Android still asks the user to
 * confirm, and refuses an APK signed with a different key. Nothing about the user's documents is
 * sent anywhere; the only requests are these two GETs.
 */
class AppUpdater(private val context: Context) {

    /** Why an update step failed, for a localized message. */
    class UpdateException(val reason: Reason, detail: String? = null) : IOException(detail ?: reason.name) {
        enum class Reason { NETWORK, BAD_RESPONSE, NO_APK, CHECKSUM, NOT_AN_UPDATE, STORAGE }
    }

    val installedVersion: String get() = BuildConfig.VERSION_NAME

    private val installedVersionCode: Long
        get() = installedInfo().longVersionCode

    private fun installedInfo(): PackageInfo = context.packageManager.getPackageInfo(context.packageName, 0)

    /** The newest release described by the website. */
    suspend fun check(): ReleaseInfo = withContext(Dispatchers.IO) {
        val text = try {
            open(LATEST_URL).use { c ->
                if (c.responseCode != HttpURLConnection.HTTP_OK) throw UpdateException(UpdateException.Reason.BAD_RESPONSE, "HTTP ${c.responseCode}")
                val out = java.io.ByteArrayOutputStream()
                c.inputStream.use { input ->
                    val buf = ByteArray(8 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        if (out.size() > MAX_JSON_BYTES) throw UpdateException(UpdateException.Reason.BAD_RESPONSE, "latest.json too large")
                    }
                }
                out.toByteArray().decodeToString()
            }
        } catch (e: UpdateException) {
            throw e
        } catch (e: IOException) {
            throw UpdateException(UpdateException.Reason.NETWORK, e.message)
        }
        val obj = try {
            Json.parseObject(text)
        } catch (e: Exception) {
            throw UpdateException(UpdateException.Reason.BAD_RESPONSE, e.message)
        }
        ReleaseInfo.fromJson(obj, LATEST_URL) ?: throw UpdateException(UpdateException.Reason.BAD_RESPONSE, "not a release")
    }

    /** The APK this device should install from [release], or null when none matches. */
    fun apkFor(release: ReleaseInfo): UpdateApk? = release.apkFor(Build.SUPPORTED_ABIS.toList())

    /**
     * Downloads [apk] into the app's cache, reporting progress 0..1, and verifies it. Returns the
     * verified file; a file that fails any check is deleted.
     */
    suspend fun download(apk: UpdateApk, onProgress: (Float) -> Unit): File = withContext(Dispatchers.IO) {
        val dir = File(context.cacheDir, "updates")
        dir.listFiles()?.forEach { it.delete() } // only ever one pending update
        if (!dir.isDirectory && !dir.mkdirs()) throw UpdateException(UpdateException.Reason.STORAGE)
        val file = File(dir, "veskra-update.apk")
        val digest = MessageDigest.getInstance("SHA-256")
        try {
            open(apk.url).use { c ->
                if (c.responseCode != HttpURLConnection.HTTP_OK) throw UpdateException(UpdateException.Reason.BAD_RESPONSE, "HTTP ${c.responseCode}")
                var done = 0L
                c.inputStream.use { input ->
                    file.outputStream().use { out ->
                        val buf = ByteArray(64 * 1024)
                        while (true) {
                            coroutineContext.ensureActive()
                            val n = input.read(buf)
                            if (n < 0) break
                            done += n
                            if (done > apk.size) throw UpdateException(UpdateException.Reason.CHECKSUM, "larger than announced")
                            digest.update(buf, 0, n)
                            out.write(buf, 0, n)
                            onProgress(done.toFloat() / apk.size)
                        }
                    }
                }
                if (done != apk.size) throw UpdateException(UpdateException.Reason.CHECKSUM, "size $done != ${apk.size}")
            }
            val hex = digest.digest().joinToString("") { "%02x".format(it) }
            if (hex != apk.sha256) throw UpdateException(UpdateException.Reason.CHECKSUM, "sha256 mismatch")
            verifyArchive(file)
            file
        } catch (e: Throwable) {
            file.delete()
            throw when (e) {
                is UpdateException -> e
                is IOException -> UpdateException(UpdateException.Reason.NETWORK, e.message)
                else -> e
            }
        }
    }

    /** The APK must be this app (same package) and a higher version code. */
    private fun verifyArchive(file: File) {
        val info = context.packageManager.getPackageArchiveInfo(file.absolutePath, 0)
            ?: throw UpdateException(UpdateException.Reason.CHECKSUM, "not an APK")
        if (info.packageName != context.packageName) throw UpdateException(UpdateException.Reason.NOT_AN_UPDATE, info.packageName)
        val code = info.longVersionCode
        if (code <= installedVersionCode) throw UpdateException(UpdateException.Reason.NOT_AN_UPDATE, "versionCode $code")
    }

    /** False until the user allows "Install unknown apps" for Veskra (Settings, once). */
    fun canInstall(): Boolean = context.packageManager.canRequestPackageInstalls()

    /** The settings page where the user allows Veskra to install updates. */
    fun installPermissionIntent(): Intent =
        Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, "package:${context.packageName}".toUri())

    /**
     * Streams [file] into a PackageInstaller session and commits it. The result comes back to
     * [MainActivity] as an [ACTION_INSTALL_STATUS] intent; the confirmation screen is shown from there.
     */
    suspend fun install(file: File) = withContext(Dispatchers.IO) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            if (Build.VERSION.SDK_INT >= 31) setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_REQUIRED)
        }
        val id = installer.createSession(params)
        try {
            installer.openSession(id).use { session ->
                file.inputStream().use { input ->
                    session.openWrite("veskra.apk", 0, file.length()).use { out ->
                        input.copyTo(out)
                        session.fsync(out)
                    }
                }
                val intent = Intent(context, MainActivity::class.java)
                    .setAction(ACTION_INSTALL_STATUS)
                    .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
                // Mutable: the installer fills in the status extras.
                val flags = PendingIntent.FLAG_UPDATE_CURRENT or (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
                session.commit(PendingIntent.getActivity(context, id, intent, flags).intentSender)
            }
        } catch (e: Exception) {
            runCatching { installer.abandonSession(id) }
            throw UpdateException(UpdateException.Reason.STORAGE, e.message)
        }
    }

    /** Deletes a downloaded update (after a failed install, or once it is no longer wanted). */
    fun clearDownloads() {
        File(context.cacheDir, "updates").listFiles()?.forEach { it.delete() }
    }

    private fun open(url: String): HttpURLConnection {
        require(url.startsWith("https://")) { "updates are only fetched over HTTPS" }
        return (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 15_000
            readTimeout = 30_000
            useCaches = false
            setRequestProperty("Cache-Control", "no-cache")
            setRequestProperty("User-Agent", "Veskra/${BuildConfig.VERSION_NAME} (Android ${Build.VERSION.RELEASE})")
        }
    }

    private fun <T> HttpURLConnection.use(block: (HttpURLConnection) -> T): T = try {
        block(this)
    } finally {
        disconnect()
    }

    companion object {
        /** Written by scripts/site_release_bundle.py on every website deploy. */
        const val LATEST_URL = "https://veskra.hairbui76.id.vn/download/latest.json"
        const val ACTION_INSTALL_STATUS = "org.veskra.dev.action.INSTALL_STATUS"
        private const val MAX_JSON_BYTES = 256 * 1024

        /** The intent the installer sends back: status, message and (pending user action) the confirmation screen. */
        fun isInstallStatus(intent: Intent?): Boolean = intent?.action == ACTION_INSTALL_STATUS

        @Suppress("DEPRECATION")
        fun confirmationIntent(intent: Intent): Intent? =
            if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java) else intent.getParcelableExtra(Intent.EXTRA_INTENT)

        fun status(intent: Intent): Int = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)

        fun statusMessage(intent: Intent): String? = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)

        /** Installed with a different signing key (e.g. a debug build): Android will refuse the update. */
        fun isConflict(status: Int): Boolean = status == PackageInstaller.STATUS_FAILURE_CONFLICT || status == PackageInstaller.STATUS_FAILURE_INCOMPATIBLE
    }
}
