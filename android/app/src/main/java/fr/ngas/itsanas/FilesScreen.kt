package fr.ngas.itsanas

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Snackbar
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
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
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun Files(shared: List<Uri>, onSettings: () -> Unit, complain: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }

    // The folder on screen. The account answers with the nearest one still there.
    var level by remember { mutableStateOf(Level("", emptyList(), emptyList())) }
    var status by remember { mutableStateOf<Status?>(null) }
    var busy by remember { mutableStateOf(false) }
    // The files a delete asks about: one from its row, or every chosen one.
    var confirmDelete by remember { mutableStateOf(emptyList<Known>()) }
    var renaming by remember { mutableStateOf<Known?>(null) }
    var versionsOf by remember { mutableStateOf<Known?>(null) }
    var trashOpen by remember { mutableStateOf(false) }
    // Files chosen by a long press, to share together. Only files of the folder on screen.
    var chosen by remember { mutableStateOf(emptySet<String>()) }

    suspend fun refresh(folder: String = level.at) {
        level = Account.level(folder)
        chosen = chosen.filter { path -> level.files.any { it.path == path } }.toSet()
        status = Account.status()
        // What a delete does here, before the person deletes anything.
        runCatching { Trash.readRules() }
    }

    // A round now, as the button does: in the background the next one can be two
    // hours away, and a person who adds a file and leaves believes it has left too.
    // A round nobody asked for says why it failed without stopping them: the first
    // file on a first device has no machine to reach, and it was added all the same.
    suspend fun syncOnce(asked: Boolean = false) {
        // Its own coroutine: awaited, a line still showing when the person leaves is
        // cancelled inside these catches, and escapes to callers that catch everything.
        fun tell(line: String) {
            scope.launch { snackbar.showSnackbar(line) }
        }
        try {
            val plan = Vault.plan(context, foreground = true)
            val answer = org.json.JSONObject(Account.syncNow(metadataOnly = !plan.movesContent))
            refresh()
            tell(changesLine(context, answer) ?: plan.because)
        } catch (left: CancellationException) {
            // The person left this screen mid-round. That is no failure, and the
            // callers catching everything would put it in a dialog over the next one.
        } catch (error: Throwable) {
            val why = error.message ?: "the round failed"
            if (asked) complain(why) else tell(why)
        }
    }

    fun open(folder: String) {
        scope.launch {
            try {
                refresh(folder)
            } catch (error: Throwable) {
                complain(error.message ?: "could not read the account")
            }
        }
    }

    // Back climbs the folders before it leaves the application.
    BackHandler(level.at.isNotEmpty()) { open(level.at.substringBeforeLast('/', "")) }

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
                syncOnce()
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
                Account.add(context, uri, nameOf(context, uri), folder = level.at)
                refresh()
                syncOnce()
            } catch (error: Throwable) {
                complain(error.message ?: "could not add that file")
            } finally {
                busy = false
            }
        }
    }

    var folderName by remember { mutableStateOf(PhoneFolder.name(context)) }
    val folderPicker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree()
    ) { tree ->
        if (tree == null) return@rememberLauncherForActivityResult
        PhoneFolder.choose(context, tree)
        folderName = PhoneFolder.name(context)
        scope.launch {
            busy = true
            try {
                PhoneFolder.sync(context)
                refresh()
                syncOnce()
            } catch (error: Throwable) {
                complain(error.message ?: "could not read that folder")
            } finally {
                busy = false
            }
        }
    }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) { Snackbar(it, Modifier.testTag("files-told")) } },
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        // One line: with three buttons beside it, a long name wrapped
                        // and the line under it was cut in half (seen on a screenshot).
                        Text(
                            status?.username ?: "ITSaNAS",
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        status?.let {
                            Text(
                                stringResource(R.string.files_summary, it.here, it.notHere, humanSize(it.bytesOnDisk)),
                                style = MaterialTheme.typography.labelSmall,
                            )
                        }
                    }
                },
                actions = {
                    IconButton({ trashOpen = true }, Modifier.testTag("trash-open")) {
                        Icon(Icons.Default.Delete, stringResource(R.string.trash_title))
                    }
                    IconButton(
                        { folderPicker.launch(null) },
                        Modifier.testTag("phone-folder"),
                    ) {
                        Icon(Icons.Default.Folder, stringResource(R.string.phone_folder_choose))
                    }
                    IconButton({
                        scope.launch {
                            busy = true
                            try {
                                syncOnce(asked = true)
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
            status?.takeIf { !it.safe }?.let {
                Surface(color = MaterialTheme.colorScheme.tertiaryContainer) {
                    Text(
                        stringResource(R.string.not_safe_yet, it.copiesElsewhere, it.copiesWanted),
                        Modifier.fillMaxWidth().padding(16.dp).testTag("not-safe"),
                    )
                }
            }
            folderName?.let {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 16.dp).testTag("phone-folder-line"),
                    verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
                ) {
                    Text(stringResource(R.string.phone_folder_on, it), Modifier.weight(1f))
                    TextButton(
                        {
                            PhoneFolder.stop(context)
                            folderName = null
                        },
                        Modifier.testTag("phone-folder-stop"),
                    ) {
                        Text(stringResource(R.string.phone_folder_stop))
                    }
                }
            }
            status?.silentFor?.let {
                Surface(color = MaterialTheme.colorScheme.tertiaryContainer) {
                    Text(
                        if (it < 48 * 3600) {
                            stringResource(R.string.not_synced_hours, it / 3600)
                        } else {
                            stringResource(R.string.not_synced_days, it / 86400)
                        },
                        Modifier.fillMaxWidth().padding(16.dp).testTag("not-synced"),
                    )
                }
            }
            status?.takeIf { it.failing > 0 }?.let {
                Surface(color = MaterialTheme.colorScheme.tertiaryContainer) {
                    Text(
                        stringResource(R.string.machines_failing, it.failing),
                        Modifier.fillMaxWidth().padding(16.dp).testTag("machines-failing"),
                    )
                }
            }
            status?.takeIf { it.vaultRefuses }?.let {
                Surface(color = MaterialTheme.colorScheme.tertiaryContainer) {
                    Text(
                        stringResource(R.string.vault_refuses),
                        Modifier.fillMaxWidth().padding(16.dp).testTag("vault-refuses"),
                    )
                }
            }
            status?.lowDisk?.let {
                Surface(color = MaterialTheme.colorScheme.tertiaryContainer) {
                    Text(
                        stringResource(R.string.low_disk, humanSize(it)),
                        Modifier.fillMaxWidth().padding(16.dp).testTag("low-disk"),
                    )
                }
            }
            if (busy) {
                LinearProgressIndicator(Modifier.fillMaxWidth().padding(8.dp))
            }

            if (level.at.isNotEmpty()) {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    IconButton(
                        { open(level.at.substringBeforeLast('/', "")) },
                        Modifier.testTag("tree-up"),
                    ) {
                        Icon(Icons.Default.ArrowUpward, stringResource(R.string.tree_up))
                    }
                    Text(
                        level.at,
                        Modifier.testTag("tree-at"),
                        style = MaterialTheme.typography.titleSmall,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }

            if (level.at.isEmpty() && level.folders.isEmpty() && level.files.isEmpty()) {
                Centred {
                    Text(
                        stringResource(R.string.files_empty),
                        Modifier.padding(32.dp),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }

            if (chosen.isNotEmpty()) {
                ChosenBar(
                    chosen.size,
                    onNone = { chosen = emptySet() },
                    onFree = {
                        val picked = level.files.filter { it.path in chosen && it.here }
                        scope.launch {
                            try {
                                // One at a time, as the row's button: each kept while no other machine holds it.
                                val freed = withContext(Dispatchers.IO) {
                                    picked.count { org.json.JSONObject(Native.release(it.path)).optBoolean("freed") }
                                }
                                chosen = emptySet()
                                refresh()
                                snackbar.showSnackbar(context.getString(R.string.files_freed, freed, picked.size - freed))
                            } catch (error: Throwable) {
                                complain(error.message ?: "could not free them")
                            }
                        }
                    },
                    onDelete = { confirmDelete = level.files.filter { it.path in chosen } },
                    onRename = level.files.singleOrNull { it.path in chosen }?.let { one -> { renaming = one } },
                    onVersions = level.files.singleOrNull { it.path in chosen }?.let { one -> { versionsOf = one } },
                ) {
                    val picked = level.files.filter { it.path in chosen }
                    scope.launch {
                        busy = true
                        try {
                            sendAll(context, picked.map { Account.openable(context, it) })
                            chosen = emptySet()
                            refresh()
                        } catch (error: Throwable) {
                            complain(error.message ?: "could not share them")
                        } finally {
                            busy = false
                        }
                    }
                }
            }

            LazyColumn(Modifier.fillMaxSize()) {
                items(level.folders, key = { "folder:${it.path}" }) { folder ->
                    FolderRow(folder) { open(folder.path) }
                }
                items(level.files, key = { "file:${it.path}" }) { file ->
                    FileRow(
                        file = file,
                        chosen = file.path in chosen,
                        selecting = chosen.isNotEmpty(),
                        onChoose = { chosen = if (file.path in chosen) chosen - file.path else chosen + file.path },
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
                        onShare = {
                            scope.launch {
                                busy = true
                                try {
                                    send(context, Account.openable(context, file))
                                    refresh()
                                } catch (error: Throwable) {
                                    complain(error.message ?: "could not share it")
                                } finally {
                                    busy = false
                                }
                            }
                        },
                        onDelete = { confirmDelete = listOf(file) },
                        onFree = {
                            scope.launch {
                                try {
                                    val answer = withContext(Dispatchers.IO) { org.json.JSONObject(Native.release(file.path)) }
                                    refresh()
                                    snackbar.showSnackbar(answer.getString("said"))
                                } catch (error: Throwable) {
                                    complain(error.message ?: "could not free it")
                                }
                            }
                        },
                    )
                }
            }
        }
    }

    if (trashOpen) {
        TrashDialog({ trashOpen = false }, { refresh() }, { complain(it) })
    }

    versionsOf?.let { file ->
        VersionsDialog(
            file,
            onClose = { versionsOf = null },
            onBack = { id ->
                versionsOf = null
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { Native.bringBack(file.path, id) }
                        chosen = emptySet()
                        refresh()
                    } catch (error: Throwable) {
                        complain(error.message ?: "could not bring it back")
                    }
                }
            },
        )
    }

    renaming?.let { file ->
        var name by remember(file.path) { mutableStateOf(file.path) }
        AlertDialog(
            onDismissRequest = { renaming = null },
            title = { Text(stringResource(R.string.rename_title, file.name)) },
            text = {
                androidx.compose.material3.OutlinedTextField(
                    name,
                    { name = it },
                    Modifier.fillMaxWidth().testTag("rename-name"),
                    singleLine = true,
                    supportingText = { Text(stringResource(R.string.rename_help)) },
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("rename-confirm"),
                    enabled = name.isNotBlank() && name.trim() != file.path,
                    onClick = {
                        val to = name.trim()
                        renaming = null
                        scope.launch {
                            try {
                                withContext(Dispatchers.IO) { Native.rename(file.path, to) }
                                chosen = emptySet()
                                refresh()
                            } catch (error: Throwable) {
                                complain(error.message ?: "could not rename it")
                            }
                        }
                    },
                ) { Text(stringResource(R.string.rename_confirm)) }
            },
            dismissButton = { TextButton({ renaming = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }

    confirmDelete.takeIf { it.isNotEmpty() }?.let { files ->
        val trashContext = androidx.compose.ui.platform.LocalContext.current
        AlertDialog(
            onDismissRequest = { confirmDelete = emptyList() },
            title = {
                Text(
                    if (files.size == 1) {
                        stringResource(R.string.delete_title, files[0].name)
                    } else {
                        stringResource(R.string.delete_title_many, files.size)
                    },
                )
            },
            text = {
                val rules = Trash.rules
                Text(
                    when (rules.mode) {
                        "direct" -> stringResource(R.string.delete_body_direct)
                        "local" -> stringResource(R.string.delete_body_local)
                        else -> stringResource(R.string.delete_body, rules.days)
                    },
                )
            },
            confirmButton = {
                TextButton(modifier = Modifier.testTag("delete-confirm"), onClick = {
                    val doomed = files
                    confirmDelete = emptyList()
                    scope.launch {
                        try {
                            // Every file goes, then one word if any waits.
                            val became = doomed.map { Trash.put(it.path) }
                            chosen = chosen - doomed.map { it.path }.toSet()
                            if ("waiting" in became) complain(trashContext.getString(R.string.delete_waiting))
                            refresh()
                        } catch (error: Throwable) {
                            complain(error.message ?: "could not delete it")
                        }
                    }
                }) { Text(stringResource(R.string.delete_everywhere)) }
            },
            dismissButton = { TextButton({ confirmDelete = emptyList() }) { Text(stringResource(R.string.delete_keep)) } },
        )
    }

}

/** What a round brought, named: "2 new: a, b · 1 changed: c", or null when nothing. */
private fun changesLine(context: android.content.Context, answer: org.json.JSONObject): String? {
    val parts = listOf(
        "new" to R.string.round_new,
        "changed" to R.string.round_changed,
        "gone" to R.string.round_gone,
    ).mapNotNull { (key, label) ->
        val part = answer.optJSONObject(key) ?: return@mapNotNull null
        val count = part.optInt("count")
        if (count == 0) return@mapNotNull null
        val names = part.optJSONArray("names") ?: org.json.JSONArray()
        val listed = List(names.length()) { names.getString(it).substringAfterLast('/') }.joinToString(", ")
        context.getString(label, count) + ": " + listed + if (count > names.length()) ", …" else ""
    }
    return parts.takeIf { it.isNotEmpty() }?.joinToString(" · ")
}
