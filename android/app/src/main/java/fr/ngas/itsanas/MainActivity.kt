package fr.ngas.itsanas

import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.fragment.app.FragmentActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The phone's thin host. Everything an account is or does -- first run,
 * recovery, settings, devices, space -- is the page the engine serves, the same
 * as on a PC; this draws only what a page cannot: the screens that ask for a
 * secret, Unlock, the files, the update offer.
 */
// A FragmentActivity because the biometric prompt is a fragment.
class MainActivity : FragmentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        Appearance.load(this)
        Appearance.phoneDark = isNight(resources.configuration)
        super.onCreate(savedInstanceState)
        Host.attach(applicationContext)
        // The Recents card would show a file list, or the words.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            setRecentsScreenshotEnabled(false)
        }
        val shared = incoming(intent)
        setContent { ITSaNASTheme { App(shared) } }
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        Appearance.phoneDark = isNight(newConfig)
    }

    private fun isNight(config: Configuration) =
        config.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES

    /**
     * Files another application shared into this one, if any.
     *
     * Two spellings because the typed accessors arrived in Android 13 and the
     * older ones are deprecated but not gone. Using only the new pair would
     * refuse to run below 33; using only the old pair warns and will one day
     * stop working. Both, chosen at runtime.
     */
    @Suppress("DEPRECATION")
    private fun incoming(intent: Intent?): List<Uri> = when (intent?.action) {
        Intent.ACTION_SEND -> {
            val stream = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
            }
            if (stream != null) listOf(stream).filter(::foreign) else listOfNotNull(sharedText(intent))
        }

        Intent.ACTION_SEND_MULTIPLE ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
            }.orEmpty().filter(::foreign)

        else -> emptyList()
    }

    /**
     * A shared stream another application serves: `content://`, and not this
     * application's own provider. A `file://` stream, or one of our own URIs,
     * would have had this application read its own private files into the
     * account at another application's request.
     */
    private fun foreign(uri: Uri): Boolean =
        uri.scheme == android.content.ContentResolver.SCHEME_CONTENT && uri.authority != "$packageName.files"

    /**
     * A share that carries words and no file -- a link from the browser, a
     * passage of text -- kept as a note, as a storage app does. Without this
     * the app opened and nothing came in, with nothing said.
     */
    private fun sharedText(intent: Intent): Uri? {
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.takeIf { it.isNotBlank() }
            ?: return null
        val stamp = java.text.SimpleDateFormat("yyyy-MM-dd HH-mm-ss", java.util.Locale.ROOT).format(java.util.Date())
        val dir = java.io.File(cacheDir, "shared-text").also { it.mkdirs() }
        val note = java.io.File(dir, "Note $stamp.txt")
        note.writeText(text)
        return Uri.fromFile(note)
    }
}

private sealed interface Screen {
    data object Opening : Screen
    data object Locked : Screen
    data object Biometric : Screen

    /** The engine's page: `setup` (first run) or `settings`. */
    data class Page(val url: String, val mode: String) : Screen
    data object Ready : Screen
}

