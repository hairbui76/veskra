use super::*;
use crate::transfer::srgb_to_linear_f64;

/// Test-only builder for synthetic ICC profiles (v2 and v4 shapes, malformed variants).
struct Builder {
    version: [u8; 2],
    class: [u8; 4],
    cs: [u8; 4],
    pcs: [u8; 4],
    tags: Vec<([u8; 4], Vec<u8>)>,
}

fn s15(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz(v: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    v.iter().for_each(|&x| t.extend_from_slice(&s15(x)));
    t
}

fn curv_gamma(g: f64) -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend_from_slice(&1u32.to_be_bytes());
    t.extend_from_slice(&((g * 256.0).round() as u16).to_be_bytes());
    t
}

fn curv_table(e: &[u16]) -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend_from_slice(&(e.len() as u32).to_be_bytes());
    e.iter().for_each(|v| t.extend_from_slice(&v.to_be_bytes()));
    t
}

fn para(ft: u16, p: &[f64]) -> Vec<u8> {
    let mut t = b"para\0\0\0\0".to_vec();
    t.extend_from_slice(&ft.to_be_bytes());
    t.extend_from_slice(&[0, 0]);
    p.iter().for_each(|&x| t.extend_from_slice(&s15(x)));
    t
}

fn desc_v2(s: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend_from_slice(&(s.len() as u32 + 1).to_be_bytes());
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t.extend_from_slice(&[0; 4 + 4 + 2 + 1 + 67]); // unicode + scriptcode parts (empty)
    t
}

fn text(s: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t
}

fn mluc(records: &[(&[u8; 4], &str)]) -> Vec<u8> {
    let mut t = b"mluc\0\0\0\0".to_vec();
    t.extend_from_slice(&(records.len() as u32).to_be_bytes());
    t.extend_from_slice(&12u32.to_be_bytes());
    let mut strings = Vec::new();
    let base = 16 + 12 * records.len();
    for (lc, s) in records {
        let u: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        t.extend_from_slice(*lc);
        t.extend_from_slice(&(u.len() as u32).to_be_bytes());
        t.extend_from_slice(&((base + strings.len()) as u32).to_be_bytes());
        strings.extend_from_slice(&u);
    }
    t.extend_from_slice(&strings);
    t
}

