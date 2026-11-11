package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
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

    @After fun after() = E2e.end()

    @Test
    fun a_file_no_other_machine_proved_it_holds_stays_on_the_phone() {
        Flows.createAccount(compose, Flows.freshName())
        val name = Flows.addFile(compose)

        Flows.click(compose, "file-free")
        E2e.waitFor("freeing said nothing, or let the file go") {
            runCatching { Flows.textOf(compose, "files-told").contains("Kept here") }.getOrDefault(false)
        }
        assertTrue(
            "the phone let go of a file no other machine holds: it is lost",
            Native.list().contains("\"here\":true") && Native.list().contains(name),
        )
    }
}
