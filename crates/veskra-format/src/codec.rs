//! Schema-1 bundle encoder/decoder (ADR-009).

use crate::dto::*;
use crate::error::{FormatError, FormatWarning};
use crate::migrate::{MIGRATIONS, has_path, migrate};
use crate::zip::{Method, ZipReader, ZipWriter};
use crate::{CURRENT_SCHEMA, FORMAT_NAME, MIMETYPE, MIN_NATIVE_SCHEMA, PIXEL_V1};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::Arc;
use veskra_color::IccProfile;
use veskra_core::{
    BlendMode, BlendSpace, ColorRgba, ErrorCode, LayerId, SampleFlags, SizeU, StrokeSample, TILE_SIZE, VeskraError,
    limits,
};
use veskra_doc::{
    DocColor, Document, DocumentParts, GroupContent, Layer, LayerKind, LayerProps, PixelContent, StrokeComposition,
    StrokeGeometry, StrokeMode, StrokeRecord,
};
use veskra_raster::{MaskGrid, MaskTile, TILE_BYTES, TILE_PIXELS, Tile, TileCoord, TileGrid};
use veskra_vector::{BezPath, PathEl};

pub(crate) const ENTRY_MIMETYPE: &str = "mimetype";
pub(crate) const ENTRY_MANIFEST: &str = "manifest.json";
pub(crate) const ENTRY_INTEGRITY: &str = "integrity.json";
pub(crate) const ENTRY_DOCUMENT: &str = "project/document.json";
pub(crate) const ENTRY_PREVIEW: &str = "preview/composite.png";

/// Maximum size of any JSON entry (bounds `serde_json::Value` memory on load).
pub const MAX_JSON_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum |tx| / |ty| of a stored tile (64 * 4096 = 262 144 px from the origin).
pub const MAX_TILE_COORD: i32 = 4096;
/// Stroke sample entry magic and version.
pub const STROKE_MAGIC: [u8; 4] = *b"VSKS";
pub const STROKE_VERSION: u16 = 1;
const STROKE_HEADER_BYTES: usize = 10;
/// x f32, y f32, t u64, pressure f32, tilt f32, azimuth f32, rotation f32, flags u16.
pub const STROKE_SAMPLE_BYTES: usize = 34;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const MAX_UNKNOWN_FIELD_REPORTS: usize = 64;

/// Caller-supplied save metadata (kept out of the document so tests can be deterministic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveMeta {
    pub app_version: String,
    /// Creation timestamp written to the manifest verbatim (e.g. RFC 3339).
    pub created: String,
}

impl SaveMeta {
    pub fn new(app_version: impl Into<String>, created: impl Into<String>) -> Self {
        Self { app_version: app_version.into(), created: created.into() }
    }
}

/// A successfully decoded bundle.
#[derive(Debug, Clone)]
pub struct LoadedDocument {
    pub doc: Document,
    pub manifest: Manifest,
    pub warnings: Vec<FormatWarning>,
}

/// One archive entry as reported by [`inspect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub name: String,
    pub method: Method,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

/// Structural summary of a bundle (for the CLI / gallery), without decoding pixels.
#[derive(Debug, Clone)]
pub struct BundleInfo {
    pub manifest: Manifest,
    /// Whether this reader can open the bundle (`minReader` gate and migration path).
    pub schema_supported: bool,
    pub entries: Vec<EntryInfo>,
    pub total_compressed: u64,
    pub total_uncompressed: u64,
}

fn corrupt(m: impl Into<String>) -> FormatError {
    FormatError::Corrupt(m.into())
}

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn doc_err(e: VeskraError) -> FormatError {
    match e.code {
        ErrorCode::AllocationLimit => FormatError::Limit(e.message),
        ErrorCode::InvalidProfile => FormatError::InvalidProfile(e.message),
        _ => FormatError::InvalidDocument(e.message),
    }
}

fn tile_name(layer: LayerId, c: TileCoord) -> String {
    format!("raster/{layer}/{}_{}.tile", c.tx, c.ty)
}
fn mask_name(layer: LayerId, c: TileCoord) -> String {
    format!("masks/{layer}/{}_{}.mask", c.tx, c.ty)
}
fn selection_name(c: TileCoord) -> String {
    format!("selection/{}_{}.mask", c.tx, c.ty)
}
fn stroke_name(id: veskra_core::StrokeId) -> String {
    format!("strokes/{id}.bin")
}
fn brush_asset_name(hash: &str) -> String {
    format!("brushes/{hash}.png")
}
fn is_asset_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn profile_name(hash: &[u8; 32]) -> String {
    format!("profiles/{}.icc", hex(hash))
}

fn coord_ok(c: TileCoord) -> bool {
    (-MAX_TILE_COORD..=MAX_TILE_COORD).contains(&c.tx) && (-MAX_TILE_COORD..=MAX_TILE_COORD).contains(&c.ty)
}

