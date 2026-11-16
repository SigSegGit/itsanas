package fr.ngas.itsanas

import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTextClearance
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Scenario 10: Forgot my passphrase. The words of another account are refused and nothing is erased; the right ones replace the keys. */
@RunWith(AndroidJUnit4::class)
class ForgotTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    @After fun after() = E2e.end()

    private fun forgotWith(phrase: String, accepted: Boolean = false) {
        Flows.click(compose, "unlock-forgot")
        Flows.typeRecovery(compose, phrase)
        Flows.waitTag(compose, "secret-continue", "no Continue on the words screen")
        E2e.waitFor("the words were not accepted by the screen that checks them") {
            runCatching { compose.onNodeWithTag("secret-continue").assertIsEnabled(); true }.getOrDefault(false)
        }
        // Words that are refused leave the screen up on purpose: tap once.
        if (accepted) Flows.continueAfterWords(compose) else Flows.click(compose, "secret-continue")
    }

    @Test
    fun the_words_of_another_account_are_refused_and_nothing_is_erased() {
        val other = Flows.createAccount(compose, Flows.freshName())
        E2e.relaunch()
        Flows.createAccount(compose, Flows.freshName())
        Flows.lock(compose)

        forgotWith(other.phrase)
        // The engine may ask for the new passphrase before it checks whose words they are.
        E2e.waitFor("the words of another account were neither refused nor asked for a passphrase") {
            Flows.has(compose, "complaint") || Host.current is Ask.NewPassphrase
        }
        if (Host.current is Ask.NewPassphrase) {
            Flows.typePassphrase(compose, Flows.OTHER_PASSPHRASE)
            Flows.click(compose, "secret-continue")
        }
        Flows.waitTag(compose, "complaint", "the words of another account were not refused, so anybody with any account's words can take this phone's account")
        Flows.closeComplaint(compose)

        // Nothing was erased: the old passphrase still opens the account.
        compose.onNodeWithTag("unlock-pass").performTextClearance()
        Flows.unlock(compose, Flows.PASSPHRASE)
        Flows.waitTag(compose, "files-add", "the refused words erased the keys: the old passphrase no longer opens the account", 90)
    }

    @Test
    fun the_right_words_replace_the_keys() {
        val mine = Flows.createAccount(compose, Flows.freshName())
        Flows.lock(compose)
        forgotWith(mine.phrase, accepted = true)
        E2e.waitFor("no new passphrase was asked after the right words") { Host.current is Ask.NewPassphrase }
        Flows.waitTag(compose, "new-pass", "the new passphrase screen is not drawn")
        Flows.typePassphrase(compose, Flows.OTHER_PASSPHRASE)
        Flows.click(compose, "secret-continue")
        Flows.waitTag(compose, "files-add", "the right words did not give the account back", 120)

        Flows.lock(compose)
        Flows.unlock(compose, Flows.PASSPHRASE)
        Flows.waitTag(compose, "complaint", "the old passphrase still opens the account after the keys were replaced")
        Flows.closeComplaint(compose)
        compose.onNodeWithTag("unlock-pass").performTextClearance()
        Flows.unlock(compose, Flows.OTHER_PASSPHRASE)
        Flows.waitTag(compose, "files-add", "the new passphrase does not open the account", 90)
    }
}
