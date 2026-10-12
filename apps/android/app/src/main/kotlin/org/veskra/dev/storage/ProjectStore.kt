package org.veskra.dev.storage

import android.content.ContentResolver
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ColorSpace
import android.net.Uri
import android.provider.OpenableColumns
import org.veskra.kit.CopyFailedException
import org.veskra.kit.ExportFileNames
import org.veskra.kit.Json
import org.veskra.kit.JsonParseException
import org.veskra.kit.SafCopyPlan
import org.veskra.kit.SaveFailureReason
import org.veskra.kit.StreamCopier
import org.veskra.kit.json
import java.io.File
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.IOException

/** A project file in app-private storage (shown in the Gallery). */
data class ProjectEntry(val name: String, val path: String, val modifiedMillis: Long, val bytes: Long)

/** An autosave checkpoint left behind by a session that did not end cleanly. */
data class RecoveryEntry(val documentName: String, val savedAtMillis: Long, val dir: String)

/**
 * App-private storage layout and SAF transfers. All functions do blocking IO: call them on
 * Dispatchers.IO, never on the UI thread.
 *
 * ```
 * filesDir/projects/<name>.veskra     gallery projects (app-private destination)
 * filesDir/recovery/                  engine autosave checkpoint + meta.json (one session)
 * cacheDir/save-staging/              staged saves before validation/copy (SafCopyPlan)
 * cacheDir/export-staging/            exports before SAF copy
 * cacheDir/open-staging/              SAF documents copied in before document.open
 * ```
 */
class ProjectStore(context: Context) {
    private val resolver: ContentResolver = context.contentResolver
    val projectsDir: File = File(context.filesDir, "projects").apply { mkdirs() }
    val recoveryDir: File = File(context.filesDir, "recovery").apply { mkdirs() }
    val saveStagingDir: File = File(context.cacheDir, "save-staging").apply { mkdirs() }
    val exportStagingDir: File = File(context.cacheDir, "export-staging").apply { mkdirs() }
    val openStagingDir: File = File(context.cacheDir, "open-staging").apply { mkdirs() }

    fun listProjects(): List<ProjectEntry> =
        projectsDir.listFiles { f -> f.isFile && f.name.endsWith("." + ExportFileNames.NATIVE_EXTENSION) }
            .orEmpty()
            .map { ProjectEntry(it.nameWithoutExtension, it.absolutePath, it.lastModified(), it.length()) }
            .sortedByDescending { it.modifiedMillis }

    fun projectFileFor(name: String): File = File(projectsDir, ExportFileNames.suggestProject(name))

    // ---- Recovery (process death / force-stop, MASTER_PLAN S8) ----

    private val recoveryMeta: File get() = File(recoveryDir, "meta.json")

    fun findRecovery(): RecoveryEntry? {
        val meta = recoveryMeta
        if (!meta.isFile || meta.length() > 64 * 1024) return null
        return try {
            val o = Json.parseObject(meta.readText())
            RecoveryEntry(o.string("name") ?: "Untitled", o.long("savedAt") ?: meta.lastModified(), recoveryDir.absolutePath)
        } catch (e: JsonParseException) {
            null
        } catch (e: IOException) {
            null
        }
    }

    /** Called after the engine wrote an autosave checkpoint into [recoveryDir]. */
    fun writeRecoveryMeta(documentName: String, nowMillis: Long) {
        val tmp = File(recoveryDir, "meta.json.tmp")
        tmp.writeText(json { "name" to documentName; "savedAt" to nowMillis }.encode())
        if (!tmp.renameTo(recoveryMeta)) {
            tmp.delete()
            throw IOException("could not publish recovery meta")
        }
    }

    /** Clean close or explicit discard: removes the checkpoint so no recovery prompt appears. */
    fun clearRecovery() {
        recoveryDir.listFiles().orEmpty().forEach { it.deleteRecursively() }
    }

    // ---- Staged save helpers ----

    /** ATOMIC_RENAME step: same filesystem (filesDir/cacheDir share the data partition). */
    fun atomicMove(staging: File, destination: File) {
        destination.parentFile?.mkdirs()
        if (!staging.renameTo(destination)) {
            // Different mount (unexpected): fall back to copy + rename of a sibling temp file.
            val sibling = File(destination.parentFile, destination.name + ".tmp")
            staging.copyTo(sibling, overwrite = true)
            if (!sibling.renameTo(destination)) {
                sibling.delete()
                throw CopyFailedException(SaveFailureReason.PROVIDER_WRITE_FAILED, "rename failed")
            }
            staging.delete()
        }
    }

