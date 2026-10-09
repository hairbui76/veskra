//! Deterministic, locale-independent number formatting for SVG path data and PDF content
//! streams.
//!
//! Rules: fixed-point rounding to `decimals` places (Rust's `{:.N}` formatting, which rounds
//! the exact binary value half-to-even and never uses exponent notation or locale
//! separators), trailing zeros and a trailing `.` stripped, `-0` normalized to `0`,
//! non-finite values written as `0` (paths produced by this crate never contain them).

/// Largest supported number of decimals; larger requests are clamped (f64 has ~15–17
/// significant digits, so more would only print noise).
pub const MAX_DECIMALS: u8 = 12;

/// Append the formatted number to `out`.
pub fn push_num(out: &mut String, v: f64, decimals: u8) {
    let d = usize::from(decimals.min(MAX_DECIMALS));
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{v:.d$}");
    let trimmed = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s.as_str() };
    if trimmed == "-0" || trimmed.is_empty() {
        out.push('0');
    } else {
        out.push_str(trimmed);
    }
}

/// Format a single number (convenience wrapper around [`push_num`]).
pub fn fmt_num(v: f64, decimals: u8) -> String {
    let mut s = String::new();
    push_num(&mut s, v, decimals);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_rules() {
        assert_eq!(fmt_num(1.0, 3), "1");
        assert_eq!(fmt_num(1.5, 3), "1.5");
        assert_eq!(fmt_num(-0.0001, 3), "0");
        assert_eq!(fmt_num(-0.0, 3), "0");
        assert_eq!(fmt_num(-0.4, 0), "0");
        assert_eq!(fmt_num(-1.25, 1), "-1.2"); // exact binary 1.25, half-to-even
        assert_eq!(fmt_num(1e20, 2), "100000000000000000000");
        assert_eq!(fmt_num(1.23456e-9, 3), "0");
        assert_eq!(fmt_num(f64::NAN, 3), "0");
        assert_eq!(fmt_num(123.456789, 200), "123.456789");
        assert!(!fmt_num(-3.0e-300, 12).contains('e'));
    }
}
