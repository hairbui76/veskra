//! Brush library (ADR-019): the user's custom and imported brushes in an app-owned directory,
//! Veskra's own `.vbrush` files, and import of Infinite Painter `.prbr` / `.przp` archives.
//!
//! ## Storage
//! `brush.library.open {dir}` scans `<dir>/*.vbrush` (one brush per file) and
//! `<dir>/assets/<hash>.png` (assets added with `brush.asset.add`). Bad files are skipped and
//! reported. Every later change is persisted there with a temp-file + rename write.
//!
//! ## `.vbrush` (Veskra brush file)
//! A ZIP (STORE/DEFLATE) with `mimetype` = `application/x-veskra-brush` (STORE, first),
//! `brush.json` (schema-3 preset JSON) and `assets/<hash>.png` for every image the preset
//! references (canonical 8-bit grayscale coverage PNG; `<hash>` is the mask content hash, verified
//! on read). Assets of built-in brushes are included too, so a file is self-contained.
//!
//! ## Hostile input
//! Files are size-checked before reading; archives are parsed by the bounded reader of
//! `veskra-format` (no zip64, safe names only — path traversal is a corrupt archive — declared
//! sizes enforced, DEFLATE ratio bounded) with per-format limits ([`PACK_POLICY`],
//! [`BRUSH_POLICY`]); JSON entries are size-bounded before parsing; PNGs are decoded by
//! `veskra-brush` with dimension and memory bounds; a pack may hold at most
//! [`MAX_PACK_BRUSHES`] brushes; archives nested deeper than one level (a pack inside a pack
//! entry) are refused. Nothing here panics on file contents.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use veskra_brush::interop::infinite_painter::{self as ip, PrbrParts};
use veskra_brush::{
    AssetStore, BrushAsset, BrushPreset, BrushSource, ImportedMask, MaskChannel, ResolvedAssets, builtin_assets,
    builtin_presets, load_preset_json, load_preset_value, preset::MAX_PRESET_JSON_BYTES,
};
use veskra_core::{ErrorCode, VeskraError};
use veskra_format::zip::{Method, ZipPolicy, ZipReader, ZipWriter};

use crate::session::EResult;

/// `mimetype` entry of a `.vbrush`.
pub const VBRUSH_MIMETYPE: &str = "application/x-veskra-brush";
/// File extension of Veskra brush files.
pub const VBRUSH_EXTENSION: &str = "vbrush";
/// Most user brushes in one library.
pub const MAX_LIBRARY_BRUSHES: usize = 2000;
/// Most stand-alone assets in one library.
pub const MAX_LIBRARY_ASSETS: usize = 4096;
/// Largest `.vbrush` / `.prbr` file.
pub const MAX_BRUSH_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Largest `.przp` file.
pub const MAX_PACK_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Most brushes imported from one pack.
pub const MAX_PACK_BRUSHES: usize = ip::MAX_PACK_BRUSHES;
/// Largest PNG accepted by `brush.asset.add` / `brush.save` assets.
pub const MAX_ASSET_FILE_BYTES: u64 = veskra_brush::asset::MAX_ASSET_PNG_BYTES as u64;

/// ZIP policy of single-brush archives (`.vbrush`, `.prbr`, and `.prbr` entries of a pack).
pub const BRUSH_POLICY: ZipPolicy = ZipPolicy {
    data_descriptors: true,
    prefixed: true,
    max_entry_bytes: 16 * 1024 * 1024,
    max_total_bytes: 64 * 1024 * 1024,
    max_entries: 64,
};
/// ZIP policy of `.przp` packs.
pub const PACK_POLICY: ZipPolicy = ZipPolicy {
    data_descriptors: true,
    prefixed: true,
    max_entry_bytes: 64 * 1024 * 1024,
    max_total_bytes: 512 * 1024 * 1024,
    max_entries: 4096,
};

