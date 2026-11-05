package fr.ngas.itsanas

import android.content.Context
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.text.input.KeyboardType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** The phone's trash: which files and how long is the library's (`trash.rs`). */
object Trash {
    private const val DEFAULT_DAYS = 30L

    class Row(val path: String, val daysLeft: Long?)

    private fun preferences(context: Context) =
        context.getSharedPreferences("trash", Context.MODE_PRIVATE)

    fun days(context: Context): Long = preferences(context).getLong("days", DEFAULT_DAYS)

    fun setDays(context: Context, days: Long) {
        preferences(context).edit().putLong("days", days).apply()
    }

    suspend fun put(context: Context, path: String) = withContext(Dispatchers.IO) {
        Native.trashPut(path, days(context).toString())
        Unit
    }

    /** At once, from the record. */
    suspend fun recorded(context: Context): List<Row> = withContext(Dispatchers.IO) {
        parse(Native.trashRecord(Vault.home(context).absolutePath))
    }

    /** With what left the folder too, once the node is free: a round may hold it for a while. */
    suspend fun rows(): List<Row> = withContext(Dispatchers.IO) { parse(Native.trashList()) }

    private fun parse(text: String): List<Row> {
        val answer = JSONObject(text)
        val now = answer.getLong("now")
        val trashed = answer.getJSONArray("trashed")
        val left = answer.getJSONArray("left")
        return List(trashed.length()) { index ->
            val row = trashed.getJSONObject(index)
            Row(row.getString("path"), (row.getLong("due") - now) / (24 * 60 * 60))
        } + List(left.length()) { index -> Row(left.getString(index), null) }
    }
}

@Composable
fun TrashDialog(onClose: () -> Unit, onChanged: suspend () -> Unit, complain: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // Null until read: "empty" said too soon reads as files lost.
    var rows by remember { mutableStateOf<List<Trash.Row>?>(null) }
    var days by remember { mutableStateOf(Trash.days(context).toString()) }
    var doomed by remember { mutableStateOf<Trash.Row?>(null) }

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
                OutlinedTextField(
                    value = days,
                    onValueChange = { typed ->
                        days = typed.filter(Char::isDigit).take(4)
                        days.toLongOrNull()?.takeIf { it >= 1 }?.let { Trash.setDays(context, it) }
                    },
                    label = { Text(stringResource(R.string.trash_days)) },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    singleLine = true,
                    modifier = Modifier.testTag("trash-days"),
                )
                if (rows?.isEmpty() == true) Text(stringResource(R.string.trash_empty))
                rows.orEmpty().forEach { row ->
                    Text(row.path)
                    Text(
                        row.daysLeft?.let { stringResource(R.string.trash_left, it) }
                            ?: stringResource(R.string.trash_left_folder),
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
                        }
                        TextButton({ doomed = row }, Modifier.testTag("trash-delete")) {
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

    doomed?.let { row ->
        AlertDialog(
            onDismissRequest = { doomed = null },
            title = { Text(stringResource(R.string.delete_title, row.path)) },
            text = { Text(stringResource(R.string.trash_delete_body)) },
            confirmButton = {
                TextButton(
                    {
                        doomed = null
                        act { withContext(Dispatchers.IO) { Native.trashDelete(row.path) } }
                    },
                    Modifier.testTag("trash-delete-confirm"),
                ) { Text(stringResource(R.string.trash_delete)) }
            },
            dismissButton = { TextButton({ doomed = null }) { Text(stringResource(R.string.delete_keep)) } },
        )
    }
}