fn srgb_colorants() -> [[f64; 3]; 3] {
    let m = RgbProfileSpec::srgb().d50_matrix().unwrap();
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

impl Builder {
    fn rgb(version: [u8; 2]) -> Self {
        Builder { version, class: *b"mntr", cs: *b"RGB ", pcs: *b"XYZ ", tags: Vec::new() }
    }
    fn tag(mut self, s: &[u8; 4], d: Vec<u8>) -> Self {
        self.tags.push((*s, d));
        self
    }
    fn with_colorants(self, c: [[f64; 3]; 3]) -> Self {
        self.tag(b"rXYZ", xyz(c[0])).tag(b"gXYZ", xyz(c[1])).tag(b"bXYZ", xyz(c[2]))
    }
    fn with_trc(self, t: Vec<u8>) -> Self {
        self.tag(b"rTRC", t.clone()).tag(b"gTRC", t.clone()).tag(b"bTRC", t)
    }
    fn build(&self) -> Vec<u8> {
        let mut h = vec![0u8; 128];
        h[8] = self.version[0];
        h[9] = self.version[1];
        h[12..16].copy_from_slice(&self.class);
        h[16..20].copy_from_slice(&self.cs);
        h[20..24].copy_from_slice(&self.pcs);
        h[36..40].copy_from_slice(b"acsp");
        let start = 132 + 12 * self.tags.len();
        let mut table = (self.tags.len() as u32).to_be_bytes().to_vec();
        let mut data = Vec::new();
        for (s, d) in &self.tags {
            table.extend_from_slice(s);
            table.extend_from_slice(&((start + data.len()) as u32).to_be_bytes());
            table.extend_from_slice(&(d.len() as u32).to_be_bytes());
            data.extend_from_slice(d);
            while !data.len().is_multiple_of(4) {
                data.push(0);
            }
        }
        let mut out = h;
        out.extend_from_slice(&table);
        out.extend_from_slice(&data);
        let n = out.len() as u32;
        out[0..4].copy_from_slice(&n.to_be_bytes());
        out
    }
}

/// HP/Microsoft-style v2 sRGB: `desc` type, `text` copyright, D65 `wtpt`, 1024-entry `curv`.
fn v2_srgb_like() -> Vec<u8> {
    let table: Vec<u16> = (0..1024).map(|i| (srgb_to_linear_f64(i as f64 / 1023.0) * 65535.0).round() as u16).collect();
    Builder::rgb([2, 0x10])
        .tag(b"desc", desc_v2("sRGB IEC61966-2.1"))
        .tag(b"cprt", text("Copyright test"))
        .tag(b"wtpt", xyz([0.9505, 1.0, 1.0891]))
        .with_colorants(srgb_colorants())
        .with_trc(curv_table(&table))
        .build()
}

#[test]
fn generated_srgb_profile_is_valid_and_deterministic() {
    let p = IccProfile::srgb();
    let again = build_rgb_profile_bytes(&RgbProfileSpec::srgb()).unwrap();
    assert_eq!(p.bytes(), &again[..], "writer must be deterministic");
    assert!(Arc::ptr_eq(&p, &IccProfile::srgb()));
    assert_eq!(p.version(), IccVersion { major: 4, minor: 3, bugfix: 0 });
    assert_eq!(p.class(), ProfileClass::Display);
    assert_eq!(p.color_space(), ProfileColorSpace::Rgb);
    assert_eq!(p.description(), "sRGB IEC61966-2.1 (Veskra)");
    assert_eq!(p.copyright(), Some("No copyright, use freely"));
    assert!(p.is_srgb_equivalent());
    assert!(!p.is_display_p3_equivalent());
    assert_eq!(p.id().semantic_name, SRGB_SEMANTIC_NAME);
    let h: [u8; 32] = Sha256::digest(p.bytes()).into();
    assert_eq!(p.id().icc_hash, h);
    assert_eq!(p.bytes().len() % 4, 0);
    // Rows of the colorant matrix sum exactly to encoded D50.
    let m = p.to_xyz_d50_matrix();
    let d50q = [63190.0 / 65536.0, 1.0, 54061.0 / 65536.0];
    for r in 0..3 {
        assert_eq!(m[r][0] + m[r][1] + m[r][2], d50q[r]);
    }
    assert_eq!(p.media_white_point(), Some(d50q));
    let chad = p.chromatic_adaptation().unwrap();
    assert!((chad[0][0] - 1.0479).abs() < 1e-3);
    match p.trc(0) {
        Some(ToneCurve::Parametric(pc)) => assert_eq!(pc.function_type(), 3),
        other => panic!("unexpected curve {other:?}"),
    }
    // MD5 profile ID is filled and verifiable.
    assert_eq!(&p.bytes()[84..100], &md5::icc_profile_id(p.bytes()).unwrap()[..]);
    assert_ne!(&p.bytes()[84..100], &[0u8; 16][..]);
}

#[test]
fn generated_p3_profile() {
    let p = IccProfile::display_p3();
    assert_eq!(p.description(), "Display P3 (Veskra)");
    assert!(p.is_display_p3_equivalent());
    assert!(!p.is_srgb_equivalent());
    assert_eq!(p.id().semantic_name, DISPLAY_P3_SEMANTIC_NAME);
    // Apple's published Display P3 D50 colorants (to 4 decimals).
    let m = p.to_xyz_d50_matrix();
    let apple = [[0.5151, 0.2920, 0.1571], [0.2412, 0.6922, 0.0666], [-0.0011, 0.0419, 0.7841]];
    assert!(math::max_abs_diff(&m, &apple) < 2e-4, "{m:?}");
}

#[test]
fn third_party_v2_srgb_is_recognized() {
    let p = IccProfile::parse(&v2_srgb_like()).unwrap();
    assert_eq!(p.version().major, 2);
    assert_eq!(p.description(), "sRGB IEC61966-2.1");
    assert_eq!(p.copyright(), Some("Copyright test"));
    assert!(p.is_srgb_equivalent());
    assert_eq!(p.id().semantic_name, SRGB_SEMANTIC_NAME);
    assert_ne!(p.id(), IccProfile::srgb().id(), "different bytes, different hash");
    assert_eq!(p.effective_media_white(), D50_XYZ, "v2 display wtpt ignored like LittleCMS");
    assert!(p.is_colorimetrically_equivalent(&IccProfile::srgb(), 5e-4, 5e-4));
}

#[test]
fn gamma_22_is_not_srgb() {
    let b = Builder::rgb([4, 0x30]).with_colorants(srgb_colorants()).with_trc(curv_gamma(2.2)).build();
    let p = IccProfile::parse(&b).unwrap();
    assert!(matches!(p.trc(1), Some(ToneCurve::Gamma(g)) if (g - 2.19921875).abs() < 1e-12));
    assert!(!p.is_srgb_equivalent());
    assert_eq!(p.description(), "");
    assert_eq!(p.id().semantic_name, "Unnamed RGB profile");
    assert_eq!(p.effective_media_white(), D50_XYZ);
}

#[test]
fn gray_profile() {
    let mut b = Builder::rgb([4, 0x20]).tag(b"kTRC", para(0, &[2.2])).tag(b"desc", mluc(&[(b"enUS", "Gray 2.2")]));
    b.cs = *b"GRAY";
    let p = IccProfile::parse(&b.build()).unwrap();
    assert_eq!(p.color_space(), ProfileColorSpace::Gray);
    assert_eq!(p.description(), "Gray 2.2");
    let x = p.to_pcs_xyz([1.0, 0.0, 0.0]);
    assert!((x[1] - 1.0).abs() < 1e-12 && (x[0] - D50_XYZ[0]).abs() < 1e-12);
    assert!(p.trc(1).is_none());
    assert!(!p.is_srgb_equivalent());
}

#[test]
fn mluc_prefers_en_us() {
    let b = Builder::rgb([4, 0x30])
        .tag(b"desc", mluc(&[(b"deDE", "Farbraum"), (b"enGB", "Colour"), (b"enUS", "Color")]))
        .with_colorants(srgb_colorants())
        .with_trc(para(0, &[1.0]))
        .build();
    assert_eq!(IccProfile::parse(&b).unwrap().description(), "Color");
}

#[test]
fn unsupported_color_spaces_and_classes() {
    let mut b = Builder::rgb([4, 0x30]).with_colorants(srgb_colorants()).with_trc(curv_gamma(2.2));
    b.cs = *b"CMYK";
    let e = IccProfile::parse(&b.build()).unwrap_err();
    assert_eq!(e, ColorError::UnsupportedColorSpace("CMYK".into()));
    assert_eq!(veskra_core::VeskraError::from(e).code, veskra_core::ErrorCode::UnsupportedColorMode);
    b.cs = *b"Lab ";
    assert!(matches!(IccProfile::parse(&b.build()), Err(ColorError::UnsupportedColorSpace(_))));
    b.cs = *b"RGB ";
    b.class = *b"link";
    assert!(matches!(IccProfile::parse(&b.build()), Err(ColorError::UnsupportedProfileClass(_))));
    b.class = *b"mntr";
    b.version = [5, 0];
    assert_eq!(IccProfile::parse(&b.build()).unwrap_err(), ColorError::UnsupportedVersion { major: 5, minor: 0 });
}

#[test]
fn lut_only_profiles_are_rejected_with_typed_error() {
    let lut = b"mft2\0\0\0\0\x03\x03\x02\0".to_vec();
    let b = Builder::rgb([2, 0x10]).tag(b"A2B0", lut.clone()).tag(b"B2A0", lut.clone()).build();
    let e = IccProfile::parse(&b).unwrap_err();
    assert_eq!(e, ColorError::LutOnlyProfile);
    assert_eq!(veskra_core::VeskraError::from(e).code, veskra_core::ErrorCode::UnsupportedColorMode);
    let mut lab = Builder::rgb([4, 0x30]).tag(b"A2B0", lut.clone());
    lab.pcs = *b"Lab ";
    assert_eq!(IccProfile::parse(&lab.build()).unwrap_err(), ColorError::LutOnlyProfile);
    // Matrix/TRC + LUT: accepted, flagged.
    let both = Builder::rgb([4, 0x30]).with_colorants(srgb_colorants()).with_trc(curv_gamma(2.2)).tag(b"A2B0", lut);
    assert!(IccProfile::parse(&both.build()).unwrap().has_lut_tags());
}

#[test]
fn structural_errors_are_typed() {
    let good = v2_srgb_like();
    assert!(IccProfile::parse(&good).is_ok());

    assert!(matches!(IccProfile::parse(&good[..100]), Err(ColorError::Truncated { .. })));
    assert!(matches!(IccProfile::parse(&good[..good.len() - 4]), Err(ColorError::Truncated { .. })));
    let mut longer = good.clone();
    longer.extend_from_slice(&[0; 4]);
    assert!(matches!(IccProfile::parse(&longer), Err(ColorError::SizeMismatch { .. })));

    let mut magic = good.clone();
    magic[36] = b'x';
    assert_eq!(IccProfile::parse(&magic).unwrap_err(), ColorError::InvalidProfile("missing 'acsp' file signature"));

    let mut oob = good.clone();
    oob[132 + 4..132 + 8].copy_from_slice(&(good.len() as u32 - 4).to_be_bytes());
    assert!(matches!(IccProfile::parse(&oob), Err(ColorError::InvalidTag { reason: "tag data out of bounds", .. })));

    let mut huge_off = good.clone();
    huge_off[132 + 4..132 + 8].copy_from_slice(&u32::MAX.to_be_bytes());
    huge_off[132 + 8..132 + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(IccProfile::parse(&huge_off), Err(ColorError::InvalidTag { .. })));

    let mut into_header = good.clone();
    into_header[132 + 4..132 + 8].copy_from_slice(&16u32.to_be_bytes());
    assert!(matches!(IccProfile::parse(&into_header), Err(ColorError::InvalidTag { .. })));

    let mut count = good.clone();
    count[128..132].copy_from_slice(&1000u32.to_be_bytes());
    assert!(matches!(IccProfile::parse(&count), Err(ColorError::Truncated { what: "tag table", .. })));
    count[128..132].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(IccProfile::parse(&count).unwrap_err(), ColorError::InvalidProfile("absurd tag count"));

    // Second tag partially overlapping the first.
    let mut overlap = good.clone();
    let first_off = u32::from_be_bytes(good[136..140].try_into().unwrap());
    overlap[148..152].copy_from_slice(&(first_off + 4).to_be_bytes());
    assert_eq!(IccProfile::parse(&overlap).unwrap_err(), ColorError::InvalidProfile("overlapping tag data"));

    // Duplicate signature.
    let mut dup = good.clone();
    dup[144..148].copy_from_slice(b"desc");
    assert!(matches!(IccProfile::parse(&dup), Err(ColorError::InvalidTag { reason: "duplicate tag signature", .. })));

    let missing = Builder::rgb([4, 0x30])
        .with_colorants(srgb_colorants())
        .tag(b"rTRC", curv_gamma(2.2))
        .tag(b"gTRC", curv_gamma(2.2))
        .build();
    assert_eq!(IccProfile::parse(&missing).unwrap_err(), ColorError::MissingTag("bTRC"));

    let singular = Builder::rgb([4, 0x30]).with_colorants([[0.5, 0.2, 0.0]; 3]).with_trc(curv_gamma(2.2)).build();
    assert_eq!(IccProfile::parse(&singular).unwrap_err(), ColorError::InvalidProfile("singular colorant matrix"));

    let mut short_curv = curv_table(&[0, 100, 200]);
    short_curv[8..12].copy_from_slice(&50u32.to_be_bytes());
    let bad = Builder::rgb([4, 0x30]).with_colorants(srgb_colorants()).with_trc(short_curv).build();
    assert!(matches!(
        IccProfile::parse(&bad),
        Err(ColorError::InvalidTag { reason: "curv entries exceed tag size", .. })
    ));

    let zero_gamma = Builder::rgb([4, 0x30]).with_colorants(srgb_colorants()).with_trc(curv_gamma(0.0)).build();
    assert!(matches!(IccProfile::parse(&zero_gamma), Err(ColorError::InvalidTag { reason: "zero gamma", .. })));

    let bad_para = Builder::rgb([4, 0x30]).with_colorants(srgb_colorants()).with_trc(para(9, &[1.0])).build();
    assert!(matches!(IccProfile::parse(&bad_para), Err(ColorError::InvalidTag { .. })));

    let wrong_type = Builder::rgb([4, 0x30]).with_colorants(srgb_colorants()).with_trc(text("x")).build();
    assert!(matches!(IccProfile::parse(&wrong_type), Err(ColorError::InvalidTag { .. })));
}

#[test]
fn oversize_profile_is_allocation_limit() {
    let mut big = vec![0u8; MAX_ICC_BYTES + 1];
    let n = big.len() as u32;
    big[0..4].copy_from_slice(&n.to_be_bytes());
    let e = IccProfile::parse(&big).unwrap_err();
    assert!(matches!(e, ColorError::TooLarge { .. }));
    assert_eq!(veskra_core::VeskraError::from(e).code, veskra_core::ErrorCode::AllocationLimit);
}

#[test]
fn shared_trc_tags_are_allowed() {
    // The generated profile shares one `para` element between r/g/bTRC.
    let p = IccProfile::srgb();
    let b = p.bytes();
    let off = |i: usize| u32::from_be_bytes(b[132 + 12 * i + 4..132 + 12 * i + 8].try_into().unwrap());
    assert_eq!(off(7), off(8));
    assert_eq!(off(8), off(9));
}

#[test]
fn debug_does_not_dump_bytes() {
    let s = format!("{:?}", IccProfile::srgb());
    assert!(s.contains("sRGB IEC61966-2.1 (Veskra)"));
    assert!(s.len() < 400);
}
