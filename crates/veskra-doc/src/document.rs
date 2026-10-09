//! The document tree and its validated mutation API.

use crate::layer::{GroupContent, Layer, LayerKind, LayerProps, PixelContent, VectorFidelity};
use crate::stroke::StrokeRecord;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use veskra_core::{
    BlendMode, BlendSpace, DocumentId, ErrorCode, LayerId, ProfileId, RectI, SizeU, StrokeId, VeskraError, limits,
};
use veskra_raster::MaskGrid;

/// Document color identity. ICC bytes are kept verbatim so they can be saved/embedded; parsing
/// and transforms belong to `veskra-color`.
#[derive(Debug, Clone, PartialEq)]
pub struct DocColor {
    pub profile: ProfileId,
    pub icc_bytes: Arc<[u8]>,
    /// The profile was assumed (e.g. untagged import), not read from the source (MASTER_PLAN §6.2).
    pub assumed: bool,
    pub blend_space: BlendSpace,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum DocumentError {
    #[error("layer not found")]
    LayerNotFound,
    #[error("stroke not found")]
    StrokeNotFound,
    #[error("duplicate id")]
    DuplicateId,
    #[error("operation requires a group layer")]
    NotAGroup,
    #[error("operation requires a pixel layer")]
    NotAPixelLayer,
    #[error("move would create a cycle")]
    Cycle,
    #[error("group nesting too deep")]
    TooDeep,
    #[error("too many layers")]
    TooManyLayers,
    #[error("layer is locked")]
    Locked,
    #[error("invalid value: {0}")]
    InvalidValue(&'static str),
    #[error("tree invariant violated: {0}")]
    Corrupt(String),
}

impl From<DocumentError> for VeskraError {
    fn from(e: DocumentError) -> Self {
        let code = match e {
            DocumentError::LayerNotFound | DocumentError::StrokeNotFound => ErrorCode::NotFound,
            DocumentError::Locked => ErrorCode::LayerLocked,
            DocumentError::TooManyLayers | DocumentError::TooDeep => ErrorCode::AllocationLimit,
            DocumentError::Corrupt(_) => ErrorCode::InvalidDocument,
            DocumentError::NotAPixelLayer | DocumentError::NotAGroup => ErrorCode::UnsupportedLayerFeature,
            _ => ErrorCode::InvalidArgument,
        };
        VeskraError::new(code, e.to_string())
    }
}

type DResult<T> = Result<T, DocumentError>;

/// Where to put a new or moved layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Top of the root stack.
    Top,
    /// Directly above a sibling (same parent).
    Above(LayerId),
    /// Directly below a sibling (same parent).
    Below(LayerId),
    /// Inside a group at `index` (bottom = 0); `None` = top of the group.
    IntoGroup { group: LayerId, index: Option<usize> },
}

/// Partial property update. `None` fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerPatch {
    pub name: Option<String>,
    pub visible: Option<bool>,
    pub opacity: Option<f32>,
    pub blend: Option<BlendMode>,
    pub locked: Option<bool>,
    pub alpha_locked: Option<bool>,
    pub clip_to_below: Option<bool>,
    pub pass_through: Option<bool>,
}

/// Owned, unvalidated document parts for loaders/writers. [`Document::from_parts`] validates.
#[derive(Debug, Clone)]
pub struct DocumentParts {
    pub id: DocumentId,
    pub size: SizeU,
    pub dpi: f32,
    pub color: DocColor,
    pub root: Vec<LayerId>,
    pub layers: Vec<Layer>,
    pub strokes: Vec<StrokeRecord>,
    pub selection: Option<MaskGrid>,
    pub active_layer: Option<LayerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub id: DocumentId,
    size: SizeU,
    pub dpi: f32,
    pub color: DocColor,
    root: Vec<LayerId>,
    layers: BTreeMap<LayerId, Arc<Layer>>,
    strokes: BTreeMap<StrokeId, Arc<StrokeRecord>>,
    /// Active selection (255 = selected). `None` = everything editable.
    pub selection: Option<MaskGrid>,
    /// Layer the user is painting on.
    pub active_layer: Option<LayerId>,
}

