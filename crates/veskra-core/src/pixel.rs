//! Pixel descriptors and color-space identity (ADR-005, ADR-006).
//!
//! Every raster buffer crossing a public API carries a [`PixelDescriptor`] so that no
//! component has to guess depth, alpha representation, transfer encoding or profile.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Authoritative raster tile edge in pixels (ADR-011, provisional pending device benchmark).
pub const TILE_SIZE: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PixelDepth {
    U8,
    U16,
    F16,
    F32,
}

impl PixelDepth {
    pub const fn bytes_per_channel(self) -> usize {
        match self {
            PixelDepth::U8 => 1,
            PixelDepth::U16 | PixelDepth::F16 => 2,
            PixelDepth::F32 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ChannelModel {
    /// R, G, B, A interleaved.
    Rgba,
    /// Single coverage/gray channel (masks, selections).
    Gray,
}

impl ChannelModel {
    pub const fn channels(self) -> usize {
        match self {
            ChannelModel::Rgba => 4,
            ChannelModel::Gray => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlphaRepresentation {
    Straight,
    Premultiplied,
    /// No alpha channel semantics (masks).
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleEncoding {
    /// Values are encoded with the profile's transfer function (e.g. sRGB curve).
    GammaEncoded,
    /// Values are linear-light in the profile's primaries.
    Linear,
}

/// Which numeric space layer blending happens in (document-level policy, ADR-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum BlendSpace {
    /// Blend on transfer-encoded values. Matches Photoshop/Krita defaults for 8/16-bit RGB
    /// documents and therefore PSD interchange. v1 default.
    #[default]
    GammaEncoded,
    /// Blend in linear light. Physically nicer, but other apps will recomposite differently;
    /// exporters must warn (PSD composite still embedded).
    Linear,
}

/// Color profile identity: SHA-256 of the exact ICC bytes plus a semantic name.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProfileId {
    #[serde(with = "hex32")]
    pub icc_hash: [u8; 32],
    pub semantic_name: String,
}

impl ProfileId {
    pub fn new(icc_hash: [u8; 32], semantic_name: impl Into<String>) -> Self {
        Self { icc_hash, semantic_name: semantic_name.into() }
    }
    pub fn hash_hex(&self) -> String {
        self.icc_hash.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProfileId({:?}, {}..)", self.semantic_name, &self.hash_hex()[..12])
    }
}

/// Full description of a pixel buffer's numeric meaning.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PixelDescriptor {
    pub depth: PixelDepth,
    pub model: ChannelModel,
    pub alpha: AlphaRepresentation,
    pub encoding: SampleEncoding,
    /// `None` only for masks/coverage buffers that carry no color.
    pub profile: Option<ProfileId>,
}

impl PixelDescriptor {
    /// v1 authoritative layer tile layout: RGBA u16, premultiplied, transfer-encoded in the
    /// document working profile. The profile itself lives on the document; this constant
    /// describes the layout/semantics and is completed with [`PixelDescriptor::with_profile`].
    pub const WORKING_V1: PixelDescriptor = PixelDescriptor {
        depth: PixelDepth::U16,
        model: ChannelModel::Rgba,
        alpha: AlphaRepresentation::Premultiplied,
        encoding: SampleEncoding::GammaEncoded,
        profile: None,
    };

    /// 8-bit coverage mask (selections, layer masks).
    pub const MASK_U8: PixelDescriptor = PixelDescriptor {
        depth: PixelDepth::U8,
        model: ChannelModel::Gray,
        alpha: AlphaRepresentation::None,
        encoding: SampleEncoding::Linear,
        profile: None,
    };

    /// Android `ARGB_8888` Bitmap memory layout: RGBA u8 premultiplied, sRGB-encoded.
    pub const DISPLAY_RGBA8_PREMUL: PixelDescriptor = PixelDescriptor {
        depth: PixelDepth::U8,
        model: ChannelModel::Rgba,
        alpha: AlphaRepresentation::Premultiplied,
        encoding: SampleEncoding::GammaEncoded,
        profile: None,
    };

    /// Straight-alpha 8-bit RGBA, the interchange layout for PNG/PSD channel data.
    pub const RGBA8_STRAIGHT: PixelDescriptor = PixelDescriptor {
        depth: PixelDepth::U8,
        model: ChannelModel::Rgba,
        alpha: AlphaRepresentation::Straight,
        encoding: SampleEncoding::GammaEncoded,
        profile: None,
    };

    pub fn with_profile(mut self, profile: ProfileId) -> Self {
        self.profile = Some(profile);
        self
    }

    pub fn bytes_per_pixel(&self) -> usize {
        self.depth.bytes_per_channel() * self.model.channels()
    }
}

mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        let hex: String = v.iter().map(|b| format!("{b:02x}")).collect();
        s.serialize_str(&hex)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        if s.len() != 64 || !s.is_ascii() {
            return Err(D::Error::custom("expected 64 hex chars"));
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(D::Error::custom)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_sizes() {
        assert_eq!(PixelDescriptor::WORKING_V1.bytes_per_pixel(), 8);
        assert_eq!(PixelDescriptor::MASK_U8.bytes_per_pixel(), 1);
        assert_eq!(PixelDescriptor::DISPLAY_RGBA8_PREMUL.bytes_per_pixel(), 4);
    }

    #[test]
    fn profile_id_serde_hex() {
        let mut h = [0u8; 32];
        h[0] = 0xab;
        h[31] = 0x01;
        let p = ProfileId::new(h, "sRGB IEC61966-2.1");
        let j = serde_json::to_string(&p).unwrap();
        assert!(j.contains("ab000000"));
        let back: ProfileId = serde_json::from_str(&j).unwrap();
        assert_eq!(back, p);
        assert!(serde_json::from_str::<ProfileId>(r#"{"icc_hash":"zz","semantic_name":"x"}"#).is_err());
    }
}
