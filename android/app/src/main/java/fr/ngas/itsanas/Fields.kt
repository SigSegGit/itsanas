package fr.ngas.itsanas

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.ContextWrapper
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.view.WindowManager
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp

/**
 * The keyboard of a secret: the Password type is the part that matters, because
 * keyboards neither correct, suggest nor learn from a password field.
 */
val SecretKeyboard = KeyboardOptions(
    keyboardType = KeyboardType.Password,
    autoCorrectEnabled = false,
    capitalization = KeyboardCapitalization.None,
)

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}

/**
 * No screenshot, no screen recording and no Recents thumbnail while a secret is
 * on screen. A screen of secrets calls this once; the flag is lifted when it
 * leaves, so Files is not locked down for good.
 */
@Composable
fun SecureScreen() {
    val window = LocalContext.current.activity()?.window
    DisposableEffect(window) {
        window?.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        onDispose { window?.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) }
    }
}

/** A passphrase field with an eye: hidden until the person asks. */
@Composable
fun PassphraseField(
    value: String,
    onChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    keyboardOptions: KeyboardOptions = SecretKeyboard,
    tag: String = "",
) {
    var shown by remember { mutableStateOf(false) }
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        singleLine = true,
        visualTransformation = if (shown) VisualTransformation.None else PasswordVisualTransformation(),
        keyboardOptions = keyboardOptions,
        trailingIcon = {
            IconButton({ shown = !shown }, Modifier.testTag("$tag-eye")) {
                Icon(
                    if (shown) Icons.Default.VisibilityOff else Icons.Default.Visibility,
                    stringResource(if (shown) R.string.hide_passphrase else R.string.show_passphrase),
                )
            }
        },
        modifier = modifier.fillMaxWidth().testTag(tag),
    )
}

/** A field for words: shown as typed (they are read against a sheet), no corrections. */
@Composable
fun WordsField(
    value: String,
    onChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    singleLine: Boolean = true,
    minLines: Int = 1,
    tag: String = "",
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        singleLine = singleLine,
        minLines = minLines,
        keyboardOptions = SecretKeyboard,
        modifier = modifier.fillMaxWidth().testTag(tag),
    )
}

/** The 24 words, numbered, in two columns of twelve. */
@Composable
fun WordsGrid(phrase: String, modifier: Modifier = Modifier) {
    val words = phrase.trim().split(Regex("\\s+"))
    Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        words.chunked(12).forEachIndexed { column, chunk ->
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                chunk.forEachIndexed { row, word ->
                    Text(
                        "${column * 12 + row + 1}. $word",
                        fontFamily = FontFamily.Monospace,
                        style = MaterialTheme.typography.bodyLarge,
                        modifier = Modifier.padding(vertical = 2.dp).testTag("word-${column * 12 + row + 1}"),
                    )
                }
            }
        }
    }
}

private val handler = Handler(Looper.getMainLooper())

/**
 * Copy the words as a sensitive clip (the keyboard and the clipboard preview
 * hide it) and clear it after 60 seconds, unless the person copied something
 * else since. FLAG_SECURE alone would take away the one way to keep the words
 * that people actually use; this is its other half.
 */
fun copyWords(context: Context, words: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    val clip = ClipData.newPlainText("ITSaNAS", words).apply {
        description.extras = PersistableBundle().apply {
            // ClipDescription.EXTRA_IS_SENSITIVE, a constant only since API 33.
            putBoolean("android.content.extra.IS_SENSITIVE", true)
        }
    }
    clipboard.setPrimaryClip(clip)
    handler.postDelayed({
        // By the label: from Android 10 an app in the background reads no clip, and
        // the person is in another app, pasting. Something copied since has its own.
        val ours = runCatching { clipboard.primaryClipDescription?.label == "ITSaNAS" }.getOrDefault(false)
        if (ours) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                clipboard.clearPrimaryClip()
            } else {
                clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
            }
        }
    }, 60_000L)
}
