package fr.ngas.itsanas

import android.app.KeyguardManager
import android.os.Build
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_WEAK
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Who is holding the phone, before a delete for good (HANDOVER §8 13): the
 * phone's own lock (fingerprint, face, PIN) when it has one, the account's
 * passphrase otherwise. One proof covers the whole action, a selection or the
 * whole trash.
 */
object Identity {
    /**
     * Off in the end-to-end tests only: a real phone's lock cannot be answered
     * by a test, its passphrase can.
     */
    @Volatile
    var systemAllowed = true

    /** Whether the system can check its owner: a lock screen, on Android 11 or later. */
    fun systemCan(activity: FragmentActivity): Boolean =
        systemAllowed &&
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            activity.getSystemService(KeyguardManager::class.java).isDeviceSecure

    fun askSystem(activity: FragmentActivity, title: String, done: (Boolean) -> Unit) {
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            .setAllowedAuthenticators(BIOMETRIC_WEAK or DEVICE_CREDENTIAL)
            .build()
        BiometricPrompt(activity, ContextCompat.getMainExecutor(activity), object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) = done(true)
            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) = done(false)
        }).authenticate(info)
    }
}

/**
 * Ask who is there while `asking`, then say whether it was proved. The caller
 * runs the action in its own scope: this gate leaves the screen as soon as it
 * answers, and a coroutine of its own would be cancelled with it. The
 * passphrase is checked by opening the account already open, which answers only
 * when it is the right one.
 */
@Composable
fun IdentityGate(asking: Boolean, onAnswer: (Boolean) -> Unit) {
    if (!asking) return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val activity = context as? FragmentActivity
    val title = stringResource(R.string.identity_title)
    var started by remember { mutableStateOf(false) }
    if (activity != null && Identity.systemCan(activity)) {
        if (!started) {
            started = true
            Identity.askSystem(activity, title, onAnswer)
        }
        return
    }
    var typed by remember { mutableStateOf("") }
    var checking by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = { onAnswer(false) },
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = typed,
                onValueChange = { typed = it },
                label = { Text(stringResource(R.string.identity_passphrase)) },
                visualTransformation = PasswordVisualTransformation(),
                singleLine = true,
                modifier = Modifier.testTag("identity-pass"),
            )
        },
        confirmButton = {
            TextButton(
                {
                    if (checking) return@TextButton
                    checking = true
                    val passphrase = typed
                    scope.launch {
                        val proved = withContext(Dispatchers.IO) {
                            runCatching { Native.open(Vault.home(context).absolutePath, passphrase) }.isSuccess
                        }
                        onAnswer(proved)
                    }
                },
                Modifier.testTag("identity-ok"),
            ) { Text(stringResource(R.string.identity_ok)) }
        },
        dismissButton = { TextButton({ onAnswer(false) }) { Text(stringResource(R.string.delete_keep)) } },
    )
}
