package fr.ngas.itsanas

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Whatever the engine is waiting for, drawn over the page. Back answers null,
 * and the engine stops cleanly, as it does when a window on a PC is closed.
 */
@Composable
fun AskOverlay(ask: Ask) {
    when (ask) {
        is Ask.NewPassphrase -> NewPassphrase(ask)
        is Ask.Passphrase -> AskPassphrase(ask)
        is Ask.Recovery -> WordsAsk(ask)
        is Ask.Confirm -> WordsCheck(ask)
        is Ask.Words -> WordsShow(ask)
    }
}

/** One screen of secrets: opaque, no screenshot, clear of the bars and the keyboard. */
@Composable
private fun SecretFrame(
    title: String,
    onBack: () -> Unit,
    content: @Composable ColumnScope.() -> Unit,
) {
    SecureScreen()
    BackHandler(onBack = onBack)
    Surface(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxSize()
                .safeDrawingPadding()
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(title, style = MaterialTheme.typography.headlineMedium)
            content()
        }
    }
}

@Composable
fun NewPassphrase(ask: Ask.NewPassphrase) {
    val rule = remember { JSONObject(Native.passphraseRule()) }
    val min = rule.getInt("min")
    val max = rule.getInt("max")
    var first by remember { mutableStateOf("") }
    var second by remember { mutableStateOf("") }
    val fine = first.length in min..max && first == second
    SecretFrame(stringResource(R.string.new_passphrase_title), { ask.answer.complete(null) }) {
        Text(stringResource(R.string.new_passphrase_help, min, max))
        PassphraseField(first, { first = it }, stringResource(R.string.passphrase_label), tag = "new-pass")
        PassphraseField(second, { second = it }, stringResource(R.string.new_passphrase_again), tag = "new-pass-again")
        if (second.isNotEmpty() && first != second) {
            Text(stringResource(R.string.new_passphrase_differ), Modifier.testTag("secret-error"), color = MaterialTheme.colorScheme.error)
        }
        Button(
            onClick = { ask.answer.complete(first) },
            enabled = fine,
            modifier = Modifier.fillMaxWidth().testTag("secret-continue"),
        ) { Text(stringResource(R.string.continue_)) }
    }
}

@Composable
fun AskPassphrase(ask: Ask.Passphrase) {
    var typed by remember { mutableStateOf("") }
    SecretFrame(stringResource(R.string.ask_passphrase_title), { ask.answer.complete(null) }) {
        Text(stringResource(R.string.ask_passphrase_help, ask.purpose))
        PassphraseField(typed, { typed = it }, stringResource(R.string.passphrase_label), tag = "ask-pass")
        Button(
            onClick = { ask.answer.complete(typed) },
            enabled = typed.isNotEmpty(),
            modifier = Modifier.fillMaxWidth().testTag("secret-continue"),
        ) { Text(stringResource(R.string.continue_)) }
    }
}

/** The 24 words of an existing account, checked as they are typed. */
@Composable
fun WordsAsk(ask: Ask.Recovery) {
    var typed by remember { mutableStateOf("") }
    var verdict by remember { mutableStateOf<JSONObject?>(null) }
    LaunchedEffect(typed) {
        delay(150)
        verdict = withContext(Dispatchers.IO) { JSONObject(Native.checkWords(typed)) }
    }
    val said = verdict
    val ok = said?.getBoolean("ok") == true
    SecretFrame(stringResource(R.string.recovery_title), { ask.answer.complete(null) }) {
        Text(stringResource(R.string.recovery_help))
        WordsField(
            typed, { typed = it }, stringResource(R.string.recovery_label),
            singleLine = false, minLines = 4, tag = "recovery-words",
        )
        if (said != null && typed.isNotBlank()) {
            when {
                ok -> Text(stringResource(R.string.recovery_ok))
                !said.isNull("wordIndex") -> Text(
                    stringResource(R.string.recovery_unknown, said.getInt("wordIndex")),
                    color = MaterialTheme.colorScheme.error,
                )
                else -> Text(stringResource(R.string.recovery_count, said.getInt("count")))
            }
        }
        Button(
            onClick = { ask.answer.complete(typed.trim()) },
            enabled = ok,
            modifier = Modifier.fillMaxWidth().testTag("secret-continue"),
        ) { Text(stringResource(R.string.continue_)) }
    }
}

