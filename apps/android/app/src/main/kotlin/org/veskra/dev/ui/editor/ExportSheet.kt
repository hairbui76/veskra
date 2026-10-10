package org.veskra.dev.ui.editor

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import org.veskra.dev.R
import org.veskra.dev.ui.UiState
import org.veskra.dev.ui.VeskraViewModel
import org.veskra.kit.ExportCopy
import org.veskra.kit.ExportDestination
import org.veskra.kit.ExportFileNames
import org.veskra.kit.ExportFormat
import org.veskra.kit.ExportProfile
import org.veskra.kit.ExportPhase
import org.veskra.kit.ExportSheetEvent
import org.veskra.kit.ExportSheetState
import org.veskra.kit.PreflightState

@StringRes
private fun destinationTitle(d: ExportDestination): Int = when (d) {
    ExportDestination.IMAGE -> R.string.export_destination_image
    ExportDestination.PHOTOSHOP -> R.string.export_destination_photoshop
    ExportDestination.ILLUSTRATOR -> R.string.export_destination_illustrator
}

@StringRes
private fun formatLabel(f: ExportFormat): Int = when (f) {
    ExportFormat.PNG -> R.string.export_format_png
    ExportFormat.PSD -> R.string.export_format_psd
    ExportFormat.SVG -> R.string.export_format_svg
    ExportFormat.PDF -> R.string.export_format_pdf
}

/**
 * Export sheet driven entirely by the pure [ExportSheetState] machine from :kit (MASTER_PLAN §2.3):
 * three independent checkboxes (vector/raster only for SVG/PDF), blocked state with an actionable
 * message, preflight counts before export, and success shown only after validation + SAF close.
 * Formats offered: PNG, PSD, SVG, PDF — there is no `.ai` output.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun ExportSheet(export: ExportSheetState, state: UiState, vm: VeskraViewModel) {
    val createDoc = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(export.format.mimeType)) { uri ->
        if (uri != null) vm.exportTo(uri)
    }
    val saveCopy = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        if (uri != null) vm.saveAs(uri)
    }
    val exporting = export.phase is ExportPhase.Exporting

    ModalBottomSheet(onDismissRequest = { if (!exporting) vm.closeExport() }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(
            Modifier.padding(horizontal = 24.dp, vertical = 8.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.export_sheet_title), style = MaterialTheme.typography.titleLarge)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ExportDestination.entries.forEach { d ->
                    FilterChip(
                        selected = export.destination == d,
                        enabled = !exporting,
                        onClick = { vm.selectExportDestination(d) },
                        label = { Text(stringResource(destinationTitle(d))) },
                    )
                }
            }
            Text(stringResource(destinationTitle(export.destination)), style = MaterialTheme.typography.titleMedium)

            if (export.destination.formats.size > 1) {
                Text(stringResource(R.string.export_format), style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    export.destination.formats.forEach { f ->
                        FilterChip(
                            selected = export.format == f,
                            enabled = !exporting,
                            onClick = { vm.onExportEvent(ExportSheetEvent.SelectFormat(f)) },
                            label = { Text(stringResource(formatLabel(f))) },
                        )
                    }
                }
            }

            if (export.vectorRasterTogglesEnabled) {
                OptionRow(
                    label = stringResource(R.string.export_opt_vector),
                    help = stringResource(R.string.export_opt_vector_help),
                    checked = export.options.includeVectorPaths,
                    enabled = !exporting,
                ) { vm.onExportEvent(ExportSheetEvent.SetIncludeVectorPaths(it)) }
                OptionRow(
                    label = stringResource(R.string.export_opt_raster),
                    help = stringResource(R.string.export_opt_raster_help),
                    checked = export.options.includeRasterAppearance,
                    enabled = !exporting,
                ) { vm.onExportEvent(ExportSheetEvent.SetIncludeRasterAppearance(it)) }
            }
            Text(stringResource(R.string.export_color_space), style = MaterialTheme.typography.labelLarge)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ExportProfile.entries.forEach { p ->
                    FilterChip(
                        selected = export.options.profile == p,
                        enabled = !exporting,
                        onClick = { vm.onExportEvent(ExportSheetEvent.SetProfile(p)) },
                        label = { Text(stringResource(profileLabel(p))) },
                    )
                }
            }
            Text(
                stringResource(if (export.options.profile == ExportProfile.DOCUMENT) R.string.export_color_space_keep else R.string.export_color_space_convert),
                style = MaterialTheme.typography.bodySmall,
            )
            if (export.bitDepthEnabled) {
                Text(stringResource(R.string.export_bit_depth), style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (bits in listOf(8, 16)) {
                        FilterChip(
                            selected = export.options.bitDepth == bits,
                            enabled = !exporting,
                            onClick = { vm.onExportEvent(ExportSheetEvent.SetBitDepth(bits)) },
                            label = { Text(stringResource(R.string.export_bit_depth_value, bits)) },
                        )
                    }
                }
                if (export.options.bitDepth == 16) Text(stringResource(R.string.export_bit_depth_16_help), style = MaterialTheme.typography.bodySmall)
            }
            OptionRow(
                label = stringResource(R.string.export_opt_profile),
                help = stringResource(if (export.embedProfileLocked) R.string.export_opt_profile_locked else R.string.export_opt_profile_help),
                checked = export.options.embedColorProfile,
                enabled = !exporting && !export.embedProfileLocked,
            ) { vm.onExportEvent(ExportSheetEvent.SetEmbedColorProfile(it)) }

            if (export.destination == ExportDestination.ILLUSTRATOR) {
                Text(stringResource(R.string.export_illustrator_hint), style = MaterialTheme.typography.bodySmall)
            }

            HorizontalDivider()
            PreflightSummary(export)

            // Actionable blocking message (both-off, or engine-blocked).
            export.blockingMessage?.let { msg ->
                val text = if (msg == ExportCopy.BOTH_OFF_MESSAGE) stringResource(R.string.export_both_off) else msg
                Text(
                    text,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
            }

            PhaseStatus(export, onAcknowledge = { vm.onExportEvent(ExportSheetEvent.Acknowledge) })

            Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                Button(
                    onClick = { createDoc.launch(ExportFileNames.suggest(state.docName, export.format)) },
                    enabled = export.canExport,
                    modifier = Modifier.heightIn(min = 48.dp),
                ) { Text(stringResource(R.string.export_action)) }
                TextButton(onClick = vm::closeExport, enabled = !exporting, modifier = Modifier.heightIn(min = 48.dp)) {
                    Text(stringResource(R.string.action_close))
                }
            }

            HorizontalDivider()
            OutlinedButton(
                onClick = { saveCopy.launch(ExportFileNames.suggestProject(state.docName)) },
                enabled = !exporting,
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.export_native_copy)) }
            Text(stringResource(R.string.export_native_copy_help), style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
private fun OptionRow(label: String, help: String, checked: Boolean, enabled: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .toggleable(value = checked, enabled = enabled, role = Role.Checkbox, onValueChange = onChange),
    ) {
        Checkbox(checked = checked, onCheckedChange = null, enabled = enabled)
        Column(Modifier.padding(start = 12.dp)) {
            Text(label, style = MaterialTheme.typography.bodyLarge)
            Text(help, style = MaterialTheme.typography.bodySmall)
        }
    }
}

@StringRes
private fun profileLabel(p: ExportProfile): Int = when (p) {
    ExportProfile.DOCUMENT -> R.string.export_profile_document
    ExportProfile.SRGB -> R.string.export_profile_srgb
    ExportProfile.DISPLAY_P3 -> R.string.export_profile_display_p3
    ExportProfile.ADOBE_RGB -> R.string.export_profile_adobe_rgb
}

/** Counts before export (MASTER_PLAN §2.3); same rules as ExportCopy.summaryLines, localized. */
@Composable
private fun PreflightSummary(export: ExportSheetState) {
    when (val p = export.preflight) {
        is PreflightState.Loading -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            CircularProgressIndicator(Modifier.padding(4.dp))
            Text(stringResource(R.string.export_preflight_loading))
        }
        is PreflightState.Failed -> Text(
            stringResource(R.string.export_preflight_failed, p.error.message.ifBlank { p.error.code }),
            color = MaterialTheme.colorScheme.error,
        )
        is PreflightState.Ready -> if (p.key == export.preflightKey) {
            val info = p.info
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (info.format.isIllustratorHandoff) {
                    Text(pluralStringResource(R.plurals.export_vector_paths, info.vectorPathCount, info.vectorPathCount))
                    Text(pluralStringResource(R.plurals.export_raster_only_strokes, info.rasterOnlyStrokeCount, info.rasterOnlyStrokeCount))
                }
                Text(pluralStringResource(R.plurals.export_rasterized_layers, info.rasterizedLayerCount, info.rasterizedLayerCount))
                Text(stringResource(R.string.export_profile_line, info.profile))
                if (info.bitDepth == 16) Text(stringResource(R.string.export_bit_depth_value, 16))
                if (info.gamutClippingPossible) Text(stringResource(R.string.export_gamut_note))
                info.warnings.forEach { w -> Text("• ${w.message}", style = MaterialTheme.typography.bodySmall) }
            }
        }
        PreflightState.Idle -> Unit
    }
}

