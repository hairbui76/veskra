//! Independent structural PDF checker for Veskra output (FMT-14 `validate`). It re-parses the
//! written bytes without any writer state: header, `startxref` → classic xref table, every
//! in-use entry pointing at `N G obj`, trailer `/Root`/`/Size`, every indirect reference
//! resolving to an in-use object, stream `/Length` framing, Flate streams decompressing, and
//! balanced `q/Q` and `BDC/EMC` in content streams, image sample counts matching the image color
//! space (`/DeviceGray` 1, `/DeviceRGB` 3, `/DeviceCMYK` 4, `[/ICCBased n]` → the profile's `/N`),
//! and — when present — an `/OutputIntents` destination profile stream with `/N` 1, 3 or 4. It
//! also extracts a small summary (MediaBox, OCG names and their default `/OFF` state, CMYK
//! images / fills, output intent, page group color space) for tests.

use std::collections::{BTreeMap, BTreeSet};

/// Facts extracted while validating.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PdfSummary {
    pub objects: usize,
    pub root: u32,
    pub streams: usize,
    pub media_box: Option<[f64; 4]>,
    /// OCG names in object order.
    pub ocg_names: Vec<String>,
    /// Names of OCGs listed in the default configuration's `/OFF` array.
    pub off_ocgs: Vec<String>,
    pub image_xobjects: usize,
    pub form_xobjects: usize,
    /// Number of `f` (fill) operators across content streams.
    pub fill_ops: usize,
    /// Image XObjects in `/DeviceCMYK`.
    pub cmyk_images: usize,
    /// Number of `k` (DeviceCMYK fill color) operators across content streams.
    pub cmyk_fill_ops: usize,
    /// `/N` of the output intent's `/DestOutputProfile`, when the catalog has `/OutputIntents`.
    pub output_intent_components: Option<u32>,
    /// Color space of the page's transparency group (`/DeviceRGB`, `/DeviceCMYK` or an object ref).
    pub page_group_cs: Option<String>,
}

const MAX_STREAM: usize = 1 << 30;

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    h.get(from..)?.windows(n.len()).position(|w| w == n).map(|p| p + from)
}

fn rfind(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).rposition(|w| w == n)
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\x0C' | 0)
}

fn is_delim(c: u8) -> bool {
    matches!(c, b'[' | b']' | b'<' | b'>' | b'(' | b')' | b'/' | b'{' | b'}' | b'%')
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(Vec<u8>),
    Delim(u8),
}

/// Tokenize PDF object syntax / content streams (strings kept as one token).
fn tokenize(b: &[u8]) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if is_ws(c) {
            i += 1;
        } else if c == b'%' {
            while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                i += 1;
            }
        } else if c == b'(' {
            let mut depth = 1;
            let mut s = Vec::new();
            i += 1;
            while i < b.len() && depth > 0 {
                match b[i] {
                    b'\\' => {
                        if let Some(n) = b.get(i + 1) {
                            s.push(match n {
                                b'n' => b'\n',
                                b'r' => b'\r',
                                b't' => b'\t',
                                x => *x,
                            });
                        }
                        i += 2;
                        continue;
                    }
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    s.push(b[i]);
                }
                i += 1;
            }
            if depth != 0 {
                return Err("unterminated string".into());
            }
            out.push(Tok::Str(s));
        } else if (c == b'<' || c == b'>') && b.get(i + 1) == Some(&c) {
            out.push(Tok::Delim(c));
            out.push(Tok::Delim(c));
            i += 2;
        } else if c == b'<' {
            let end = find(b, b">", i).ok_or("unterminated hex string")?;
            let hex: Vec<u8> = b[i + 1..end].iter().copied().filter(|c| !is_ws(*c)).collect();
            let mut s = Vec::new();
            for pair in hex.chunks(2) {
                let t = std::str::from_utf8(pair).map_err(|_| "bad hex string")?;
                let t = if t.len() == 1 { format!("{t}0") } else { t.to_string() };
                s.push(u8::from_str_radix(&t, 16).map_err(|_| "bad hex string")?);
            }
            out.push(Tok::Str(s));
            i = end + 1;
        } else if c == b'/' {
            let start = i;
            i += 1;
            while i < b.len() && !is_ws(b[i]) && !is_delim(b[i]) {
                i += 1;
            }
            out.push(Tok::Word(String::from_utf8_lossy(&b[start..i]).into_owned()));
        } else if is_delim(c) {
            out.push(Tok::Delim(c));
            i += 1;
        } else {
            let start = i;
            while i < b.len() && !is_ws(b[i]) && !is_delim(b[i]) {
                i += 1;
            }
            out.push(Tok::Word(String::from_utf8_lossy(&b[start..i]).into_owned()));
        }
    }
    Ok(out)
}

