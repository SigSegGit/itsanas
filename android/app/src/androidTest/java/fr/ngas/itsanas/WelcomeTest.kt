package fr.ngas.itsanas

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/** Scenario 1: a phone with nothing on it opens on Welcome, then the account question. */
@RunWith(AndroidJUnit4::class)
class WelcomeTest {
    @Before fun before() = E2e.begin()

    @After fun after() = E2e.end()

    @Test
    fun welcome_then_the_account_question_with_the_existing_account_first() {
        E2e.waitFor("a phone with no account never showed Welcome, so a new user sees a blank screen") {
            E2e.onSection("welcome")
        }
        E2e.shotBoth("01-welcome")

        // A phone has no data folder or setup name to show.
        assertTrue(
            "Welcome shows a data folder and a setup name, which a phone has no use for",
            !E2e.visible("#machine-home") && !E2e.visible("#machine-instance"),
        )

        E2e.tap("section[data-page=welcome] [data-next]")
        E2e.waitFor("Start did nothing, so nobody gets past Welcome") { E2e.onSection("account") }
        E2e.shotBoth("02-account")

        // The existing-account card is above the new-account card.
        val order = E2e.js(
            "(function(){var j=document.querySelector('input[name=account][value=join]');" +
                "var n=document.querySelector('input[name=account][value=new]');" +
                "return j.getBoundingClientRect().top < n.getBoundingClientRect().top && " +
                "!!(j.compareDocumentPosition(n) & Node.DOCUMENT_POSITION_FOLLOWING)})()",
        )
        assertTrue(
            "the new-account choice is above the existing-account one, so a person who already has an " +
                "account is steered into making a second one",
            order == "true",
        )
    }
}
