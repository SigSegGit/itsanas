package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Android 15's six hours a day of syncing, cut to seconds with the setting
 * Android documents for it. 0.1.16 crashed here on a real phone, over and over:
 * `ForegroundServiceDidNotStopInTimeException` at the limit, then
 * `ForegroundServiceStartNotAllowedException` on each restart. A crash ends this
 * process, so the run itself goes red.
 */
@RunWith(AndroidJUnit4::class)
class ServiceLimitTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() {
        // Google's services may sync the settings back while the test runs.
        E2e.shell("cmd device_config set_sync_disabled_for_tests persistent")
        E2e.shell("cmd device_config put activity_manager $LIMIT 5000")
        E2e.begin(compose)
    }

    @After fun after() {
        E2e.shell("cmd device_config delete activity_manager $LIMIT")
        E2e.shell("cmd device_config set_sync_disabled_for_tests none")
        E2e.end()
    }

    @Test
    fun a_phone_past_its_limit_stops_syncing_and_does_not_crash() {
        Flows.createAccount(compose, Flows.freshName())
        E2e.waitFor("the account opened and no service started, so nothing here met the limit", 30) {
            SyncService.running
        }

        // The limit counts only while the app is off the screen.
        E2e.shell("cmd device_config put activity_manager $LIMIT 5000")
        E2e.shell("input keyevent KEYCODE_HOME")
        E2e.waitFor("the service outlived its limit, and Android kills an app for that", 60) {
            !SyncService.running
        }

        // The budget is spent: the start Android refuses, as after a system restart.
        SyncService.start(E2e.context)
        // Past the wait Android gives a late service before it crashes the app.
        Thread.sleep(CRASH_WAIT_MS)
        assertFalse("a refused service reported itself running, so the worker would stand aside and nothing would sync", SyncService.running)
    }

    private companion object {
        const val LIMIT = "data_sync_fgs_timeout_duration"
        const val CRASH_WAIT_MS = 15_000L
    }
}
