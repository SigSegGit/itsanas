package fr.ngas.itsanas

import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.assertContentDescriptionEquals
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Scenario 3: the native passphrase screen. FLAG_SECURE is on, so its
 * screenshots are black on purpose; everything here is read from the screen's
 * own state instead.
 */
@RunWith(AndroidJUnit4::class)
class PassphraseTest {
    @get:Rule val compose = createEmptyComposeRule()

    private val rule = JSONObject(Native.passphraseRule())
    private val min = rule.getInt("min")
    private val max = rule.getInt("max")

    @Before fun before() {
        E2e.begin(compose)
        Flows.toPassphraseScreen(compose, Flows.freshName())
    }

    @After fun after() = E2e.end()

    private fun both(first: String, second: String) {
        compose.onNodeWithTag("new-pass").performTextClearance()
        compose.onNodeWithTag("new-pass-again").performTextClearance()
        Flows.type(compose, "new-pass", first)
        Flows.type(compose, "new-pass-again", second)
    }

    @Test
    fun too_short_is_refused() {
        val short = "a".repeat(min - 1)
        both(short, short)
        E2e.shot("07-passphrase-short")
        compose.onNodeWithTag("secret-continue").assertIsNotEnabled()
    }

    @Test
    fun too_long_is_refused() {
        val long = "a".repeat(max + 1)
        both(long, long)
        compose.onNodeWithTag("secret-continue").assertIsNotEnabled()
    }

    @Test
    fun a_different_second_copy_is_refused_and_said() {
        both("a".repeat(min), "b".repeat(min))
        assertTrue("no word says the two copies differ, so the person does not know why Continue is off", Flows.has(compose, "secret-error"))
        compose.onNodeWithTag("secret-continue").assertIsNotEnabled()
    }

    @Test
    fun the_minimum_and_an_equal_copy_are_accepted() {
        val ok = "a".repeat(min)
        both(ok, ok)
        compose.onNodeWithTag("secret-continue").assertIsEnabled()
        assertFalse("an error is shown for two equal passphrases", Flows.has(compose, "secret-error"))
    }

    @Test
    fun the_eye_reveals_and_hides() {
        both("visible-secret", "visible-secret")
        val hidden = Flows.textOf(compose, "new-pass")
        assertFalse("the passphrase is on screen before anybody asked to see it: '$hidden'", hidden.contains("visible-secret"))
        compose.onNodeWithTag("new-pass-eye").assertContentDescriptionEquals(Flows.string(R.string.show_passphrase))
        Flows.click(compose, "new-pass-eye")
        val shown = Flows.textOf(compose, "new-pass")
        assertTrue("the eye did not reveal the passphrase, so a typo cannot be checked: '$shown'", shown.contains("visible-secret"))
        compose.onNodeWithTag("new-pass-eye").assertContentDescriptionEquals(Flows.string(R.string.hide_passphrase))
        Flows.click(compose, "new-pass-eye")
        val again = Flows.textOf(compose, "new-pass")
        assertFalse("the eye did not hide the passphrase again: '$again'", again.contains("visible-secret"))
    }
}
