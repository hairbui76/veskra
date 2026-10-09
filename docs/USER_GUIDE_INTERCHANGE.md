# Sharing your artwork: Photoshop, Illustrator and color (user guide draft, FMT-16)

> Draft for the in-app help and website. Statements are limited to what has been tested; see
> `docs/COMPATIBILITY.md` for the evidence behind each line. Adobe applications have **not** been
> tested yet; this guide says so wherever it matters.

## Your project file (`.veskra`)
* Always keep the `.veskra` file: it is the only format that keeps everything editable — every
  stroke with its pressure samples, brush settings, layers, masks, selection and color profile.
* Saving is all-or-nothing: Veskra writes a temporary copy, checks it completely, and only then
  replaces your file. If saving fails (for example the storage is full), your previous file is
  untouched and the app keeps showing **Unsaved changes**.
* Veskra also keeps a few automatic recovery copies in its private storage. Opening a recovery copy
  never overwrites your original file.

## Photoshop (`.psd`)
* Exports 8-bit RGB with your layers, groups, names, visibility, opacity, blend modes (Normal,
  Multiply, Screen, Overlay), clipping, layer masks, the merged image and the embedded color profile.
* Vector-stroke layers become ordinary pixel layers in the PSD; their editable strokes stay in your
  `.veskra` file.
* Anything that cannot be represented is converted to pixels on that layer and listed in the export
  summary — nothing is silently dropped.
* Tested with psd-tools and ImageMagick (layer tree, profile, merged image identical to Veskra) and
  Krita 5.2.2. **Krita 5.2.2 ignores clipping and group opacity when it opens PSD files**, so such
  documents look different in Krita. **Photoshop has not been tested yet.**

## Illustrator (`.svg` or `.pdf`) — "Export for Illustrator"
Veskra does **not** create native `.ai` files. Export SVG or PDF, open it in Illustrator, then use
*File › Save As › Adobe Illustrator (.ai)* if you need an `.ai` file.

The export sheet has three independent options:

| Option | What it does |
|---|---|
| **Editable vector strokes** | Pen, ink, marker and solid pencil strokes become editable, recolorable filled paths. They are outlines of the stroke shape, not Illustrator pressure brushes. |
| **Preserve original appearance** | Adds an exact picture of your artwork (textures, airbrush, charcoal, fills, masks). |
| **Embed color profile** | Writes the sRGB profile (PDF) or tags embedded images as sRGB (SVG). |

* With both of the first two options on, the file still looks exactly like your canvas — strokes are
  never drawn twice:
  * **SVG:** the picture is visible and the editable strokes are in a hidden layer
    called "Editable vectors".
  * **PDF:** editable strokes are visible and the picture only contains what cannot be a vector.
    (We chose this for PDF because some apps drop hidden PDF layers when importing.)
* Textured, airbrush and charcoal strokes, fills, and transformed or masked layers can never become
  editable vectors; the export sheet counts them before you export, and they are still included as
  pictures so nothing is lost.
* You cannot export with both of the first two options off.
* Tested with rsvg, Inkscape, Poppler and qpdf. **Adobe Illustrator has not been tested yet.**

## Color
* Veskra paints in **sRGB**. Exports carry an sRGB profile (or an sRGB tag) so other apps know how
  to display the numbers.
* Imported PNGs are converted from their embedded profile into sRGB. If a PNG has no profile, Veskra
  assumes sRGB and **tells you**; you can also choose "assign" to keep the numbers unchanged.
  Damaged profiles are refused rather than guessed.
* Identical numbers do not guarantee identical appearance on every screen: uncalibrated displays
  differ. Veskra's color math is measured against LittleCMS and colour-science
  (`docs/reports/color-baseline.md`); on-device display accuracy has **not** been measured yet.
* Wide-gamut (Display P3) painting is not available yet.
