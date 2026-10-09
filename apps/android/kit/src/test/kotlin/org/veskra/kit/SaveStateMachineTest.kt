package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertInstanceOf
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.OutputStream

class SaveStateMachineTest {
    private val saf = SaveDestination.Saf("content://provider/doc/1")
    private val staging = "/data/user/0/org.veskra.dev/cache/save-staging/"

    private fun runAll(m: SaveStateMachine, plan: SafCopyPlan) {
        var step: SaveStep? = plan.steps.first()
        while (step != null) step = m.onStepSucceeded(plan.jobId, step)
    }

    @Test
    fun `plan stages in app private storage and orders steps`() {
        val plan = SafCopyPlan.create(7, 3, staging, saf)
        assertEquals("/data/user/0/org.veskra.dev/cache/save-staging/save-7-v3.veskra.tmp", plan.stagingPath)
        assertEquals(listOf(SaveStep.ENCODE_TO_STAGING, SaveStep.VALIDATE_STAGING, SaveStep.COPY_TO_DESTINATION, SaveStep.CLEANUP_STAGING), plan.steps)
        assertEquals(SaveStep.COPY_TO_DESTINATION, plan.commitStep)
        assertEquals("wt", SafCopyPlan.SAF_WRITE_MODE)
        val local = SafCopyPlan.create(1, 1, staging, SaveDestination.AppPrivate("/files/projects/a.veskra"))
        assertEquals(SaveStep.ATOMIC_RENAME, local.commitStep)
        assertTrue(SaveStep.ENCODE_TO_STAGING.runsOnEngineThread && SaveStep.VALIDATE_STAGING.runsOnEngineThread)
        assertFalse(SaveStep.COPY_TO_DESTINATION.runsOnEngineThread)
    }

    @Test
    fun `saved version is set only after the commit step`() {
        val m = SaveStateMachine()
        m.onDocumentVersion(5)
        assertEquals(SaveStatus.NeverSaved(hasEdits = true), m.status)
        val plan = m.begin(saf, staging)!!
        assertEquals(SaveStep.VALIDATE_STAGING, m.onStepSucceeded(plan.jobId, SaveStep.ENCODE_TO_STAGING))
        assertEquals(SaveStep.COPY_TO_DESTINATION, m.onStepSucceeded(plan.jobId, SaveStep.VALIDATE_STAGING))
        assertNull(m.savedVersion, "not saved after encode/validate")
        assertEquals(SaveStatus.Saving(SaveStep.COPY_TO_DESTINATION, 5), m.status)
        assertEquals(SaveStep.CLEANUP_STAGING, m.onStepSucceeded(plan.jobId, SaveStep.COPY_TO_DESTINATION))
        assertEquals(5L, m.savedVersion)
        assertNull(m.onStepSucceeded(plan.jobId, SaveStep.CLEANUP_STAGING))
        assertFalse(m.isSaving)
        assertEquals(SaveStatus.Saved(5), m.status)
        assertEquals(saf, m.lastDestination)
    }

    @Test
    fun `SAV-002 edits during a save keep the document dirty`() {
        val m = SaveStateMachine(initialVersion = 10, initialSavedVersion = 10)
        m.onDocumentVersion(11)
        val plan = m.begin(saf, staging)!!
        assertEquals(11L, plan.version)
        m.onStepSucceeded(plan.jobId, SaveStep.ENCODE_TO_STAGING)
        m.onDocumentVersion(12) // user keeps drawing while the save copies
        runAllFrom(m, plan, SaveStep.VALIDATE_STAGING)
        assertEquals(11L, m.savedVersion)
        assertTrue(m.isDirty)
        assertEquals(SaveStatus.Unsaved(savedVersion = 11, currentVersion = 12), m.status)
    }

    private fun runAllFrom(m: SaveStateMachine, plan: SafCopyPlan, from: SaveStep) {
        var step: SaveStep? = from
        while (step != null) step = m.onStepSucceeded(plan.jobId, step)
    }

    @Test
    fun `failure before commit keeps the previous saved version`() {
        val m = SaveStateMachine(initialVersion = 3, initialSavedVersion = 3)
        m.onDocumentVersion(4)
        val plan = m.begin(saf, staging)!!
        m.onStepSucceeded(plan.jobId, SaveStep.ENCODE_TO_STAGING)
        m.onStepSucceeded(plan.jobId, SaveStep.VALIDATE_STAGING)
        assertTrue(m.onStepFailed(plan.jobId, SaveStep.COPY_TO_DESTINATION, SaveFailureReason.STORAGE_FULL, "no space"))
        assertEquals(3L, m.savedVersion)
        val st = m.status as SaveStatus.Failed
        assertEquals(SaveFailureReason.STORAGE_FULL, st.failure.reason)
        assertTrue(st.hasUnsavedChanges)
        assertFalse(m.isSaving)
        // Retry succeeds and clears the failure.
        runAll(m, m.begin(saf, staging)!!)
        assertEquals(SaveStatus.Saved(4), m.status)
    }

