package fr.ngas.itsanas

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.CloudDownload
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.OpenInNew
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
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
        super.onCreate(savedInstanceState)
        Host.attach(applicationContext)
        // The Recents card would show a file list, or the words.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            setRecentsScreenshotEnabled(false)
        }
        val shared = incoming(intent)
        setContent { ITSaNASTheme { App(shared) } }
    }

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
        Intent.ACTION_SEND -> listOfNotNull(
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
            }
        )

        Intent.ACTION_SEND_MULTIPLE ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
            }.orEmpty()

        else -> emptyList()
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
            android.util.Log.i("itsanas", "router event $name=$value on ${current::class.simpleName}")
            try {
                when (name) {
                    "closed" -> if (current is Screen.Page) {
                        screen = if (value == "done") {
                            Account.markOpen()
                            SyncService.start(context)
                            Screen.Ready
                        } else {
                            // Nobody touched it for a while: a fresh page, a fresh key.
                            page(current.mode)
                        }
                    }

                    "signedOut" -> {
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
                Centred { CircularProgressIndicator() }
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

    offered?.let { offer ->
        AlertDialog(
            onDismissRequest = { offered = null },
            title = { Text(stringResource(R.string.update_title, offer.version)) },
            text = { Text(stringResource(R.string.update_body, BuildConfig.VERSION_NAME)) },
            confirmButton = {
                TextButton({
                    offered = null
                    scope.launch {
                        try {
                            if (!Updater.install(context, offer)) {
                                complaint = context.getString(R.string.update_allow)
                            }
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
                TextButton({ complaint = null }) { Text(stringResource(R.string.close)) }
            },
            title = { Text(stringResource(R.string.did_not_work)) },
            text = { Text(message, Modifier.testTag("complaint")) },
        )
    }
}

@Composable
private fun Centred(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { content() }
}

@Composable
private fun Unlock(onOpen: () -> Unit, complain: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var passphrase by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }

    SecureScreen()
    Column(
        Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(stringResource(R.string.unlock_title), style = MaterialTheme.typography.headlineMedium)
        PassphraseField(passphrase, { passphrase = it }, stringResource(R.string.passphrase_label), tag = "unlock-pass")
        Button(
            onClick = {
                busy = true
                scope.launch {
                    try {
                        Account.unlock(context, passphrase)
                        onOpen()
                    } catch (error: Throwable) {
                        complain(error.message ?: "unknown failure")
                    } finally {
                        busy = false
                    }
                }
            },
            enabled = !busy && passphrase.isNotEmpty(),
            modifier = Modifier.fillMaxWidth().testTag("unlock-open"),
        ) { Text(stringResource(R.string.unlock_open)) }
        TextButton(
            onClick = {
                busy = true
                scope.launch {
                    try {
                        // The 24 words and a new passphrase, asked on the app's own screens.
                        withContext(Dispatchers.IO) {
                            Native.forgot(Vault.home(context).absolutePath)
                        }
                        // The old passphrase behind a fingerprint is stale now.
                        Biometric.disable(context)
                        val remembered = Vault.rememberedPassphrase(context)
                        if (remembered != null) {
                            Account.unlock(context, remembered)
                            onOpen()
                        }
                    } catch (error: Throwable) {
                        complain(error.message ?: "unknown failure")
                    } finally {
                        busy = false
                    }
                }
            },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth().testTag("unlock-forgot"),
        ) { Text(stringResource(R.string.unlock_forgot)) }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Files(shared: List<Uri>, onSettings: () -> Unit, complain: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }

    var files by remember { mutableStateOf<List<Known>>(emptyList()) }
    var status by remember { mutableStateOf<Status?>(null) }
    var busy by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf<Known?>(null) }

    suspend fun refresh() {
        files = Account.files()
        status = Account.status()
    }

    LaunchedEffect(Unit) {
        try {
            refresh()
            // Anything shared into the application on this launch goes in
            // straight away: the person already chose, and asking again is a
            // second decision for the same intention.
            shared.forEach { uri ->
                val name = nameOf(context, uri)
                Account.add(context, uri, name, folder = "")
            }
            if (shared.isNotEmpty()) {
                refresh()
                snackbar.showSnackbar(context.getString(R.string.files_added, shared.size))
            }
        } catch (error: Throwable) {
            complain(error.message ?: "could not read the account")
        }
    }

    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocument()
    ) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            busy = true
            try {
                Account.add(context, uri, nameOf(context, uri), folder = "")
                refresh()
            } catch (error: Throwable) {
                complain(error.message ?: "could not add that file")
            } finally {
                busy = false
            }
        }
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(status?.username ?: "ITSaNAS")
                        status?.let {
                            Text(
                                stringResource(R.string.files_summary, it.here, it.notHere, humanSize(it.bytesOnDisk)),
                                style = MaterialTheme.typography.labelSmall,
                            )
                        }
                    }
                },
                actions = {
                    IconButton({
                        scope.launch {
                            busy = true
                            try {
                                val plan = Vault.plan(context, foreground = true)
                                Account.syncNow(metadataOnly = !plan.movesContent)
                                refresh()
                                snackbar.showSnackbar(plan.because)
                            } catch (error: Throwable) {
                                complain(error.message ?: "the round failed")
                            } finally {
                                busy = false
                            }
                        }
                    }) { Icon(Icons.Default.Refresh, stringResource(R.string.files_sync)) }
                    IconButton(onSettings, Modifier.testTag("files-settings")) {
                        Icon(Icons.Default.Settings, stringResource(R.string.files_settings))
                    }
                },
            )
        },
        floatingActionButton = {
            FloatingActionButton({ picker.launch(arrayOf("*/*")) }, Modifier.testTag("files-add")) {
                Icon(Icons.Default.Add, stringResource(R.string.files_add))
            }
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            if (busy) {
                LinearProgressIndicator(Modifier.fillMaxWidth().padding(8.dp))
            }

            if (files.isEmpty()) {
                Centred {
                    Text(
                        stringResource(R.string.files_empty),
                        Modifier.padding(32.dp),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }

            LazyColumn(Modifier.fillMaxSize()) {
                items(files, key = { it.path }) { file ->
                    FileRow(
                        file = file,
                        onOpen = {
                            scope.launch {
                                busy = true
                                try {
                                    val ready = Account.openable(context, file)
                                    share(context, ready)
                                    refresh()
                                } catch (error: Throwable) {
                                    complain(error.message ?: "could not open it")
                                } finally {
                                    busy = false
                                }
                            }
                        },
                        onDelete = { confirmDelete = file },
                    )
                }
            }
        }
    }

    confirmDelete?.let { file ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text(stringResource(R.string.delete_title, file.name)) },
            text = {
                Text(stringResource(R.string.delete_body))
            },
            confirmButton = {
                TextButton({
                    val doomed = file
                    confirmDelete = null
                    scope.launch {
                        try {
                            Account.remove(doomed.path)
                            refresh()
                        } catch (error: Throwable) {
                            complain(error.message ?: "could not delete it")
                        }
                    }
                }) { Text(stringResource(R.string.delete_everywhere)) }
            },
            dismissButton = { TextButton({ confirmDelete = null }) { Text(stringResource(R.string.delete_keep)) } },
        )
    }

}

