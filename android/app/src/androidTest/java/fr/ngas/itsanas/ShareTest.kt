package fr.ngas.itsanas

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.net.Uri
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.click
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.percentOffset
import androidx.compose.ui.test.performTouchInput
import androidx.test.espresso.intent.Intents
import androidx.test.espresso.intent.Intents.intending
import androidx.test.espresso.intent.matcher.IntentMatchers.hasAction
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Sharing, both ways, as any storage app on the phone: a file of the account
 * goes out through the system's share sheet (messaging, mail, Bluetooth), and
 * what another app shares comes into the account. Never a public link.
 */
@RunWith(AndroidJUnit4::class)
class ShareTest {
    @get:Rule val compose = createEmptyComposeRule()

    @Before fun before() = E2e.begin(compose)

    @After fun after() {
        runCatching { Intents.release() }
        E2e.end()
    }

    @Test
    fun a_file_goes_out_through_the_systems_share_sheet_with_its_content() {
        Flows.createAccount(compose, Flows.freshName())
        val name = Flows.addFile(compose, "to-send.txt")
        intending(hasAction(Intent.ACTION_CHOOSER)).respondWith(
            Instrumentation.ActivityResult(Activity.RESULT_OK, null),
        )
        Flows.click(compose, "file-share")
        compose.waitUntil(30_000) { Intents.getIntents().any { it.action == Intent.ACTION_CHOOSER } }
        val chooser = Intents.getIntents().last { it.action == Intent.ACTION_CHOOSER }

        @Suppress("DEPRECATION")
        val sent = chooser.getParcelableExtra(Intent.EXTRA_INTENT) as? Intent
        assertEquals(
            "Share opened something other than the system's send: the file cannot reach a message or a mail",
            Intent.ACTION_SEND,
            sent?.action,
        )

        @Suppress("DEPRECATION")
        val stream = sent?.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
        assertTrue(
            "the app receiving the file is not allowed to read it, so it arrives empty",
            (sent?.flags ?: 0) and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0,
        )
        val text = stream?.let { uri ->
            E2e.context.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString() }
        }
        assertEquals("the file shared is not $name as the account holds it", "hello from the protocol", text)
    }

    @Test
    fun several_chosen_files_go_out_together_through_one_share_sheet() {
        Flows.createAccount(compose, Flows.freshName())
        Flows.addFile(compose, "first.txt")
        Intents.release()
        Flows.addFile(compose, "second.txt")
        compose.waitUntil(90_000) { compose.onAllNodesWithTag("file-row").fetchSemanticsNodes().size == 2 }
        intending(hasAction(Intent.ACTION_CHOOSER)).respondWith(
            Instrumentation.ActivityResult(Activity.RESULT_OK, null),
        )
        // On the name, left of the row's buttons.
        val rows = compose.onAllNodesWithTag("file-row")
        rows[0].performTouchInput { longClick(percentOffset(0.1f, 0.5f)) }
        compose.waitUntil(10_000) { compose.onAllNodesWithTag("files-share-chosen").fetchSemanticsNodes().isNotEmpty() }
        rows[1].performTouchInput { click(percentOffset(0.1f, 0.5f)) }
        compose.waitUntil(10_000) { compose.onAllNodesWithTag("file-chosen").fetchSemanticsNodes().size == 2 }
        Flows.click(compose, "files-share-chosen")
        compose.waitUntil(30_000) { Intents.getIntents().any { it.action == Intent.ACTION_CHOOSER } }
        val chooser = Intents.getIntents().last { it.action == Intent.ACTION_CHOOSER }

        @Suppress("DEPRECATION")
        val sent = chooser.getParcelableExtra(Intent.EXTRA_INTENT) as? Intent
        assertEquals(
            "two chosen files did not leave together: the person shares them one by one",
            Intent.ACTION_SEND_MULTIPLE,
            sent?.action,
        )
        assertTrue(
            "the app receiving the files is not allowed to read them, so they arrive empty",
            (sent?.flags ?: 0) and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0,
        )
        assertEquals("the grant does not cover every file sent", 2, sent?.clipData?.itemCount)

        @Suppress("DEPRECATION")
        val streams = sent?.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
        val texts = streams.map { uri ->
            E2e.context.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString() }
        }
        assertEquals(
            "the files shared are not the two the person chose, as the account holds them",
            listOf("hello from the protocol", "hello from the protocol"),
            texts,
        )
    }

    @Test
    fun words_another_app_shares_are_kept_in_the_account_as_a_note() {
        Flows.createAccount(compose, Flows.freshName())
        val before = Native.list()
        val link = "https://example.org/an-article-to-keep"
        val share = Intent(E2e.context, MainActivity::class.java)
            .setAction(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_TEXT, link)
        E2e.reopenWith(share)
        compose.waitUntil(90_000) { Native.list() != before && Native.list().contains("Note ") }
        val note = Regex("Note [0-9 -]+\\.txt").find(Native.list())?.value
        assertTrue("a link shared from the browser left no note in the account", note != null)
        val out = java.io.File(E2e.context.cacheDir, "e2e-note-back.txt")
        Native.get(note!!, out.absolutePath)
        assertEquals("the note does not hold the words shared", link, out.readText())
    }
}
