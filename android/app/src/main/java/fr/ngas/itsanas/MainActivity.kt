package fr.ngas.itsanas

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.OpenableColumns
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import kotlinx.coroutines.launch

/**
 * The whole application.
 *
 * Four states and no navigation library: this has one screen with a settings
 * sheet, and a router would be scaffolding around a decision nobody has to
 * make. Everything sizes itself — no fixed widths, one column that fills
 * whatever it is given — so a tall phone, a folded one and a tablet all work
 * without a layout each.
 */
class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val shared = incoming(intent)
        setContent { MaterialTheme { App(shared) } }
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
    data object NoAccount : Screen
    data object Locked : Screen
    data class Phrase(val words: String) : Screen
    data object Ready : Screen
}

@Composable
private fun App(shared: List<Uri>) {
    val context = LocalContext.current
    var screen by remember { mutableStateOf<Screen>(Screen.Opening) }
    var complaint by remember { mutableStateOf<String?>(null) }

    // Unlock without asking, when the passphrase was remembered. An application
    // that demands a long passphrase on every launch gets a short one.
    LaunchedEffect(Unit) {
        screen = try {
            if (!Account.exists(context)) {
                Screen.NoAccount
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

    Box(Modifier.fillMaxSize()) {
        when (val current = screen) {
            Screen.Opening -> Centred { CircularProgressIndicator() }
            Screen.NoAccount -> FirstRun(
                onCreated = { screen = Screen.Phrase(it) },
                onRestored = { SyncService.start(context); screen = Screen.Ready },
                complain = { complaint = it },
            )
            Screen.Locked -> Unlock(
                onOpen = { SyncService.start(context); screen = Screen.Ready },
                complain = { complaint = it },
            )
            is Screen.Phrase -> RecoveryPhrase(current.words) {
                SyncService.start(context)
                screen = Screen.Ready
            }
            Screen.Ready -> Files(shared) { complaint = it }
        }
    }

    complaint?.let { message ->
        AlertDialog(
            onDismissRequest = { complaint = null },
            confirmButton = { TextButton({ complaint = null }) { Text("Close") } },
            title = { Text("That did not work") },
            text = { Text(message) },
        )
    }
}

@Composable
private fun Centred(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { content() }
}

@Composable
private fun FirstRun(
    onCreated: (String) -> Unit,
    onRestored: () -> Unit,
    complain: (String) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var username by remember { mutableStateOf("") }
    var passphrase by remember { mutableStateOf("") }
    var phrase by remember { mutableStateOf("") }
    var restoring by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("ITSaNAS", style = MaterialTheme.typography.headlineMedium)
        Text(
            if (restoring) {
                "Restore an account on this phone from its twenty-four words."
            } else {
                "Create an account. You will be shown twenty-four words once: " +
                    "they are your data, and nobody can give them back to you."
            },
            style = MaterialTheme.typography.bodyMedium,
        )

        OutlinedTextField(
            value = username,
            onValueChange = { username = it },
            label = { Text("Account name") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )

        if (restoring) {
            OutlinedTextField(
                value = phrase,
                onValueChange = { phrase = it },
                label = { Text("The twenty-four words") },
                minLines = 3,
                modifier = Modifier.fillMaxWidth(),
            )
        }

        OutlinedTextField(
            value = passphrase,
            onValueChange = { passphrase = it },
            label = { Text("Passphrase for this phone") },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            modifier = Modifier.fillMaxWidth(),
        )

        Button(
            onClick = {
                busy = true
                scope.launch {
                    try {
                        if (restoring) {
                            Account.login(context, username.trim(), phrase, passphrase)
                            onRestored()
                        } else {
                            onCreated(Account.create(context, username.trim(), passphrase))
                        }
                    } catch (error: Throwable) {
                        complain(error.message ?: "unknown failure")
                    } finally {
                        busy = false
                    }
                }
            },
            enabled = !busy && username.isNotBlank() && passphrase.length >= 8 &&
                (!restoring || phrase.trim().split(Regex("\\s+")).size == 24),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(if (restoring) "Restore" else "Create the account")
        }

        TextButton({ restoring = !restoring }, Modifier.fillMaxWidth()) {
            Text(if (restoring) "Create a new account instead" else "I already have an account")
        }

        if (busy) Centred { CircularProgressIndicator() }
    }
}

@Composable
private fun RecoveryPhrase(words: String, onAcknowledged: () -> Unit) {
    var written by remember { mutableStateOf(false) }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("Write these down", style = MaterialTheme.typography.headlineMedium)
        Text(
            "On paper, somewhere your house burning down would not reach. They " +
                "are shown once and stored nowhere. Anyone who has them can read " +
                "everything you store; without them, and without your passphrase, " +
                "every byte is gone.",
            style = MaterialTheme.typography.bodyMedium,
        )

        Card(Modifier.fillMaxWidth()) {
            Text(
                words,
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.bodyLarge,
                modifier = Modifier.padding(16.dp),
            )
        }

        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = written, onCheckedChange = { written = it })
            Spacer(Modifier.widthIn(min = 12.dp))
            Text("I have written them down")
        }

        Button(onAcknowledged, enabled = written, modifier = Modifier.fillMaxWidth()) {
            Text("Continue")
        }
    }
}

