package fr.ngas.itsanas

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
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

    private fun preferences(context: Context) =
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    fun chosen(context: Context): Uri? =
        preferences(context).getString(TREE, null)?.let(Uri::parse)

    /** The folder's name, for the line on the Files screen. */
    fun name(context: Context): String? = chosen(context)?.let { tree ->
        runCatching { displayName(context, tree) }.getOrNull() ?: tree.lastPathSegment
    }

    /** Keep the system's permission across restarts, which a picker's answer does not. */
    fun choose(context: Context, tree: Uri) {
        runCatching {
            context.contentResolver.takePersistableUriPermission(
                tree,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
        preferences(context).edit().putString(TREE, tree.toString()).apply()
    }

    /** Files already in the account stay there: stopping is not deleting. */
    fun stop(context: Context) {
        chosen(context)?.let { tree ->
            runCatching {
                context.contentResolver.releasePersistableUriPermission(
                    tree,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION,
                )
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
        var count = 0
        for (index in 0 until wanted.length()) {
            val path = wanted.getJSONObject(index).getString("path")
            val file = found[path] ?: continue
            val copy = File.createTempFile("folder", null, staged)
            try {
                context.contentResolver.openInputStream(file.uri).use { input ->
                    requireNotNull(input) { "$path could not be read" }
                    copy.outputStream().use { input.copyTo(it) }
                }
                Native.folderImport(file.json(path).toString(), copy.absolutePath)
                count++
            } finally {
                copy.delete()
            }
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
