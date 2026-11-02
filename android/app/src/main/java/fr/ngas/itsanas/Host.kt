package fr.ngas.itsanas

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.flow.MutableSharedFlow
import java.util.concurrent.CompletableFuture

/** A secret the engine is waiting for, drawn by a Compose screen over the page. */
sealed interface Ask {
    /** Null: the person went Back, and the engine stops as a closed PC window does. */
    val answer: CompletableFuture<String?>

    class NewPassphrase(override val answer: CompletableFuture<String?>) : Ask
    class Passphrase(val purpose: String, override val answer: CompletableFuture<String?>) : Ask
    class Recovery(override val answer: CompletableFuture<String?>) : Ask

    /** The words, then three of them typed back: [positions] count from 1. */
    class Confirm(
        val phrase: String,
        val positions: List<Int>,
        override val answer: CompletableFuture<String?>,
    ) : Ask

    /** The words, read; Done answers anything. */
    class Words(val phrase: String, override val answer: CompletableFuture<String?>) : Ask
}

/**
 * What the Rust engine calls from its own thread (`host.rs`). Each question
 * parks that thread on a future while a screen is on top of the web view.
 * `@JvmStatic` because Rust looks the methods up on the class.
 */
object Host {
    @Volatile
    private var context: Context? = null

    /** The screen on top of the page, if the engine is waiting for a secret. */
    var current by mutableStateOf<Ask?>(null)
        private set

    /** What the page changed that the shell also draws or keeps: name, value. */
    val events = MutableSharedFlow<Pair<String, String>>(extraBufferCapacity = 32)

    fun attach(application: Context) {
        context = application.applicationContext
    }

    private fun ask(make: (CompletableFuture<String?>) -> Ask): String? {
        val future = CompletableFuture<String?>()
        val ask = make(future)
        current = ask
        return try {
            future.get()
        } catch (_: Exception) {
            null
        } finally {
            if (current === ask) current = null
        }
    }

    /** The page is leaving: a question nobody can answer any more is answered null. */
    fun cancelAll() {
        current?.answer?.complete(null)
    }

    @JvmStatic fun askNewPassphrase(): String? = ask { Ask.NewPassphrase(it) }

    @JvmStatic fun askPassphrase(purpose: String): String? = ask { Ask.Passphrase(purpose, it) }

    @JvmStatic fun askRecoveryPhrase(): String? = ask { Ask.Recovery(it) }

    @JvmStatic
    fun showAndConfirm(phrase: String, a: Int, b: Int, c: Int): String? =
        ask { Ask.Confirm(phrase, listOf(a, b, c), it) }

    @JvmStatic
    fun showWords(phrase: String) {
        ask { Ask.Words(phrase, it) }
    }

    /** The remembered copy; null when the phone holds it behind a fingerprint or nowhere. */
    @JvmStatic
    fun storedPassphrase(): String? = context?.let { Vault.rememberedPassphrase(it) }

    @JvmStatic
    fun storePassphrase(p: String) {
        val c = context ?: return
        if (!Biometric.enabled(c)) Vault.remember(c, p)
    }

    @JvmStatic
    fun forgetPassphrase() {
        context?.let { Vault.forget(it) }
    }

    @JvmStatic
    fun serviceStart() {
        val c = context ?: return
        SyncService.start(c)
    }

    /**
     * Not `Account.close`: the engine may hold the node while it asks, and
     * closing waits for the same lock. The router closes it when it hears
     * `signedOut`, `removed` or `closed`.
     */
    @JvmStatic
    fun serviceStop() {
        val c = context ?: return
        SyncService.stop(c)
        Account.markClosed()
    }

    @JvmStatic
    fun event(name: String, value: String) {
        val c = context
        if (c != null) {
            when (name) {
                "theme" -> Appearance.follow(c, value)
                "metered" -> Vault.setContentOnMetered(c, value == "on")
            }
        }
        events.tryEmit(name to value)
    }

    /** What the shell holds for the page's settings; null when it does not say. */
    @JvmStatic
    fun value(name: String): String? {
        val c = context ?: return null
        return when (name) {
            "biometric" -> if (Biometric.enabled(c)) "on" else "off"
            "metered" -> if (Vault.contentOnMetered(c)) "on" else "off"
            "theme" -> Appearance.word()
            else -> null
        }
    }
}
