package org.veskra.kit

import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/** Where a project save ends up. */
sealed interface SaveDestination {
    /** App-private file (gallery projects): finished by atomic rename of the validated staging file. */
    data class AppPrivate(val path: String) : SaveDestination

    /** User-chosen Storage Access Framework document; [uri] is opaque to :kit. */
    data class Saf(val uri: String) : SaveDestination
}

/** Ordered steps of a staged save (ARCHITECTURE §G persistence protocol). */
enum class SaveStep {
    /** Engine thread: `document.save` of version N to the app-private staging path. */
    ENCODE_TO_STAGING,

    /** Engine thread: `document.validate` re-reads the staging file (manifest, checksums, decode). */
    VALIDATE_STAGING,

    /** IO thread: `ContentResolver.openOutputStream(uri, "wt")`, stream copy, flush, close. */
    COPY_TO_DESTINATION,

    /** IO thread: atomic rename staging → app-private destination (same filesystem). */
    ATOMIC_RENAME,

    /** Best effort: delete the staging file. Failure here does not undo a completed save. */
    CLEANUP_STAGING,
}

/** Steps that must run on the engine thread (native session is single-threaded). */
val SaveStep.runsOnEngineThread: Boolean get() = this == SaveStep.ENCODE_TO_STAGING || this == SaveStep.VALIDATE_STAGING

/**
 * Immutable plan for one staged save of document version [version]. The staging file always lives in
 * app-private storage so a failed/non-atomic SAF provider write never destroys the only good copy.
 */
data class SafCopyPlan(
    val jobId: Long,
    val version: Long,
    val stagingPath: String,
    val destination: SaveDestination,
) {
    val steps: List<SaveStep> = when (destination) {
        is SaveDestination.Saf -> listOf(SaveStep.ENCODE_TO_STAGING, SaveStep.VALIDATE_STAGING, SaveStep.COPY_TO_DESTINATION, SaveStep.CLEANUP_STAGING)
        is SaveDestination.AppPrivate -> listOf(SaveStep.ENCODE_TO_STAGING, SaveStep.VALIDATE_STAGING, SaveStep.ATOMIC_RENAME, SaveStep.CLEANUP_STAGING)
    }

    /** The step after which the destination holds the complete, validated version. */
    val commitStep: SaveStep get() = steps[2]

    fun next(after: SaveStep): SaveStep? = steps.getOrNull(steps.indexOf(after) + 1)

    companion object {
        /** SAF open mode: write + truncate (some providers otherwise leave stale trailing bytes). */
        const val SAF_WRITE_MODE: String = "wt"

        fun create(jobId: Long, version: Long, stagingDir: String, destination: SaveDestination): SafCopyPlan {
            require(version >= 0 && jobId >= 0)
            val dir = stagingDir.trimEnd('/')
            return SafCopyPlan(jobId, version, "$dir/save-$jobId-v$version.veskra.tmp", destination)
        }
    }
}

enum class SaveFailureReason { ENCODE_FAILED, VALIDATION_FAILED, PERMISSION_DENIED, STORAGE_FULL, PROVIDER_WRITE_FAILED, CANCELLED, UNKNOWN }

data class SaveFailure(val reason: SaveFailureReason, val step: SaveStep, val message: String)

/** What the editor's save indicator shows. */
sealed interface SaveStatus {
    /** New document, never saved anywhere. */
    data class NeverSaved(val hasEdits: Boolean) : SaveStatus
    data class Saved(val version: Long) : SaveStatus
    data class Unsaved(val savedVersion: Long, val currentVersion: Long) : SaveStatus
    data class Saving(val step: SaveStep, val version: Long) : SaveStatus
    data class Failed(val failure: SaveFailure, val hasUnsavedChanges: Boolean) : SaveStatus
}

/**
 * Save/dirty-version logic (SAV-002, ARCHITECTURE §G). Pure and single-threaded: the ViewModel calls it
 * from one thread and dispatches each returned [SaveStep] to the right executor.
 *
 * Invariants:
 * - `savedVersion` changes ONLY after the plan's commit step succeeded (stream closed / rename done).
 * - Saving snapshots version N; edits during the save move `currentVersion` to N+1 and the document
 *   remains dirty after the save completes.
 * - Results for a stale job id are ignored. Only one job runs at a time; a request during a job is
 *   coalesced into one follow-up save of the then-current version.
 */
class SaveStateMachine(initialVersion: Long = 0L, initialSavedVersion: Long? = null) {
    var currentVersion: Long = initialVersion
        private set
    var savedVersion: Long? = initialSavedVersion
        private set
    var activePlan: SafCopyPlan? = null
        private set
    var activeStep: SaveStep? = null
        private set
    var lastFailure: SaveFailure? = null
        private set
    /** A save was requested while another was running. */
    var followUpRequested: Boolean = false
        private set
    /** Destination of the last successful save (the "Save" target; null → ask with SAF "Save as"). */
    var lastDestination: SaveDestination? = null
        private set

    private var nextJobId = 1L

    val isDirty: Boolean get() = savedVersion != currentVersion
    val isSaving: Boolean get() = activePlan != null

    val status: SaveStatus
        get() {
            val plan = activePlan
            val step = activeStep
            val saved = savedVersion
            val failure = lastFailure
            return when {
                plan != null && step != null -> SaveStatus.Saving(step, plan.version)
                failure != null -> SaveStatus.Failed(failure, isDirty)
                saved == null -> SaveStatus.NeverSaved(hasEdits = currentVersion != 0L)
                saved == currentVersion -> SaveStatus.Saved(saved)
                else -> SaveStatus.Unsaved(saved, currentVersion)
            }
        }

