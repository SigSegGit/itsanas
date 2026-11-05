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

    @Before fun before() = E2e.begin(compose)

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

    private fun fieldIs(id: String, value: String) =
        E2e.pageHas("document.getElementById('$id').value === '$value'")

    private fun assertTyped(moment: String) {
        assertTrue("$moment, the minutes went back: nobody can change them", fieldIs("st-charging", "10"))
        assertTrue("$moment, the battery level went back: nobody can change it", fieldIs("st-floor", "30"))
        assertTrue("$moment, the mobile data stayed at its old value: nobody can allow any", fieldIs("st-allowance", "2"))
        assertTrue("$moment, the period went back to its old value", fieldIs("st-period", "day"))
    }

    /** The page reloads every two seconds, on a real phone over what was being typed. */
    @Test
    fun what_is_typed_in_the_settings_stays_and_apply_keeps_it() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.openSettings(compose)
        E2e.waitFor("the settings never showed the minutes") { E2e.visible("#st-charging") && !fieldIs("st-charging", "") }
        E2e.typeInto("st-charging", "10")
        E2e.typeInto("st-floor", "30")
        E2e.typeInto("st-allowance", "2")
        E2e.js("document.getElementById('st-period').value = 'day'")
        E2e.pause(5_000)
        assertTyped("five seconds after typing")
        E2e.tapId("apply")
        E2e.waitFor("Apply said nothing") { E2e.textOf("#st-said").isNotEmpty() }
        E2e.pause(5_000)
        assertTyped("five seconds after Apply")
    }

    /** An emulator's disk may hold under 10 GB, so the slider moves to 0, which always exists. */
    @Test
    fun the_space_slider_fills_the_field_and_the_button_answers() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.openSettings(compose)
        E2e.waitFor("the settings never showed the space slider") { E2e.visible("#st-pledge-slider") }
        E2e.js("document.getElementById('st-pledge-slider').value = '0'")
        E2e.js("document.getElementById('st-pledge-slider').dispatchEvent(new Event('input'))")
        E2e.waitFor("moving the slider did not reach the field, so Apply would change nothing") { fieldIs("st-pledge", "0") }
        assertTrue("the slider does not say what the offer earns", E2e.textOf("#st-pledge-earns").isNotEmpty())
        E2e.tapId("st-pledge-same")
        E2e.waitFor("the default button said nothing") { E2e.textOf("#st-pledge-said").isNotEmpty() }
    }

    /** The button is the way back when the once-a-day offer went wrong. */
    @Test
    fun check_for_updates_reaches_the_updater() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.openSettings(compose)
        val before = Updater.askedChecks
        E2e.tapId("st-check-update")
        E2e.waitFor("the update button did nothing, so a person who missed the offer waits a day", 60) {
            Updater.askedChecks > before
        }
    }

    private fun chooseTheme(word: String) {
        E2e.js(
            "(function(){var s=document.getElementById('st-theme');s.value='$word';" +
                "s.dispatchEvent(new Event('change',{bubbles:true}));return s.value})()",
        )
    }

    @Test
    fun the_theme_changes_the_screen_at_once_and_follows_the_phone() {
        Flows.createAccount(compose, Flows.freshName())
        // The file list is drawn by the app, not the page: it must follow the phone too.
        E2e.shell("cmd uimode night yes")
        E2e.waitFor("the file list stayed light when the phone went to night mode") { E2e.dark() }
        E2e.shell("cmd uimode night no")
        E2e.waitFor("the file list stayed dark when the phone went back to day") { !E2e.dark() }
        Flows.openSettings(compose)
        E2e.waitFor("Settings is wider than the phone: its right side cannot be read without scrolling sideways") {
            E2e.js("document.documentElement.scrollWidth<=window.innerWidth") == "true"
        }
        chooseTheme("dark")
        E2e.waitFor("Dark did not darken the screen") { E2e.dark() }
        E2e.shot("41-theme-dark")
        chooseTheme("light")
        E2e.waitFor("Light did not lighten the screen") { !E2e.dark() }
        E2e.shot("42-theme-light")
        chooseTheme("system")
        E2e.shell("cmd uimode night yes")
        E2e.waitFor("Same as the system did not follow the phone into night mode") { E2e.dark() }
        E2e.shell("cmd uimode night no")
        E2e.waitFor("Same as the system did not follow the phone back to day") { !E2e.dark() }
    }
}