    /**
     * COPY_TO_DESTINATION step for SAF: streams the validated staging file to [uri] opened in "wt" mode.
     * Returns only after the provider stream was closed successfully.
     */
    fun copyToSaf(staging: File, uri: Uri, isCancelled: () -> Boolean = { false }): Long =
        StreamCopier.copyAndClose(
            openInput = { FileInputStream(staging) },
            openOutput = {
                try {
                    resolver.openOutputStream(uri, SafCopyPlan.SAF_WRITE_MODE)
                        ?: throw CopyFailedException(SaveFailureReason.PROVIDER_WRITE_FAILED, "provider returned no stream")
                } catch (e: FileNotFoundException) {
                    throw CopyFailedException(SaveFailureReason.PROVIDER_WRITE_FAILED, "document not found", e)
                }
            },
            expectedBytes = staging.length(),
            isCancelled = isCancelled,
        )

    /** Copies a user-picked SAF document into app-private staging so the engine reads a plain file. */
    fun copyFromSaf(uri: Uri): File {
        val size = querySize(uri)
        if (size != null && size > MAX_OPEN_BYTES) {
            throw CopyFailedException(SaveFailureReason.VALIDATION_FAILED, "file too large")
        }
        val dst = File(openStagingDir, "open-${System.nanoTime()}.veskra")
        StreamCopier.copyAndClose(
            openInput = { resolver.openInputStream(uri) ?: throw IOException("provider returned no stream") },
            openOutput = { dst.outputStream() },
            expectedBytes = null,
        )
        return dst
    }

    /**
     * Decodes any image Android understands (PNG, JPEG, WebP, …) and stages it as an sRGB PNG for
     * `layer.import_image`. Wide-gamut sources are converted to sRGB by the platform decoder, and
     * images larger than [MAX_IMPORT_EDGE] are downsampled by a power of two. Caller deletes the file.
     */
    fun stageImageAsPng(uri: Uri): File {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) } ?: throw IOException("provider returned no stream")
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) throw IOException("not a decodable image")
        var sample = 1
        while (bounds.outWidth / sample > MAX_IMPORT_EDGE || bounds.outHeight / sample > MAX_IMPORT_EDGE) sample *= 2
        val opts = BitmapFactory.Options().apply {
            inSampleSize = sample
            inPreferredConfig = Bitmap.Config.ARGB_8888
            inPreferredColorSpace = ColorSpace.get(ColorSpace.Named.SRGB)
        }
        val bitmap = resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, opts) } ?: throw IOException("image could not be decoded")
        return try {
            stageBitmapAsPng(bitmap)
        } finally {
            bitmap.recycle()
        }
    }

    /** Imported printer (CMYK) profiles, kept app-private so exports can reference them by path. */
    val profilesDir: File = File(context.filesDir, "profiles").apply { mkdirs() }

    /**
     * Copies an ICC profile picked through SAF into [profilesDir] under a sanitized name. The engine
     * validates it afterwards (`profile.inspect`); the caller deletes the copy if it is unusable.
     */
    fun importProfile(uri: Uri): File {
        val size = querySize(uri)
        if (size != null && size > MAX_PROFILE_BYTES) throw IOException("profile too large")
        val base = (displayName(uri) ?: "profile").substringBeforeLast('.')
            .map { if (it.isLetterOrDigit() || it == '-' || it == '_') it else '_' }.joinToString("").take(60).ifBlank { "profile" }
        var dst = File(profilesDir, "$base.icc")
        var n = 2
        while (dst.exists()) dst = File(profilesDir, "$base-${n++}.icc")
        StreamCopier.copyAndClose(
            openInput = { resolver.openInputStream(uri) ?: throw IOException("provider returned no stream") },
            openOutput = { dst.outputStream() },
            expectedBytes = null,
        )
        if (dst.length() > MAX_PROFILE_BYTES) {
            dst.delete()
            throw IOException("profile too large")
        }
        return dst
    }

    /** Writes [bitmap] (sRGB) as a PNG into the open-staging cache dir. Caller deletes the file. */
    fun stageBitmapAsPng(bitmap: Bitmap): File {
        val dst = File(openStagingDir, "import-${System.nanoTime()}.png")
        dst.outputStream().use { out ->
            if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)) throw IOException("PNG encode failed")
        }
        return dst
    }

    fun displayName(uri: Uri): String? = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) c.getString(0) else null
    }

    private fun querySize(uri: Uri): Long? = resolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use { c ->
        if (c.moveToFirst() && !c.isNull(0)) c.getLong(0) else null
    }

    companion object {
        /** Matches the engine's total bundle bound (veskra_core::limits::MAX_BUNDLE_TOTAL_BYTES). */
        const val MAX_OPEN_BYTES: Long = 4L * 1024 * 1024 * 1024

        /** Matches the engine's limit for CMYK output profiles. */
        const val MAX_PROFILE_BYTES: Long = 32L * 1024 * 1024

        /** Longest edge accepted for an imported image before power-of-two downsampling. */
        const val MAX_IMPORT_EDGE: Int = 8192
    }
}
