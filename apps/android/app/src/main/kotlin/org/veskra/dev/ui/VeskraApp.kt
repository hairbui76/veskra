package org.veskra.dev.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.veskra.dev.R
import org.veskra.dev.ui.editor.EditorScreen
import org.veskra.dev.ui.gallery.GalleryScreen

@Composable
fun VeskraApp(vm: VeskraViewModel) {
    val state by vm.ui.collectAsStateWithLifecycle()
    Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        when {
            !state.engineAvailable -> EngineUnavailable()
            state.screen == Screen.GALLERY -> GalleryScreen(state, vm)
            else -> EditorScreen(state, vm)
        }
    }
    state.message?.let { msg ->
        AlertDialog(
            onDismissRequest = vm::dismissMessage,
            confirmButton = { TextButton(onClick = vm::dismissMessage) { Text(stringResource(R.string.action_ok)) } },
            text = { Text(stringResource(msg.id, *msg.args.toTypedArray())) },
        )
    }
}

@Composable
private fun EngineUnavailable() {
    Column(
        modifier = Modifier.fillMaxSize().safeDrawingPadding().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(stringResource(R.string.engine_unavailable_title), style = MaterialTheme.typography.headlineSmall)
        Text(stringResource(R.string.engine_unavailable_body), style = MaterialTheme.typography.bodyLarge)
    }
}