    @Test
    fun `validation failure aborts before touching the destination`() {
        val m = SaveStateMachine()
        m.onDocumentVersion(1)
        val plan = m.begin(saf, staging)!!
        m.onStepSucceeded(plan.jobId, SaveStep.ENCODE_TO_STAGING)
        m.onStepFailed(plan.jobId, SaveStep.VALIDATE_STAGING, SaveFailureReason.VALIDATION_FAILED, "checksum")
        assertNull(m.savedVersion)
        assertNull(m.onStepSucceeded(plan.jobId, SaveStep.COPY_TO_DESTINATION), "late results of an aborted job are ignored")
        assertNull(m.savedVersion)
    }

    @Test
    fun `cleanup failure after commit is not a save failure`() {
        val m = SaveStateMachine()
        m.onDocumentVersion(2)
        val plan = m.begin(saf, staging)!!
        m.onStepSucceeded(plan.jobId, SaveStep.ENCODE_TO_STAGING)
        m.onStepSucceeded(plan.jobId, SaveStep.VALIDATE_STAGING)
        m.onStepSucceeded(plan.jobId, SaveStep.COPY_TO_DESTINATION)
        m.onStepFailed(plan.jobId, SaveStep.CLEANUP_STAGING, SaveFailureReason.UNKNOWN, "rm failed")
        assertEquals(SaveStatus.Saved(2), m.status)
    }

    @Test
    fun `stale and out of order reports are ignored`() {
        val m = SaveStateMachine()
        m.onDocumentVersion(1)
        val plan = m.begin(saf, staging)!!
        assertNull(m.onStepSucceeded(plan.jobId + 1, SaveStep.ENCODE_TO_STAGING))
        assertNull(m.onStepSucceeded(plan.jobId, SaveStep.COPY_TO_DESTINATION), "cannot skip validation")
        assertFalse(m.onStepFailed(plan.jobId, SaveStep.COPY_TO_DESTINATION, SaveFailureReason.UNKNOWN, ""))
        assertNull(m.savedVersion)
        assertEquals(SaveStatus.Saving(SaveStep.ENCODE_TO_STAGING, 1), m.status)
    }

    @Test
    fun `requests during a save coalesce into one follow up`() {
        val m = SaveStateMachine()
        m.onDocumentVersion(1)
        val plan = m.begin(saf, staging)!!
        assertNull(m.begin(saf, staging))
        assertNull(m.begin(saf, staging))
        assertFalse(m.consumeFollowUp(), "not while running")
        m.onDocumentVersion(2)
        runAll(m, plan)
        assertTrue(m.consumeFollowUp())
        assertFalse(m.consumeFollowUp())
        assertEquals(2L, m.begin(saf, staging)!!.version)
    }

    @Test
    fun `versions are monotonic and reset on open`() {
        val m = SaveStateMachine(initialVersion = 5)
        m.onDocumentVersion(3)
        assertEquals(5L, m.currentVersion)
        m.reset(version = 0, savedVersion = 0, destination = saf)
        assertEquals(SaveStatus.Saved(0), m.status)
        assertEquals(saf, m.lastDestination)
    }

    // ---- StreamCopier ----

    @Test
    fun `copies all bytes and reports only after close`() {
        val data = ByteArray(200_000) { (it * 31).toByte() }
        val out = ByteArrayOutputStream()
        val n = StreamCopier.copyAndClose({ ByteArrayInputStream(data) }, { out }, expectedBytes = data.size.toLong())
        assertEquals(data.size.toLong(), n)
        assertArrayEquals(data, out.toByteArray())
    }

    @Test
    fun `close failure is a failure`() {
        val out = object : OutputStream() {
            override fun write(b: Int) {}
            override fun close() = throw IOException("ENOSPC (No space left on device)")
        }
        val e = assertThrows(CopyFailedException::class.java) {
            StreamCopier.copyAndClose({ ByteArrayInputStream(ByteArray(10)) }, { out })
        }
        assertEquals(SaveFailureReason.STORAGE_FULL, e.reason)
    }

    @Test
    fun `typed failures for permission write size and cancel`() {
        val perm = assertThrows(CopyFailedException::class.java) {
            StreamCopier.copyAndClose({ ByteArrayInputStream(ByteArray(1)) }, { throw SecurityException("revoked") })
        }
        assertEquals(SaveFailureReason.PERMISSION_DENIED, perm.reason)
        var closed = false
        val failing = object : OutputStream() {
            override fun write(b: Int) = throw IOException("provider gone")
            override fun write(b: ByteArray, off: Int, len: Int) = throw IOException("provider gone")
            override fun close() { closed = true }
        }
        val w = assertThrows(CopyFailedException::class.java) {
            StreamCopier.copyAndClose({ ByteArrayInputStream(ByteArray(5)) }, { failing })
        }
        assertEquals(SaveFailureReason.PROVIDER_WRITE_FAILED, w.reason)
        assertTrue(closed, "output closed on failure")
        val size = assertThrows(CopyFailedException::class.java) {
            StreamCopier.copyAndClose({ ByteArrayInputStream(ByteArray(5)) }, { ByteArrayOutputStream() }, expectedBytes = 6)
        }
        assertEquals(SaveFailureReason.PROVIDER_WRITE_FAILED, size.reason)
        val cancel = assertThrows(CopyFailedException::class.java) {
            StreamCopier.copyAndClose({ ByteArrayInputStream(ByteArray(5)) }, { ByteArrayOutputStream() }, isCancelled = { true })
        }
        assertEquals(SaveFailureReason.CANCELLED, cancel.reason)
        assertInstanceOf(IOException::class.java, cancel)
    }
}