/// A `brush.save` / `brush.asset.add` image: a path (channel `auto`) or `{path, channel, invert}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssetSpec {
    Path(String),
    File(AssetFileSpec),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetFileSpec {
    pub path: String,
    #[serde(default)]
    pub channel: MaskChannel,
    #[serde(default)]
    pub invert: bool,
}

impl AssetSpec {
    fn parts(&self) -> (&str, MaskChannel, bool) {
        match self {
            AssetSpec::Path(p) => (p, MaskChannel::Auto, false),
            AssetSpec::File(f) => (&f.path, f.channel, f.invert),
        }
    }
}

/// One brush of a `brush.import` result.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedItem {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub approximated: Vec<String>,
    pub unsupported: Vec<String>,
}

/// `brush.import` result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    pub imported: Vec<ImportedItem>,
    /// `(name, reason)` of brushes that could not be imported.
    pub failed: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

fn io_err(what: &str, e: std::io::Error) -> VeskraError {
    let code = match e.kind() {
        std::io::ErrorKind::NotFound => ErrorCode::NotFound,
        std::io::ErrorKind::PermissionDenied => ErrorCode::FilePermissionDenied,
        std::io::ErrorKind::StorageFull => ErrorCode::StorageFull,
        _ => ErrorCode::Io,
    };
    VeskraError::new(code, format!("{what}: {}", e.kind()))
}

/// Read a file of at most `limit` bytes (checked before reading).
fn read_bounded(path: &Path, limit: u64, what: &str) -> EResult<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|e| io_err(what, e))?;
    if !meta.is_file() {
        return Err(VeskraError::invalid(format!("{what}: not a file")));
    }
    if meta.len() > limit {
        return Err(VeskraError::alloc(format!("{what} is larger than {} MiB", limit / (1024 * 1024))));
    }
    std::fs::read(path).map_err(|e| io_err(what, e))
}

/// Write `bytes` to `path` via a sibling temp file, fsync and rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> EResult<()> {
    use std::io::Write;
    let parent = path.parent().ok_or_else(|| VeskraError::invalid("path has no parent directory"))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("brush");
    let tmp = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err("write failed", e));
    }
    Ok(())
}

fn corrupt(m: impl Into<String>) -> VeskraError {
    VeskraError::new(ErrorCode::CorruptData, m.into())
}

/// FNV-1a 64 (library-local id derivation; not a security boundary).
fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.iter().chain(&[0xff]) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Encode a `.vbrush` for `preset` with `assets` (every image the preset references).
pub fn encode_vbrush(preset: &BrushPreset, assets: &[Arc<BrushAsset>]) -> EResult<Vec<u8>> {
    let json = preset.to_json().map_err(VeskraError::from)?;
    let mut z = ZipWriter::new(Vec::new());
    z.add("mimetype", VBRUSH_MIMETYPE.as_bytes(), Method::Stored)?;
    z.add("brush.json", json.as_bytes(), Method::Deflated)?;
    let mut sorted: Vec<&Arc<BrushAsset>> = assets.iter().collect();
    sorted.sort_by(|a, b| a.hash().cmp(b.hash()));
    sorted.dedup_by(|a, b| a.hash() == b.hash());
    for a in sorted {
        z.add(&format!("assets/{}.png", a.hash()), a.png_bytes(), Method::Stored)?;
    }
    Ok(z.finish()?.0)
}

/// A decoded `.vbrush`: preset (migrated, ignored keys reported) and its verified assets.
#[derive(Debug, Clone)]
pub struct DecodedVbrush {
    pub preset: BrushPreset,
    pub assets: Vec<Arc<BrushAsset>>,
    pub warnings: Vec<String>,
}