@Composable
private fun App(shared: List<Uri>) {
    val context = LocalContext.current
    var screen by remember { mutableStateOf<Screen>(Screen.Opening) }
    var complaint by remember { mutableStateOf<String?>(null) }
    var offered by remember { mutableStateOf<Updater.Offer?>(null) }
    var offerAsked by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    suspend fun page(mode: String): Screen = withContext(Dispatchers.IO) {
        val home = Vault.home(context).also { it.mkdirs() }
        Screen.Page(Native.pageStart(home.absolutePath, mode, Biometric.available(context)), mode)
    }

    // Leaving the page: the screen changes first, so the engine's late
    // "closed" is not read as a page that ended by itself.
    fun leave(next: Screen) {
        screen = next
        // Not on another thread: it does not wait for the engine, and a queued stop
        // could land after the next page opened, and stop that one.
        runCatching { Native.pageStop() }
        Host.cancelAll()
    }

    // A newer release, asked at most once a day when the application opens:
    // installed by hand, it hears of one from nothing else (Updater). Quiet
    // when offline or when GitHub does not answer: a failure here is not news
    // worth a dialog at every launch.
    LaunchedEffect(Unit) {
        offered = Updater.dueOffer(context)
    }

    // Unlock without asking, when the passphrase was remembered. An application
    // that demands a long passphrase on every launch gets a short one.
    LaunchedEffect(Unit) {
        screen = try {
            if (!Account.exists(context)) {
                page("setup")
            } else if (Account.open) {
                // The service kept the process, and the node, alive.
                Screen.Ready
            } else if (Biometric.enabled(context)) {
                // The fingerprint or face check below decides.
                Screen.Biometric
            } else {
                val remembered = Vault.rememberedPassphrase(context)
                if (remembered == null) {
                    Screen.Locked
                } else {
                    Account.unlock(context, remembered)
                    SyncService.start(context)
                    Screen.Ready
                }
            }
        } catch (error: Throwable) {
            complaint = error.message
            Screen.Locked
        }
    }

    // What the page changed that this app also draws or keeps.
    LaunchedEffect(Unit) {
        Host.events.collect { (name, value) ->
            val current = screen
            try {
                when (name) {
                    "update" -> scope.launch {
                        try {
                            val offer = Updater.check()
                            if (offer == null) {
                                val latest = context.getString(R.string.update_none, BuildConfig.VERSION_NAME)
                                Toast.makeText(context, latest, Toast.LENGTH_LONG).show()
                            }
                            offerAsked = offer != null
                            offered = offer
                        } catch (e: Exception) {
                            complaint = context.getString(R.string.update_failed, e.message ?: "")
                        } finally {
                            Updater.askedChecks += 1
                        }
                    }
                    "closed" -> if (current is Screen.Page) {
                        screen = if (value == "done") {
                            // The engine leaves the node closed: open it here, or
                            // Ready would stand on an empty slot.
                            val remembered =
                                if (Account.open) null else Vault.rememberedPassphrase(context)
                            if (remembered != null) Account.unlock(context, remembered)
                            if (Account.open) {
                                SyncService.start(context)
                                Screen.Ready
                            } else {
                                Screen.Locked
                            }
                        } else {
                            // Nobody touched it for a while: a fresh page, a fresh key.
                            page(current.mode)
                        }
                    }

                    "signedOut" -> {
                        // Or a fingerprint would still open what was signed out of.
                        Biometric.disable(context)
                        Account.close(context)
                        leave(Screen.Locked)
                    }

                    "removed", "account_removed" -> {
                        Account.close(context)
                        Vault.forget(context)
                        Biometric.disable(context)
                        screen = page("setup")
                    }

                    "biometric" -> {
                        if (value == "on") {
                            val passphrase = Vault.rememberedPassphrase(context)
                            if (passphrase != null) {
                                Biometric.enable(context as FragmentActivity, passphrase) { on ->
                                    if (!on) complaint = context.getString(R.string.biometric_failed)
                                }
                            }
                        } else {
                            Biometric.disable(context)
                        }
                    }
                }
            } catch (error: Throwable) {
                complaint = error.message
                screen = Screen.Locked
            }
        }
    }

    Box(Modifier.fillMaxSize()) {
        when (val current = screen) {
            Screen.Opening -> Centred { CircularProgressIndicator() }
            Screen.Biometric -> {
                // Android can drop its prompt without a word, and a spinner alone
                // would then be a dead end.
                Centred {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        CircularProgressIndicator()
                        TextButton({ screen = Screen.Locked }) {
                            Text(stringResource(R.string.unlock_use_passphrase))
                        }
                    }
                }
                LaunchedEffect(Unit) {
                    val activity = context as FragmentActivity
                    Biometric.unlock(activity, { passphrase ->
                        scope.launch {
                            screen = try {
                                Account.unlock(context, passphrase)
                                SyncService.start(context)
                                Screen.Ready
                            } catch (error: Throwable) {
                                // What the fingerprint holds opened nothing: the passphrase
                                // was changed since. Typing it works; asking again never would.
                                Biometric.disable(context)
                                complaint = error.message
                                Screen.Locked
                            }
                        }
                    }, { screen = Screen.Locked })
                }
            }
            is Screen.Page -> Page(
                url = current.url,
                onLeave = {
                    if (current.mode == "setup") {
                        // Back from the first step: out of the app, nothing was made.
                        leave(Screen.Opening)
                        (context as FragmentActivity).finish()
                    } else {
                        leave(Screen.Ready)
                    }
                },
                modifier = Modifier.fillMaxSize().safeDrawingPadding(),
            )
            Screen.Locked -> Unlock(
                onOpen = { SyncService.start(context); screen = Screen.Ready },
                complain = { complaint = it },
            )
            Screen.Ready -> {
                Files(
                    shared,
                    onSettings = {
                        scope.launch {
                            try {
                                screen = page("settings")
                            } catch (error: Throwable) {
                                complaint = error.message
                            }
                        }
                    },
                ) { complaint = it }
                // Offered once, after the passphrase was typed and worked.
                var offer by remember { mutableStateOf(Biometric.shouldOffer(context)) }
                if (offer) {
                    AlertDialog(
                        onDismissRequest = { offer = false; Biometric.answered(context) },
                        title = { Text(stringResource(R.string.biometric_title)) },
                        text = { Text(stringResource(R.string.biometric_body)) },
                        confirmButton = {
                            TextButton({
                                offer = false
                                Biometric.answered(context)
                                val passphrase = Vault.rememberedPassphrase(context)
                                if (passphrase != null) {
                                    Biometric.enable(context as FragmentActivity, passphrase) { on ->
                                        if (!on) complaint = context.getString(R.string.biometric_failed)
                                    }
                                }
                            }) { Text(stringResource(R.string.biometric_on)) }
                        },
                        dismissButton = {
                            TextButton({ offer = false; Biometric.answered(context) }) {
                                Text(stringResource(R.string.biometric_not_now))
                            }
                        },
                    )
                }
            }
        }

        // A secret the engine is waiting for, over whatever is showing.
        Host.current?.let { AskOverlay(it) }
    }

    // Not over the fingerprint: answering one interrupted the other, on a real phone,
    // and the screen under them never came back. Settings asked for this one.
    offered?.takeIf { screen == Screen.Ready || (offerAsked && screen is Screen.Page) }?.let { offer ->
        AlertDialog(
            onDismissRequest = { offered = null },
            title = { Text(stringResource(R.string.update_title, offer.version)) },
            text = { Text(stringResource(R.string.update_body, BuildConfig.VERSION_NAME)) },
            confirmButton = {
                TextButton({
                    // Without the permission, Android's settings open and this stays
                    // for the person coming back: the offer is made once a day.
                    if (context.packageManager.canRequestPackageInstalls()) offered = null
                    scope.launch {
                        try {
                            Updater.install(context, offer)
                        } catch (e: Exception) {
                            complaint = context.getString(R.string.update_failed, e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.update_install)) }
            },
            dismissButton = {
                TextButton({ offered = null }) { Text(stringResource(R.string.update_later)) }
            },
        )
    }

    complaint?.let { message ->
        AlertDialog(
            onDismissRequest = { complaint = null },
            confirmButton = {
                TextButton({ complaint = null }, Modifier.testTag("complaint-close")) {
                    Text(stringResource(R.string.close))
                }
            },
            title = { Text(stringResource(R.string.did_not_work)) },
            text = { Text(message, Modifier.testTag("complaint")) },
        )
    }
}

@Composable
internal fun Centred(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { content() }
}