fn refs(toks: &[Tok]) -> Vec<u32> {
    let mut out = Vec::new();
    for w in toks.windows(3) {
        if let (Tok::Word(a), Tok::Word(g), Tok::Word(r)) = (&w[0], &w[1], &w[2])
            && r == "R"
            && let (Ok(n), Ok(_)) = (a.parse::<u32>(), g.parse::<u16>())
        {
            out.push(n);
        }
    }
    out
}

fn word_after<'a>(toks: &'a [Tok], key: &str) -> Option<&'a str> {
    let i = toks.iter().position(|t| matches!(t, Tok::Word(w) if w == key))?;
    match toks.get(i + 1)? {
        Tok::Word(w) => Some(w.as_str()),
        _ => None,
    }
}

fn has_pair(toks: &[Tok], key: &str, val: &str) -> bool {
    word_after(toks, key) == Some(val)
}

fn ref_list_after(toks: &[Tok], key: &str) -> Vec<u32> {
    let Some(i) = toks.iter().position(|t| matches!(t, Tok::Word(w) if w == key)) else { return Vec::new() };
    let Some(end) = toks.iter().skip(i).position(|t| *t == Tok::Delim(b']')) else { return Vec::new() };
    refs(&toks[i..i + end])
}

fn text_string(s: &[u8]) -> String {
    if s.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = s[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        s.iter().map(|&c| c as char).collect()
    }
}

fn parse_usize(b: &[u8]) -> Option<usize> {
    std::str::from_utf8(b).ok()?.trim().parse().ok()
}

