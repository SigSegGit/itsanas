package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.work.ListenableWorker
import androidx.work.testing.TestListenableWorkerBuilder
import androidx.work.workDataOf
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The background once the app is closed (HANDOVER §8 10c): `SyncWorker` run as
 * WorkManager runs it, with no service and no open account. It reopens what the
 * system closed, and never what the person closed.
 */
@RunWith(AndroidJUnit4::class)
class WorkerTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    @After fun after() {
        Vault.noteClosedByPerson(E2e.context, false)
        E2e.end()
    }

    private fun stopTheService() {
        SyncService.stop(E2e.context)
        E2e.waitFor("the service did not stop, so the worker would stand aside", 30) {
            !SyncService.running
        }
    }

    private fun runTheWorker(): ListenableWorker.Result = runBlocking {
        TestListenableWorkerBuilder<SyncWorker>(E2e.context)
            .setInputData(workDataOf(SyncWorker.WINDOW_KEY to WINDOW_MS))
            .build()
            .doWork()
    }

    @Test
    fun an_account_the_system_closed_is_reopened_by_the_background_worker() {
        Flows.createAccount(compose, Flows.freshName())
        assertNotNull(
            "no passphrase kept for the background, so a killed app never syncs again until opened",
            Vault.rememberedPassphrase(E2e.context),
        )
        stopTheService()
        // What a killed process leaves: the node closed, the person never asked.
        runBlocking { Account.close() }

        assertEquals(ListenableWorker.Result.success(), runTheWorker())
        assertTrue(
            "the worker left the account closed: with the app killed, nothing syncs in the background",
            Account.open,
        )
    }

    @Test
    fun an_account_the_person_closed_stays_closed_in_the_background() {
        Flows.createAccount(compose, Flows.freshName())
        stopTheService()
        runBlocking { Account.close(E2e.context) }

        assertEquals(ListenableWorker.Result.success(), runTheWorker())
        assertFalse(
            "the worker opened an account its owner closed, behind their back",
            Account.open,
        )
    }

    private companion object {
        const val WINDOW_MS = 1_000L
    }
}
