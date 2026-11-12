package fr.ngas.itsanas

import android.app.Activity
import android.app.LocaleManager
import android.os.Build
import android.os.LocaleList
import android.os.ParcelFileDescriptor
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.test.core.app.ActivityScenario
import androidx.test.espresso.web.assertion.WebViewAssertions.webMatches
import androidx.test.espresso.web.sugar.Web.onWebView
import androidx.test.espresso.web.webdriver.DriverAtoms.findElement
import androidx.test.espresso.web.webdriver.DriverAtoms.webClick
import androidx.test.espresso.web.webdriver.DriverAtoms.webKeys
import androidx.test.espresso.web.webdriver.DriverAtoms.clearElement
import androidx.test.espresso.web.webdriver.Locator
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONTokener
import org.junit.Assert.fail
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * What every scenario of the end-to-end protocol shares: a phone with nothing on
 * it, pinned to the local coordinator the script started, the page driven by
 * taps and keys, and a screenshot at each step for somebody to look at.
 *
 * Nothing here talks to the official coordinator. A run without the
 * `coordinator` argument fails in [begin], saying so, before it can.
 */
object E2e {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    val context get() = instrumentation.targetContext

    var scenario: ActivityScenario<MainActivity>? = null
        private set

    /** Where the shots go: the script pulls this directory off the emulator. */
    private const val SHOTS = "/data/local/tmp/e2e"

    /**
     * The compose rule of a test that has one. Its test clock draws no frame
     * unless somebody advances it, so every wait below does: without that the
     * app stays on its first screen for ever.
     */
    private var compose: androidx.compose.ui.test.junit4.ComposeTestRule? = null

    /** Wait [millis], drawing frames meanwhile. */
    fun pause(millis: Long) {
        val end = System.nanoTime() + millis * 1_000_000L
        while (System.nanoTime() < end) {
            compose?.mainClock?.advanceTimeBy(100)
            Thread.sleep(100)
        }
    }

    /** Pin the coordinator, empty the phone, open the app. */
    fun begin(rule: androidx.compose.ui.test.junit4.ComposeTestRule? = null) {
        compose = rule
        val args = InstrumentationRegistry.getArguments()
        val address = args.getString("coordinator").orEmpty()
        if (address.isEmpty()) {
            fail(
                "no `coordinator` instrumentation argument: run scripts/android-e2e.sh, which starts a " +
                    "local coordinator. Without it this test would register names at the official " +
                    "coordinator, where they stay for good."
            )
        }
        val answer = Native.useCoordinatorForTests(address, args.getString("coordinatorDevice").orEmpty())
        check(answer.contains(address)) { "the test coordinator was not taken: $answer" }
        wipe()
        speakEnglish()
        scenario = ActivityScenario.launch(MainActivity::class.java)
    }

    /**
     * The scenarios read English. An emulator is English already; a person's own
     * phone speaks its owner's language, and every check of a text failed on a
     * French Galaxy S25 while the page said the right thing in French. The app's
     * own screens follow the app's language, the page the one kept in its home.
     */
    private fun speakEnglish() {
        if (Build.VERSION.SDK_INT >= 33) {
            context.getSystemService(LocaleManager::class.java).applicationLocales =
                LocaleList.forLanguageTags("en")
        }
        val home = Vault.home(context)
        home.mkdirs()
        File(home, "language").writeText("en\n")
        // A real phone has a fingerprint, so the app offers it once an account
        // opens; its dialog dims the screen behind, which read as a night mode
        // that never ended on a Galaxy S25. The emulator has none, so CI never
        // saw it. The offer is answered here; no scenario is about it.
        Biometric.answered(context)
    }

    fun end() {
        scenario?.close()
        scenario = null
        wipe()
    }

