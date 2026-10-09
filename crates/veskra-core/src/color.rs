//! Color value contract between UI, brush, document and exporters.

use serde::{Deserialize, Serialize};

/// A color in the **document working space**, transfer-encoded (sRGB curve for v1),
/// with **straight** (non-premultiplied) alpha. Components are `[0, 1]`.
///
/// This is what the color picker produces, what brushes paint with and what
/// exporters write as vector fill colors (after any destination transform).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl ColorRgba {
    pub const BLACK: ColorRgba = ColorRgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: ColorRgba = ColorRgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    pub const TRANSPARENT: ColorRgba = ColorRgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0)
    }

    /// Parse `#RRGGBB` or `#RRGGBBAA` (leading `#` optional). sRGB hex entry per MASTER_PLAN §1.1.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if !(s.len() == 6 || s.len() == 8) || !s.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        let a = if s.len() == 8 { byte(6)? } else { 255 };
        Some(Self::from_rgba8(byte(0)?, byte(2)?, byte(4)?, a))
    }

    pub fn to_rgba8(&self) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [q(self.r), q(self.g), q(self.b), q(self.a)]
    }

    /// `#RRGGBB` (alpha omitted when opaque).
    pub fn to_hex(&self) -> String {
        let [r, g, b, a] = self.to_rgba8();
        if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
    }

    pub fn is_valid(&self) -> bool {
        [self.r, self.g, self.b, self.a].iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }

    pub fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = ColorRgba::from_hex("#FF8000").unwrap();
        assert_eq!(c.to_rgba8(), [255, 128, 0, 255]);
        assert_eq!(c.to_hex(), "#FF8000");
        assert_eq!(ColorRgba::from_hex("10203040").unwrap().to_hex(), "#10203040");
        assert!(ColorRgba::from_hex("#12345").is_none());
        assert!(ColorRgba::from_hex("#GGGGGG").is_none());
        assert!(ColorRgba::from_hex("#ÿÿÿ").is_none());
    }
}
