package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTextClearance
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Scenario 7: reopening needs no passphrase; Lock now asks for it, and a wrong one is refused. */
@RunWith(AndroidJUnit4::class)
class SessionTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin()

    @After fun after() = E2e.end()

    @Test
    fun reopening_needs_no_passphrase() {
        Flows.createAccount(compose, Flows.freshName())
        E2e.reopen()
        Flows.waitTag(compose, "files-add", "reopening the app did not reach the files, so the person is locked out of an account they just made", 90)
        assertFalse("reopening asked for the passphrase although the phone remembers it", Flows.has(compose, "unlock-pass"))
    }

    @Test
    fun lock_now_then_a_wrong_passphrase_is_refused_and_the_right_one_opens() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.lock(compose)
        E2e.shot("30-unlock")

        Flows.unlock(compose, "not the passphrase at all")
        Flows.waitTag(compose, "complaint", "a wrong passphrase was not refused, so anybody holding the phone could try without a word")
        Flows.closeComplaint(compose)
        assertTrue("the Unlock screen went away after a refusal", Flows.has(compose, "unlock-pass"))

        compose.onNodeWithTag("unlock-pass").performTextClearance()
        Flows.unlock(compose, Flows.PASSPHRASE)
        Flows.waitTag(compose, "files-add", "the right passphrase did not open the account", 90)
    }
}
