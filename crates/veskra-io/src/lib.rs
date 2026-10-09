//! # veskra-io — exporters (FMT)
//!
//! Every exporter implements [`Exporter`]: a side-effect-free `preflight` that reports what will be
//! preserved/flattened, and `export`, which writes to a `Write` sink from an immutable document
//! snapshot and returns an [`ExportReport`] describing what was *actually* written.
//!
//! Module ownership (parallel work): `png`, `psd` → FMT-A; `plan`, `svg`, `pdf` → FMT-B;
//! `lib.rs`, `cancel` → ORCH. Writing to files (temp + validate + rename) is done by
//! [`export_to_path`], which only reports success after flush, close and structural validation.

pub mod cancel;
pub mod import;
pub mod pdf;
pub mod plan;
pub mod png;
pub mod psd;
pub mod svg;

pub use cancel::CancelToken;

use std::io::Write;
use std::path::Path;
use veskra_core::export::{ExportFormat, ExportOptions, ExportPreflight, ExportReport, ValidationStatus};
use veskra_core::{ErrorCode, VeskraError};
use veskra_doc::Document;

/// Common exporter contract (ARCHITECTURE §H).
pub trait Exporter {
    fn format(&self) -> ExportFormat;
    /// What the export would do; never fails (blocked states are reported in `blocked_reason`).
    fn preflight(&self, doc: &Document, opts: &ExportOptions) -> ExportPreflight;
    /// Write the file. Must check `cancel` regularly and return `ErrorCode::Cancelled` when set.
    fn export(
        &self,
        doc: &Document,
        opts: &ExportOptions,
        out: &mut dyn Write,
        cancel: &CancelToken,
    ) -> Result<ExportReport, VeskraError>;
    /// Structural re-validation of written bytes (independent of the writer's internal state).
    fn validate(&self, bytes: &[u8]) -> Result<(), VeskraError>;
}

/// Exporter for a format.
pub fn exporter_for(format: ExportFormat) -> Box<dyn Exporter + Send + Sync> {
    match format {
        ExportFormat::Png => Box::new(png::PngExporter),
        ExportFormat::Psd => Box::new(psd::PsdExporter),
        ExportFormat::Svg => Box::new(svg::SvgExporter::default()),
        // ADR-012 (provisional, 2026-10-09): Inkscape 1.2.2 drops hidden optional-content
        // layers on PDF import, so PDF defaults to the residual strategy (nothing editable is
        // hidden). SVG keeps hidden editable vectors (preserved by Inkscape re-save).
        ExportFormat::Pdf => {
            Box::new(pdf::PdfExporter { strategy: plan::HybridStrategy::VectorsVisibleRasterResidual })
        }
        #[allow(unreachable_patterns)]
        _ => Box::new(png::PngExporter),
    }
}

/// Export to `path` safely: write `<path>.tmp-*`, flush + fsync, re-read and validate, then rename.
/// On any error or cancellation the temp file is removed and no file is reported as completed
/// (PSD-007). The extension of `path` must match the format (never `.ai`, ILL-008).
pub fn export_to_path(
    doc: &Document,
    format: ExportFormat,
    opts: &ExportOptions,
    path: &Path,
    cancel: &CancelToken,
) -> Result<ExportReport, VeskraError> {
    opts.validate(format)?;
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    if ext.as_deref() != Some(format.extension()) {
        return Err(VeskraError::invalid(format!("output file must use the .{} extension", format.extension())));
    }
    let exporter = exporter_for(format);
    let start = std::time::Instant::now();
    let mut buf: Vec<u8> = Vec::new();
    let mut report = exporter.export(doc, opts, &mut buf, cancel)?;
    if cancel.is_cancelled() {
        return Err(VeskraError::new(ErrorCode::Cancelled, "export cancelled"));
    }
    exporter.validate(&buf).map_err(|e| VeskraError::new(e.code, format!("validation failed: {}", e.message)))?;
    let tmp = path.with_extension(format!("{}.tmp-{:08x}", format.extension(), std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&buf)?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        let back = std::fs::read(&tmp)?;
        if back != buf {
            return Err(std::io::Error::other("read-back mismatch"));
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        let code = if e.raw_os_error() == Some(28) || e.kind() == std::io::ErrorKind::StorageFull {
            ErrorCode::StorageFull
        } else if e.kind() == std::io::ErrorKind::PermissionDenied {
            ErrorCode::FilePermissionDenied
        } else {
            ErrorCode::Io
        };
        return Err(VeskraError::new(code, format!("could not write export: {}", e.kind())));
    }
    report.success = true;
    report.file_bytes = buf.len() as u64;
    report.duration_ms = start.elapsed().as_millis() as u64;
    report.validation = ValidationStatus::StructurallyValidated;
    report.file_uri = path.file_name().map(|n| n.to_string_lossy().into_owned());
    Ok(report)
}