impl Document {
    pub fn new(id: DocumentId, width: u32, height: u32, dpi: f32, color: DocColor) -> Result<Self, VeskraError> {
        limits::check_document_size(width, height)?;
        if !(dpi.is_finite() && (1.0..=10_000.0).contains(&dpi)) {
            return Err(VeskraError::invalid("dpi out of range"));
        }
        if color.icc_bytes.len() > limits::MAX_ICC_BYTES {
            return Err(VeskraError::new(ErrorCode::InvalidProfile, "ICC profile too large"));
        }
        Ok(Self {
            id,
            size: SizeU::new(width, height),
            dpi,
            color,
            root: Vec::new(),
            layers: BTreeMap::new(),
            strokes: BTreeMap::new(),
            selection: None,
            active_layer: None,
        })
    }

    pub fn size(&self) -> SizeU {
        self.size
    }
    pub fn width(&self) -> u32 {
        self.size.width
    }
    pub fn height(&self) -> u32 {
        self.size.height
    }
    /// Canvas rectangle `[0,w) x [0,h)`.
    pub fn bounds(&self) -> RectI {
        RectI::from_size(self.size.width, self.size.height)
    }

    // ---------- read access ----------

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(&id).map(|a| a.as_ref())
    }
    pub fn layer_arc(&self, id: LayerId) -> Option<&Arc<Layer>> {
        self.layers.get(&id)
    }
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
    pub fn root(&self) -> &[LayerId] {
        &self.root
    }
    /// Children of a group, or the root list for `None`.
    pub fn children_of(&self, parent: Option<LayerId>) -> DResult<&[LayerId]> {
        match parent {
            None => Ok(&self.root),
            Some(p) => self
                .layer(p)
                .ok_or(DocumentError::LayerNotFound)?
                .group()
                .map(|g| g.children.as_slice())
                .ok_or(DocumentError::NotAGroup),
        }
    }
    /// All layers in id order (stable, not paint order).
    pub fn iter_layers(&self) -> impl Iterator<Item = &Layer> {
        self.layers.values().map(|a| a.as_ref())
    }
    /// Depth-first paint order, bottom to top; a group precedes its children.
    pub fn paint_order(&self) -> Vec<LayerId> {
        let mut out = Vec::with_capacity(self.layers.len());
        fn walk(doc: &Document, ids: &[LayerId], out: &mut Vec<LayerId>) {
            for id in ids {
                out.push(*id);
                if let Some(g) = doc.layer(*id).and_then(|l| l.group()) {
                    walk(doc, &g.children, out);
                }
            }
        }
        walk(self, &self.root, &mut out);
        out
    }
    pub fn stroke(&self, id: StrokeId) -> Option<&StrokeRecord> {
        self.strokes.get(&id).map(|a| a.as_ref())
    }
    pub fn stroke_count(&self) -> usize {
        self.strokes.len()
    }
    pub fn iter_strokes(&self) -> impl Iterator<Item = &StrokeRecord> {
        self.strokes.values().map(|a| a.as_ref())
    }
    /// Strokes of a pixel layer in paint order.
    pub fn strokes_of(&self, layer: LayerId) -> Vec<&StrokeRecord> {
        self.layer(layer)
            .and_then(|l| l.pixels())
            .map(|p| p.strokes.iter().filter_map(|s| self.stroke(*s)).collect())
            .unwrap_or_default()
    }

    /// Group nesting depth of a layer (root children have depth 0).
    pub fn depth(&self, id: LayerId) -> usize {
        let mut d = 0;
        let mut cur = self.layer(id).and_then(|l| l.parent);
        while let Some(p) = cur {
            d += 1;
            if d > limits::MAX_GROUP_DEPTH + 1 {
                break;
            }
            cur = self.layer(p).and_then(|l| l.parent);
        }
        d
    }

    /// Locked directly or via any ancestor group.
    pub fn is_effectively_locked(&self, id: LayerId) -> bool {
        let mut cur = Some(id);
        let mut guard = 0;
        while let Some(c) = cur {
            match self.layer(c) {
                Some(l) if l.props.locked => return true,
                Some(l) => cur = l.parent,
                None => return false,
            }
            guard += 1;
            if guard > limits::MAX_GROUP_DEPTH + 2 {
                return false;
            }
        }
        false
    }

    /// Visible directly and via every ancestor group.
    pub fn is_effectively_visible(&self, id: LayerId) -> bool {
        let mut cur = Some(id);
        let mut guard = 0;
        while let Some(c) = cur {
            match self.layer(c) {
                Some(l) if !l.props.visible => return false,
                Some(l) => cur = l.parent,
                None => return false,
            }
            guard += 1;
            if guard > limits::MAX_GROUP_DEPTH + 2 {
                return false;
            }
        }
        true
    }

    fn is_ancestor(&self, ancestor: LayerId, mut id: LayerId) -> bool {
        let mut guard = 0;
        while let Some(p) = self.layer(id).and_then(|l| l.parent) {
            if p == ancestor {
                return true;
            }
            id = p;
            guard += 1;
            if guard > limits::MAX_GROUP_DEPTH + 2 {
                return true; // treat corrupt chains as cyclic
            }
        }
        false
    }

    // ---------- mutation ----------

    /// Mutable access (copy-on-write). Does not check locks; callers editing pixels/props use
    /// [`Document::layer_mut_editable`].
    pub fn layer_mut(&mut self, id: LayerId) -> DResult<&mut Layer> {
        self.layers.get_mut(&id).map(Arc::make_mut).ok_or(DocumentError::LayerNotFound)
    }

    /// Mutable access that refuses effectively locked layers.
    pub fn layer_mut_editable(&mut self, id: LayerId) -> DResult<&mut Layer> {
        if !self.layers.contains_key(&id) {
            return Err(DocumentError::LayerNotFound);
        }
        if self.is_effectively_locked(id) {
            return Err(DocumentError::Locked);
        }
        self.layer_mut(id)
    }

    /// Mutable pixel content of an editable raster/vector layer.
    pub fn pixels_mut_editable(&mut self, id: LayerId) -> DResult<&mut PixelContent> {
        self.layer_mut_editable(id)?.pixels_mut().ok_or(DocumentError::NotAPixelLayer)
    }

    fn resolve_placement(&self, placement: Placement) -> DResult<(Option<LayerId>, usize)> {
        match placement {
            Placement::Top => Ok((None, self.root.len())),
            Placement::Above(s) | Placement::Below(s) => {
                let parent = self.layer(s).ok_or(DocumentError::LayerNotFound)?.parent;
                let list = self.children_of(parent)?;
                let pos = list
                    .iter()
                    .position(|x| *x == s)
                    .ok_or_else(|| DocumentError::Corrupt("sibling missing".into()))?;
                Ok((parent, if matches!(placement, Placement::Above(_)) { pos + 1 } else { pos }))
            }
            Placement::IntoGroup { group, index } => {
                let g =
                    self.layer(group).ok_or(DocumentError::LayerNotFound)?.group().ok_or(DocumentError::NotAGroup)?;
                let n = g.children.len();
                Ok((Some(group), index.map_or(n, |i| i.min(n))))
            }
        }
    }

    fn list_mut(&mut self, parent: Option<LayerId>) -> DResult<&mut Vec<LayerId>> {
        match parent {
            None => Ok(&mut self.root),
            Some(p) => Ok(&mut self.layer_mut(p)?.group_mut().ok_or(DocumentError::NotAGroup)?.children),
        }
    }

    /// Insert a new layer. Group layers must be created with an empty child list.
    pub fn add_layer(&mut self, id: LayerId, props: LayerProps, kind: LayerKind, placement: Placement) -> DResult<()> {
        if self.layers.contains_key(&id) {
            return Err(DocumentError::DuplicateId);
        }
        if self.layers.len() >= limits::MAX_LAYERS {
            return Err(DocumentError::TooManyLayers);
        }
        Self::check_props(&props)?;
        if let LayerKind::Group(g) = &kind
            && !g.children.is_empty()
        {
            return Err(DocumentError::InvalidValue("new group must be empty"));
        }
        if let Some(p) = kind_pixels(&kind)
            && !p.strokes.is_empty()
        {
            return Err(DocumentError::InvalidValue("new layer cannot reference strokes"));
        }
        let (parent, index) = self.resolve_placement(placement)?;
        if let Some(p) = parent
            && self.depth(p) + 1 > limits::MAX_GROUP_DEPTH
        {
            return Err(DocumentError::TooDeep);
        }
        self.list_mut(parent)?.insert(index, id);
        self.layers.insert(id, Arc::new(Layer { id, parent, props, kind }));
        Ok(())
    }

    fn check_props(p: &LayerProps) -> DResult<()> {
        if p.name.len() > limits::MAX_NAME_BYTES {
            return Err(DocumentError::InvalidValue("layer name too long"));
        }
        if !(p.opacity.is_finite() && (0.0..=1.0).contains(&p.opacity)) {
            return Err(DocumentError::InvalidValue("opacity out of range"));
        }
        Ok(())
    }

    /// Remove a layer (and, for groups, its whole subtree) plus all strokes they own.
    /// Returns removed layer ids in paint order.
    pub fn remove_layer(&mut self, id: LayerId) -> DResult<Vec<LayerId>> {
        let parent = self.layer(id).ok_or(DocumentError::LayerNotFound)?.parent;
        if self.is_effectively_locked(id) {
            return Err(DocumentError::Locked);
        }
        let mut subtree = Vec::new();
        self.collect_subtree(id, &mut subtree);
        self.list_mut(parent)?.retain(|x| *x != id);
        for l in &subtree {
            if let Some(layer) = self.layers.remove(l)
                && let Some(p) = layer.pixels()
            {
                for s in &p.strokes {
                    self.strokes.remove(s);
                }
            }
        }
        if self.active_layer.is_some_and(|a| subtree.contains(&a)) {
            self.active_layer = None;
        }
        Ok(subtree)
    }

    fn collect_subtree(&self, id: LayerId, out: &mut Vec<LayerId>) {
        out.push(id);
        if let Some(g) = self.layer(id).and_then(|l| l.group()) {
            for c in &g.children {
                self.collect_subtree(*c, out);
            }
        }
    }

    /// Move a layer (with its subtree). Rejects moves into its own subtree (DOC-01 cycle rejection).
    pub fn move_layer(&mut self, id: LayerId, placement: Placement) -> DResult<()> {
        let old_parent = self.layer(id).ok_or(DocumentError::LayerNotFound)?.parent;
        let target_group = match placement {
            Placement::Top => None,
            Placement::Above(s) | Placement::Below(s) => {
                if s == id {
                    return Ok(());
                }
                self.layer(s).ok_or(DocumentError::LayerNotFound)?.parent
            }
            Placement::IntoGroup { group, .. } => Some(group),
        };
        if let Some(g) = target_group
            && (g == id || self.is_ancestor(id, g))
        {
            return Err(DocumentError::Cycle);
        }
        // Depth check: deepest node of the subtree after the move.
        let mut sub = Vec::new();
        self.collect_subtree(id, &mut sub);
        let sub_depth = sub.iter().map(|l| self.depth(*l) - self.depth(id)).max().unwrap_or(0);
        let new_base = target_group.map_or(0, |g| self.depth(g) + 1);
        if new_base + sub_depth > limits::MAX_GROUP_DEPTH {
            return Err(DocumentError::TooDeep);
        }
        let backup = self.clone();
        let res = (|| {
            self.list_mut(old_parent)?.retain(|x| *x != id);
            let (parent, index) = self.resolve_placement(placement)?;
            self.list_mut(parent)?.insert(index, id);
            self.layer_mut(id)?.parent = parent;
            Ok(())
        })();
        if res.is_err() {
            *self = backup; // atomic: no partial mutation on failure
        }
        res
    }

    /// Deep-copy a layer subtree with fresh ids from `next_id`, placed directly above the original.
    /// Returns the new top-level layer id.
    pub fn duplicate_layer(&mut self, id: LayerId, next_id: &mut dyn FnMut() -> u128) -> DResult<LayerId> {
        let mut sub = Vec::new();
        self.collect_subtree(id, &mut sub);
        if self.layers.len() + sub.len() > limits::MAX_LAYERS {
            return Err(DocumentError::TooManyLayers);
        }
        let mut map: BTreeMap<LayerId, LayerId> = BTreeMap::new();
        for old in &sub {
            map.insert(*old, LayerId::from_u128(next_id()));
        }
        let new_top = map[&id];
        let backup = self.clone();
        let res = (|| {
            for old in &sub {
                let src = self.layer(*old).ok_or(DocumentError::LayerNotFound)?.clone();
                let new_id = map[old];
                let parent = if *old == id { src.parent } else { src.parent.map(|p| map[&p]) };
                let mut props = src.props.clone();
                if *old == id {
                    props.name = copy_name(&props.name);
                }
                let kind = match src.kind {
                    LayerKind::Group(g) => LayerKind::Group(GroupContent {
                        children: g.children.iter().map(|c| map[c]).collect(),
                        pass_through: g.pass_through,
                        collapsed: g.collapsed,
                    }),
                    LayerKind::Raster(p) => LayerKind::Raster(self.dup_pixels(p, new_id, next_id)),
                    LayerKind::Vector(p) => LayerKind::Vector(self.dup_pixels(p, new_id, next_id)),
                };
                self.layers.insert(new_id, Arc::new(Layer { id: new_id, parent, props, kind }));
            }
            let (parent, index) = self.resolve_placement(Placement::Above(id))?;
            self.list_mut(parent)?.insert(index, new_top);
            Ok(new_top)
        })();
        if res.is_err() {
            *self = backup;
        }
        res
    }

    fn dup_pixels(&mut self, p: PixelContent, new_layer: LayerId, next_id: &mut dyn FnMut() -> u128) -> PixelContent {
        let mut strokes = Vec::with_capacity(p.strokes.len());
        for s in &p.strokes {
            if let Some(rec) = self.strokes.get(s).cloned() {
                let nid = StrokeId::from_u128(next_id());
                self.strokes.insert(nid, Arc::new(rec.with_identity(nid, new_layer)));
                strokes.push(nid);
            }
        }
        PixelContent { tiles: p.tiles, mask: p.mask, strokes, raster_edits: p.raster_edits }
    }

    /// Apply a property patch. Lock state and visibility may change on locked layers; every other
    /// field requires the layer to be editable.
    pub fn apply_patch(&mut self, id: LayerId, patch: &LayerPatch) -> DResult<()> {
        let only_lock_or_visibility = patch.name.is_none()
            && patch.opacity.is_none()
            && patch.blend.is_none()
            && patch.alpha_locked.is_none()
            && patch.clip_to_below.is_none()
            && patch.pass_through.is_none();
        let layer = if only_lock_or_visibility { self.layer_mut(id)? } else { self.layer_mut_editable(id)? };
        let mut props = layer.props.clone();
        if let Some(n) = &patch.name {
            props.name = n.clone();
        }
        if let Some(v) = patch.visible {
            props.visible = v;
        }
        if let Some(o) = patch.opacity {
            props.opacity = o;
        }
        if let Some(b) = patch.blend {
            props.blend = b;
        }
        if let Some(l) = patch.locked {
            props.locked = l;
        }
        if let Some(a) = patch.alpha_locked {
            props.alpha_locked = a;
        }
        if let Some(c) = patch.clip_to_below {
            props.clip_to_below = c;
        }
        Self::check_props(&props)?;
        if let Some(pt) = patch.pass_through {
            layer.group_mut().ok_or(DocumentError::NotAGroup)?.pass_through = pt;
        }
        layer.props = props;
        Ok(())
    }

    /// Register a committed stroke on its layer (pixels must already be applied by the caller).
    pub fn add_stroke(&mut self, rec: StrokeRecord) -> DResult<()> {
        if self.strokes.contains_key(&rec.id) {
            return Err(DocumentError::DuplicateId);
        }
        let id = rec.id;
        let is_vector_layer = matches!(self.layer(rec.layer).map(|l| &l.kind), Some(LayerKind::Vector(_)));
        if is_vector_layer && !rec.has_vector_form() {
            return Err(DocumentError::InvalidValue("vector layers accept only vector-representable strokes"));
        }
        let px = self.pixels_mut_editable(rec.layer)?;
        px.strokes.push(id);
        self.strokes.insert(id, Arc::new(rec));
        Ok(())
    }

    /// Record a non-stroke raster edit on a raster layer (fill/transform/clear/import). Vector
    /// layers reject these edits so that they always stay exactly vector-representable.
    pub fn mark_raster_edit(&mut self, id: LayerId, reason: &str) -> DResult<()> {
        let layer = self.layer_mut_editable(id)?;
        match &mut layer.kind {
            LayerKind::Raster(p) => {
                if !p.raster_edits.iter().any(|r| r == reason) {
                    p.raster_edits.push(reason.to_string());
                }
                Ok(())
            }
            LayerKind::Vector(_) => Err(DocumentError::InvalidValue("vector layers do not accept raster edits")),
            LayerKind::Group(_) => Err(DocumentError::NotAPixelLayer),
        }
    }

    /// Classify whether the layer's strokes faithfully represent its pixels (VEC-03).
    pub fn vector_fidelity(&self, id: LayerId) -> Option<VectorFidelity> {
        let p = self.layer(id)?.pixels()?;
        if !p.raster_edits.is_empty() || p.mask.is_some() {
            let mut reasons = p.raster_edits.clone();
            if p.mask.is_some() {
                reasons.push("layer_mask".into());
            }
            return Some(VectorFidelity::RasterAuthoritative { reasons });
        }
        let all_vector = p.strokes.iter().filter_map(|s| self.stroke(*s)).all(|s| s.has_vector_form());
        Some(if all_vector { VectorFidelity::Exact } else { VectorFidelity::PartiallyVector })
    }

    /// Full structural validation (used after load and in tests).
    pub fn validate(&self) -> Result<(), VeskraError> {
        limits::check_document_size(self.size.width, self.size.height)?;
        let corrupt = |m: String| -> VeskraError { DocumentError::Corrupt(m).into() };
        if self.layers.len() > limits::MAX_LAYERS {
            return Err(DocumentError::TooManyLayers.into());
        }
        let mut seen = BTreeSet::new();
        // Walk from root verifying parent links, uniqueness and depth.
        let mut stack: Vec<(LayerId, Option<LayerId>, usize)> = self.root.iter().rev().map(|i| (*i, None, 0)).collect();
        while let Some((id, parent, depth)) = stack.pop() {
            if depth > limits::MAX_GROUP_DEPTH {
                return Err(DocumentError::TooDeep.into());
            }
            if !seen.insert(id) {
                return Err(corrupt(format!("layer {id} referenced more than once (cycle or duplicate)")));
            }
            let l = self.layer(id).ok_or_else(|| corrupt(format!("dangling layer reference {id}")))?;
            if l.id != id {
                return Err(corrupt("layer id key mismatch".into()));
            }
            if l.parent != parent {
                return Err(corrupt(format!("parent link mismatch for {id}")));
            }
            Self::check_props(&l.props).map_err(VeskraError::from)?;
            match &l.kind {
                LayerKind::Group(g) => {
                    for c in g.children.iter().rev() {
                        stack.push((*c, Some(id), depth + 1));
                    }
                }
                LayerKind::Raster(p) | LayerKind::Vector(p) => {
                    for s in &p.strokes {
                        let rec = self.stroke(*s).ok_or_else(|| corrupt(format!("dangling stroke {s}")))?;
                        if rec.layer != id {
                            return Err(corrupt(format!("stroke {s} owner mismatch")));
                        }
                    }
                }
            }
        }
        if seen.len() != self.layers.len() {
            return Err(corrupt("unreachable (orphan) layers present".into()));
        }
        let referenced: usize = self.iter_layers().filter_map(|l| l.pixels()).map(|p| p.strokes.len()).sum();
        if referenced != self.strokes.len() {
            return Err(corrupt("orphan or multiply-referenced strokes".into()));
        }
        if let Some(a) = self.active_layer
            && !self.layers.contains_key(&a)
        {
            return Err(corrupt("active layer missing".into()));
        }
        Ok(())
    }

    /// Construct from raw parts (used by the native-format loader); validates before returning.
    pub fn from_parts(parts: DocumentParts) -> Result<Self, VeskraError> {
        let DocumentParts { id, size, dpi, color, root, layers, strokes, selection, active_layer } = parts;
        let mut doc = Document::new(id, size.width, size.height, dpi, color)?;
        doc.root = root;
        for l in layers {
            if doc.layers.insert(l.id, Arc::new(l)).is_some() {
                return Err(DocumentError::DuplicateId.into());
            }
        }
        for s in strokes {
            if doc.strokes.insert(s.id, Arc::new(s)).is_some() {
                return Err(DocumentError::DuplicateId.into());
            }
        }
        doc.selection = selection;
        doc.active_layer = active_layer;
        doc.validate()?;
        Ok(doc)
    }

    /// Decompose into owned parts (used by the native-format writer).
    pub fn to_parts(&self) -> DocumentParts {
        DocumentParts {
            id: self.id,
            size: self.size,
            dpi: self.dpi,
            color: self.color.clone(),
            root: self.root.clone(),
            layers: self.layers.values().map(|l| (**l).clone()).collect(),
            strokes: self.strokes.values().map(|s| (**s).clone()).collect(),
            selection: self.selection.clone(),
            active_layer: self.active_layer,
        }
    }

    /// Bytes of pixel tiles in this document (all layers).
    pub fn pixel_bytes(&self) -> usize {
        self.iter_layers().filter_map(|l| l.pixels()).map(|p| p.tiles.byte_size()).sum()
    }
}

fn kind_pixels(k: &LayerKind) -> Option<&PixelContent> {
    match k {
        LayerKind::Raster(p) | LayerKind::Vector(p) => Some(p),
        LayerKind::Group(_) => None,
    }
}

fn copy_name(name: &str) -> String {
    let mut n = format!("{name} copy");
    while n.len() > limits::MAX_NAME_BYTES {
        n.pop();
    }
    n
}