@Composable
private fun Unlock(onOpen: () -> Unit, complain: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var passphrase by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }

    Column(
        Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Unlock", style = MaterialTheme.typography.headlineMedium)
        OutlinedTextField(
            value = passphrase,
            onValueChange = { passphrase = it },
            label = { Text("Passphrase") },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            modifier = Modifier.fillMaxWidth(),
        )
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
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Open") }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Files(shared: List<Uri>, complain: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }

    var files by remember { mutableStateOf<List<Known>>(emptyList()) }
    var status by remember { mutableStateOf<Status?>(null) }
    var busy by remember { mutableStateOf(false) }
    var settings by remember { mutableStateOf(false) }
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
                snackbar.showSnackbar("${shared.size} file(s) added")
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
                                "${it.here} here · ${it.notHere} not here · " +
                                    humanSize(it.bytesOnDisk),
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
                    }) { Icon(Icons.Default.Refresh, "Sync now") }
                    IconButton({ settings = true }) {
                        Icon(Icons.Default.Settings, "Settings")
                    }
                },
            )
        },
        floatingActionButton = {
            FloatingActionButton({ picker.launch(arrayOf("*/*")) }) {
                Icon(Icons.Default.Add, "Add a file")
            }
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            if (busy) {
                CircularProgressIndicator(Modifier.fillMaxWidth().padding(8.dp))
            }

            if (files.isEmpty()) {
                Centred {
                    Text(
                        "Nothing here yet. Add a file, or sync with one of your machines.",
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
            title = { Text("Delete ${file.name}?") },
            text = {
                Text(
                    "It goes from every machine in this account, not just this " +
                        "phone. There is no undo."
                )
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
                }) { Text("Delete everywhere") }
            },
            dismissButton = { TextButton({ confirmDelete = null }) { Text("Keep it") } },
        )
    }

    if (settings) {
        Settings(
            status = status,
            onClose = { settings = false },
            onChanged = { scope.launch { refresh() } },
            complain = complain,
        )
    }
}

@Composable
private fun FileRow(file: Known, onOpen: () -> Unit, onDelete: () -> Unit) {
    Card(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
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
                        if (!file.here) append(" · not on this phone")
                    },
                    style = MaterialTheme.typography.labelSmall,
                )
            }
            IconButton(onOpen) {
                Icon(
                    if (file.here) Icons.Default.OpenInNew else Icons.Default.CloudDownload,
                    if (file.here) "Open" else "Fetch and open",
                )
            }
            IconButton(onDelete) { Icon(Icons.Default.Delete, "Delete") }
        }
    }
}

