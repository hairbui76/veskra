//! Schema migration framework (ADR-009: migrations are functions `vN -> vN+1` with fixtures).
//!
//! Only schema 1 exists, so the production registry [`MIGRATIONS`] is empty and schema-1
//! documents pass through unchanged. When schema 2 is introduced, add
//! `Migration { from: 1, apply: v1_to_v2 }` here plus a committed schema-1 fixture that the
//! new reader must open.

use crate::error::FormatError;
use crate::{CURRENT_SCHEMA, Manifest};
use serde_json::Value;

/// One `from -> from + 1` step over the `project/document.json` value.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub from: u32,
    pub apply: fn(Value) -> Result<Value, FormatError>,
}

/// Production registry (empty: schema 1 is the first released schema).
pub const MIGRATIONS: &[Migration] = &[];

/// Migrate a document JSON value from `manifest.schema` to [`CURRENT_SCHEMA`].
/// Newer schemas are returned unchanged (the caller has already applied the `minReader` gate).
pub fn migrate(manifest: &Manifest, doc_json: Value) -> Result<Value, FormatError> {
    migrate_with(MIGRATIONS, manifest.schema, CURRENT_SCHEMA, doc_json)
}

/// Whether a migration path `from -> to` exists in `registry` (trivially true when `from >= to`).
pub fn has_path(registry: &[Migration], from: u32, to: u32) -> bool {
    (from..to).all(|v| registry.iter().any(|m| m.from == v))
}

/// Apply registered steps `from -> from+1 -> ... -> to` in order.
pub fn migrate_with(registry: &[Migration], from: u32, to: u32, mut doc_json: Value) -> Result<Value, FormatError> {
    for v in from..to {
        let step = registry.iter().find(|m| m.from == v).ok_or(FormatError::UnsupportedSchema {
            schema: from,
            min_reader: from,
            supported: to,
        })?;
        doc_json = (step.apply)(doc_json)?;
    }
    Ok(doc_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Synthetic, never-released "schema 0" used only to exercise the framework:
    /// it called the layer list `layerList` and had no `activeLayer`.
    fn v0_to_v1(mut v: Value) -> Result<Value, FormatError> {
        let obj = v.as_object_mut().ok_or_else(|| FormatError::Corrupt("document is not an object".into()))?;
        let layers = obj.remove("layerList").ok_or_else(|| FormatError::Corrupt("v0: layerList missing".into()))?;
        obj.insert("layers".into(), layers);
        obj.entry("activeLayer").or_insert(Value::Null);
        Ok(v)
    }

    #[test]
    fn v1_passes_through_production_registry() {
        let v = json!({"layers": [], "x": 1});
        assert_eq!(migrate_with(MIGRATIONS, 1, CURRENT_SCHEMA, v.clone()).unwrap(), v);
        assert!(has_path(MIGRATIONS, 1, 1));
        assert!(!has_path(MIGRATIONS, 0, 1));
        assert!(matches!(
            migrate_with(MIGRATIONS, 0, 1, v).unwrap_err(),
            FormatError::UnsupportedSchema { schema: 0, .. }
        ));
    }

    #[test]
    fn synthetic_v0_chain() {
        let reg = [Migration { from: 0, apply: v0_to_v1 }];
        let out = migrate_with(&reg, 0, 1, json!({"layerList": [1, 2]})).unwrap();
        assert_eq!(out, json!({"layers": [1, 2], "activeLayer": null}));
        assert!(migrate_with(&reg, 0, 1, json!({"nope": 1})).is_err());
        // Missing intermediate step is a typed error, not a panic.
        assert!(migrate_with(&reg, 0, 2, json!({"layerList": []})).is_err());
    }
}