fn decode_vbrush_zip(z: &ZipReader<'_>) -> EResult<DecodedVbrush> {
    let e = z.entry("brush.json").ok_or_else(|| corrupt("brush file has no brush.json"))?;
    if e.uncompressed_size > MAX_PRESET_JSON_BYTES as u64 {
        return Err(VeskraError::alloc("brush.json too large"));
    }
    let json = String::from_utf8(z.read(e)?).map_err(|_| corrupt("brush.json is not UTF-8"))?;
    let load = load_preset_json(&json).map_err(VeskraError::from)?;
    let mut warnings = Vec::new();
    if let Some(from) = load.migrated_from {
        warnings.push(format!("brush migrated from preset schema {from}"));
    }
    if !load.ignored_keys.is_empty() {
        warnings.push(format!("unknown preset field(s) ignored: {}", load.ignored_keys.join(", ")));
    }
    let mut assets = Vec::new();
    for h in load.preset.asset_refs() {
        match z.entry(&format!("assets/{h}.png")) {
            Some(ae) => {
                let a = BrushAsset::from_stored_png(&z.read(ae)?).map_err(VeskraError::from)?;
                if a.hash() != h {
                    return Err(corrupt("brush asset does not match its content hash"));
                }
                assets.push(Arc::new(a));
            }
            None => match builtin_assets().get(&h) {
                Some(a) => assets.push(a.clone()),
                None => return Err(VeskraError::new(ErrorCode::NotFound, format!("brush file lacks asset {h}"))),
            },
        }
    }
    Ok(DecodedVbrush { preset: load.preset, assets, warnings })
}

/// Decode a `.vbrush` (bounded).
pub fn decode_vbrush(bytes: &[u8]) -> EResult<DecodedVbrush> {
    let z = ZipReader::parse_with(bytes, &BRUSH_POLICY)?;
    decode_vbrush_zip(&z)
}

/// What an archive is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Vbrush,
    Prbr,
    Przp,
}

fn detect(z: &ZipReader<'_>) -> Option<Kind> {
    let has = |n: &str| z.entry(n).is_some();
    if has("brush.json") {
        Some(Kind::Vbrush)
    } else if has("properties.json") {
        Some(Kind::Prbr)
    } else if has("index.json") || z.entries().iter().any(|e| e.name.to_ascii_lowercase().ends_with(".prbr")) {
        Some(Kind::Przp)
    } else {
        None
    }
}

fn is_archive_name(n: &str) -> bool {
    let n = n.to_ascii_lowercase();
    n.ends_with(".prbr") || n.ends_with(".przp") || n.ends_with(".zip") || n.ends_with(".vbrush")
}

fn file_stem(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.rsplit_once('.').map_or(base, |(s, _)| s).to_string()
}

/// Map one `.prbr` archive. Nested archives are never opened: a `.prbr` that itself contains a
/// pack (or brush archives) is refused (depth > 1).
fn map_prbr_zip(z: &ZipReader<'_>, name: &str, folder: &str) -> EResult<ip::ImportedBrush> {
    if z.entry("index.json").is_some() || z.entries().iter().any(|e| is_archive_name(&e.name)) {
        return Err(VeskraError::invalid("nested brush archive (depth > 1) is not supported"));
    }
    let props_e = z.entry("properties.json").ok_or_else(|| corrupt("properties.json missing"))?;
    if props_e.uncompressed_size > ip::MAX_PROPERTIES_BYTES as u64 {
        return Err(VeskraError::alloc("properties.json too large"));
    }
    let props = z.read(props_e)?;
    let head = z.entry("head").map(|e| z.read(e)).transpose()?;
    let texture = z.entry("texture").map(|e| z.read(e)).transpose()?;
    let parts = PrbrParts { properties: &props, head: head.as_deref(), texture: texture.as_deref(), file_name: name };
    ip::map_prbr(&parts, folder).map_err(VeskraError::from)
}

/// The user's brush library.
#[derive(Debug, Default)]
pub struct BrushLibrary {
    dir: Option<PathBuf>,
    user: BTreeMap<String, BrushPreset>,
    assets: AssetStore,
}

