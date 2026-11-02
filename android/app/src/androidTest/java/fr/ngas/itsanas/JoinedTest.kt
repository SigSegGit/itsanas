package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Scenarios 2 and 5: once an account exists the coordinator knows its name. A
 * second phone (the same phone, emptied) is told the name is taken when it
 * asks for a new account, and that the account is found when it asks to join.
 */
@RunWith(AndroidJUnit4::class)
class JoinedTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin()

    @After fun after() = E2e.end()

    @Test
    fun the_coordinator_knows_the_name_after_the_account_is_made() {
        val made = Flows.createAccount(compose, Flows.freshName())
        E2e.shotBoth("10-files-first")

        E2e.relaunch()
        Flows.toAccountStep()
        Flows.newAccountNamed(made.name)
        E2e.waitFor("a name the coordinator already holds was not called taken") {
            E2e.textOf("#username-check") == "This username is already taken. Choose another."
        }
        E2e.shotBoth("11-name-taken")
        Flows.nextOnAccount()
        Thread.sleep(1500)
        assertTrue(
            "Next went on with a taken name, so a second person would be handed the first one's account",
            E2e.onSection("account"),
        )

        // The same name as an existing account: found, and the space step offers a suggestion.
        E2e.tap("input[name=account][value=join]")
        E2e.waitFor("the existing account was not found by its name") {
            E2e.textOf("#username-check") == "Account found."
        }
        Flows.nextOnAccount()
        E2e.waitFor("Next did nothing for an account that was found") { E2e.onSection("folder") }
        E2e.waitFor("the space step has no number to offer") { E2e.js("document.getElementById('pledge').value")?.let { it != "\"\"" && it.isNotEmpty() } == true }
        E2e.shotBoth("12-space-join")
    }
}