fn blend_space_str(b: BlendSpace) -> &'static str {
    match b {
        BlendSpace::GammaEncoded => "gammaEncoded",
        BlendSpace::Linear => "linear",
    }
}

fn kind_str(k: &LayerKind) -> &'static str {
    match k {
        LayerKind::Raster(_) => "raster",
        LayerKind::Vector(_) => "vector",
        LayerKind::Group(_) => "group",
    }
}

// ---------------------------------------------------------------------------------------------
// Binary payload codecs
// ---------------------------------------------------------------------------------------------

/// Tile payload: 64*64*4 `u16` little-endian, row-major premultiplied RGBA.
pub fn encode_tile(t: &Tile) -> Vec<u8> {
    let mut out = Vec::with_capacity(TILE_BYTES);
    for v in t.pixels() {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Inverse of [`encode_tile`]; enforces size and the premultiplied invariant (`r,g,b <= a`).
pub fn decode_tile(data: &[u8]) -> Result<Tile, FormatError> {
    if data.len() != TILE_BYTES {
        return Err(FormatError::CorruptTile(format!("tile payload is {} bytes, expected {TILE_BYTES}", data.len())));
    }
    let px: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Tile::from_pixels(&px).map_err(|e| FormatError::CorruptTile(e.to_string()))
}

fn opt_bits(v: Option<f32>) -> [u8; 4] {
    v.unwrap_or(f32::NAN).to_bits().to_le_bytes()
}

/// `strokes/<id>.bin`: `"VSKS"`, version u16, count u32, then per sample
/// x f32, y f32, t_ns u64, pressure f32, tilt f32, azimuth f32, rotation f32, flags u16 (LE;
/// NaN encodes an absent axis).
pub fn encode_samples(samples: &[StrokeSample]) -> Vec<u8> {
    let mut out = Vec::with_capacity(STROKE_HEADER_BYTES + samples.len() * STROKE_SAMPLE_BYTES);
    out.extend_from_slice(&STROKE_MAGIC);
    out.extend_from_slice(&STROKE_VERSION.to_le_bytes());
    out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.x.to_bits().to_le_bytes());
        out.extend_from_slice(&s.y.to_bits().to_le_bytes());
        out.extend_from_slice(&s.t_ns.to_le_bytes());
        out.extend_from_slice(&s.pressure.to_bits().to_le_bytes());
        out.extend_from_slice(&opt_bits(s.tilt));
        out.extend_from_slice(&opt_bits(s.azimuth));
        out.extend_from_slice(&opt_bits(s.rotation));
        out.extend_from_slice(&s.flags.0.to_le_bytes());
    }
    out
}

