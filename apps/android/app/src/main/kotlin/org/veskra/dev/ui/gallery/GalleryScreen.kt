package org.veskra.dev.ui.gallery

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.storage.ProjectEntry
import org.veskra.dev.storage.RecoveryEntry
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.dev.ui.icons.VeskraIcons
import org.veskra.kit.formatBytes
import java.text.DateFormat
import java.util.Date

@Composable
fun GalleryScreen(state: UiState, vm: VeskraViewModel) {
    var showNewCanvas by rememberSaveable { mutableStateOf(false) }
    // SAF picker; no storage permission needed. `.veskra` has no registered MIME type, so providers
    // report octet-stream/zip or vendor types: accept any and let the engine validate (hostile input).
    val openLauncher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) vm.openFromSaf(uri)
    }

    Box(Modifier.fillMaxSize()) {
        Column(
            modifier = Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = 24.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            // FlowRow: on a narrow screen the buttons move to their own line instead of being squeezed.
            FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    stringResource(R.string.gallery_title),
                    style = MaterialTheme.typography.headlineMedium,
                    modifier = Modifier.weight(1f).align(Alignment.CenterVertically),
                )
                OutlinedButton(onClick = { openLauncher.launch(arrayOf("*/*")) }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Icon(VeskraIcons.Open, contentDescription = null)
                    Spacer(Modifier.width(8.dp))
                    Text(stringResource(R.string.gallery_open), maxLines = 1, softWrap = false)
                }
                Button(onClick = { showNewCanvas = true }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Icon(VeskraIcons.Add, contentDescription = null)
                    Spacer(Modifier.width(8.dp))
                    Text(stringResource(R.string.gallery_new_canvas), maxLines = 1, softWrap = false)
                }
            }

            state.recovery?.let { RecoveryCard(it, onRestore = vm::restoreRecovery, onDiscard = vm::discardRecovery) }

            Text(stringResource(R.string.gallery_recent), style = MaterialTheme.typography.titleMedium)
            if (state.projects.isEmpty()) {
                Text(stringResource(R.string.gallery_empty), style = MaterialTheme.typography.bodyLarge)
            } else {
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(minSize = 240.dp),
                    modifier = Modifier.weight(1f),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    items(state.projects, key = { it.path }) { p -> ProjectCard(p, onOpen = { vm.openProject(p) }) }
                }
            }
            Text(stringResource(R.string.gallery_storage_note), style = MaterialTheme.typography.bodySmall)
        }
        if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth().align(Alignment.TopCenter).safeDrawingPadding())
    }

    if (showNewCanvas) {
        NewCanvasDialog(
            onDismiss = { showNewCanvas = false },
            onCreate = { spec, background, name ->
                showNewCanvas = false
                vm.newCanvas(spec, background, name)
            },
        )
    }
}

@Composable
private fun RecoveryCard(entry: RecoveryEntry, onRestore: () -> Unit, onDiscard: () -> Unit) {
    val time = DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(entry.savedAtMillis))
    ElevatedCard(Modifier.fillMaxWidth()) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            Icon(VeskraIcons.Recover, contentDescription = null, modifier = Modifier.size(32.dp))
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.gallery_recovery_title), style = MaterialTheme.typography.titleMedium)
                Text(stringResource(R.string.gallery_recovery_body, time), style = MaterialTheme.typography.bodyMedium)
                Text(entry.documentName, style = MaterialTheme.typography.bodySmall)
            }
            TextButton(onClick = onDiscard, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.gallery_recovery_discard)) }
            FilledTonalButton(onClick = onRestore, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.gallery_recovery_restore)) }
        }
    }
}

@Composable
private fun ProjectCard(p: ProjectEntry, onOpen: () -> Unit) {
    val modified = DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(p.modifiedMillis))
    val cd = stringResource(R.string.gallery_open_project_cd, p.name)
    Card(onClick = onOpen, modifier = Modifier.fillMaxWidth().heightIn(min = 96.dp).semantics { contentDescription = cd }) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(p.name, style = MaterialTheme.typography.titleMedium)
            Text(stringResource(R.string.gallery_item_modified, modified), style = MaterialTheme.typography.bodySmall)
            Text(formatBytes(p.bytes), style = MaterialTheme.typography.bodySmall)
        }
    }
}
