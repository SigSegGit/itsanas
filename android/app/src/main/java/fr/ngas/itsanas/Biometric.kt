package fr.ngas.itsanas

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Unlocking with a fingerprint or a face instead of the passphrase.
 *
 * Opt-in, offered once after the passphrase is typed. The passphrase is then
 * kept only encrypted, under a key in the phone's Keystore that is usable only
 * right after a strong biometric check, and that Android destroys when a new
 * fingerprint or face is enrolled -- so somebody who adds their own finger to
 * the phone gets the passphrase screen, not the account. On any failure the
 * passphrase screen is the way in, and the 24 words remain the recovery.
 */
object Biometric {
    private const val KEY = "itsanas.biometric"
    private const val PREFS = "itsanas.biometric"
    private const val CIPHERTEXT = "ciphertext"
    private const val IV = "iv"
    private const val ASKED = "asked"

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Whether this phone has a strong biometric enrolled and usable. */
    fun available(context: Context): Boolean =
        BiometricManager.from(context).canAuthenticate(BIOMETRIC_STRONG) ==
            BiometricManager.BIOMETRIC_SUCCESS

    /** Whether the passphrase is kept behind the biometric check. */
    fun enabled(context: Context): Boolean = prefs(context).contains(CIPHERTEXT)

    /** Whether to offer it now: available, not enabled, never answered. */
    fun shouldOffer(context: Context): Boolean =
        available(context) && !enabled(context) && !prefs(context).getBoolean(ASKED, false)

    /** The person answered the offer; it is not asked again. */
    fun answered(context: Context) {
        prefs(context).edit().putBoolean(ASKED, true).apply()
    }

    /** Turn it off: the key and the encrypted passphrase go. */
    fun disable(context: Context) {
        prefs(context).edit().remove(CIPHERTEXT).remove(IV).apply()
        runCatching { keyStore().deleteEntry(KEY) }
    }

    /**
     * Ask for a fingerprint or face, then keep [passphrase] encrypted under it
     * and forget the plain copy. [done] says whether it is now enabled.
     */
    fun enable(activity: FragmentActivity, passphrase: String, done: (Boolean) -> Unit) {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, key()) }
        prompt(activity, cipher, "Use your fingerprint or face to unlock ITSaNAS", { unlocked ->
            val sealed = unlocked.doFinal(passphrase.toByteArray(Charsets.UTF_8))
            prefs(activity).edit()
                .putString(CIPHERTEXT, Base64.encodeToString(sealed, Base64.NO_WRAP))
                .putString(IV, Base64.encodeToString(unlocked.iv, Base64.NO_WRAP))
                .apply()
            Vault.forget(activity)
            done(true)
        }, { done(false) })
    }

    /**
     * Ask for a fingerprint or face and hand back the passphrase; [fallback]
     * when the person cancels, the check fails, or the key was invalidated by
     * a new enrolment (then it is turned off, and offered again later).
     */
    fun unlock(activity: FragmentActivity, passphrase: (String) -> Unit, fallback: () -> Unit) {
        val stored = prefs(activity)
        val sealed = stored.getString(CIPHERTEXT, null)
        val iv = stored.getString(IV, null)
        if (sealed == null || iv == null) return fallback()
        val cipher = try {
            Cipher.getInstance(TRANSFORMATION).apply {
                init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP)))
            }
        } catch (e: KeyPermanentlyInvalidatedException) {
            disable(activity)
            stored.edit().remove(ASKED).apply()
            return fallback()
        }
        prompt(activity, cipher, "Unlock ITSaNAS", { unlocked ->
            passphrase(String(unlocked.doFinal(Base64.decode(sealed, Base64.NO_WRAP)), Charsets.UTF_8))
        }, fallback)
    }

    private fun prompt(
        activity: FragmentActivity,
        cipher: Cipher,
        title: String,
        success: (Cipher) -> Unit,
        failure: () -> Unit,
    ) {
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            .setNegativeButtonText("Use the passphrase")
            .setAllowedAuthenticators(BIOMETRIC_STRONG)
            .build()
        BiometricPrompt(activity, ContextCompat.getMainExecutor(activity), object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val unlocked = result.cryptoObject?.cipher ?: return failure()
                try {
                    success(unlocked)
                } catch (e: Exception) {
                    failure()
                }
            }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) = failure()
        }).authenticate(info, BiometricPrompt.CryptoObject(cipher))
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun key(): SecretKey {
        (keyStore().getKey(KEY, null) as? SecretKey)?.let { return it }
        val spec = KeyGenParameterSpec.Builder(KEY, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setUserAuthenticationRequired(true)
            .setInvalidatedByBiometricEnrollment(true)
            .apply {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                } else {
                    @Suppress("DEPRECATION")
                    setUserAuthenticationValidityDurationSeconds(-1)
                }
            }
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            .apply { init(spec) }
            .generateKey()
    }

    private const val TRANSFORMATION = "AES/GCM/NoPadding"
}
