package fr.ngas.itsanas

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.webkit.MimeTypeMap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * The folder this phone keeps in the account. Only the walk is here, because only the
 * application holds the system's permission: which files are new is the library's
 * (`phone_folder.rs`).
 */
object PhoneFolder {
    private const val PREFERENCES = "phone-folder"
    private const val TREE = "tree"

    /** One segment per batch, not per photo (issue 93), and a bounded cache meanwhile. */
    private const val BATCH_FILES = 64
    private const val BATCH_BYTES = 256L * 1024 * 1024

    private fun preferences(context: Context) =
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    fun chosen(context: Context): Uri? =
        preferences(context).getString(TREE, null)?.let(Uri::parse)

    /** The folder's name, for the line on the Files screen. */
    fun name(context: Context): String? = chosen(context)?.let { tree ->
        runCatching { displayName(context, tree) }.getOrNull() ?: tree.lastPathSegment
    }

    /**
     * Keep the system's permission across restarts, which a picker's answer does not.
     * Writing too, so the trash can put back a file that left the folder; read alone
     * when a provider grants no more, and the folder still syncs.
     */
    fun choose(context: Context, tree: Uri) {
        val both = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        runCatching { context.contentResolver.takePersistableUriPermission(tree, both) }
            .recoverCatching {
                context.contentResolver.takePersistableUriPermission(
                    tree,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION,
                )
            }
        preferences(context).edit().putString(TREE, tree.toString()).apply()
    }

    /** Whether the chosen folder may be written to: a choice made before 0.1.20 may not. */
    fun writable(context: Context): Boolean = chosen(context)?.let { tree ->
        context.contentResolver.persistedUriPermissions.any { it.uri == tree && it.isWritePermission }
    } ?: false

    /**
     * Write a file of the account back where it was in the chosen folder: Restore,
     * for a file that left it. `accountPath` is `Phone/<folder>/...`, the folder's
     * own name first. Its next listing finds it, and nothing is lost if it is read
     * in again: the same bytes are already in the account.
     */
    suspend fun restore(context: Context, accountPath: String) = withContext(Dispatchers.IO) {
        val tree = chosen(context) ?: error("no folder is chosen on this phone")
        if (!writable(context)) throw SecurityException("not writable")
        val parts = accountPath.removePrefix("Phone/").split('/')
        require(parts.size >= 2) { "$accountPath is not a file of the chosen folder" }
        var parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        for (name in parts.subList(1, parts.size - 1)) {
            parent = child(context, tree, parent, name)
                ?: DocumentsContract.createDocument(
                    context.contentResolver,
                    parent,
                    DocumentsContract.Document.MIME_TYPE_DIR,
                    name,
                )
                ?: error("$name could not be made in the folder")
        }
        val name = parts.last()
        val staged = File.createTempFile("restore", null, context.cacheDir)
        try {
            Native.get(accountPath, staged.absolutePath)
            val mime = MimeTypeMap.getSingleton()
                .getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
                ?: "application/octet-stream"
            val target = DocumentsContract.createDocument(context.contentResolver, parent, mime, name)
                ?: error("$name could not be made in the folder")
            context.contentResolver.openOutputStream(target).use { output ->
                requireNotNull(output) { "$name could not be written" }
                staged.inputStream().use { it.copyTo(output) }
            }
        } finally {
            staged.delete()
        }
    }

    /** The child of `parent` called `name`, if there is one. */
    private fun child(context: Context, tree: Uri, parent: Uri, name: String): Uri? {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(
            tree,
            DocumentsContract.getDocumentId(parent),
        )
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        )
        context.contentResolver.query(children, columns, null, null, null)?.use { rows ->
            while (rows.moveToNext()) {
                if (rows.getString(1) == name) {
                    return DocumentsContract.buildDocumentUriUsingTree(tree, rows.getString(0))
                }
            }
        }
        return null
    }

    /** Files already in the account stay there: stopping is not deleting. */
    fun stop(context: Context) {
        chosen(context)?.let { tree ->
            runCatching {
                val granted = context.contentResolver.persistedUriPermissions
                    .firstOrNull { it.uri == tree }
                var flags = Intent.FLAG_GRANT_READ_URI_PERMISSION
                if (granted?.isWritePermission == true) flags = flags or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                context.contentResolver.releasePersistableUriPermission(tree, flags)
            }
        }
        preferences(context).edit().remove(TREE).apply()
    }

    /** Copy what the library asks for; how many came in. */
    suspend fun sync(context: Context): Int = withContext(Dispatchers.IO) {
        val tree = chosen(context) ?: return@withContext 0
        val found = HashMap<String, Listed>()
        val top = DocumentsContract.getTreeDocumentId(tree)
        walk(context, tree, top, displayName(context, tree) ?: "Folder", found)

        val listing = JSONArray()
        found.forEach { (path, file) -> listing.put(file.json(path)) }
        val wanted = JSONObject(Native.folderWanted(listing.toString())).getJSONArray("wanted")

        val staged = File(context.cacheDir, "staged").also { it.mkdirs() }
        val copies = ArrayList<File>()
        val batch = JSONArray()
        var bytes = 0L
        var count = 0
        fun send() {
            try {
                if (batch.length() > 0) Native.folderImport(batch.toString())
                count += batch.length()
            } finally {
                copies.forEach { it.delete() }
                copies.clear()
                while (batch.length() > 0) batch.remove(0)
                bytes = 0
            }
        }
        try {
            for (index in 0 until wanted.length()) {
                val path = wanted.getJSONObject(index).getString("path")
                val file = found[path] ?: continue
                val copy = File.createTempFile("folder", null, staged)
                copies.add(copy)
                context.contentResolver.openInputStream(file.uri).use { input ->
                    requireNotNull(input) { "$path could not be read" }
                    copy.outputStream().use { input.copyTo(it) }
                }
                batch.put(JSONObject().put("file", file.json(path)).put("source", copy.absolutePath))
                bytes += copy.length()
                if (batch.length() >= BATCH_FILES || bytes >= BATCH_BYTES) send()
            }
            send()
        } finally {
            copies.forEach { it.delete() }
        }
        count
    }

    private class Listed(val uri: Uri, val size: Long, val modified: Long) {
        fun json(path: String): JSONObject = JSONObject()
            .put("path", path)
            .put("size", size.coerceAtLeast(0))
            .put("modified", modified.coerceAtLeast(0))
    }

    private fun displayName(context: Context, tree: Uri): String? {
        val document = DocumentsContract.buildDocumentUriUsingTree(
            tree,
            DocumentsContract.getTreeDocumentId(tree),
        )
        val columns = arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
        return context.contentResolver.query(document, columns, null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        }
    }

    private fun walk(
        context: Context,
        tree: Uri,
        parent: String,
        prefix: String,
        found: MutableMap<String, Listed>,
    ) {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent)
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        )
        context.contentResolver.query(children, columns, null, null, null)?.use { rows ->
            while (rows.moveToNext()) {
                val id = rows.getString(0)
                val path = "$prefix/${rows.getString(1)}"
                if (rows.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                    walk(context, tree, id, path, found)
                } else {
                    found[path] = Listed(
                        DocumentsContract.buildDocumentUriUsingTree(tree, id),
                        rows.getLong(3),
                        rows.getLong(4),
                    )
                }
            }
        }
    }
}
