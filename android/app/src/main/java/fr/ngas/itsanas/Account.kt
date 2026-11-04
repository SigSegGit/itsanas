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

    /** The engine stopped the service; the node is closed by the router. */
    fun markClosed() {
        open = false
    }

    suspend fun exists(context: Context): Boolean = withContext(Dispatchers.IO) {
        Native.exists(Vault.home(context).absolutePath)
    }

    /** Unlock the node already on this phone. */
    suspend fun unlock(context: Context, passphrase: String) = withContext(Dispatchers.IO) {
        Native.open(Vault.home(context).absolutePath, passphrase)
        if (!Biometric.enabled(context)) Vault.remember(context, passphrase)
        open = true
    }

    suspend fun close(context: Context? = null) = withContext(Dispatchers.IO) {
        Native.close()
        open = false
        context?.let {
            Vault.noteClosedByPerson(it, true)
            clearOpened(it, olderThanMillis = 0L)
        }
    }

    /** Open again what the person left open, for background work after the process died. */
    suspend fun reopen(context: Context, passphrase: String) = withContext(Dispatchers.IO) {
        Native.open(Vault.home(context).absolutePath, passphrase)
        open = true
    }

    /**
     * The plaintext copies made to open a file. They sit in the cache, outside
     * the vault, so they are not kept: emptied at open when older than a day,
     * and at Lock and logout altogether.
     */
    fun clearOpened(context: Context, olderThanMillis: Long) {
        val cutoff = System.currentTimeMillis() - olderThanMillis
        File(context.cacheDir, "opened").listFiles()?.forEach {
            if (it.lastModified() <= cutoff) it.delete()
        }
    }

    suspend fun level(folder: String): Level = withContext(Dispatchers.IO) {
        Answers.level(Native.listAt(folder))
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
        clearOpened(context, olderThanMillis = 24 * 60 * 60 * 1000L)
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

}