/// Validate a PDF produced by Veskra. Returns a summary on success.
pub fn validate_pdf(b: &[u8]) -> Result<PdfSummary, String> {
    if !b.starts_with(b"%PDF-1.") {
        return Err("missing %PDF-1.x header".into());
    }
    let end = b.iter().rposition(|c| !is_ws(*c)).map(|p| p + 1).unwrap_or(0);
    if !b[..end].ends_with(b"%%EOF") {
        return Err("missing %%EOF".into());
    }
    let sx = rfind(b, b"startxref").ok_or("missing startxref")?;
    let xref_off = parse_usize(b.get(sx + 9..end - 5).unwrap_or(&[])).ok_or("bad startxref value")?;
    if b.get(xref_off..xref_off + 4) != Some(b"xref") {
        return Err("startxref does not point at an xref table".into());
    }
    // xref subsections.
    let mut pos = xref_off + 4;
    let mut entries: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
    let mut max_num = 0u32;
    loop {
        while pos < b.len() && is_ws(b[pos]) {
            pos += 1;
        }
        if b.get(pos..pos + 7) == Some(b"trailer") {
            break;
        }
        let eol = find(b, b"\n", pos).ok_or("truncated xref")?;
        let line = std::str::from_utf8(&b[pos..eol]).map_err(|_| "bad xref line")?;
        let mut it = line.split_whitespace();
        let (Some(first), Some(count), None) = (it.next(), it.next(), it.next()) else {
            return Err(format!("bad xref subsection header '{line}'"));
        };
        let first: u32 = first.parse().map_err(|_| "bad xref first")?;
        let count: u32 = count.parse().map_err(|_| "bad xref count")?;
        if count > 8_000_000 {
            return Err("xref too large".into());
        }
        pos = eol + 1;
        for k in 0..count {
            let e = b.get(pos..pos + 20).ok_or("truncated xref entry")?;
            let off = parse_usize(&e[0..10]).ok_or("bad xref offset")?;
            let gen_ = parse_usize(&e[11..16]).ok_or("bad xref generation")? as u16;
            let num = first + k;
            max_num = max_num.max(num);
            match e[17] {
                b'n' => {
                    entries.insert(num, (off, gen_));
                }
                b'f' => {}
                _ => return Err("bad xref entry type".into()),
            }
            if !(is_ws(e[18]) && is_ws(e[19])) {
                return Err("xref entry is not 20 bytes".into());
            }
            pos += 20;
        }
    }
    let tr = find(b, b"<<", pos).ok_or("missing trailer dictionary")?;
    let trailer = tokenize(b.get(tr..sx).ok_or("bad trailer")?)?;
    let size: u32 = word_after(&trailer, "/Size").and_then(|s| s.parse().ok()).ok_or("trailer without /Size")?;
    if size != max_num + 1 {
        return Err(format!("/Size {size} does not match xref ({} entries)", max_num + 1));
    }
    let root = refs(&trailer).first().copied().ok_or("trailer without /Root")?;
    if !has_pair(&trailer, "/Root", &root.to_string()) {
        return Err("trailer /Root is not an indirect reference".into());
    }
    let mut summary = PdfSummary { objects: entries.len(), root, ..Default::default() };
    let mut all_refs: BTreeSet<u32> = refs(&trailer).into_iter().collect();
    let mut dicts: BTreeMap<u32, Vec<Tok>> = BTreeMap::new();
    // Images whose color space is an indirect object: (object, samples, color space object).
    let mut indirect_images: Vec<(u32, usize, u32)> = Vec::new();
    for (&num, &(off, gen_)) in &entries {
        let head = format!("{num} {gen_} obj");
        if b.get(off..off + head.len()) != Some(head.as_bytes()) {
            return Err(format!("xref offset of object {num} does not point at '{head}'"));
        }
        let start = off + head.len();
        let endobj = find(b, b"endobj", start).ok_or(format!("object {num} without endobj"))?;
        let stream_kw = find(b, b"stream", start).filter(|s| *s < endobj);
        let dict_end = stream_kw.unwrap_or(endobj);
        let toks = tokenize(&b[start..dict_end])?;
        all_refs.extend(refs(&toks));
        if let Some(sk) = stream_kw {
            summary.streams += 1;
            let len = word_after(&toks, "/Length")
                .and_then(|v| v.parse::<usize>().ok())
                .ok_or(format!("object {num}: stream without direct /Length"))?;
            if len > MAX_STREAM {
                return Err("stream too large".into());
            }
            let mut ds = sk + 6;
            if b.get(ds) == Some(&b'\r') {
                ds += 1;
            }
            if b.get(ds) != Some(&b'\n') {
                return Err(format!("object {num}: stream keyword not followed by EOL"));
            }
            ds += 1;
            let data = b.get(ds..ds + len).ok_or(format!("object {num}: stream data truncated"))?;
            let mut after = ds + len;
            while after < b.len() && is_ws(b[after]) {
                after += 1;
            }
            if b.get(after..after + 9) != Some(b"endstream") {
                return Err(format!("object {num}: /Length does not end at endstream"));
            }
            let real_endobj = find(b, b"endobj", after).ok_or("missing endobj after stream")?;
            if !b.get(after + 9..real_endobj).is_some_and(|t| t.iter().all(|c| is_ws(*c))) {
                return Err(format!("object {num}: junk after endstream"));
            }
            let decoded = if has_pair(&toks, "/Filter", "/FlateDecode") {
                miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, MAX_STREAM)
                    .map_err(|_| format!("object {num}: Flate stream does not decompress"))?
            } else {
                data.to_vec()
            };
            let subtype = word_after(&toks, "/Subtype");
            match subtype {
                Some("/Image") => {
                    summary.image_xobjects += 1;
                    let w: usize =
                        word_after(&toks, "/Width").and_then(|v| v.parse().ok()).ok_or("image without /Width")?;
                    let h: usize =
                        word_after(&toks, "/Height").and_then(|v| v.parse().ok()).ok_or("image without /Height")?;
                    let samples = w.checked_mul(h).ok_or("image too large")?;
                    let comps = match word_after(&toks, "/ColorSpace") {
                        Some("/DeviceGray") => 1,
                        Some("/DeviceRGB") => 3,
                        Some("/DeviceCMYK") => {
                            summary.cmyk_images += 1;
                            4
                        }
                        Some(r) => {
                            let r: u32 =
                                r.parse().map_err(|_| format!("object {num}: unsupported image color space"))?;
                            if decoded.len() % samples.max(1) != 0 {
                                return Err(format!("object {num}: image data is not a whole number of samples"));
                            }
                            indirect_images.push((num, decoded.len() / samples.max(1), r));
                            0
                        }
                        None => return Err(format!("object {num}: image without /ColorSpace")),
                    };
                    let need = samples * comps;
                    if comps > 0 && decoded.len() != need {
                        return Err(format!("object {num}: image data {} bytes, expected {need}", decoded.len()));
                    }
                }
                // Streams with `/N` are ICC profiles, not content.
                None | Some("/Form") if word_after(&toks, "/N").is_none() => {
                    if subtype == Some("/Form") {
                        summary.form_xobjects += 1;
                    }
                    let ct = tokenize(&decoded)?;
                    let (mut q, mut mc) = (0i64, 0i64);
                    for t in &ct {
                        if let Tok::Word(w) = t {
                            match w.as_str() {
                                "q" => q += 1,
                                "Q" => q -= 1,
                                "BDC" | "BMC" => mc += 1,
                                "EMC" => mc -= 1,
                                "f" => summary.fill_ops += 1,
                                "k" => summary.cmyk_fill_ops += 1,
                                _ => {}
                            }
                            if q < 0 || mc < 0 {
                                return Err(format!("object {num}: unbalanced q/Q or BDC/EMC"));
                            }
                        }
                    }
                    if q != 0 || mc != 0 {
                        return Err(format!("object {num}: unbalanced q/Q or BDC/EMC"));
                    }
                }
                _ => {}
            }
        }
        dicts.insert(num, toks);
    }
    for r in &all_refs {
        if !entries.contains_key(r) {
            return Err(format!("reference to missing object {r}"));
        }
    }
    let root_dict = dicts.get(&root).ok_or("root object missing")?;
    if !has_pair(root_dict, "/Type", "/Catalog") {
        return Err("root is not a /Catalog".into());
    }
    // `/N` of the ICC stream an object reference (or a `[/ICCBased n 0 R]` array object) names.
    let icc_components = |r: u32| -> Option<u32> {
        let t = dicts.get(&r)?;
        let stream = if t.iter().any(|x| matches!(x, Tok::Word(w) if w == "/ICCBased")) {
            dicts.get(refs(t).first()?)?
        } else {
            t
        };
        word_after(stream, "/N")?.parse().ok()
    };
    for (num, comps, cs) in indirect_images {
        match icc_components(cs) {
            Some(n) if n as usize == comps && matches!(n, 1 | 3 | 4) => {}
            _ => return Err(format!("object {num}: image samples do not match its ICCBased color space")),
        }
    }
    if root_dict.iter().any(|t| matches!(t, Tok::Word(w) if w == "/OutputIntents")) {
        let dest = word_after(root_dict, "/DestOutputProfile")
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or("output intent without /DestOutputProfile")?;
        match icc_components(dest) {
            Some(n) if matches!(n, 1 | 3 | 4) => summary.output_intent_components = Some(n),
            _ => return Err("output intent profile stream without a valid /N".into()),
        }
    }
    let mut ocg_name_by_num = BTreeMap::new();
    for (num, toks) in &dicts {
        if has_pair(toks, "/Type", "/Page")
            && let Some(i) = toks.iter().position(|t| matches!(t, Tok::Word(w) if w == "/MediaBox"))
        {
            let v: Vec<f64> = toks
                .iter()
                .skip(i + 2)
                .take(4)
                .filter_map(|t| if let Tok::Word(w) = t { w.parse().ok() } else { None })
                .collect();
            if let [a, b2, c, d] = v[..] {
                summary.media_box = Some([a, b2, c, d]);
            }
            summary.page_group_cs = word_after(toks, "/CS").map(str::to_string);
        }
        if has_pair(toks, "/Type", "/OCG") {
            let i = toks.iter().position(|t| matches!(t, Tok::Word(w) if w == "/Name"));
            let name = match i.and_then(|i| toks.get(i + 1)) {
                Some(Tok::Str(s)) => text_string(s),
                _ => return Err(format!("OCG {num} without /Name string")),
            };
            ocg_name_by_num.insert(*num, name.clone());
            summary.ocg_names.push(name);
        }
    }
    for r in ref_list_after(root_dict, "/OFF") {
        if let Some(n) = ocg_name_by_num.get(&r) {
            summary.off_ocgs.push(n.clone());
        }
    }
    Ok(summary)
}