@Composable
private fun Settings(
    status: Status?,
    onClose: () -> Unit,
    onChanged: () -> Unit,
    complain: (String) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var peer by remember { mutableStateOf("") }
    var keepGiB by remember {
        mutableStateOf(status?.keepBytes?.let { (it / (1024 * 1024 * 1024)).toString() } ?: "")
    }
    var pledgeGiB by remember {
        mutableStateOf((status?.pledgeBytes?.div(1024 * 1024 * 1024) ?: 0L).toString())
    }
    var onMetered by remember { mutableStateOf(Vault.contentOnMetered(context)) }

    AlertDialog(
        onDismissRequest = onClose,
        confirmButton = { TextButton(onClose) { Text("Done") } },
        title = { Text("Settings") },
        text = {
            Column(
                Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                status?.let {
                    Text(
                        "This phone holds ${humanSize(it.bytesOnDisk)} of your own data " +
                            "and ${humanSize(it.vaultBytes)} for other people.",
                        style = MaterialTheme.typography.bodySmall,
                    )
                    Text(
                        if (it.onlyHere > 0) {
                            "${it.onlyHere} chunk(s) exist only on this phone."
                        } else {
                            "Every chunk this phone holds is on ${it.copiesElsewhere} " +
                                "other machine(s)."
                        },
                        style = MaterialTheme.typography.bodySmall,
                    )
                }

                Text("Machines to sync with", style = MaterialTheme.typography.titleSmall)
                status?.peers?.forEach { address ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(address, Modifier.weight(1f))
                        TextButton({
                            scope.launch {
                                try {
                                    Account.removePeer(address)
                                    onChanged()
                                } catch (error: Throwable) {
                                    complain(error.message ?: "could not remove it")
                                }
                            }
                        }) { Text("Forget") }
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = peer,
                        onValueChange = { peer = it },
                        label = { Text("host:port") },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton({
                        val address = peer
                        peer = ""
                        scope.launch {
                            try {
                                Account.addPeer(address)
                                onChanged()
                            } catch (error: Throwable) {
                                complain(error.message ?: "could not add it")
                            }
                        }
                    }) { Text("Add") }
                }

                Text("How much of your data to keep here", style = MaterialTheme.typography.titleSmall)
                Text(
                    "In gigabytes. Empty means all of it. What does not fit stays " +
                        "listed and downloads when you open it — most recently " +
                        "changed first.",
                    style = MaterialTheme.typography.bodySmall,
                )
                OutlinedTextField(
                    value = keepGiB,
                    onValueChange = { keepGiB = it.filter(Char::isDigit) },
                    label = { Text("GiB") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    modifier = Modifier.fillMaxWidth(),
                )

                Text("Space you offer other people", style = MaterialTheme.typography.titleSmall)
                OutlinedTextField(
                    value = pledgeGiB,
                    onValueChange = { pledgeGiB = it.filter(Char::isDigit) },
                    label = { Text("GiB") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    modifier = Modifier.fillMaxWidth(),
                )

                Row(verticalAlignment = Alignment.CenterVertically) {
                    Switch(
                        checked = onMetered,
                        onCheckedChange = {
                            onMetered = it
                            Vault.setContentOnMetered(context, it)
                        },
                    )
                    Spacer(Modifier.widthIn(min = 12.dp))
                    Column {
                        Text("Download over mobile data")
                        Text(
                            "Off by default. The file list still arrives — it costs " +
                                "kilobytes — and tapping a file always downloads it.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }

                Button(
                    onClick = {
                        scope.launch {
                            try {
                                val keep = keepGiB.toLongOrNull()
                                    ?.times(1024L * 1024 * 1024)
                                Account.setKeep(keep, "newest", emptyList())
                                Account.setPledge(
                                    (pledgeGiB.toLongOrNull() ?: 0L) * 1024 * 1024 * 1024
                                )
                                onChanged()
                            } catch (error: Throwable) {
                                complain(error.message ?: "could not save that")
                            }
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("Save limits") }

                Spacer(Modifier.height(8.dp))
                TextButton(
                    {
                        SyncService.stop(context)
                        Vault.forget(context)
                        scope.launch { Account.close() }
                        onClose()
                    },
                    Modifier.fillMaxWidth(),
                ) { Text("Lock this phone and stop syncing") }
            }
        },
    )
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
