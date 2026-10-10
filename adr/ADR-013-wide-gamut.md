# ADR-013: Wide gamut
Status: Accepted
Date: 2026-10-09
Owner: CLR
Related tickets: CLR-04, NEXT-02, COL-011, COL-012

## Decision
v1 is sRGB-only for authoring, display and export. A generated Display P3 profile exists solely for
conversion tests. No P3 UI toggle may ship until NEXT-02 passes device surface negotiation and
export readback tests. Requests for unsupported output spaces return a typed warning/error, never a
mislabelled profile (COL-012).

## Amendment
2026-10-10: ADR-017 allows *export* to convert to and embed sRGB, Display P3 or an Adobe RGB
compatible profile. Authoring and display remain sRGB-only as decided here.

## Review
ORCH 2026-10-09.