    /** Engine reported a (monotonic) document version. Older/equal values are ignored. */
    fun onDocumentVersion(v: Long) {
        if (v > currentVersion) currentVersion = v
    }

    /** New or opened document: resets all tracking (e.g. opened file is saved at [version]). */
    fun reset(version: Long, savedVersion: Long?, destination: SaveDestination? = null) {
        currentVersion = version
        this.savedVersion = savedVersion
        activePlan = null
        activeStep = null
        lastFailure = null
        followUpRequested = false
        lastDestination = destination
    }

    /**
     * Starts saving the current version. Returns the plan (first step = [SafCopyPlan.steps]`[0]`), or null
     * if a save is already running (then a follow-up is queued).
     */
    fun begin(destination: SaveDestination, stagingDir: String): SafCopyPlan? {
        if (activePlan != null) {
            followUpRequested = true
            return null
        }
        val plan = SafCopyPlan.create(nextJobId++, currentVersion, stagingDir, destination)
        activePlan = plan
        activeStep = plan.steps.first()
        lastFailure = null
        return plan
    }

    /**
     * Reports success of [step] for [jobId]. Returns the next step to run, or null when the job is
     * finished (or the report was stale).
     */
    fun onStepSucceeded(jobId: Long, step: SaveStep): SaveStep? {
        val plan = activePlan ?: return null
        if (plan.jobId != jobId || activeStep != step) return null
        if (step == plan.commitStep) {
            // Only now is the destination known-good for version N.
            savedVersion = plan.version
            lastDestination = plan.destination
        }
        val next = plan.next(step)
        if (next == null) {
            activePlan = null
            activeStep = null
        } else {
            activeStep = next
        }
        return next
    }

    /**
     * Reports failure. Before the commit step this aborts the job and keeps the previous saved version.
     * A cleanup failure after commit is not a save failure (the file is saved; staging is stale).
     * Returns true if the failure was applied (false if stale).
     */
    fun onStepFailed(jobId: Long, step: SaveStep, reason: SaveFailureReason, message: String): Boolean {
        val plan = activePlan ?: return false
        if (plan.jobId != jobId || activeStep != step) return false
        activePlan = null
        activeStep = null
        if (step != SaveStep.CLEANUP_STAGING) lastFailure = SaveFailure(reason, step, message)
        return true
    }

    /** After a job ends: whether to immediately start the coalesced follow-up save. Clears the flag. */
    fun consumeFollowUp(): Boolean {
        if (activePlan != null) return false
        val f = followUpRequested && isDirty
        followUpRequested = false
        return f
    }

    fun clearFailure() {
        lastFailure = null
    }
}

class CopyFailedException(val reason: SaveFailureReason, message: String, cause: Throwable? = null) : IOException(message, cause)

/**
 * Stream copy used for SAF destinations. Success is returned ONLY after the output was flushed and
 * CLOSED without error (providers may report write errors on close). Pure JVM, testable.
 */
object StreamCopier {
    const val BUFFER_BYTES: Int = 64 * 1024

    /**
     * Copies everything from [openInput] to [openOutput], closing both. Verifies the byte count equals
     * [expectedBytes] when given. [isCancelled] is polled between chunks.
     * @return bytes copied.
     * @throws CopyFailedException with a typed reason.
     */
    fun copyAndClose(
        openInput: () -> InputStream,
        openOutput: () -> OutputStream,
        expectedBytes: Long? = null,
        isCancelled: () -> Boolean = { false },
    ): Long {
        val input = try {
            openInput()
        } catch (e: IOException) {
            throw CopyFailedException(SaveFailureReason.VALIDATION_FAILED, "staging file unreadable", e)
        }
        var total = 0L
        input.use { ins ->
            val out = try {
                openOutput()
            } catch (e: SecurityException) {
                throw CopyFailedException(SaveFailureReason.PERMISSION_DENIED, "destination permission denied", e)
            } catch (e: IOException) {
                throw CopyFailedException(SaveFailureReason.PROVIDER_WRITE_FAILED, "destination could not be opened", e)
            }
            try {
                val buf = ByteArray(BUFFER_BYTES)
                while (true) {
                    if (isCancelled()) throw CopyFailedException(SaveFailureReason.CANCELLED, "cancelled")
                    val n = ins.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    total += n
                }
                out.flush()
            } catch (e: CopyFailedException) {
                closeQuietly(out)
                throw e
            } catch (e: IOException) {
                closeQuietly(out)
                throw CopyFailedException(classify(e), "write failed", e)
            }
            try {
                out.close()
            } catch (e: IOException) {
                throw CopyFailedException(classify(e), "destination close failed", e)
            }
        }
        if (expectedBytes != null && total != expectedBytes) {
            throw CopyFailedException(SaveFailureReason.PROVIDER_WRITE_FAILED, "copied $total of $expectedBytes bytes")
        }
        return total
    }

    private fun classify(e: IOException): SaveFailureReason {
        val m = (e.message ?: "").lowercase()
        return if ("enospc" in m || "no space" in m) SaveFailureReason.STORAGE_FULL else SaveFailureReason.PROVIDER_WRITE_FAILED
    }

    private fun closeQuietly(c: OutputStream) {
        try {
            c.close()
        } catch (_: IOException) {
        }
    }
}