/// Inverse of [`encode_samples`]. Validation of each sample (confirmed-only, ranges,
/// monotonic time) is done by `StrokeRecord::new`.
pub fn decode_samples(data: &[u8]) -> Result<Vec<StrokeSample>, FormatError> {
    let head = data.get(..STROKE_HEADER_BYTES).ok_or_else(|| corrupt("stroke entry truncated"))?;
    if head[..4] != STROKE_MAGIC {
        return Err(corrupt("stroke entry has bad magic"));
    }
    let version = u16::from_le_bytes([head[4], head[5]]);
    if version != STROKE_VERSION {
        return Err(FormatError::UnsupportedFeature(format!("stroke sample version {version}")));
    }
    let count = u32::from_le_bytes([head[6], head[7], head[8], head[9]]) as usize;
    if count > limits::MAX_STROKE_SAMPLES {
        return Err(FormatError::Limit("stroke exceeds sample limit".into()));
    }
    let body = &data[STROKE_HEADER_BYTES..];
    if body.len() != count * STROKE_SAMPLE_BYTES {
        return Err(corrupt("stroke entry length does not match its sample count"));
    }
    let f = |b: &[u8], o: usize| f32::from_bits(u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]));
    let opt = |v: f32| if v.is_nan() { None } else { Some(v) };
    let mut out = Vec::with_capacity(count);
    for b in body.chunks_exact(STROKE_SAMPLE_BYTES) {
        let mut t = [0u8; 8];
        t.copy_from_slice(&b[8..16]);
        out.push(StrokeSample {
            x: f(b, 0),
            y: f(b, 4),
            t_ns: u64::from_le_bytes(t),
            pressure: f(b, 16),
            tilt: opt(f(b, 20)),
            azimuth: opt(f(b, 24)),
            rotation: opt(f(b, 28)),
            flags: SampleFlags(u16::from_le_bytes([b[32], b[33]])),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------------------------

enum Source<'a> {
    Bytes(Cow<'a, [u8]>),
    Tile(&'a Tile),
    Mask(&'a MaskTile),
    Samples(&'a [StrokeSample]),
}

impl Source<'_> {
    fn bytes(&self) -> Cow<'_, [u8]> {
        match self {
            Source::Bytes(b) => Cow::Borrowed(b.as_ref()),
            Source::Tile(t) => Cow::Owned(encode_tile(t)),
            Source::Mask(m) => Cow::Borrowed(m.data()),
            Source::Samples(s) => Cow::Owned(encode_samples(s)),
        }
    }
}

struct Plan<'a> {
    entries: BTreeMap<String, (Source<'a>, Method)>,
}

impl<'a> Plan<'a> {
    fn add(&mut self, name: String, src: Source<'a>, method: Method) -> Result<(), FormatError> {
        if self.entries.len() >= crate::zip::MAX_ENTRIES {
            return Err(FormatError::Limit(format!(
                "document needs more than {} bundle entries (zip64 unsupported in schema 1)",
                crate::zip::MAX_ENTRIES
            )));
        }
        if self.entries.insert(name, (src, method)).is_some() {
            return Err(FormatError::InvalidDocument("duplicate bundle entry".into()));
        }
        Ok(())
    }

    fn mask(&mut self, m: &'a MaskGrid, name: impl Fn(TileCoord) -> String) -> Result<MaskDto, FormatError> {
        let default = m.default_value();
        let mut tiles = Vec::new();
        for (c, t) in m.iter_tiles() {
            if t.uniform_value() == Some(default) {
                continue; // equal to "no tile" (MaskGrid equality treats them the same)
            }
            if !coord_ok(c) {
                return Err(FormatError::Limit("mask tile coordinate out of range".into()));
            }
            self.add(name(c), Source::Mask(t), Method::Deflated)?;
            tiles.push([c.tx, c.ty]);
        }
        Ok(MaskDto { default, tiles })
    }
}

fn path_finite(p: &BezPath) -> bool {
    p.els.iter().all(|e| match e {
        PathEl::MoveTo(a) | PathEl::LineTo(a) => a.is_finite(),
        PathEl::CubicTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
        PathEl::Close => true,
    })
}

/// Encode `doc` into an in-memory bundle.
pub fn encode(doc: &Document, meta: &SaveMeta, preview_png: Option<&[u8]>) -> Result<Vec<u8>, FormatError> {
    let mut out = Vec::new();
    encode_to(doc, meta, preview_png, &mut out)?;
    Ok(out)
}

/// Encode `doc` streaming into `w`. Returns the number of bytes written. Output is
/// deterministic: identical inputs give identical bytes.
pub fn encode_to<W: Write>(
    doc: &Document,
    meta: &SaveMeta,
    preview_png: Option<&[u8]>,
    w: W,
) -> Result<u64, FormatError> {
    if let Some(p) = preview_png
        && !p.starts_with(&PNG_SIGNATURE)
    {
        return Err(FormatError::InvalidArgument("preview is not a PNG".into()));
    }
    doc.validate().map_err(doc_err)?;
    let icc: &[u8] = &doc.color.icc_bytes;
    if icc.len() > limits::MAX_ICC_BYTES {
        return Err(FormatError::Limit("ICC profile too large".into()));
    }
    if sha256(icc) != doc.color.profile.icc_hash {
        return Err(FormatError::InvalidDocument("document ICC bytes do not match its profile hash".into()));
    }

    let mut plan = Plan { entries: BTreeMap::new() };
    let profile_entry = profile_name(&doc.color.profile.icc_hash);
    plan.add(profile_entry.clone(), Source::Bytes(Cow::Borrowed(icc)), Method::Deflated)?;

    let mut layers = Vec::with_capacity(doc.layer_count());
    for l in doc.iter_layers() {
        let props = PropsDto {
            name: l.props.name.clone(),
            visible: l.props.visible,
            opacity: l.props.opacity,
            blend: l.props.blend.as_str().to_string(),
            locked: l.props.locked,
            alpha_locked: l.props.alpha_locked,
            clip_to_below: l.props.clip_to_below,
        };
        let (group, pixels) = match &l.kind {
            LayerKind::Group(g) => (
                Some(GroupDto { children: g.children.clone(), pass_through: g.pass_through, collapsed: g.collapsed }),
                None,
            ),
            LayerKind::Raster(p) | LayerKind::Vector(p) => {
                let mut tiles = Vec::new();
                for (c, t) in p.tiles.iter_tiles() {
                    if t.is_transparent() {
                        continue; // equal to "no tile" (TileGrid equality treats them the same)
                    }
                    if !coord_ok(c) {
                        return Err(FormatError::Limit("raster tile coordinate out of range".into()));
                    }
                    plan.add(tile_name(l.id, c), Source::Tile(t), Method::Deflated)?;
                    tiles.push([c.tx, c.ty]);
                }
                let id = l.id;
                let mask = match &p.mask {
                    Some(m) => Some(plan.mask(m, |c| mask_name(id, c))?),
                    None => None,
                };
                (
                    None,
                    Some(PixelsDto { strokes: p.strokes.clone(), raster_edits: p.raster_edits.clone(), tiles, mask }),
                )
            }
        };
        layers.push(LayerDto { id: l.id, parent: l.parent, kind: kind_str(&l.kind).into(), props, group, pixels });
    }

    let mut strokes = Vec::with_capacity(doc.stroke_count());
    for s in doc.iter_strokes() {
        let geometry = match &s.geometry {
            Some(g) => {
                if !(path_finite(&g.centerline) && path_finite(&g.outline) && g.fit_error.is_finite()) {
                    return Err(FormatError::InvalidDocument("stroke geometry is not finite".into()));
                }
                Some(GeometryDto {
                    centerline: g.centerline.clone(),
                    outline: g.outline.clone(),
                    fit_error: g.fit_error,
                })
            }
            None => None,
        };
        let sample_entry = stroke_name(s.id);
        plan.add(sample_entry.clone(), Source::Samples(s.samples()), Method::Deflated)?;
        strokes.push(StrokeDto {
            id: s.id,
            layer: s.layer,
            brush: BrushDto {
                id: s.brush_id.clone(),
                version: s.brush_version,
                params: s.brush_params.clone(),
                assets: s.brush_assets.clone(),
            },
            color: [s.color.r, s.color.g, s.color.b, s.color.a],
            seed: s.deterministic_seed,
            profile: s.color_profile.clone(),
            composition: CompositionDto {
                mode: match s.composition.mode {
                    StrokeMode::Paint => "paint".into(),
                    StrokeMode::Erase => "erase".into(),
                },
                opacity: s.composition.opacity,
                alpha_locked: s.composition.alpha_locked,
                selection_clipped: s.composition.selection_clipped,
            },
            vector_eligible: s.vector_eligible,
            geometry,
            bounds: s.bounds,
            sample_entry,
            sample_count: s.samples().len() as u32,
        });
    }

    let selection = match &doc.selection {
        Some(m) => Some(plan.mask(m, selection_name)?),
        None => None,
    };

    // Schema 2: embed the brush assets the strokes reference. A reference whose bytes the
    // document does not hold (it was already missing when the file was opened) stays a
    // reference; the reader reports it as `MissingBrushAssets`.
    let mut brush_assets = Vec::new();
    for hash in doc.referenced_brush_assets() {
        if let Some(png) = doc.brush_asset(hash) {
            plan.add(brush_asset_name(hash), Source::Bytes(Cow::Borrowed(png)), Method::Stored)?;
            brush_assets.push(hash.to_string());
        }
    }
    let uses_v2 = !brush_assets.is_empty() || doc.iter_strokes().any(|s| !s.brush_assets.is_empty());
    let schema = if uses_v2 { CURRENT_SCHEMA } else { MIN_NATIVE_SCHEMA };

    let dto = DocumentDto {
        id: doc.id,
        width: doc.width(),
        height: doc.height(),
        dpi: doc.dpi,
        color: ColorDto {
            profile: doc.color.profile.clone(),
            profile_entry,
            assumed: doc.color.assumed,
            blend_space: blend_space_str(doc.color.blend_space).into(),
        },
        root: doc.root().to_vec(),
        layers,
        strokes,
        selection,
        active_layer: doc.active_layer,
        brush_assets,
    };
    let doc_json = to_json(&dto)?;
    plan.add(ENTRY_DOCUMENT.into(), Source::Bytes(Cow::Owned(doc_json)), Method::Deflated)?;
    if let Some(p) = preview_png {
        plan.add(ENTRY_PREVIEW.into(), Source::Bytes(Cow::Borrowed(p)), Method::Stored)?;
    }

    let manifest = Manifest {
        format: FORMAT_NAME.into(),
        schema,
        min_reader: MIN_NATIVE_SCHEMA,
        app_version: meta.app_version.clone(),
        created: meta.created.clone(),
        document_id: doc.id,
        tile_size: TILE_SIZE,
        pixel: PIXEL_V1.into(),
        entries: plan.entries.keys().cloned().collect(),
    };

    let mut zw = ZipWriter::new(w);
    let mut integ = Vec::with_capacity(plan.entries.len() + 2);
    let mut put = |zw: &mut ZipWriter<W>, name: &str, data: &[u8], m: Method| -> Result<(), FormatError> {
        zw.add(name, data, m)?;
        integ.push(IntegrityEntry { name: name.into(), size: data.len() as u64, sha256: hex(&sha256(data)) });
        Ok(())
    };
    put(&mut zw, ENTRY_MIMETYPE, MIMETYPE.as_bytes(), Method::Stored)?;
    put(&mut zw, ENTRY_MANIFEST, &to_json(&manifest)?, Method::Deflated)?;
    for (name, (src, method)) in &plan.entries {
        put(&mut zw, name, &src.bytes(), *method)?;
    }
    integ.sort_by(|a, b| a.name.cmp(&b.name));
    let integrity = Integrity { algorithm: "sha256".into(), entries: integ };
    zw.add(ENTRY_INTEGRITY, &to_json(&integrity)?, Method::Deflated)?;
    let (_, total) = zw.finish()?;
    Ok(total)
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, FormatError> {
    serde_json::to_vec_pretty(v).map_err(|e| FormatError::InvalidDocument(format!("JSON encoding failed: {e}")))
}

// ---------------------------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------------------------

/// Schema gate on the raw manifest value. Returns `(schema, minReader, warnings)`.
fn gate(v: &Value) -> Result<(u32, u32, Vec<FormatWarning>), FormatError> {
    if v.get("format").and_then(Value::as_str) != Some(FORMAT_NAME) {
        return Err(corrupt("manifest does not declare format \"veskra\""));
    }
    let num = |k: &str| v.get(k).and_then(Value::as_u64).and_then(|x| u32::try_from(x).ok());
    let schema = num("schema").ok_or_else(|| corrupt("manifest schema missing or invalid"))?;
    let min_reader = num("minReader").ok_or_else(|| corrupt("manifest minReader missing or invalid"))?;
    if min_reader > schema {
        return Err(corrupt("manifest minReader exceeds schema"));
    }
    let unsupported = FormatError::UnsupportedSchema { schema, min_reader, supported: CURRENT_SCHEMA };
    if min_reader > CURRENT_SCHEMA {
        return Err(unsupported);
    }
    let mut warnings = Vec::new();
    if schema > CURRENT_SCHEMA {
        warnings.push(FormatWarning::NewerSchema { schema });
    } else if schema < MIN_NATIVE_SCHEMA {
        if !has_path(MIGRATIONS, schema, MIN_NATIVE_SCHEMA) {
            return Err(unsupported);
        }
        warnings.push(FormatWarning::Migrated { from: schema, to: MIN_NATIVE_SCHEMA });
    }
    Ok((schema, min_reader, warnings))
}

/// Collect JSON object keys present in `orig` but absent from the re-serialized `canon`.
fn unknown_fields(orig: &Value, canon: &Value, path: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_UNKNOWN_FIELD_REPORTS {
        return;
    }
    match (orig, canon) {
        (Value::Object(o), Value::Object(c)) => {
            for (k, v) in o {
                let p = format!("{path}/{k}");
                match c.get(k) {
                    None => {
                        if out.len() < MAX_UNKNOWN_FIELD_REPORTS {
                            out.push(p);
                        }
                    }
                    Some(cv) => unknown_fields(v, cv, &p, out),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                unknown_fields(x, y, &format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

/// Parse a JSON entry into `T`, reporting unknown fields as a warning.
fn parse_dto<T>(v: &Value, entry: &str, warnings: &mut Vec<FormatWarning>) -> Result<T, FormatError>
where
    T: for<'de> Deserialize<'de> + serde::Serialize,
{
    let dto = T::deserialize(v).map_err(|e| corrupt(format!("{entry}: {e}")))?;
    let canon = serde_json::to_value(&dto).map_err(|e| corrupt(format!("{entry}: {e}")))?;
    let mut paths = Vec::new();
    unknown_fields(v, &canon, "", &mut paths);
    if !paths.is_empty() {
        warnings.push(FormatWarning::UnknownFields { entry: entry.into(), paths });
    }
    Ok(dto)
}

/// An opened, structurally validated bundle whose manifest passed the schema gate.
struct Bundle<'a> {
    zip: ZipReader<'a>,
    manifest: Manifest,
    integrity: BTreeMap<String, [u8; 32]>,
    warnings: Vec<FormatWarning>,
}

impl<'a> Bundle<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let zip = ZipReader::parse(bytes)?;
        check_mimetype(&zip)?;
        let (manifest_raw, mv) = read_manifest_value(&zip)?;
        let (_schema, _min_reader, mut warnings) = gate(&mv)?;
        let manifest: Manifest = parse_dto(&mv, ENTRY_MANIFEST, &mut warnings)?;
        if manifest.tile_size != TILE_SIZE || manifest.pixel != PIXEL_V1 {
            return Err(FormatError::UnsupportedFeature(format!(
                "tile size {} / pixel layout {:?}",
                manifest.tile_size, manifest.pixel
            )));
        }

        // integrity.json: every other entry listed exactly once with matching size.
        let ie = zip.entry(ENTRY_INTEGRITY).ok_or_else(|| corrupt("integrity.json missing"))?;
        if ie.uncompressed_size > MAX_JSON_BYTES {
            return Err(FormatError::Limit("integrity.json too large".into()));
        }
        let iv: Value = serde_json::from_slice(&zip.read(ie)?).map_err(|_| corrupt("integrity.json is not JSON"))?;
        let integ: Integrity = parse_dto(&iv, ENTRY_INTEGRITY, &mut warnings)?;
        if integ.algorithm != "sha256" {
            return Err(FormatError::UnsupportedFeature(format!("integrity algorithm {:?}", integ.algorithm)));
        }
        let mut integrity = BTreeMap::new();
        for e in integ.entries {
            let z = zip.entry(&e.name).ok_or_else(|| corrupt(format!("integrity lists missing entry {}", e.name)))?;
            if e.name == ENTRY_INTEGRITY || z.uncompressed_size != e.size {
                return Err(corrupt(format!("integrity size mismatch for {}", e.name)));
            }
            let h = parse_hex32(&e.sha256).ok_or_else(|| corrupt("integrity hash malformed"))?;
            if integrity.insert(e.name, h).is_some() {
                return Err(corrupt("integrity lists an entry twice"));
            }
        }
        if let Some(e) = zip.entries().iter().find(|e| e.name != ENTRY_INTEGRITY && !integrity.contains_key(&e.name)) {
            return Err(corrupt(format!("entry {} is not covered by integrity.json", e.name)));
        }
        let b = Bundle { zip, manifest, integrity, warnings };
        b.check_hash(ENTRY_MANIFEST, &manifest_raw)?;
        for name in &b.manifest.entries {
            if b.zip.entry(name).is_none() {
                return Err(corrupt(format!("manifest lists missing entry {name}")));
            }
        }
        Ok(b)
    }

    fn check_hash(&self, name: &str, data: &[u8]) -> Result<(), FormatError> {
        match self.integrity.get(name) {
            Some(h) if *h == sha256(data) => Ok(()),
            _ => Err(corrupt(format!("integrity hash mismatch for {name}"))),
        }
    }

    fn declared_size(&self, name: &str) -> Result<u64, FormatError> {
        self.zip.entry(name).map(|e| e.uncompressed_size).ok_or_else(|| corrupt(format!("missing entry {name}")))
    }

    /// Read, CRC-check and hash-check one entry.
    fn read_verified(&self, name: &str) -> Result<Vec<u8>, FormatError> {
        let data = self.zip.read_name(name)?;
        self.check_hash(name, &data)?;
        Ok(data)
    }

    fn read_json(&self, name: &str) -> Result<Value, FormatError> {
        if self.declared_size(name)? > MAX_JSON_BYTES {
            return Err(FormatError::Limit(format!("{name} too large")));
        }
        serde_json::from_slice(&self.read_verified(name)?)
            .map_err(|e| corrupt(format!("{name} is not valid JSON: {e}")))
    }
}

fn check_mimetype(zip: &ZipReader<'_>) -> Result<(), FormatError> {
    let first = zip.entries().first().ok_or_else(|| corrupt("empty archive"))?;
    if first.name != ENTRY_MIMETYPE || first.method != Method::Stored || zip.read(first)? != MIMETYPE.as_bytes() {
        return Err(corrupt("not a Veskra bundle (mimetype entry missing or wrong)"));
    }
    Ok(())
}

fn read_manifest_value(zip: &ZipReader<'_>) -> Result<(Vec<u8>, Value), FormatError> {
    let me = zip.entry(ENTRY_MANIFEST).ok_or_else(|| corrupt("manifest.json missing"))?;
    if me.uncompressed_size > MAX_JSON_BYTES {
        return Err(FormatError::Limit("manifest.json too large".into()));
    }
    let raw = zip.read(me)?;
    let v = serde_json::from_slice(&raw).map_err(|_| corrupt("manifest.json is not valid JSON"))?;
    Ok((raw, v))
}

/// Summarize a bundle (entries, sizes, manifest) without decoding pixels. Works for bundles
/// this reader cannot open (`schema_supported == false`) as long as the manifest parses.
pub fn inspect(bytes: &[u8]) -> Result<BundleInfo, FormatError> {
    let zip = ZipReader::parse(bytes)?;
    check_mimetype(&zip)?;
    let (_, mv) = read_manifest_value(&zip)?;
    let schema_supported = gate(&mv).is_ok();
    let manifest = Manifest::deserialize(&mv).map_err(|e| corrupt(format!("manifest.json: {e}")))?;
    let entries: Vec<EntryInfo> = zip
        .entries()
        .iter()
        .map(|e| EntryInfo {
            name: e.name.clone(),
            method: e.method,
            compressed_size: e.compressed_size,
            uncompressed_size: e.uncompressed_size,
        })
        .collect();
    Ok(BundleInfo {
        manifest,
        schema_supported,
        total_compressed: entries.iter().map(|e| e.compressed_size).sum(),
        total_uncompressed: entries.iter().map(|e| e.uncompressed_size).sum(),
        entries,
    })
}

/// Integrity-checked preview PNG of a bundle, if it has one.
pub fn read_preview(bytes: &[u8]) -> Result<Option<Vec<u8>>, FormatError> {
    let b = Bundle::open(bytes)?;
    if b.zip.entry(ENTRY_PREVIEW).is_none() {
        return Ok(None);
    }
    b.read_verified(ENTRY_PREVIEW).map(Some)
}

struct Decoder<'b, 'a> {
    b: &'b Bundle<'a>,
    used: BTreeSet<String>,
}

impl Decoder<'_, '_> {
    fn take(&mut self, name: &str) -> Result<Vec<u8>, FormatError> {
        if !self.used.insert(name.to_string()) {
            return Err(corrupt(format!("entry {name} referenced twice")));
        }
        self.b.read_verified(name)
    }

    fn tiles(&mut self, coords: &[[i32; 2]], name: impl Fn(TileCoord) -> String) -> Result<TileGrid, FormatError> {
        let mut grid = TileGrid::new();
        for [tx, ty] in coords {
            let c = TileCoord::new(*tx, *ty);
            if !coord_ok(c) {
                return Err(FormatError::Limit("tile coordinate out of range".into()));
            }
            if grid.tile(c).is_some() {
                return Err(corrupt("tile listed twice"));
            }
            let n = name(c);
            if self.b.declared_size(&n)? != TILE_BYTES as u64 {
                return Err(FormatError::CorruptTile("tile payload has the wrong size".into()));
            }
            grid.set_tile(c, Arc::new(decode_tile(&self.take(&n)?)?));
        }
        Ok(grid)
    }

    fn mask(&mut self, m: &MaskDto, name: impl Fn(TileCoord) -> String) -> Result<MaskGrid, FormatError> {
        let mut grid = match m.default {
            0 => MaskGrid::new_empty(),
            255 => MaskGrid::new_full(),
            _ => return Err(corrupt("mask default must be 0 or 255")),
        };
        for [tx, ty] in &m.tiles {
            let c = TileCoord::new(*tx, *ty);
            if !coord_ok(c) {
                return Err(FormatError::Limit("mask tile coordinate out of range".into()));
            }
            if grid.tile(c).is_some() {
                return Err(corrupt("mask tile listed twice"));
            }
            let n = name(c);
            if self.b.declared_size(&n)? != TILE_PIXELS as u64 {
                return Err(FormatError::CorruptTile("mask tile payload has the wrong size".into()));
            }
            let t = MaskTile::from_data(&self.take(&n)?).map_err(|e| FormatError::CorruptTile(e.to_string()))?;
            grid.set_tile(c, Arc::new(t));
        }
        Ok(grid)
    }
}

/// Decode and fully validate a bundle: ZIP structure, integrity hashes, schema gate/migration,
/// ICC profile, every tile (size + premultiplied invariant), stroke samples, and finally the
/// document tree via `Document::from_parts`.
pub fn decode(bytes: &[u8]) -> Result<LoadedDocument, FormatError> {
    let b = Bundle::open(bytes)?;
    let mut warnings = b.warnings.clone();
    let mut value = b.read_json(ENTRY_DOCUMENT)?;
    if b.manifest.schema < MIN_NATIVE_SCHEMA {
        value = migrate(&b.manifest, value)?;
    }
    let dto: DocumentDto = parse_dto(&value, ENTRY_DOCUMENT, &mut warnings)?;
    drop(value);

    let mut d = Decoder { b: &b, used: BTreeSet::new() };
    for n in [ENTRY_MIMETYPE, ENTRY_MANIFEST, ENTRY_INTEGRITY, ENTRY_DOCUMENT] {
        d.used.insert(n.to_string());
    }
    if dto.id != b.manifest.document_id {
        return Err(corrupt("manifest and document disagree on the document id"));
    }
    if dto.layers.len() > limits::MAX_LAYERS {
        return Err(FormatError::Limit("too many layers".into()));
    }

    // Color profile: bounded, hash-verified, parseable.
    let c = &dto.color;
    if c.profile_entry != profile_name(&c.profile.icc_hash) {
        return Err(corrupt("profile entry name does not match the profile hash"));
    }
    if b.declared_size(&c.profile_entry)? > limits::MAX_ICC_BYTES as u64 {
        return Err(FormatError::Limit("ICC profile exceeds MAX_ICC_BYTES".into()));
    }
    let icc = d.take(&c.profile_entry)?;
    if sha256(&icc) != c.profile.icc_hash {
        return Err(corrupt("ICC bytes do not match the profile hash"));
    }
    IccProfile::parse(&icc).map_err(|e| FormatError::InvalidProfile(e.to_string()))?;
    let blend_space = match c.blend_space.as_str() {
        "gammaEncoded" => BlendSpace::GammaEncoded,
        "linear" => BlendSpace::Linear,
        _ => return Err(corrupt("unknown blend space")),
    };
    let color = DocColor { profile: c.profile.clone(), icc_bytes: Arc::from(icc), assumed: c.assumed, blend_space };

    let mut layers = Vec::with_capacity(dto.layers.len());
    for l in &dto.layers {
        let blend = BlendMode::parse(&l.props.blend).ok_or_else(|| corrupt("unknown blend mode"))?;
        let props = LayerProps {
            name: l.props.name.clone(),
            visible: l.props.visible,
            opacity: l.props.opacity,
            blend,
            locked: l.props.locked,
            alpha_locked: l.props.alpha_locked,
            clip_to_below: l.props.clip_to_below,
        };
        let kind = match (l.kind.as_str(), &l.group, &l.pixels) {
            ("group", Some(g), None) => LayerKind::Group(GroupContent {
                children: g.children.clone(),
                pass_through: g.pass_through,
                collapsed: g.collapsed,
            }),
            (k @ ("raster" | "vector"), None, Some(p)) => {
                let id = l.id;
                let tiles = d.tiles(&p.tiles, |c| tile_name(id, c))?;
                let mask = match &p.mask {
                    Some(m) => Some(d.mask(m, |c| mask_name(id, c))?),
                    None => None,
                };
                let content =
                    PixelContent { tiles, mask, strokes: p.strokes.clone(), raster_edits: p.raster_edits.clone() };
                if k == "raster" { LayerKind::Raster(content) } else { LayerKind::Vector(content) }
            }
            _ => return Err(corrupt("layer kind does not match its content")),
        };
        layers.push(Layer { id: l.id, parent: l.parent, props, kind });
    }

    // Embedded brush assets (schema 2): bounded, hash-named, PNG-signed. The mask hash itself is
    // verified by veskra-brush when a stroke is re-rendered.
    if dto.brush_assets.len() > veskra_doc::MAX_DOCUMENT_BRUSH_ASSETS {
        return Err(FormatError::Limit("too many brush assets".into()));
    }
    let mut assets: Vec<(String, Vec<u8>)> = Vec::with_capacity(dto.brush_assets.len());
    for hash in &dto.brush_assets {
        if !is_asset_hash(hash) {
            return Err(corrupt("brush asset hash malformed"));
        }
        let name = brush_asset_name(hash);
        if b.declared_size(&name)? > veskra_doc::MAX_BRUSH_ASSET_BYTES as u64 {
            return Err(FormatError::Limit("brush asset exceeds MAX_BRUSH_ASSET_BYTES".into()));
        }
        let png = d.take(&name)?;
        if !png.starts_with(&PNG_SIGNATURE) {
            return Err(corrupt("brush asset is not a PNG"));
        }
        assets.push((hash.clone(), png));
    }
    let embedded: BTreeSet<&str> = dto.brush_assets.iter().map(String::as_str).collect();
    let mut missing: BTreeSet<&str> = BTreeSet::new();

    let mut strokes = Vec::with_capacity(dto.strokes.len());
    for s in &dto.strokes {
        if s.sample_entry != stroke_name(s.id) {
            return Err(corrupt("stroke sample entry name mismatch"));
        }
        let samples = decode_samples(&d.take(&s.sample_entry)?)?;
        if samples.len() != s.sample_count as usize {
            return Err(corrupt("stroke sample count mismatch"));
        }
        let mode = match s.composition.mode.as_str() {
            "paint" => StrokeMode::Paint,
            "erase" => StrokeMode::Erase,
            _ => return Err(corrupt("unknown stroke mode")),
        };
        let [r, g, bl, a] = s.color;
        let rec = StrokeRecord::new(
            s.id,
            s.layer,
            samples,
            s.brush.id.clone(),
            s.brush.version,
            s.brush.params.clone(),
            ColorRgba::new(r, g, bl, a),
            s.seed,
            s.profile.clone(),
            StrokeComposition {
                mode,
                opacity: s.composition.opacity,
                alpha_locked: s.composition.alpha_locked,
                selection_clipped: s.composition.selection_clipped,
            },
            s.vector_eligible,
            s.geometry.as_ref().map(|g| StrokeGeometry {
                centerline: g.centerline.clone(),
                outline: g.outline.clone(),
                fit_error: g.fit_error,
            }),
            s.bounds,
        )
        .map_err(doc_err)?
        .with_brush_assets(s.brush.assets.clone())
        .map_err(|_| corrupt("stroke brush asset references malformed"))?;
        missing.extend(s.brush.assets.iter().map(String::as_str).filter(|h| !embedded.contains(h)));
        strokes.push(rec);
    }

    let selection = match &dto.selection {
        Some(m) => Some(d.mask(m, selection_name)?),
        None => None,
    };

    // Integrity-check everything else; report entries this reader does not understand.
    if b.zip.entry(ENTRY_PREVIEW).is_some() {
        d.take(ENTRY_PREVIEW)?;
    }
    let mut unknown = Vec::new();
    for e in b.zip.entries() {
        if !d.used.contains(&e.name) {
            b.read_verified(&e.name)?;
            unknown.push(e.name.clone());
        }
    }
    if !unknown.is_empty() {
        warnings.push(FormatWarning::UnknownEntries { names: unknown });
    }

    let parts = DocumentParts {
        id: dto.id,
        size: SizeU::new(dto.width, dto.height),
        dpi: dto.dpi,
        color,
        root: dto.root,
        layers,
        strokes,
        selection,
        active_layer: dto.active_layer,
    };
    let mut doc = Document::from_parts(parts).map_err(doc_err)?;
    for (hash, png) in assets {
        doc.insert_brush_asset(&hash, Arc::from(png)).map_err(doc_err)?;
    }
    if !missing.is_empty() {
        warnings.push(FormatWarning::MissingBrushAssets { count: missing.len() });
    }
    Ok(LoadedDocument { doc, manifest: b.manifest, warnings })
}
