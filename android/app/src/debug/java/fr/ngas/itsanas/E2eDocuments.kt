package fr.ngas.itsanas

import android.database.Cursor
import android.database.MatrixCursor
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import java.io.File

/**
 * A folder for the end-to-end test to choose, in debug builds only: the real walk of
 * `PhoneFolder` runs against a real provider, which no stub of the picker could prove.
 * A document's id is its path under `cache/e2e-tree`, and the top of it is "Camera".
 */
class E2eDocuments : DocumentsProvider() {
    private val base: File get() = File(requireNotNull(context).cacheDir, "e2e-tree")

    override fun onCreate() = true

    override fun queryRoots(projection: Array<out String>?): Cursor =
        MatrixCursor(projection ?: arrayOf(Root.COLUMN_ROOT_ID))

    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor =
        cursor(projection).also { row(it, documentId, File(base, documentId)) }

    override fun queryChildDocuments(
        parentDocumentId: String,
        projection: Array<out String>?,
        sortOrder: String?,
    ): Cursor = cursor(projection).also { rows ->
        File(base, parentDocumentId).listFiles()?.forEach {
            row(rows, "$parentDocumentId/${it.name}", it)
        }
    }

    override fun isChildDocument(parentDocumentId: String, documentId: String) =
        documentId.startsWith("$parentDocumentId/")

    override fun openDocument(
        documentId: String,
        mode: String,
        signal: CancellationSignal?,
    ): ParcelFileDescriptor =
        ParcelFileDescriptor.open(File(base, documentId), ParcelFileDescriptor.MODE_READ_ONLY)

    private fun cursor(projection: Array<out String>?) = MatrixCursor(
        projection ?: arrayOf(
            Document.COLUMN_DOCUMENT_ID,
            Document.COLUMN_DISPLAY_NAME,
            Document.COLUMN_MIME_TYPE,
            Document.COLUMN_SIZE,
            Document.COLUMN_LAST_MODIFIED,
        ),
    )

    private fun row(rows: MatrixCursor, id: String, file: File) {
        val line = rows.newRow()
        rows.columnNames.forEach { column ->
            line.add(
                when (column) {
                    Document.COLUMN_DOCUMENT_ID -> id
                    Document.COLUMN_DISPLAY_NAME -> file.name
                    Document.COLUMN_MIME_TYPE ->
                        if (file.isDirectory) Document.MIME_TYPE_DIR else "application/octet-stream"
                    Document.COLUMN_SIZE -> file.length()
                    Document.COLUMN_LAST_MODIFIED -> file.lastModified()
                    else -> null
                },
            )
        }
    }
}
