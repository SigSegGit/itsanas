package fr.ngas.itsanas

import android.content.Context
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * The trash: which files, how long, and what a delete does are the library's
 * (`deletion.rs`), set per machine in Settings, Trash (HANDOVER §8 13).
 */
object Trash {
    /** `kind`: shared (the account's trash), here, waiting (to confirm), protected. */
    class Row(val path: String, val daysLeft: Long?, val kind: String = "here")

    /** This phone's rules, as the last answer said them; protective until then. */
    class Rules(val mode: String, val days: Long, val askAuth: Boolean, val confirm: Boolean)

    @Volatile
    var rules = Rules("trash", 30, askAuth = true, confirm = true)
        private set

    /** Delete as this phone's rules say; the word for what it became. */
    suspend fun put(path: String): String = withContext(Dispatchers.IO) {
        JSONObject(Native.trashPut(path, "0")).optString("became")
    }

    /** At once, from the record. */
    suspend fun recorded(context: Context): List<Row> = withContext(Dispatchers.IO) {
        parse(Native.trashRecord(Vault.home(context).absolutePath))
    }

    /**
     * The same, on the calling thread: a small file, no node. The dialog's first
     * rows come from here: on an emulator, a coroutine started as the dialog opened
     * did not return for 30 s while a round ran, and the trash looked empty.
     */
    fun recordedNow(context: Context): List<Row>? =
        runCatching { parse(Native.trashRecord(Vault.home(context).absolutePath)) }.getOrNull()

    /** With what left the folder too, once the node is free: a round may hold it for a while. */
    suspend fun rows(): List<Row> = withContext(Dispatchers.IO) { parse(Native.trashList()) }

    /** Read this phone's rules once the node answers (the record alone has none). */
    suspend fun readRules() = withContext(Dispatchers.IO) { parse(Native.trashList()) }

    private fun parse(text: String): List<Row> {
        val answer = JSONObject(text)
        if (answer.has("mode")) {
            rules = Rules(
                answer.getString("mode"),
                answer.optLong("days", 30),
                answer.optBoolean("ask_auth", true),
                answer.optBoolean("confirm", true),
            )
        }
        val now = answer.getLong("now")
        val trashed = answer.getJSONArray("trashed")
        val left = answer.getJSONArray("left")
        return List(trashed.length()) { index ->
            val row = trashed.getJSONObject(index)
            val kind = row.optString("kind", "here")
            Row(row.getString("path"), (row.getLong("due") - now) / (24 * 60 * 60), kind)
        } + List(left.length()) { index -> Row(left.getString(index), null) }
    }
}

@Composable
fun TrashDialog(onClose: () -> Unit, onChanged: suspend () -> Unit, complain: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // Null until read: "empty" said too soon reads as files lost.
    var rows by remember { mutableStateOf(Trash.recordedNow(context)) }
    // What a delete for good would delete: one row, or the whole trash.
    var doomed by remember { mutableStateOf<List<Trash.Row>?>(null) }
    var proving by remember { mutableStateOf<(suspend () -> Unit)?>(null) }

    suspend fun reload() {
        rows = Trash.recorded(context)
        rows = Trash.rows()
        onChanged()
    }

    fun act(what: suspend () -> Unit) {
        scope.launch {
            try {
                what()
                reload()
            } catch (error: Exception) {
                complain(error.message ?: "the trash did not answer")
            }
        }
    }

    LaunchedEffect(Unit) { act { } }

    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(stringResource(R.string.trash_title)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Text(stringResource(R.string.trash_rules_where), style = MaterialTheme.typography.labelSmall)
                if (rows?.isEmpty() == true) Text(stringResource(R.string.trash_empty))
                val forGood = rows.orEmpty().filter { it.daysLeft != null }
                if (forGood.isNotEmpty()) {
                    TextButton({ doomed = forGood }, Modifier.testTag("trash-empty")) {
                        Text(stringResource(R.string.trash_empty_all))
                    }
                }
                rows.orEmpty().forEach { row ->
                    Text(row.path)
                    Text(
                        when {
                            row.daysLeft == null -> stringResource(R.string.trash_left_folder)
                            row.kind == "waiting" -> stringResource(R.string.trash_waiting)
                            row.kind == "protected" -> stringResource(R.string.trash_protected, row.daysLeft)
                            else -> stringResource(R.string.trash_left, row.daysLeft)
                        },
                        Modifier.testTag("trash-row"),
                        style = MaterialTheme.typography.labelSmall,
                    )
                    Row {
                        if (row.daysLeft != null) {
                            TextButton(
                                {
                                    act {
                                        withContext(Dispatchers.IO) {
                                            Native.trashRestore(Vault.home(context).absolutePath, row.path)
                                        }
                                    }
                                },
                                Modifier.testTag("trash-restore"),
                            ) { Text(stringResource(R.string.trash_restore)) }
                        } else {
                            TextButton(
                                {
                                    act {
                                        try {
                                            PhoneFolder.restore(context, row.path)
                                        } catch (refused: SecurityException) {
                                            complain(context.getString(R.string.trash_folder_not_writable))
                                            return@act
                                        }
                                        // Back in the folder: not gone from it any more.
                                        withContext(Dispatchers.IO) {
                                            Native.trashKeep(Vault.home(context).absolutePath, row.path)
                                        }
                                    }
                                },
                                Modifier.testTag("trash-put-back"),
                            ) { Text(stringResource(R.string.trash_put_back)) }
                            TextButton(
                                {
                                    act {
                                        withContext(Dispatchers.IO) {
                                            Native.trashKeep(Vault.home(context).absolutePath, row.path)
                                        }
                                    }
                                },
                                Modifier.testTag("trash-keep"),
                            ) { Text(stringResource(R.string.trash_keep)) }
                        }
                        TextButton({ doomed = listOf(row) }, Modifier.testTag("trash-delete")) {
                            Text(stringResource(R.string.trash_delete))
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClose, Modifier.testTag("trash-close")) { Text(stringResource(R.string.close)) }
        },
    )

    // Deleted for good: after "are you sure" when the rules ask it, and after the
    // phone's own check when they ask that, once for the whole selection.
    fun deleteForGood(chosen: List<Trash.Row>) {
        val action: suspend () -> Unit = {
            act { withContext(Dispatchers.IO) { chosen.forEach { Native.trashDelete(it.path) } } }
        }
        if (Trash.rules.askAuth) proving = action else scope.launch { action() }
    }

    doomed?.let { chosen ->
        if (!Trash.rules.confirm) {
            doomed = null
            deleteForGood(chosen)
            return@let
        }
        AlertDialog(
            onDismissRequest = { doomed = null },
            title = {
                Text(
                    if (chosen.size == 1) stringResource(R.string.delete_title, chosen[0].path)
                    else stringResource(R.string.trash_empty_title, chosen.size),
                )
            },
            text = { Text(stringResource(R.string.trash_delete_body)) },
            confirmButton = {
                TextButton(
                    {
                        doomed = null
                        deleteForGood(chosen)
                    },
                    Modifier.testTag("trash-delete-confirm"),
                ) { Text(stringResource(R.string.trash_delete)) }
            },
            dismissButton = { TextButton({ doomed = null }) { Text(stringResource(R.string.delete_keep)) } },
        )
    }

    IdentityGate(proving != null) { proved ->
        val action = proving
        proving = null
        if (!proved) complain(context.getString(R.string.identity_refused))
        else if (action != null) scope.launch { action() }
    }
}
