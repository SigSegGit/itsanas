package fr.ngas.itsanas

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
internal fun Unlock(onOpen: () -> Unit, complain: (String) -> Unit) {
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