@Composable
private fun FileRow(file: Known, onOpen: () -> Unit, onDelete: () -> Unit) {
    Card(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .testTag("file-row")
    ) {
        Row(
            Modifier.padding(12.dp).fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text(
                    file.name,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    buildString {
                        append(humanSize(file.size))
                        if (file.folder.isNotEmpty()) append(" · ${file.folder}")
                        if (!file.here) append(" · ${stringResource(R.string.file_not_here)}")
                    },
                    style = MaterialTheme.typography.labelSmall,
                )
            }
            IconButton(onOpen) {
                Icon(
                    if (file.here) Icons.Default.OpenInNew else Icons.Default.CloudDownload,
                    stringResource(if (file.here) R.string.file_open else R.string.file_fetch),
                )
            }
            IconButton(onDelete) { Icon(Icons.Default.Delete, stringResource(R.string.file_delete)) }
        }
    }
}

/** The name a picked document has, or something usable if it will not say. */
private fun nameOf(context: android.content.Context, uri: Uri): String {
    context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
        val column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (column >= 0 && cursor.moveToFirst()) {
            val name = cursor.getString(column)
            if (!name.isNullOrBlank()) return name
        }
    }
    return uri.lastPathSegment?.substringAfterLast('/') ?: "file"
}

/**
 * Hand a downloaded file to whatever the person wants to do with it.
 *
 * Viewing first, sharing as the fallback. A great many of the things people
 * keep have no viewer installed — an archive, a `.bin`, somebody's export —
 * and "No apps can perform this action" is a dead end for a file the person
 * can plainly see. Sharing always has somewhere to go: another application, a
 * messaging app, or the file manager's save.
 */
private fun share(context: android.content.Context, file: java.io.File) {
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val type = context.contentResolver.getType(uri) ?: "application/octet-stream"

    val view = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, type)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    val intent = if (view.resolveActivity(context.packageManager) != null) {
        view
    } else {
        Intent(Intent.ACTION_SEND).apply {
            putExtra(Intent.EXTRA_STREAM, uri)
            setType(type)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }

    context.startActivity(Intent.createChooser(intent, file.name))
}
