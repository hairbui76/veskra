# ADR-009: `.veskra` container v1, staged save and recovery
Status: Accepted
Date: 2026-10-09
Owner: FMT
Related tickets: FMT-01..05, SAV-001..009

## Decision
* Container: **ZIP** (PKZIP APPNOTE subset: local headers, central directory, STORE and DEFLATE only,
  no encryption/zip64/multi-disk), read and written by Veskra's own bounded implementation in
  `veskra-format` (limits from `veskra_core::limits`). Entries:
  `mimetype` (stored first, `application/x-veskra`), `manifest.json` (schema version, app version,
  creation time, entry list), `project/document.json`, `profiles/<sha256>.icc`, `strokes/<id>.bin`
  (versioned little-endian binary of confirmed samples), `raster/<layer>/<tx>_<ty>.tile`
  (deflate of u16 LE RGBA premultiplied tile, with descriptor in document.json), `masks/...`,
  `preview/composite.png` (sRGB-tagged thumbnail), `integrity.json` (SHA-256 + length of every entry).
* Schema version **1**. Loader: unknown *newer* major → `UnsupportedSchema` (refuse, never rewrite);
  migrations are functions `vN → vN+1` with fixtures.
* Save protocol: snapshot → encode to `<target>.tmp-<random>` → fsync → re-open and fully validate
  (checksums + decode) → atomic rename → fsync parent dir → only then report `saved_version = N`.
  SAF destinations: Kotlin copies the validated temp file; completion only after stream close.
* Autosave: app-private recovery directory, bounded to the latest K checkpoints (default 3), written by
  the same validated protocol; recovery never overwrites the original document.

## Alternatives considered
1. SQLite bundle — random access, but heavier and harder to inspect/validate.
2. Third-party `zip` crate — broader features than needed; more parser surface to fuzz.

## Acceptance experiment
SAV-001, SAV-003 (kill during temp write simulated by truncation/injected failures), SAV-005..007,
SAV-009 automated; SAV-004/008 need device (BLOCKED).

## Review
ORCH 2026-10-09.
