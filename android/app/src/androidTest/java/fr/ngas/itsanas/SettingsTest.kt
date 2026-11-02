package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTextClearance
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Scenario 8: Settings. The words behind the passphrase, a new passphrase, the theme. */
@RunWith(AndroidJUnit4::class)
class SettingsTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin()

    @After fun after() {
        E2e.shell("cmd uimode night no")
        E2e.end()
    }

    private fun settled(button: String) =
        E2e.waitFor("the job behind $button never finished", 90) { E2e.enabled("#$button") && Host.current == null }

    @Test
    fun show_my_24_words_needs_the_passphrase_and_shows_the_same_words() {
        val made = Flows.createAccount(compose, Flows.freshName())
        Flows.openSettings(compose)
        E2e.shotBoth("40-settings")

        E2e.waitFor("Show my 24 words is not available") { E2e.enabled("#st-words") }
        E2e.tapId("st-words")
        E2e.waitFor("no passphrase was asked before showing the words") { Host.current is Ask.Passphrase }
        Flows.waitTag(compose, "ask-pass", "the passphrase screen is not drawn")
        Flows.type(compose, "ask-pass", "not the passphrase at all")
        Flows.click(compose, "secret-continue")
        settled("st-words")
        assertTrue("the words were shown for a wrong passphrase", Host.current !is Ask.Words)
        assertTrue("a wrong passphrase was refused without a word", E2e.textOf("#st-words-said").isNotEmpty())

        E2e.tapId("st-words")
        E2e.waitFor("no passphrase was asked the second time") { Host.current is Ask.Passphrase }
        Flows.waitTag(compose, "ask-pass", "the passphrase screen is not drawn the second time")
        Flows.type(compose, "ask-pass", Flows.PASSPHRASE)
        Flows.click(compose, "secret-continue")
        E2e.waitFor("the right passphrase did not show the words") { Host.current is Ask.Words }
        Flows.waitTag(compose, "words-have-them", "the words screen is not drawn")
        assertEquals(
            "the words shown now are not the words written down at setup, so the sheet is useless",
            made.phrase.trim().split(Regex("\\s+")),
            Flows.wordsShown(compose),
        )
        Flows.click(compose, "words-have-them")
        settled("st-words")
    }

    @Test
    fun a_new_passphrase_replaces_the_old_one() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.openSettings(compose)
        E2e.waitFor("Change my passphrase is not available") { E2e.enabled("#st-passphrase") }
        E2e.tapId("st-passphrase")
        E2e.waitFor("the current passphrase was not asked") { Host.current is Ask.Passphrase }
        Flows.waitTag(compose, "ask-pass", "the passphrase screen is not drawn")
        Flows.type(compose, "ask-pass", Flows.PASSPHRASE)
        Flows.click(compose, "secret-continue")
        E2e.waitFor("no new passphrase was asked after the old one") { Host.current is Ask.NewPassphrase }
        Flows.waitTag(compose, "new-pass", "the new passphrase screen is not drawn")
        Flows.typePassphrase(compose, Flows.OTHER_PASSPHRASE)
        Flows.click(compose, "secret-continue")
        settled("st-passphrase")

        Flows.lock(compose)
        Flows.unlock(compose, Flows.PASSPHRASE)
        Flows.waitTag(compose, "complaint", "the OLD passphrase still opens the account after it was changed")
        Flows.closeComplaint(compose)
        compose.onNodeWithTag("unlock-pass").performTextClearance()
        Flows.unlock(compose, Flows.OTHER_PASSPHRASE)
        Flows.waitTag(compose, "files-add", "the new passphrase does not open the account", 90)
    }

    private fun chooseTheme(word: String) {
        E2e.js(
            "(function(){var s=document.getElementById('st-theme');s.value='$word';" +
                "s.dispatchEvent(new Event('change',{bubbles:true}));return s.value})()",
        )
    }

    private fun dark(): Boolean = E2e.brightness(0.03, 0.5) < 110

    @Test
    fun the_theme_changes_the_screen_at_once_and_follows_the_phone() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.openSettings(compose)
        chooseTheme("dark")
        E2e.waitFor("Dark did not darken the screen") { dark() }
        E2e.shot("41-theme-dark")
        chooseTheme("light")
        E2e.waitFor("Light did not lighten the screen") { !dark() }
        E2e.shot("42-theme-light")
        chooseTheme("system")
        E2e.shell("cmd uimode night yes")
        E2e.waitFor("Same as the system did not follow the phone into night mode") { dark() }
        E2e.shell("cmd uimode night no")
        E2e.waitFor("Same as the system did not follow the phone back to day") { !dark() }
    }
}