impl BrushLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        self.dir.is_some()
    }

    fn dir(&self) -> EResult<&Path> {
        self.dir.as_deref().ok_or_else(|| {
            VeskraError::new(ErrorCode::InvalidState, "no brush library is open (call brush.library.open first)")
        })
    }

    /// Open (and create if needed) the library directory, replacing any previous one.
    /// Returns `(brushes, assets, warnings)`.
    pub fn open(&mut self, dir: &Path) -> EResult<(usize, usize, Vec<String>)> {
        std::fs::create_dir_all(dir.join("assets")).map_err(|e| io_err("brush library directory", e))?;
        let mut user = BTreeMap::new();
        let mut assets = AssetStore::new();
        let mut warnings = Vec::new();
        // Stand-alone assets first (brush.asset.add).
        let mut asset_files: Vec<PathBuf> = std::fs::read_dir(dir.join("assets"))
            .map_err(|e| io_err("brush library assets", e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
            .collect();
        asset_files.sort();
        for p in asset_files.into_iter().take(MAX_LIBRARY_ASSETS) {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            let stem = file_stem(&name);
            let res = read_bounded(&p, MAX_ASSET_FILE_BYTES, "brush asset")
                .and_then(|b| BrushAsset::from_stored_png(&b).map_err(VeskraError::from));
            match res {
                Ok(a) if a.hash() == stem => assets.insert(Arc::new(a)),
                Ok(_) => warnings.push(format!("{name}: content does not match its hash; skipped")),
                Err(e) => warnings.push(format!("{name}: {}; skipped", e.message)),
            }
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| io_err("brush library directory", e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(VBRUSH_EXTENSION))
            .collect();
        files.sort();
        if files.len() > MAX_LIBRARY_BRUSHES {
            warnings.push(format!("only the first {MAX_LIBRARY_BRUSHES} brush files were loaded"));
            files.truncate(MAX_LIBRARY_BRUSHES);
        }
        let builtin_ids: BTreeSet<String> = builtin_presets().into_iter().map(|p| p.id).collect();
        for p in files {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            let res = read_bounded(&p, MAX_BRUSH_FILE_BYTES, "brush file").and_then(|b| decode_vbrush(&b));
            match res {
                Ok(d) => {
                    let id = d.preset.id.clone();
                    if builtin_ids.contains(&id) || d.preset.source == BrushSource::Builtin {
                        warnings.push(format!("{name}: uses a built-in id or source; skipped"));
                        continue;
                    }
                    match user.entry(id) {
                        std::collections::btree_map::Entry::Occupied(e) => {
                            warnings.push(format!("{name}: duplicate brush id {}; skipped", e.key()));
                        }
                        std::collections::btree_map::Entry::Vacant(e) => {
                            for w in d.warnings {
                                warnings.push(format!("{name}: {w}"));
                            }
                            for a in d.assets {
                                assets.insert(a);
                            }
                            e.insert(d.preset);
                        }
                    }
                }
                Err(e) => warnings.push(format!("{name}: {}; skipped", e.message)),
            }
        }
        self.dir = Some(dir.to_path_buf());
        self.user = user;
        self.assets = assets;
        Ok((self.user.len(), self.assets.len(), warnings))
    }

    /// Built-in or user brush by id.
    pub fn get(&self, id: &str) -> Option<BrushPreset> {
        self.user.get(id).cloned().or_else(|| veskra_brush::builtin_preset(id))
    }

    /// Every brush in display order: built-ins, then user brushes by folder, name and id.
    pub fn list(&self) -> Vec<BrushPreset> {
        let mut user: Vec<&BrushPreset> = self.user.values().collect();
        user.sort_by(|a, b| (&a.folder, &a.name, &a.id).cmp(&(&b.folder, &b.name, &b.id)));
        builtin_presets().into_iter().chain(user.into_iter().cloned()).collect()
    }

    pub fn is_user(&self, id: &str) -> bool {
        self.user.contains_key(id)
    }

    /// Library asset or built-in asset by hash.
    pub fn find_asset(&self, hash: &str) -> Option<Arc<BrushAsset>> {
        self.assets.get(hash).or_else(|| builtin_assets().get(hash)).cloned()
    }

    /// Images a preset needs, from the library and the built-ins.
    pub fn resolve(&self, preset: &BrushPreset) -> ResolvedAssets {
        ResolvedAssets::resolve(preset, &[&self.assets, builtin_assets()])
    }

    fn assets_of(&self, p: &BrushPreset) -> EResult<Vec<Arc<BrushAsset>>> {
        p.asset_refs()
            .iter()
            .map(|h| {
                self.find_asset(h)
                    .ok_or_else(|| VeskraError::new(ErrorCode::NotFound, format!("brush asset {h} is not available")))
            })
            .collect()
    }

    fn new_id(&self, prefix: &str, name: &str) -> String {
        let mut n: u64 = self.user.len() as u64;
        loop {
            let id = format!("{prefix}.{:012x}", fnv64(&[name.as_bytes(), &n.to_le_bytes()]) & 0xFFFF_FFFF_FFFF);
            if !self.user.contains_key(&id) && veskra_brush::builtin_preset(&id).is_none() {
                return id;
            }
            n = n.wrapping_add(1);
        }
    }

    fn persist(&self, p: &BrushPreset) -> EResult<()> {
        let bytes = encode_vbrush(p, &self.assets_of(p)?)?;
        write_atomic(&self.dir()?.join(format!("{}.{VBRUSH_EXTENSION}", p.id)), &bytes)
    }

    fn check_capacity(&self, adding: usize) -> EResult<()> {
        if self.user.len() + adding > MAX_LIBRARY_BRUSHES {
            return Err(VeskraError::alloc(format!("the brush library holds at most {MAX_LIBRARY_BRUSHES} brushes")));
        }
        Ok(())
    }

    /// Convert and store a PNG (`brush.asset.add`).
    pub fn add_asset(&mut self, path: &Path, channel: MaskChannel, invert: bool) -> EResult<ImportedMask> {
        let dir = self.dir()?.to_path_buf();
        let bytes = read_bounded(path, MAX_ASSET_FILE_BYTES, "image")?;
        let m = BrushAsset::from_png(&bytes, channel, invert).map_err(VeskraError::from)?;
        if !self.assets.contains(m.asset.hash()) {
            if self.assets.len() >= MAX_LIBRARY_ASSETS {
                return Err(VeskraError::alloc("too many brush assets in the library"));
            }
            write_atomic(&dir.join("assets").join(format!("{}.png", m.asset.hash())), m.asset.png_bytes())?;
            self.assets.insert(Arc::new(m.asset.clone()));
        }
        Ok(m)
    }

    /// `brush.save`: create or update a custom brush. Returns the stored preset and warnings.
    pub fn save(
        &mut self,
        preset: Value,
        assets: Option<&BTreeMap<String, AssetSpec>>,
    ) -> EResult<(BrushPreset, Vec<String>)> {
        self.dir()?;
        let mut v = preset;
        let obj = v.as_object_mut().ok_or_else(|| VeskraError::invalid("preset must be a JSON object"))?;
        let mut warnings = Vec::new();
        // Images supplied with the save: keys are the placeholders the preset references.
        let mut new_assets: Vec<Arc<BrushAsset>> = Vec::new();
        let mut renamed: BTreeMap<String, String> = BTreeMap::new();
        if let Some(map) = assets {
            if map.len() > 8 {
                return Err(VeskraError::invalid("at most 8 assets per brush.save"));
            }
            for (key, spec) in map {
                let (path, channel, invert) = spec.parts();
                let bytes = read_bounded(Path::new(path), MAX_ASSET_FILE_BYTES, "image")?;
                let m = BrushAsset::from_png(&bytes, channel, invert).map_err(VeskraError::from)?;
                if let Some((w, h)) = m.downscaled_from {
                    warnings.push(format!("image {key} was downscaled from {w}×{h} px"));
                }
                renamed.insert(key.clone(), m.asset.hash().to_string());
                new_assets.push(Arc::new(m.asset));
            }
        }
        for section in ["tip", "texture"] {
            if let Some(Value::Object(s)) = obj.get_mut(section)
                && let Some(Value::String(r)) = s.get("image")
                && let Some(h) = renamed.get(r)
            {
                s.insert("image".into(), Value::String(h.clone()));
            }
        }
        let given = obj.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        obj.insert("id".into(), Value::from("custom.pending"));
        obj.insert("vector_eligible".into(), Value::Bool(false));
        obj.insert("source".into(), Value::from("custom"));
        obj.entry("schema").or_insert(Value::from(veskra_brush::PRESET_SCHEMA_VERSION));
        if serde_json::to_vec(&v).map_or(true, |b| b.len() > MAX_PRESET_JSON_BYTES) {
            return Err(VeskraError::alloc("preset JSON too large"));
        }
        let load = load_preset_value(v).map_err(VeskraError::from)?;
        if !load.ignored_keys.is_empty() {
            return Err(VeskraError::invalid(format!("unknown preset field(s): {}", load.ignored_keys.join(", "))));
        }
        if let Some(from) = load.migrated_from {
            warnings.push(format!("preset migrated from schema {from}"));
        }
        let mut p = load.preset;
        for h in p.asset_refs() {
            if !new_assets.iter().any(|a| a.hash() == h) && self.find_asset(&h).is_none() {
                return Err(VeskraError::new(
                    ErrorCode::NotFound,
                    format!("unknown brush asset {h} (add it with brush.asset.add or pass it in assets)"),
                ));
            }
        }
        match self.user.get(&given) {
            Some(old) => {
                p.id = given.clone();
                p.version = old.version.saturating_add(1);
            }
            None => {
                self.check_capacity(1)?;
                if !given.is_empty() && veskra_brush::builtin_preset(&given).is_some() {
                    warnings.push(format!("{given} is a built-in brush; saved as a new custom brush"));
                }
                p.id = self.new_id("custom", &p.name);
                p.version = 1;
            }
        }
        p.source = BrushSource::Custom;
        p.vector_eligible = p.satisfies_vector_rule();
        p.validate().map_err(VeskraError::from)?;
        for a in &new_assets {
            self.assets.insert(a.clone());
        }
        self.persist(&p)?;
        self.user.insert(p.id.clone(), p.clone());
        Ok((p, warnings))
    }

    /// `brush.delete` (custom / imported only).
    pub fn delete(&mut self, id: &str) -> EResult<()> {
        let dir = self.dir()?.to_path_buf();
        if !self.user.contains_key(id) {
            return Err(if veskra_brush::builtin_preset(id).is_some() {
                VeskraError::invalid("built-in brushes cannot be deleted")
            } else {
                VeskraError::new(ErrorCode::NotFound, format!("unknown brush {id}"))
            });
        }
        match std::fs::remove_file(dir.join(format!("{id}.{VBRUSH_EXTENSION}"))) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err("delete failed", e)),
        }
        self.user.remove(id);
        Ok(())
    }

    /// `brush.duplicate`: a custom copy of any brush.
    pub fn duplicate(&mut self, id: &str, name: Option<&str>) -> EResult<BrushPreset> {
        self.dir()?;
        let src = self.get(id).ok_or_else(|| VeskraError::new(ErrorCode::NotFound, format!("unknown brush {id}")))?;
        self.check_capacity(1)?;
        let mut p = src;
        p.name = match name {
            Some(n) => ip::clean_name(n),
            None => ip::clean_name(&format!("{} copy", p.name)),
        };
        p.id = self.new_id("custom", &p.name);
        p.version = 1;
        p.source = BrushSource::Custom;
        p.vector_eligible = p.satisfies_vector_rule();
        p.validate().map_err(VeskraError::from)?;
        self.persist(&p)?;
        self.user.insert(p.id.clone(), p.clone());
        Ok(p)
    }

    /// `brush.export` → `.vbrush` bytes written to `path`.
    pub fn export(&self, id: &str, path: &Path) -> EResult<u64> {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if ext.as_deref() != Some(VBRUSH_EXTENSION) {
            return Err(VeskraError::invalid("brush export path must end with .vbrush"));
        }
        let p = self.get(id).ok_or_else(|| VeskraError::new(ErrorCode::NotFound, format!("unknown brush {id}")))?;
        let bytes = encode_vbrush(&p, &self.assets_of(&p)?)?;
        write_atomic(path, &bytes)?;
        Ok(bytes.len() as u64)
    }

    /// Store one imported preset (new id unless its own id is free) and its assets.
    fn adopt(&mut self, mut p: BrushPreset, assets: Vec<Arc<BrushAsset>>, prefix: &str) -> EResult<BrushPreset> {
        self.check_capacity(1)?;
        let keep = !p.id.is_empty()
            && p.id != ip::PENDING_ID
            && !self.user.contains_key(&p.id)
            && veskra_brush::builtin_preset(&p.id).is_none();
        if !keep {
            p.id = self.new_id(prefix, &p.name);
        }
        if p.source == BrushSource::Builtin {
            p.source = BrushSource::Custom;
        }
        p.vector_eligible = p.satisfies_vector_rule();
        p.validate().map_err(VeskraError::from)?;
        for a in assets {
            self.assets.insert(a);
        }
        self.persist(&p)?;
        self.user.insert(p.id.clone(), p.clone());
        Ok(p)
    }

    /// `brush.import`: `.vbrush`, `.prbr` or `.przp`, detected by content.
    pub fn import(&mut self, path: &Path) -> EResult<ImportReport> {
        self.dir()?;
        let bytes = read_bounded(path, MAX_PACK_FILE_BYTES, "brush file")?;
        let display = file_stem(&path.to_string_lossy());
        let z = ZipReader::parse_with(&bytes, &PACK_POLICY).map_err(|e| {
            if bytes.starts_with(b"PK") {
                VeskraError::from(e)
            } else {
                VeskraError::invalid("not a brush file (.vbrush, .prbr or .przp archive expected)")
            }
        })?;
        let kind =
            detect(&z).ok_or_else(|| VeskraError::invalid("archive contains no brush (.vbrush, .prbr or .przp)"))?;
        let mut report = ImportReport::default();
        match kind {
            Kind::Vbrush => {
                if bytes.len() as u64 > MAX_BRUSH_FILE_BYTES {
                    return Err(VeskraError::alloc("brush file too large"));
                }
                let z = ZipReader::parse_with(&bytes, &BRUSH_POLICY)?;
                let d = decode_vbrush_zip(&z)?;
                report.warnings.extend(d.warnings);
                let p = self.adopt(d.preset, d.assets, "custom")?;
                report.imported.push(ImportedItem {
                    id: p.id,
                    name: p.name,
                    folder: p.folder,
                    approximated: vec![],
                    unsupported: vec![],
                });
            }
            Kind::Prbr => {
                if bytes.len() as u64 > MAX_BRUSH_FILE_BYTES {
                    return Err(VeskraError::alloc("brush file too large"));
                }
                let z = ZipReader::parse_with(&bytes, &BRUSH_POLICY)?;
                let b = map_prbr_zip(&z, &display, "Imported")?;
                self.adopt_ip(b, &mut report)?;
            }
            Kind::Przp => self.import_pack(&z, &display, &mut report)?,
        }
        Ok(report)
    }

    fn adopt_ip(&mut self, b: ip::ImportedBrush, report: &mut ImportReport) -> EResult<()> {
        let assets = b.assets.into_iter().map(Arc::new).collect();
        let p = self.adopt(b.preset, assets, "imp")?;
        report.imported.push(ImportedItem {
            id: p.id,
            name: p.name,
            folder: p.folder,
            approximated: b.approximated,
            unsupported: b.unsupported,
        });
        Ok(())
    }

    fn import_pack(&mut self, z: &ZipReader<'_>, pack_name: &str, report: &mut ImportReport) -> EResult<()> {
        let prbr: Vec<&str> =
            z.entries().iter().map(|e| e.name.as_str()).filter(|n| n.to_ascii_lowercase().ends_with(".prbr")).collect();
        if prbr.len() > MAX_PACK_BRUSHES {
            return Err(VeskraError::alloc(format!("brush pack holds more than {MAX_PACK_BRUSHES} brushes")));
        }
        // (entry name, display name, folder) in index order, then unlisted entries.
        let mut plan: Vec<(String, String, String)> = Vec::new();
        let mut listed = BTreeSet::new();
        if let Some(ie) = z.entry("index.json") {
            if ie.uncompressed_size > ip::MAX_PROPERTIES_BYTES as u64 {
                return Err(VeskraError::alloc("index.json too large"));
            }
            let index = ip::parse_pack_index(&z.read(ie)?).map_err(VeskraError::from)?;
            for f in index.folders {
                let folder = if f.name.is_empty() { pack_name.to_string() } else { f.name };
                for (name, _hidden) in f.brushes {
                    let entry = format!("Brushes/{name}.prbr");
                    if !listed.insert(entry.clone()) {
                        continue;
                    }
                    plan.push((entry, name, folder.clone()));
                }
            }
        }
        let mut unlisted = 0;
        for n in &prbr {
            if !listed.contains(*n) {
                unlisted += 1;
                plan.push((n.to_string(), file_stem(n), pack_name.to_string()));
            }
        }
        if unlisted > 0 && z.entry("index.json").is_some() {
            report.warnings.push(format!("{unlisted} brush file(s) not listed in index.json were imported too"));
        }
        if plan.len() > MAX_PACK_BRUSHES {
            return Err(VeskraError::alloc(format!("brush pack lists more than {MAX_PACK_BRUSHES} brushes")));
        }
        self.check_capacity(0)?;
        for (entry, name, folder) in plan {
            let res = (|| -> EResult<ip::ImportedBrush> {
                let e =
                    z.entry(&entry).ok_or_else(|| VeskraError::new(ErrorCode::NotFound, "missing from the pack"))?;
                if e.uncompressed_size > MAX_BRUSH_FILE_BYTES {
                    return Err(VeskraError::alloc("brush entry too large"));
                }
                let inner = z.read(e)?;
                let iz = ZipReader::parse_with(&inner, &BRUSH_POLICY)?;
                map_prbr_zip(&iz, &name, &folder)
            })();
            match res.and_then(|b| self.adopt_ip(b, report)) {
                Ok(()) => {}
                Err(e) if e.code == ErrorCode::AllocationLimit && self.user.len() >= MAX_LIBRARY_BRUSHES => {
                    report.failed.push((name, e.message));
                    break;
                }
                Err(e) => report.failed.push((name, e.message)),
            }
        }
        Ok(())
    }
}

/// Standard base64 (RFC 4648, with padding).
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], c.get(1).copied().unwrap_or(0), c.get(2).copied().unwrap_or(0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc4648_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn vbrush_round_trip_includes_builtin_assets() {
        let p = veskra_brush::builtin_preset("paint.round_bristle").unwrap();
        let lib = BrushLibrary::new();
        let bytes = encode_vbrush(&p, &lib.assets_of(&p).unwrap()).unwrap();
        let d = decode_vbrush(&bytes).unwrap();
        assert_eq!(d.preset, p);
        assert_eq!(d.assets.len(), 1);
        assert!(d.warnings.is_empty());
        assert_eq!(file_stem("/a/b/Pack Name.przp"), "Pack Name");
    }
}
