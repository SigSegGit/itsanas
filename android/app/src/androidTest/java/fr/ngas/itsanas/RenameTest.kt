package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.performTouchInput
import androidx.test.espresso.intent.Intents
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** A file chosen on the Files screen takes a new name, and a slash moves it into a folder. */
@RunWith(AndroidJUnit4::class)
class RenameTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    // `Flows.addFile` stubs the document picker through Intents.
    @After fun after() {
        runCatching { Intents.release() }
        E2e.end()
    }

    @Test
    fun a_chosen_file_renamed_into_a_folder_leaves_its_old_name() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.addFile(compose, "draft.txt")
        compose.onAllNodesWithTag("file-row")[0].performTouchInput { longClick(percentOffset(0.1f, 0.5f)) }
        Flows.waitTag(compose, "files-rename-chosen", "one file chosen offers no Rename", 10)
        Flows.click(compose, "files-rename-chosen")
        Flows.waitTag(compose, "rename-name", "Rename asked for no new name", 10)
        compose.onNodeWithTag("rename-name").performTextClearance()
        Flows.type(compose, "rename-name", "notes/final.txt")
        Flows.click(compose, "rename-confirm")

        E2e.waitFor("the file never took its new name") { Native.list().contains("notes/final.txt") }
        val listed = Native.list()
        assertFalse(
            "the old name is still listed beside the new: a rename doubled the file ($listed)",
            listed.contains("\"draft.txt\""),
        )
    }
}