@Composable
private fun PhaseStatus(export: ExportSheetState, onAcknowledge: () -> Unit) {
    val mod = Modifier.semantics { liveRegion = LiveRegionMode.Polite }
    when (val ph = export.phase) {
        ExportPhase.Editing -> Unit
        is ExportPhase.Exporting -> Row(mod, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            CircularProgressIndicator()
            Text(stringResource(R.string.export_in_progress))
        }
        is ExportPhase.Succeeded -> Column(mod) {
            Text(stringResource(R.string.export_done_title), style = MaterialTheme.typography.titleMedium)
            Text(
                stringResource(
                    R.string.export_done_body,
                    stringResource(formatLabel(ph.report.format)),
                    ph.report.vectorObjectCount,
                    ph.report.rasterizedObjectCount,
                    ph.report.profile,
                ),
            )
            if (ph.report.outOfGamutPixels > 0) Text(stringResource(R.string.export_clipped_pixels, ph.report.outOfGamutPixels))
            ph.report.warnings.forEach { w -> Text("• ${w.message}", style = MaterialTheme.typography.bodySmall) }
            TextButton(onClick = onAcknowledge) { Text(stringResource(R.string.action_ok)) }
        }
        is ExportPhase.Failed -> Column(mod) {
            Text(stringResource(R.string.export_failed_title), style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.error)
            Text(if (ph.message == ExportCopy.NOT_VERIFIED_MESSAGE) stringResource(R.string.export_not_verified) else ph.message)
            TextButton(onClick = onAcknowledge) { Text(stringResource(R.string.action_ok)) }
        }
    }
}