    /** A phone that has never seen ITSaNAS: no node, no remembered passphrase, no update question. */
    fun wipe() {
        runCatching { Native.pageStop() }
        // Host outlives a test: a question left waiting would be read as the next
        // test's, whose engine then replaces it with other words.
        Host.cancelAll()
        val end = System.nanoTime() + 5_000_000_000L
        while (Host.current != null && System.nanoTime() < end) Thread.sleep(50)
        runCatching { Native.close() }
        Account.markClosed()
        runCatching { SyncService.stop(context) }
        Vault.home(context).deleteRecursively()
        File(context.cacheDir, "opened").deleteRecursively()
        runCatching { Vault.forget(context) }
        runCatching { Biometric.disable(context) }
        // The launch-time question about a newer release would put a dialog over the page.
        context.getSharedPreferences("updates", android.content.Context.MODE_PRIVATE)
            .edit().putLong("asked", System.currentTimeMillis()).commit()
    }

    /** The phone emptied again and the app opened afresh; the coordinator keeps what it knows. */
    fun relaunch() {
        scenario?.close()
        wipe()
        speakEnglish()
        scenario = ActivityScenario.launch(MainActivity::class.java)
    }

    /** The app closed and opened again, the phone's data kept: a person coming back later. */
    fun reopen() {
        scenario?.close()
        runCatching { Native.pageStop() }
        scenario = ActivityScenario.launch(MainActivity::class.java)
    }

    fun shell(command: String): String {
        val fd: ParcelFileDescriptor = instrumentation.uiAutomation.executeShellCommand(command)
        return ParcelFileDescriptor.AutoCloseInputStream(fd).use { String(it.readBytes()) }
    }

    /** Brightness, 0 to 255, of one pixel of what is on the screen now. */
    fun brightness(xFraction: Double, yFraction: Double): Int {
        val bitmap = instrumentation.uiAutomation.takeScreenshot()
        val pixel = bitmap.getPixel((bitmap.width * xFraction).toInt(), (bitmap.height * yFraction).toInt())
        return (android.graphics.Color.red(pixel) + android.graphics.Color.green(pixel) + android.graphics.Color.blue(pixel)) / 3
    }

    /** Recreate the activity, as Android does after the app was left in the background. */
    fun recreate() {
        scenario!!.recreate()
    }

    // ------------------------------------------------------------------ page

    private fun findWebView(root: View): WebView? {
        if (root is WebView) return root
        if (root is ViewGroup) {
            for (i in 0 until root.childCount) findWebView(root.getChildAt(i))?.let { return it }
        }
        return null
    }

    /** Run [script] in the page; the answer is its JSON text, or null when there is no page yet. */
    fun js(script: String): String? {
        var view: WebView? = null
        scenario?.onActivity { activity ->
            view = findWebView(activity.window.decorView)
        }
        val web = view ?: return null
        val latch = CountDownLatch(1)
        var out: String? = null
        instrumentation.runOnMainSync {
            web.evaluateJavascript("(function(){try{return JSON.stringify($script)}catch(e){return JSON.stringify('ERR '+e)}})()") {
                out = it
                latch.countDown()
            }
        }
        latch.await(10, TimeUnit.SECONDS)
        // evaluateJavascript hands back the JSON string of a JSON string.
        return out?.let {
            val inner = JSONTokener(it).nextValue()
            if (inner is String) inner else it
        }
    }

    fun pageHas(script: String): Boolean = js(script) == "true"

    /** [section] is `welcome`, `account`, ... : the page's own `data-page`. */
    fun onSection(section: String): Boolean =
        pageHas("(function(){var e=document.querySelector('section[data-page=$section]');return !!e && e.getClientRects().length>0})()")

    fun textOf(css: String): String =
        js("(function(){var e=document.querySelector('$css');return e?e.textContent.trim():''})()")
            ?.let { JSONTokener(it).nextValue().toString() } ?: ""

    fun visible(css: String): Boolean =
        pageHas("(function(){var e=document.querySelector('$css');return !!e && e.getClientRects().length>0})()")

    fun enabled(css: String): Boolean =
        pageHas("(function(){var e=document.querySelector('$css');return !!e && !e.disabled && e.getAttribute('aria-disabled')!=='true'})()")

