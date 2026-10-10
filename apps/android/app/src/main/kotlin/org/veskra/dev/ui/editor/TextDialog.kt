package org.veskra.dev.ui.editor

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.canvas.TextRasterizer
import kotlin.math.roundToInt

/**
 * Text tool input: the words and a size in document pixels. [docHeight] scales the size range so
 * the default is readable on any canvas. The text is placed with Transform afterwards.
 */
@Composable
fun TextDialog(docHeight: Int, onDismiss: () -> Unit, onAdd: (text: String, sizePx: Float) -> Unit) {
    var text by rememberSaveable { mutableStateOf("") }
    val maxSize = (docHeight / 3f).coerceIn(48f, 1200f)
    var size by rememberSaveable { mutableFloatStateOf((docHeight / 14f).coerceIn(12f, maxSize)) }
    val sizeLabel = stringResource(R.string.text_size_label, size.roundToInt())
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.tool_text)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = text,
                    onValueChange = { text = it.take(TextRasterizer.MAX_CHARS) },
                    label = { Text(stringResource(R.string.text_field_label)) },
                    minLines = 2,
                    maxLines = 5,
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(sizeLabel, style = MaterialTheme.typography.labelLarge)
                Slider(
                    value = size,
                    onValueChange = { size = it },
                    valueRange = 12f..maxSize,
                    modifier = Modifier.semantics { contentDescription = sizeLabel },
                )
                Text(stringResource(R.string.text_raster_note), style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = {
            TextButton(onClick = { onAdd(text, size) }, enabled = text.isNotBlank(), modifier = Modifier.heightIn(min = 48.dp)) {
                Text(stringResource(R.string.text_add))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss, modifier = Modifier.heightIn(min = 48.dp)) { Text(stringResource(R.string.action_cancel)) } },
    )
}
