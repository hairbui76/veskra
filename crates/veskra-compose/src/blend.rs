//! Per-pixel blend math on **premultiplied `f32`** RGBA (PNT-007, DOC-02).
//!
//! # Formulas (W3C Compositing and Blending Level 1)
//! With premultiplied backdrop `b = (cb, αb)` and source `s = (cs, αs)`:
//!
//! ```text
//! co = cs·(1 − αb) + cb·(1 − αs) + αs·αb·B(Cb, Cs)       (per color channel)
//! αo = αs + αb·(1 − αs)
//! ```
//!
//! where `Cb = cb/αb`, `Cs = cs/αs` are the *unpremultiplied* values (0 when the alpha is 0),
//! clamped to `[0, 1]`, and `B` is the separable blend function:
//!
//! | mode     | `B(Cb, Cs)`                                                  |
//! |----------|--------------------------------------------------------------|
//! | Normal   | `Cs` (implemented as plain source-over `cs + cb·(1 − αs)`)  |
//! | Multiply | `Cb·Cs`                                                      |
//! | Screen   | `Cb + Cs − Cb·Cs`                                            |
//! | Overlay  | `HardLight(Cs, Cb)`: `Cb ≤ 0.5 ? 2·Cs·Cb : Screen(Cs, 2·Cb − 1)` |
//!
//! These are the same functions psd-tools uses (`psd_tools/composite/blend.py`), so the
//! GammaEncoded results are the Photoshop-compatible ones (ADR-005, ADR-014).
//!
//! The functions here are blend-space agnostic: callers pass values already in the blend space
//! (encoded or linear-light); see the crate docs for [`veskra_core::BlendSpace`] handling.
//! Output color channels are clamped to `[0, αo]` so the premultiplied invariant holds.
//! Any mode added to the `#[non_exhaustive]` [`BlendMode`] in the future falls back to Normal
//! here until it is implemented (exporters must not claim support for it).

use veskra_core::BlendMode;

/// The separable blend function `B(Cb, Cs)` on **straight** values in `[0, 1]`.
#[inline]
pub fn separable(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::Overlay => {
            if cb <= 0.5 {
                2.0 * cs * cb
            } else {
                let t = 2.0 * cb - 1.0;
                cs + t - cs * t
            }
        }
        _ => cs,
    }
}

/// Composite premultiplied `src` over premultiplied `backdrop` with `mode` (formulas above).
/// A source with `αs ≤ 0` returns the backdrop unchanged.
#[inline]
pub fn blend_px(backdrop: [f32; 4], src: [f32; 4], mode: BlendMode) -> [f32; 4] {
    if src[3] <= 0.0 {
        return backdrop;
    }
    match mode {
        BlendMode::Multiply => general_px(backdrop, src, |b, s| b * s),
        BlendMode::Screen => general_px(backdrop, src, |b, s| b + s - b * s),
        BlendMode::Overlay => general_px(backdrop, src, overlay_fn),
        _ => normal_px(backdrop, src),
    }
}

#[inline(always)]
pub(crate) fn overlay_fn(cb: f32, cs: f32) -> f32 {
    separable(BlendMode::Overlay, cb, cs)
}

/// Normal (plain source-over) for `αs > 0`: `co = cs + cb·(1 − αs)`.
#[inline(always)]
pub(crate) fn normal_px(backdrop: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - src[3];
    let ao = (src[3] + backdrop[3] * k).min(1.0);
    [
        (src[0] + backdrop[0] * k).clamp(0.0, ao),
        (src[1] + backdrop[1] * k).clamp(0.0, ao),
        (src[2] + backdrop[2] * k).clamp(0.0, ao),
        ao,
    ]
}

/// General W3C formula for `αs > 0` with separable `b(Cb, Cs)`.
#[inline(always)]
pub(crate) fn general_px(backdrop: [f32; 4], src: [f32; 4], b: impl Fn(f32, f32) -> f32) -> [f32; 4] {
    let (sa, ba) = (src[3], backdrop[3]);
    if ba <= 0.0 {
        // Over a fully transparent backdrop every mode reduces to the source (αb = 0 ⇒ co = cs).
        return normal_px(backdrop, src);
    }
    let ao = (sa + ba * (1.0 - sa)).min(1.0);
    let (inv_sa, inv_ba, sb) = (1.0 / sa, 1.0 / ba, sa * ba);
    let ch = |i: usize| {
        let cb = (backdrop[i] * inv_ba).clamp(0.0, 1.0);
        let cs = (src[i] * inv_sa).clamp(0.0, 1.0);
        (src[i] * (1.0 - ba) + backdrop[i] * (1.0 - sa) + sb * b(cb, cs)).clamp(0.0, ao)
    };
    [ch(0), ch(1), ch(2), ao]
}

/// Premultiplied linear interpolation `a + (b − a)·t` (pass-through group opacity/mask).
#[inline]
pub fn lerp_px(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let out_a = a[3] + (b[3] - a[3]) * t;
    let f = |i: usize| (a[i] + (b[i] - a[i]) * t).clamp(0.0, out_a.max(0.0));
    [f(0), f(1), f(2), out_a.clamp(0.0, 1.0)]
}

/// Multiply all four premultiplied channels by `k` (opacity × mask coverage).
#[inline]
pub fn scale_px(p: [f32; 4], k: f32) -> [f32; 4] {
    [p[0] * k, p[1] * k, p[2] * k, p[3] * k]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_source_is_identity() {
        let b = [0.2, 0.3, 0.1, 0.5];
        for m in BlendMode::ALL {
            assert_eq!(blend_px(b, [0.0; 4], m), b);
        }
    }

    #[test]
    fn overlay_is_hardlight_swapped() {
        for &cb in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            for &cs in &[0.0f32, 0.3, 0.6, 1.0] {
                let hard_light = |b: f32, s: f32| {
                    if s <= 0.5 { b * 2.0 * s } else { separable(BlendMode::Screen, b, 2.0 * s - 1.0) }
                };
                assert!((separable(BlendMode::Overlay, cb, cs) - hard_light(cs, cb)).abs() < 1e-6);
            }
        }
    }
}
