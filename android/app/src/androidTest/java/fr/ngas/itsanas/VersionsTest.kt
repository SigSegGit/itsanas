package fr.ngas.itsanas

import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.performTouchInput
import androidx.test.espresso.intent.Intents
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** A file added twice lists two versions, and the earlier one comes back from the dialog. */
@RunWith(AndroidJUnit4::class)
class VersionsTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    @After fun after() {
        runCatching { Intents.release() }
        E2e.end()
    }

    @Test
    fun an_earlier_version_comes_back_from_the_versions_dialog() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.addFile(compose, "plan.txt")
        Intents.release()
        // The same name again: a second version of the same file.
        Thread.sleep(1100)
        Flows.addFile(compose, "plan.txt")
        E2e.waitFor("adding a file again made no second version") {
            org.json.JSONObject(Native.versions("plan.txt")).getJSONArray("versions").length() == 2
        }

        compose.onAllNodesWithTag("file-row")[0].performTouchInput { longClick(percentOffset(0.1f, 0.5f)) }
        Flows.waitTag(compose, "files-versions-chosen", "one file chosen offers no Versions", 10)
        Flows.click(compose, "files-versions-chosen")
        Flows.waitTag(compose, "version-back-1", "the earlier version offers no Bring back", 30)
        Flows.click(compose, "version-back-1")

        E2e.waitFor("bringing a version back wrote nothing") {
            org.json.JSONObject(Native.versions("plan.txt")).getJSONArray("versions").length() == 3
        }
        val newest = org.json.JSONObject(Native.versions("plan.txt")).getJSONArray("versions").getJSONObject(0)
        assertEquals("the version brought back is not the file now", true, newest.getBoolean("current"))
    }
}
