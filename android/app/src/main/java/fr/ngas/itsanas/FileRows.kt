package fr.ngas.itsanas

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.CloudDownload
import androidx.compose.material.icons.filled.CloudUpload
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.OpenInNew
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@Composable
internal fun FolderRow(folder: KnownFolder, onOpen: () -> Unit) {
    Card(
        onOpen,
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .testTag("folder-row"),
    ) {
        Row(
            Modifier.padding(12.dp).fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Default.Folder, null, Modifier.padding(end = 12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    folder.name,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    buildString {
                        append(
                            if (folder.files == 1L) {
                                stringResource(R.string.tree_one_file, humanSize(folder.size))
                            } else {
                                stringResource(R.string.tree_files, folder.files, humanSize(folder.size))
                            },
                        )
                        if (folder.notHere > 0) {
                            append(" · ${stringResource(R.string.tree_not_here, folder.notHere)}")
                        }
                    },
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }
    }
}

@Composable
internal fun FileRow(
    file: Known,
    chosen: Boolean,
    selecting: Boolean,
    onChoose: () -> Unit,
    onOpen: () -> Unit,
    onShare: () -> Unit,
    onDelete: () -> Unit,
    onFree: () -> Unit,
) {
    Card(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .testTag("file-row")
            // A long press chooses the file; once one is chosen, a tap chooses or drops another.
            .pointerInput(selecting) {
                detectTapGestures(onLongPress = { onChoose() }, onTap = { if (selecting) onChoose() })
            },
        colors = if (chosen) {
            CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer)
        } else {
            CardDefaults.cardColors()
        },
    ) {
        Row(
            Modifier.padding(12.dp).fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (chosen) {
                Icon(
                    Icons.Default.CheckCircle,
                    stringResource(R.string.file_chosen),
                    Modifier.padding(end = 8.dp).testTag("file-chosen"),
                )
            }
            Column(Modifier.weight(1f)) {
                Text(
                    file.name,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    buildString {
                        append(humanSize(file.size))
                        if (!file.here) append(" · ${stringResource(R.string.file_not_here)}")
                    },
                    style = MaterialTheme.typography.labelSmall,
                )
            }
            IconButton(onOpen) {
                Icon(
                    if (file.here) Icons.Default.OpenInNew else Icons.Default.CloudDownload,
                    stringResource(if (file.here) R.string.file_open else R.string.file_fetch),
                )
            }
            IconButton(onShare, Modifier.testTag("file-share")) {
                Icon(Icons.Default.Share, stringResource(R.string.file_share))
            }
            if (file.here) {
                IconButton(onFree, Modifier.testTag("file-free")) {
                    Icon(Icons.Default.CloudUpload, stringResource(R.string.file_free))
                }
            }
            IconButton(onDelete, Modifier.testTag("file-delete")) { Icon(Icons.Default.Delete, stringResource(R.string.file_delete)) }
        }
    }
}

/** Every version of one file, newest first; one held here and not current can come back. */
@Composable
internal fun VersionsDialog(file: Known, onClose: () -> Unit, onBack: (String) -> Unit) {
    var listed by remember(file.path) { mutableStateOf<org.json.JSONArray?>(null) }
    LaunchedEffect(file.path) {
        listed = withContext(Dispatchers.IO) {
            runCatching { org.json.JSONObject(Native.versions(file.path)).getJSONArray("versions") }
                .getOrDefault(org.json.JSONArray())
        }
    }
    val format = remember { java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.MEDIUM, java.text.DateFormat.SHORT) }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(stringResource(R.string.versions_title, file.name)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                val versions = listed
                if (versions == null) {
                    Text(stringResource(R.string.versions_reading))
                } else {
                    for (at in 0 until versions.length()) {
                        val version = versions.getJSONObject(at)
                        Row(Modifier.fillMaxWidth().testTag("version-row"), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(format.format(java.util.Date(version.getLong("modified") * 1000)))
                                Text(
                                    buildString {
                                        append(humanSize(version.getLong("size")))
                                        if (version.getBoolean("current")) append(" · ${stringResource(R.string.versions_current)}")
                                        if (!version.getBoolean("here")) append(" · ${stringResource(R.string.file_not_here)}")
                                    },
                                    style = MaterialTheme.typography.labelSmall,
                                )
                            }
                            if (version.getBoolean("here") && !version.getBoolean("current")) {
                                TextButton({ onBack(version.getString("id")) }, Modifier.testTag("version-back-$at")) {
                                    Text(stringResource(R.string.versions_back))
                                }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClose) { Text(stringResource(R.string.cancel)) } },
    )
}

/** The bar over the list while files are chosen: how many, drop them all, then what to do with them. */
@Composable
internal fun ChosenBar(
    count: Int,
    onNone: () -> Unit,
    onFree: () -> Unit,
    onDelete: () -> Unit,
    onRename: (() -> Unit)?,
    onVersions: (() -> Unit)?,
    onShare: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onNone, Modifier.testTag("files-choose-none")) {
            Icon(Icons.Default.Close, stringResource(R.string.files_choose_none))
        }
        Text(
            stringResource(R.string.files_chosen, count),
            Modifier.weight(1f).testTag("files-chosen"),
            style = MaterialTheme.typography.titleSmall,
        )
        IconButton(onShare, Modifier.testTag("files-share-chosen")) {
            Icon(Icons.Default.Share, stringResource(R.string.files_share_chosen))
        }
        // One file chosen: it can take a new name, which moves it too.
        if (onRename != null) {
            IconButton(onRename, Modifier.testTag("files-rename-chosen")) {
                Icon(Icons.Default.Edit, stringResource(R.string.files_rename_chosen))
            }
        }
        if (onVersions != null) {
            IconButton(onVersions, Modifier.testTag("files-versions-chosen")) {
                Icon(Icons.Default.History, stringResource(R.string.files_versions_chosen))
            }
        }
        IconButton(onFree, Modifier.testTag("files-free-chosen")) {
            Icon(Icons.Default.CloudUpload, stringResource(R.string.files_free_chosen))
        }
        IconButton(onDelete, Modifier.testTag("files-delete-chosen")) {
            Icon(Icons.Default.Delete, stringResource(R.string.files_delete_chosen))
        }
    }
}
