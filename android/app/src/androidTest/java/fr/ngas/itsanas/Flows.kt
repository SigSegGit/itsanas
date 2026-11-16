package fr.ngas.itsanas

import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.junit4.ComposeTestRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import androidx.test.espresso.Espresso.onView
import androidx.test.espresso.action.ViewActions.click
import androidx.test.espresso.matcher.RootMatchers.isDialog
import androidx.test.espresso.matcher.ViewMatchers.withText

/** The steps a person takes, shared by the scenarios, each as the screen presents it. */
object Flows {
    /** A test value, used only against the local coordinator's scratch state. */
    const val PASSPHRASE = "e2e passphrase one"
    const val OTHER_PASSPHRASE = "e2e passphrase two"

    private const val RETAP_NANOS = 5_000_000_000L

    /** A name the coordinator has not seen: lowercase letters and digits only. */
    fun freshName() = "e2e" + (System.nanoTime() % 100_000_000_000L)

    class Made(val name: String, val phrase: String)

    fun string(id: Int): String = E2e.context.getString(id)

    fun has(compose: ComposeTestRule, tag: String): Boolean =
        compose.onAllNodesWithTag(tag).fetchSemanticsNodes().isNotEmpty()

    fun enabled(compose: ComposeTestRule, tag: String): Boolean =
        runCatching { compose.onNodeWithTag(tag).assertIsEnabled(); true }.getOrDefault(false)

    fun waitTag(compose: ComposeTestRule, tag: String, why: String, seconds: Long = 30) {
        E2e.waitFor(why, seconds) { has(compose, tag) }
    }

    fun waitNoTag(compose: ComposeTestRule, tag: String, why: String, seconds: Long = 60) {
        E2e.waitFor(why, seconds) { !has(compose, tag) }
    }

    fun type(compose: ComposeTestRule, tag: String, text: String) {
        runCatching { compose.onNodeWithTag(tag).performScrollTo() }
        compose.onNodeWithTag(tag).performTextInput(text)
    }

    /** A tap on what a person would see: scrolled into view first, because the touch lands where the node is. */
    fun click(compose: ComposeTestRule, tag: String) {
        runCatching { compose.onNodeWithTag(tag).performScrollTo() }
        compose.onNodeWithTag(tag).performClick()
    }

    /** What a node shows, as one string. */
    /** What a field shows as typed (its label is Text too), else the node's text. */
    fun textOf(compose: ComposeTestRule, tag: String): String {
        val config = compose.onNodeWithTag(tag).fetchSemanticsNode().config
        return config.getOrNull(SemanticsProperties.EditableText)?.text
            ?: config.getOrNull(SemanticsProperties.Text)?.joinToString("") { it.text }
            ?: ""
    }

    // ------------------------------------------------------------------ page

    fun toAccountStep() {
        E2e.waitFor("Welcome never appeared on a phone with no account") { E2e.onSection("welcome") }
        E2e.tap("section[data-page=welcome] [data-next]")
        E2e.waitFor("Start on Welcome did nothing") { E2e.onSection("account") }
        E2e.waitFor("the second step does not say 2: a person cannot tell how far along they are") {
            E2e.js(
                "getComputedStyle(document.querySelector('#progress li[aria-current=step]'),'::before').content",
            ).orEmpty().contains("2")
        }
    }

    /** Pick "new account" and type a name; returns once the coordinator has said something. */
    fun newAccountNamed(name: String) {
        E2e.tap("input[name=account][value=new]")
        E2e.typeInto("username", name)
        E2e.waitFor("the coordinator never answered the name check") {
            E2e.textOf("#username-check").isNotEmpty()
        }
    }

    fun nextOnAccount() = E2e.tap("section[data-page=account] [data-next]")

    // ---------------------------------------------------------------- secrets

    /** From a fresh phone to the 24 words, which the engine asks for before any passphrase. */
    fun toWords(compose: ComposeTestRule, name: String): Ask.Confirm {
        toAccountStep()
        newAccountNamed(name)
        E2e.waitFor("a free name was not accepted") { E2e.textOf("#username-check") == "This username is free." }
        nextOnAccount()
        E2e.waitFor("Next on the account step did nothing") { E2e.onSection("folder") }
        E2e.waitFor("Start setting up never became available") { E2e.enabled("#start") }
        E2e.tapId("start")
        E2e.waitFor("the 24 words never appeared, so no account can be made") { Host.current is Ask.Confirm }
        waitTag(compose, "words-have-them", "the words screen is not drawn")
        return Host.current as Ask.Confirm
    }

    fun typePassphrase(compose: ComposeTestRule, pass: String) {
        type(compose, "new-pass", pass)
        type(compose, "new-pass-again", pass)
    }

