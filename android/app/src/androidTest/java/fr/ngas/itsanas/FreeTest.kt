package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.test.espresso.intent.Intents
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Freeing a file on the phone never lets go of the only copy anybody can prove. */
@RunWith(AndroidJUnit4::class)
class FreeTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    // `Flows.addFile` stubs the document picker through Intents: left
    // initialised, the next class to stub one fails on "init twice".
    @After fun after() {
        runCatching { Intents.release() }
        E2e.end()
    }

    @Test
    fun a_file_no_other_machine_proved_it_holds_stays_on_the_phone() {
        Flows.createAccount(compose, Flows.freshName())
        val name = Flows.addFile(compose)

        Flows.click(compose, "file-free")
        E2e.waitFor("freeing said nothing, or let the file go") {
            compose.onAllNodesWithText("Kept here", substring = true).fetchSemanticsNodes().isNotEmpty()
        }
        val listed = Native.list()
        assertTrue(
            "the phone let go of a file no other machine holds: it is lost ($listed)",
            listed.contains(name) && listed.contains("\"here\":true"),
        )
    }
}
