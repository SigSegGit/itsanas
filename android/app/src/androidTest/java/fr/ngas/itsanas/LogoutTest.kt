package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.onNodeWithTag
import androidx.test.espresso.intent.Intents
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Scenarios 9: remove the account from the phone, then log in again from the 24 words. */
@RunWith(AndroidJUnit4::class)
class LogoutTest {
    @get:Rule val compose = createEmptyComposeRule()

    private val home get() = Vault.home(E2e.context).absolutePath

    @Before fun before() = E2e.begin(compose)

    @After fun after() {
        runCatching { Intents.release() }
        E2e.end()
    }

    private fun removeHere() {
        Flows.openSettings(compose)
        E2e.waitFor("Check what is safe to remove is not available") { E2e.enabled("#st-remove-check") }
        E2e.tapId("st-remove-check")
        E2e.waitFor("the removal check never answered") { E2e.visible("#st-remove-sure") }
    }

    @Test
    fun pieces_held_only_here_need_the_name_typed_and_then_the_node_is_gone() {
        val made = Flows.createAccount(compose, Flows.freshName())
        Flows.addFile(compose)
        removeHere()
        E2e.shotBoth("50-remove-warning")
        assertTrue(
            "no name was asked although a file exists only on this phone, so one tap destroys it",
            E2e.visible("#st-remove-name-box"),
        )
        assertFalse("Remove is enabled before the name was typed", E2e.enabled("#st-remove-go"))
        E2e.typeInto("st-remove-name", made.name)
        E2e.waitFor("typing the name did not enable Remove") { E2e.enabled("#st-remove-go") }
        E2e.tapId("st-remove-go")
        E2e.waitFor("removing the account did not bring the first screen back") { E2e.onSection("welcome") }
        assertFalse("the node is still on the phone after Remove", Native.exists(home))
    }

    @Test
    fun logging_in_again_from_the_same_words_is_accepted() {
        val made = Flows.createAccount(compose, Flows.freshName())
        removeHere()
        E2e.waitFor("Remove is not enabled for an account with nothing held only here") { E2e.enabled("#st-remove-go") }
        E2e.tapId("st-remove-go")
        E2e.waitFor("removing the account did not bring the first screen back") { E2e.onSection("welcome") }
        assertFalse("the node is still on the phone after Remove", Native.exists(home))

        Flows.toAccountStep()
        E2e.tap("input[name=account][value=join]")
        E2e.typeInto("username", made.name)
        E2e.waitFor("the account was not found by its name") { E2e.textOf("#username-check") == "Account found." }
        Flows.nextOnAccount()
        E2e.waitFor("Next did nothing") { E2e.onSection("folder") }
        E2e.waitFor("Start setting up never became available") { E2e.enabled("#start") }
        E2e.tapId("start")
        Flows.typeRecovery(compose, made.phrase)
        Flows.waitTag(compose, "secret-continue", "no Continue on the words screen")
        Flows.click(compose, "secret-continue")
        Flows.waitTag(compose, "new-pass", "no new passphrase was asked after the words")
        Flows.typePassphrase(compose, Flows.OTHER_PASSPHRASE)
        Flows.click(compose, "secret-continue")
        E2e.waitFor("setup did not finish after logging in from the words", 240) { E2e.onSection("final") }
        E2e.tapId("finish")
        Flows.waitTag(compose, "files-add", "the account did not open after logging in again", 90)
    }

    @Test
    fun an_unknown_name_and_bad_words_are_refused() {
        val made = Flows.createAccount(compose, Flows.freshName())
        removeHere()
        E2e.waitFor("Remove is not enabled") { E2e.enabled("#st-remove-go") }
        E2e.tapId("st-remove-go")
        E2e.waitFor("removing the account did not bring the first screen back") { E2e.onSection("welcome") }

        // A name nobody has: the page says so and will not go on without a tick.
        Flows.toAccountStep()
        E2e.tap("input[name=account][value=join]")
        E2e.typeInto("username", Flows.freshName() + "x")
        E2e.waitFor("an unknown name was not said to be unknown") {
            E2e.textOf("#username-check").startsWith("No account has this name")
        }
        Flows.nextOnAccount()
        E2e.pause(1500)
        assertTrue("Next went on with a name no account has", E2e.onSection("account"))

        // The right name, then words that are not the account's.
        E2e.typeInto("username", made.name)
        E2e.waitFor("the account was not found by its name") { E2e.textOf("#username-check") == "Account found." }
        Flows.nextOnAccount()
        E2e.waitFor("Next did nothing") { E2e.onSection("folder") }
        E2e.waitFor("Start setting up never became available") { E2e.enabled("#start") }
        E2e.tapId("start")
        Flows.waitTag(compose, "recovery-words", "the screen asking for the 24 words never opened")
        compose.onNodeWithTag("recovery-words").performTextInput("zzzz yyyy xxxx")
        E2e.pause(1500)
        E2e.shot("51-recovery-bad")
        try {
            compose.onNodeWithTag("secret-continue").assertIsNotEnabled()
        } catch (e: AssertionError) {
            fail("the words screen offered Continue for words that are not words, so a typo is found only after the passphrase")
        }
    }
}