/** The words on screen with Copy; [onDone] when the person has them. */
@Composable
private fun WordsBody(phrase: String, onDone: () -> Unit, notice: String? = null) {
    val context = LocalContext.current
    var asking by remember { mutableStateOf(false) }
    Text(stringResource(R.string.words_text))
    Text(stringResource(R.string.words_advice))
    notice?.let { Text(it, Modifier.testTag("secret-error"), color = MaterialTheme.colorScheme.error) }
    WordsGrid(phrase)
    OutlinedButton({ asking = true }, Modifier.fillMaxWidth().testTag("words-copy")) {
        Text(stringResource(R.string.words_copy))
    }
    Button(onDone, Modifier.fillMaxWidth().testTag("words-have-them")) { Text(stringResource(R.string.words_have_them)) }
    if (asking) {
        AlertDialog(
            onDismissRequest = { asking = false },
            title = { Text(stringResource(R.string.words_copy_title)) },
            text = { Text(stringResource(R.string.words_copy_body)) },
            confirmButton = {
                TextButton({
                    asking = false
                    copyWords(context, phrase)
                }) { Text(stringResource(R.string.words_copy)) }
            },
            dismissButton = {
                TextButton({ asking = false }) { Text(stringResource(R.string.cancel)) }
            },
        )
    }
}

/** The words, read and not proved: Done answers. */
@Composable
fun WordsShow(ask: Ask.Words) {
    SecretFrame(stringResource(R.string.words_title), { ask.answer.complete(null) }) {
        WordsBody(ask.phrase, { ask.answer.complete("") })
    }
}

/** The words, then three of them typed back; three misses show them again. */
@Composable
fun WordsCheck(ask: Ask.Confirm) {
    val words = remember(ask) { ask.phrase.trim().split(Regex("\\s+")) }
    var checking by remember { mutableStateOf(false) }
    var tries by remember { mutableIntStateOf(3) }
    var failed by remember { mutableStateOf(false) }
    var again by remember { mutableStateOf(false) }
    val typed = remember { mutableStateOf(List(3) { "" }) }

    if (!checking) {
        SecretFrame(stringResource(R.string.words_title), { ask.answer.complete(null) }) {
            WordsBody(
                ask.phrase,
                { checking = true; failed = false },
                notice = if (again) stringResource(R.string.check_again) else null,
            )
        }
        return
    }
    SecretFrame(stringResource(R.string.check_title), { checking = false }) {
        Text(stringResource(R.string.check_help))
        ask.positions.forEachIndexed { index, position ->
            WordsField(
                typed.value[index],
                { value -> typed.value = typed.value.toMutableList().also { it[index] = value } },
                stringResource(R.string.check_word, position),
                tag = "check-$index",
            )
        }
        if (failed) {
            Text(stringResource(R.string.check_wrong, tries), Modifier.testTag("secret-error"), color = MaterialTheme.colorScheme.error)
        }
        Button(
            onClick = {
                val right = ask.positions.indices.all { index ->
                    typed.value[index].trim().equals(words.getOrNull(ask.positions[index] - 1), ignoreCase = true)
                }
                if (right) {
                    ask.answer.complete(typed.value.joinToString(" ") { it.trim() })
                } else {
                    tries -= 1
                    failed = true
                    if (tries <= 0) {
                        tries = 3
                        again = true
                        checking = false
                        typed.value = List(3) { "" }
                    }
                }
            },
            enabled = typed.value.all { it.isNotBlank() },
            modifier = Modifier.fillMaxWidth().testTag("check-confirm"),
        ) { Text(stringResource(R.string.check_confirm)) }
        OutlinedButton({ checking = false }, Modifier.fillMaxWidth().testTag("check-show-again")) {
            Text(stringResource(R.string.check_show_again))
        }
    }
}
