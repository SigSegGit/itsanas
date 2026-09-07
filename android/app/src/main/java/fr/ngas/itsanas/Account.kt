package fr.ngas.itsanas

import android.content.Context
import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/**
 * The account, as the screens see it.
 *
 * Everything here runs off the main thread and turns the core's JSON into
 * ordinary Kotlin values. No screen calls [Native] directly: the rule keeps the
 * "must not run on the main thread" promise in one place instead of in every
 * button handler.
 */
object Account {

    @Volatile
    var open: Boolean = false
        private set

    suspend fun exists(context: Context): Boolean = withContext(Dispatchers.IO) {
        Native.exists(Vault.home(context).absolutePath)
    }

    /** Create an account. Returns the twenty-four words, once. */
    suspend fun create(context: Context, username: String, passphrase: String): String =
        withContext(Dispatchers.IO) {
            val home = Vault.home(context).also { it.mkdirs() }
            val answer = Native.create(home.absolutePath, username, passphrase)
            Vault.remember(context, passphrase)
            open = true
            Answers.phrase(answer)
        }

    /** Restore an account from its twenty-four words. */
    suspend fun login(
        context: Context,
        username: String,
        phrase: String,
        passphrase: String,
    ) = withContext(Dispatchers.IO) {
        val home = Vault.home(context).also { it.mkdirs() }
        Native.login(home.absolutePath, username, phrase.trim(), passphrase)
        Vault.remember(context, passphrase)
        open = true
    }

    /** Unlock the node already on this phone. */
    suspend fun unlock(context: Context, passphrase: String) = withContext(Dispatchers.IO) {
        Native.open(Vault.home(context).absolutePath, passphrase)
        Vault.remember(context, passphrase)
        open = true
    }

    suspend fun close() = withContext(Dispatchers.IO) {
        Native.close()
        open = false
    }

    suspend fun files(): List<Known> = withContext(Dispatchers.IO) {
        Answers.files(Native.list()).sortedWith(
            compareBy({ it.folder }, { it.name.lowercase() })
        )
    }

    suspend fun status(): Status = withContext(Dispatchers.IO) {
        Answers.status(Native.status())
    }

    suspend fun syncNow(metadataOnly: Boolean): String = withContext(Dispatchers.IO) {
        Native.sync(metadataOnly)
    }

    /**
     * Fetch a file and hand back something another application can open.
     *
     * Written into `cache/opened`, not into the node's own directory: this copy
     * is for the viewer the person is about to pick, it is theirs to keep for
     * as long as the system lets it, and losing it costs one download rather
     * than anything from the account.
     */
    suspend fun openable(context: Context, file: Known): File = withContext(Dispatchers.IO) {
        val out = File(context.cacheDir, "opened").also { it.mkdirs() }
        val destination = File(out, file.name)
        Native.get(file.path, destination.absolutePath)
        destination
    }

    /**
     * Put a document the person picked into the account.
     *
     * Copied through the cache first because the core reads a path and a
     * `content://` URI is not one. The copy is deleted straight after: a phone
     * short of room should not end up holding every file twice because of how
     * this application talks to itself.
     */
    suspend fun add(context: Context, uri: Uri, name: String, folder: String) =
        withContext(Dispatchers.IO) {
            val staged = File(context.cacheDir, "staged").also { it.mkdirs() }
            val copy = File(staged, name)
            context.contentResolver.openInputStream(uri).use { input ->
                requireNotNull(input) { "that file could not be read" }
                copy.outputStream().use { input.copyTo(it) }
            }
            try {
                val path = if (folder.isBlank()) name else "$folder/$name"
                Native.put(path, copy.absolutePath)
            } finally {
                copy.delete()
            }
        }

    suspend fun remove(path: String) = withContext(Dispatchers.IO) {
        Native.remove(path)
        Unit
    }

    suspend fun addPeer(address: String) = withContext(Dispatchers.IO) {
        Native.addPeer(address.trim())
        Unit
    }

    suspend fun removePeer(address: String) = withContext(Dispatchers.IO) {
        Native.removePeer(address)
        Unit
    }

    suspend fun setKeep(bytes: Long?, order: String, only: List<String>) =
        withContext(Dispatchers.IO) {
            Native.setKeep(bytes ?: 0L, order, only.joinToString("\n"))
            Unit
        }

    suspend fun setPledge(bytes: Long) = withContext(Dispatchers.IO) {
        Native.setPledge(bytes)
        Unit
    }
}