    /** Wait until [what] holds; otherwise fail with [consequence], naming what the page showed. */
    fun waitFor(consequence: String, seconds: Long = 30, what: () -> Boolean) {
        val end = System.nanoTime() + seconds * 1_000_000_000L
        var lastError: Throwable? = null
        while (System.nanoTime() < end) {
            compose?.mainClock?.advanceTimeBy(100)
            val tried = runCatching(what)
            if (tried.getOrDefault(false)) return
            lastError = tried.exceptionOrNull()
            Thread.sleep(250)
        }
        shot("FAILED-${consequence.take(40).replace(Regex("[^A-Za-z0-9]+"), "-")}")
        val said = js("document.getElementById('problem') ? document.getElementById('problem').textContent : ''")
        fail("$consequence (the page's problem line: ${said ?: "no page"}; the visible section: ${visibleSection()}; asked on screen: ${Host.current?.javaClass?.simpleName}; last error: ${lastError?.toString()?.take(300)})")
    }

    fun visibleSection(): String = js(
        "(function(){var s=[].slice.call(document.querySelectorAll('section[data-page]')).filter(function(e){return e.getClientRects().length>0});return s.map(function(e){return e.getAttribute('data-page')}).join(',')})()"
    ) ?: "no page"

    /** A tap on the page's own element, found by css. */
    fun tap(css: String) {
        reveal(css)
        onWebView().withElement(findElement(Locator.CSS_SELECTOR, css)).perform(webClick())
    }

    /**
     * For a click the page answers with window.confirm: a webClick looks for the
     * web view again while the dialog holds the window, and does not find it.
     */
    fun tapBehindConfirm(id: String) {
        reveal(id)
        js("(function(){setTimeout(function(){document.getElementById('$id').click()},0);return true})()")
    }

    /**
     * Open the Settings section an element sits in: a closed one hides its
     * controls, and a webClick on a hidden element fails.
     */
    fun reveal(idOrCss: String) {
        js("(function(){return window.itsanasReveal ? window.itsanasReveal('$idOrCss') : false})()")
    }

    fun tapId(id: String) {
        reveal(id)
        onWebView().withElement(findElement(Locator.ID, id)).perform(webClick())
    }

    /** Type into a field of the page, key by key, after emptying it. */
    fun typeInto(id: String, text: String) {
        reveal(id)
        onWebView().withElement(findElement(Locator.ID, id))
            .perform(clearElement())
            .perform(webKeys(text))
    }

    /** Kept for scenarios that assert on the DOM through Espresso rather than the script. */
    @Suppress("unused")
    fun matches(css: String, predicate: org.hamcrest.Matcher<String>) {
        onWebView().withElement(findElement(Locator.CSS_SELECTOR, css))
            .check(webMatches(androidx.test.espresso.web.webdriver.DriverAtoms.getText(), predicate))
    }

    // ----------------------------------------------------------------- shots

    /**
     * A screenshot named for what it shows. A screen that sets FLAG_SECURE (the
     * passphrase and the words) comes out black: that is the flag working, not
     * a bug, and those screens are checked through their text instead.
     */
    fun shot(name: String) {
        shell("mkdir -p $SHOTS")
        shell("screencap -p $SHOTS/$name.png")
    }

    /** Whether the screen is dark now, read from a pixel of the margin. */
    fun dark(): Boolean = brightness(0.03, 0.5) < 110

    /**
     * The same screen in the phone's light theme, then its dark one, then light again.
     * A plain sleep draws no frame under the test's clock: the screens this app draws
     * itself stayed light in every "dark" shot, and nobody could tell from the run.
     */
    fun shotBoth(name: String) {
        shot("$name-light")
        shell("cmd uimode night yes")
        waitFor("$name stayed light when the phone went to night mode, unreadable at night") { dark() }
        shot("$name-dark")
        shell("cmd uimode night no")
        waitFor("$name stayed dark when the phone went back to day") { !dark() }
    }
}
