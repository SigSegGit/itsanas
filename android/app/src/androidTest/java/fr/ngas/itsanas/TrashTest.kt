package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Deleting puts a file in the trash; it comes back, or leaves for good on a second answer. */
@RunWith(AndroidJUnit4::class)
class TrashTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    @After fun after() = E2e.end()

    private fun deleteOnScreen() {
        Flows.click(compose, "file-delete")
        Flows.waitTag(compose, "delete-confirm", "Delete asked nothing")
        Flows.click(compose, "delete-confirm")
        Flows.waitNoTag(compose, "file-row", "a file in the trash is still listed among the files", 120)
    }

    /** A tap right after another dialog closed can land on nothing: tap again if nothing opened. */
    private fun openTrash() {
        var tapped = 0L
        E2e.waitFor("the trash never opened") {
            val open = Flows.has(compose, "trash-close")
            if (!open && System.nanoTime() - tapped > 3_000_000_000L) {
                Flows.click(compose, "trash-open")
                tapped = System.nanoTime()
            }
            open
        }
    }

    @Test
    fun a_deleted_file_can_be_restored_and_only_a_second_answer_removes_it() {
        Flows.createAccount(compose, Flows.freshName())
        val name = Flows.addFile(compose)

        deleteOnScreen()
        assertFalse("a file in the trash is still in the listing", Native.list().contains(name))
        assertTrue("the trash lost the file", Native.trashList().contains(name))

        openTrash()
        Flows.waitTag(compose, "trash-restore", "the trash does not offer the file back")
        Flows.click(compose, "trash-restore")
        Flows.waitNoTag(compose, "trash-restore", "restoring did nothing")
        Flows.click(compose, "trash-close")
        Flows.waitTag(compose, "file-row", "a restored file did not come back", 120)
        assertTrue("a restored file is not in the account", Native.list().contains(name))

        deleteOnScreen()
        openTrash()
        Flows.waitTag(compose, "trash-delete", "the trash does not offer to delete the file")
        Flows.click(compose, "trash-delete")
        assertTrue("a file is still in the trash before the second answer", Native.trashList().contains(name))
        Flows.waitTag(compose, "trash-delete-confirm", "deleting from the network asked no confirmation")
        Flows.click(compose, "trash-delete-confirm")
        Flows.waitNoTag(compose, "trash-delete", "deleting from the network did nothing", 120)
        assertFalse("a file deleted for good is still in the trash", Native.trashList().contains(name))
        Flows.click(compose, "trash-close")
        Flows.waitNoTag(compose, "file-row", "a file deleted for good came back")
    }
}