    /** Words proved: the engine now asks for the passphrase. */
    fun toPassphraseScreen(compose: ComposeTestRule, name: String): Ask.Confirm {
        val ask = toWords(compose, name)
        click(compose, "words-have-them")
        waitTag(compose, "check-confirm", "the typing-back screen never opened")
        typeCheckedWords(compose, ask)
        // The run's first test types while the keyboard is first opening, and twice
        // in that spot the Confirm tap changed nothing. It says so in the log, with
        // whether the button was even enabled, so a real fault stays readable.
        var tapped = System.nanoTime()
        E2e.waitFor("the passphrase screen never opened after the words were proved, so no account can be made") {
            val open = has(compose, "new-pass")
            if (!open && System.nanoTime() - tapped > RETAP_NANOS && has(compose, "check-confirm") && !has(compose, "secret-error")) {
                android.util.Log.w("e2e", "the Confirm tap did nothing; enabled=${enabled(compose, "check-confirm")}")
                click(compose, "check-confirm")
                tapped = System.nanoTime()
            }
            open
        }
        return ask
    }

    fun typeCheckedWords(compose: ComposeTestRule, ask: Ask.Confirm, wrong: Boolean = false) {
        val words = ask.phrase.trim().split(Regex("\\s+"))
        ask.positions.forEachIndexed { index, position ->
            type(compose, "check-$index", if (wrong) "zzzzzz" else words[position - 1])
        }
        click(compose, "check-confirm")
    }

    /** A whole new account, ending on the file list. */
    fun createAccount(compose: ComposeTestRule, name: String, pass: String = PASSPHRASE): Made {
        val ask = toPassphraseScreen(compose, name)
        typePassphrase(compose, pass)
        click(compose, "secret-continue")
        E2e.waitFor("setup never reached its final check after the words were typed back", 240) {
            E2e.onSection("final")
        }
        E2e.shotBoth("10-final")
        if (E2e.visible("#final-fix")) {
            org.junit.Assert.fail(
                "setup failed on a phone: ${E2e.textOf("#final-title")} / ${E2e.textOf("#final-fix")} / ${E2e.textOf("#report")}"
            )
        }
        E2e.waitFor("Close on the final check is not there") { E2e.visible("#finish") }
        E2e.tapId("finish")
        waitTag(compose, "files-add", "the file list never opened after setup, so the account is unusable", 90)
        return Made(name, ask.phrase)
    }

    // --------------------------------------------------------------- settings

    fun openSettings(compose: ComposeTestRule) {
        click(compose, "files-settings")
        E2e.waitFor("Settings never opened") { E2e.onSection("settings") }
    }

    fun okInDialog() {
        onView(withText(android.R.string.ok)).inRoot(isDialog()).perform(click())
    }

    fun closeComplaint(compose: ComposeTestRule) {
        waitTag(compose, "complaint", "no refusal was shown")
        // Drawn by Compose, so Espresso's view matchers cannot see its button.
        click(compose, "complaint-close")
        waitNoTag(compose, "complaint", "the refusal could not be closed")
    }

    /** Settings, then Sign out, and the confirmation: the Unlock screen follows. */
    fun lock(compose: ComposeTestRule) {
        if (!E2e.onSection("settings")) openSettings(compose)
        E2e.waitFor("Sign out is not available") { E2e.enabled("#signout") }
        E2e.tapBehindConfirm("signout")
        okInDialog()
        waitTag(compose, "unlock-pass", "signing out did not lock the app, so the account stays open")
    }

    fun unlock(compose: ComposeTestRule, pass: String) {
        type(compose, "unlock-pass", pass)
        click(compose, "unlock-open")
    }

    /** Add a real file through the stubbed system picker; returns its name. */
    fun addFile(compose: ComposeTestRule, name: String = "e2e-note.txt"): String {
        val dir = java.io.File(E2e.context.cacheDir, "e2e-pick").apply { mkdirs() }
        val file = java.io.File(dir, name).apply { writeText("hello from the protocol") }
        androidx.test.espresso.intent.Intents.init()
        androidx.test.espresso.intent.Intents.intending(
            androidx.test.espresso.intent.matcher.IntentMatchers.hasAction(android.content.Intent.ACTION_OPEN_DOCUMENT),
        ).respondWith(
            android.app.Instrumentation.ActivityResult(
                android.app.Activity.RESULT_OK,
                android.content.Intent().setData(android.net.Uri.fromFile(file)),
            ),
        )
        click(compose, "files-add")
        waitTag(compose, "file-row", "the file chosen with + was never listed", 90)
        return name
    }

    /** The words typed into the recovery screen, which checks them as they come. */
    fun typeRecovery(compose: ComposeTestRule, phrase: String) {
        waitTag(compose, "recovery-words", "the screen asking for the 24 words never opened")
        type(compose, "recovery-words", phrase)
    }

    /**
     * Continue on the words screen until the screen is gone: on the emulator a
     * tap right after the words were checked was lost now and then, and the test
     * waited for a passphrase screen that nothing had asked for.
     */
    fun continueAfterWords(compose: ComposeTestRule) {
        repeat(3) {
            click(compose, "secret-continue")
            val end = System.nanoTime() + 10_000_000_000L
            while (System.nanoTime() < end) {
                if (Host.current !is Ask.Recovery) return
                E2e.pause(200)
            }
        }
    }

    /** The 24 words on a words screen, in order. */
    fun wordsShown(compose: ComposeTestRule): List<String> =
        (1..24).map { textOf(compose, "word-$it").substringAfter(". ") }
}
