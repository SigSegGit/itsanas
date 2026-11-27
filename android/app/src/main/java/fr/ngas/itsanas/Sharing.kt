package fr.ngas.itsanas

import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.content.FileProvider

/** The name a picked document has, or something usable if it will not say. */
internal fun nameOf(context: android.content.Context, uri: Uri): String {
    context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
        val column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (column >= 0 && cursor.moveToFirst()) {
            val name = cursor.getString(column)
            if (!name.isNullOrBlank()) return name
        }
    }
    return uri.lastPathSegment?.substringAfterLast('/') ?: "file"
}

/**
 * Hand a downloaded file to whatever the person wants to do with it.
 *
 * Viewing first, sharing as the fallback. A great many of the things people
 * keep have no viewer installed — an archive, a `.bin`, somebody's export —
 * and "No apps can perform this action" is a dead end for a file the person
 * can plainly see. Sharing always has somewhere to go: another application, a
 * messaging app, or the file manager's save.
 */
internal fun share(context: android.content.Context, file: java.io.File) {
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val type = context.contentResolver.getType(uri) ?: "application/octet-stream"

    val view = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, type)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    val intent = if (view.resolveActivity(context.packageManager) != null) view else sending(context, file)

    context.startActivity(Intent.createChooser(intent, file.name))
}

/** Send a file elsewhere -- messaging, mail, Bluetooth -- as any app on the phone does. */
internal fun send(context: android.content.Context, file: java.io.File) {
    context.startActivity(Intent.createChooser(sending(context, file), file.name))
}

/**
 * Several files at once to one person or app, as the gallery sends several photos:
 * one share sheet, every file readable by whoever receives them.
 */
internal fun sendAll(context: android.content.Context, files: List<java.io.File>) {
    if (files.size == 1) return send(context, files[0])
    val uris = files.map { FileProvider.getUriForFile(context, "${context.packageName}.files", it) }
    val types = uris.map { context.contentResolver.getType(it) ?: "application/octet-stream" }.toSet()
    val family = types.map { it.substringBefore('/') }.toSet().singleOrNull()
    val intent = Intent(Intent.ACTION_SEND_MULTIPLE).apply {
        putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
        setType(types.singleOrNull() ?: family?.let { "$it/*" } ?: "*/*")
        // The grant reaches every file only through the clip; EXTRA_STREAM alone carries the first.
        clipData = android.content.ClipData.newRawUri(null, uris[0]).apply {
            uris.drop(1).forEach { addItem(android.content.ClipData.Item(it)) }
        }
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    context.startActivity(Intent.createChooser(intent, context.getString(R.string.files_chosen, files.size)))
}

/** The system's "send this file" intent: one person or app at a time, never a public link. */
private fun sending(context: android.content.Context, file: java.io.File): Intent {
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    return Intent(Intent.ACTION_SEND).apply {
        putExtra(Intent.EXTRA_STREAM, uri)
        setType(context.contentResolver.getType(uri) ?: "application/octet-stream")
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
}
