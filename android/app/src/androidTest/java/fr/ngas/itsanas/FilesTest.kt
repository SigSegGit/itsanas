package fr.ngas.itsanas

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.net.Uri
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.test.espresso.intent.Intents
import androidx.test.espresso.intent.matcher.IntentMatchers.hasAction
import androidx.test.espresso.intent.Intents.intending
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Scenario 6: Files. + opens the system's picker, and the file chosen is listed: the crash of 0.1.14. */
@RunWith(AndroidJUnit4::class)
class FilesTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    @After fun after() {
        runCatching { Intents.release() }
        E2e.end()
    }

    /** A real file for the stubbed picker to hand back. */
    private fun pickable(name: String, text: String): Uri {
        val dir = File(E2e.context.cacheDir, "e2e-pick").apply { mkdirs() }
        val file = File(dir, name)
        file.writeText(text)
        return Uri.fromFile(file)
    }

    /** The real walk, against a real provider (`E2eDocuments`, debug builds only). */
    @Test
    fun a_chosen_folder_reaches_the_account_without_its_hidden_files() {
        Flows.createAccount(compose, Flows.freshName())
        val top = File(E2e.context.cacheDir, "e2e-tree/Camera").apply { deleteRecursively() }
        File(top, "trip").mkdirs()
        File(top, "a.jpg").writeText("first photo")
        File(top, "trip/b.jpg").writeText("second photo")
        File(top, ".pending-c.jpg").writeText("a photo still being taken")

        Intents.init()
        val tree = android.provider.DocumentsContract.buildTreeDocumentUri(
            "${E2e.context.packageName}.e2edocs",
            "Camera",
        )
        intending(hasAction(Intent.ACTION_OPEN_DOCUMENT_TREE)).respondWith(
            Instrumentation.ActivityResult(Activity.RESULT_OK, Intent().setData(tree)),
        )
        Flows.click(compose, "phone-folder")
        Flows.waitTag(compose, "phone-folder-line", "the folder chosen is not shown as chosen")
        compose.waitUntil(60_000) {
            val listed = Native.list()
            listed.contains("Phone/Camera/a.jpg") && listed.contains("Phone/Camera/trip/b.jpg")
        }
        assertTrue(
            "a photo still being taken reached the account, and from there every machine",
            !Native.list().contains("pending"),
        )
        E2e.shotBoth("22-files-folder")

        Flows.click(compose, "phone-folder-stop")
        Flows.waitNoTag(compose, "phone-folder-line", "Stop did not stop the folder")
        assertTrue(
            "stopping the folder took its files out of the account",
            Native.list().contains("Phone/Camera/a.jpg"),
        )
    }

    @Test
    fun plus_adds_a_real_file_and_it_is_listed() {
        Flows.createAccount(compose, Flows.freshName())
        E2e.shotBoth("20-files-empty")
        Flows.waitNoTag(compose, "not-safe", "an empty account is told its files are not safe")

        Intents.init()
        val name = "e2e-note.txt"
        intending(hasAction(Intent.ACTION_OPEN_DOCUMENT)).respondWith(
            Instrumentation.ActivityResult(Activity.RESULT_OK, Intent().setData(pickable(name, "hello from the protocol"))),
        )
        Flows.click(compose, "files-add")
        Flows.waitTag(compose, "file-row", "the file chosen with + was never listed, so adding a file does nothing", 90)
        Flows.waitTag(compose, "not-safe", "a file on this phone alone and no warning: losing the phone loses it, unwarned")
        E2e.shotBoth("21-files-listed")
        val rows = compose.onAllNodesWithTag("file-row").fetchSemanticsNodes().size
        assertTrue("expected exactly one row, saw $rows", rows == 1)
        // The row carries the file's name.
        assertTrue(
            "the row does not name the file, so the person cannot tell what was added",
            compose.onAllNodesWithText(name, substring = true).fetchSemanticsNodes().isNotEmpty(),
        )
    }
}
