# Licenses of the Infinite Painter property fixtures

These two files are brush *settings* (JSON), not brush images. They are used only as input
for Veskra's own importer tests (`crates/veskra-brush/tests/interop_ip.rs`,
`crates/veskra-engine/tests/brush_library.rs`), which build **synthetic** `.prbr` / `.przp`
archives around them with procedurally generated PNGs. No Infinite Painter application asset
(built-in head or texture image) is included anywhere in this repository.

| File | Source | License |
|---|---|---|
| `template-properties.json` | `src/template-properties.json` of <https://github.com/Iurika-N-T-R/ABR-to-PRZP-Brush-converter> | CC0-1.0 (public domain dedication) |
| `manga_inker.json` | `manga_inker.json` of <https://github.com/CanRau/infinite-painter-brushes> | The Unlicense (public domain dedication) |

Both are copied byte for byte (SHA-256 `78baad98…5cff` and `6c35a2aa…b2dd` respectively).
Infinite Painter is a product of Infinite Studio; Veskra is not affiliated with it. The field
knowledge these files illustrate is summarized, with its uncertainty, in
`docs/interop/infinite-painter-brushes.md`.
