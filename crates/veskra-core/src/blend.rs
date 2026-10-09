//! Layer blend modes (v1 committed set: Normal, Multiply, Screen, Overlay).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
}

impl BlendMode {
    pub const ALL: [BlendMode; 4] = [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay];

    /// Stable machine name used by the native format, CLI and FFI.
    pub fn as_str(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
        }
    }

    pub fn parse(s: &str) -> Option<BlendMode> {
        BlendMode::ALL.into_iter().find(|m| m.as_str() == s)
    }

    /// Adobe PSD 4-character blend key (Photoshop File Formats spec, layer record).
    pub fn psd_key(self) -> &'static [u8; 4] {
        match self {
            BlendMode::Normal => b"norm",
            BlendMode::Multiply => b"mul ",
            BlendMode::Screen => b"scrn",
            BlendMode::Overlay => b"over",
        }
    }

    /// PDF/SVG (`mix-blend-mode`) names; both use the same keywords for this set.
    pub fn css_name(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
        }
    }

    /// PDF `/BM` name.
    pub fn pdf_name(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_roundtrip() {
        for m in BlendMode::ALL {
            assert_eq!(BlendMode::parse(m.as_str()), Some(m));
            assert_eq!(m.psd_key().len(), 4);
        }
        assert_eq!(BlendMode::parse("dissolve"), None);
    }
}
