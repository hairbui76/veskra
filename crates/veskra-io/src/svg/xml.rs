//! Small XML 1.0 well-formedness checker and element tree for validating written SVG (FMT-B).
//!
//! Checks: single root element, matching start/end tags, quoted attributes without `<`,
//! no duplicate attributes, valid entity/character references, valid XML characters, comments
//! without `--`, CDATA sections, processing instructions, and that every namespace prefix used
//! by an element or attribute is declared in scope. No DTDs (an SVG export never needs one).
//! Iterative (no recursion), so deeply nested input cannot overflow the stack.

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<usize>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Parsed document: `elements[0]` is the root.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct XmlDoc {
    pub elements: Vec<Element>,
}

impl XmlDoc {
    /// Indices of every descendant of `i` (pre-order, excluding `i`).
    pub fn descendants(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> =
            self.elements.get(i).map(|e| e.children.iter().rev().copied().collect()).unwrap_or_default();
        while let Some(c) = stack.pop() {
            out.push(c);
            if let Some(e) = self.elements.get(c) {
                stack.extend(e.children.iter().rev().copied());
            }
        }
        out
    }
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':' || (c as u32) >= 0xC0
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.' || c == '\u{B7}'
}

struct Cursor<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        self.s.get(self.pos..).unwrap_or("")
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn starts(&self, p: &str) -> bool {
        self.rest().starts_with(p)
    }
    fn bump(&mut self, n: usize) {
        self.pos += n;
    }
    fn skip_ws(&mut self) -> bool {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if matches!(c, ' ' | '\t' | '\n' | '\r') {
                self.bump(1);
            } else {
                break;
            }
        }
        self.pos > start
    }
    fn name(&mut self) -> Result<String, String> {
        let start = self.pos;
        match self.peek() {
            Some(c) if is_name_start(c) => self.bump(c.len_utf8()),
            _ => return Err(format!("expected name at byte {}", self.pos)),
        }
        while let Some(c) = self.peek() {
            if is_name_char(c) {
                self.bump(c.len_utf8());
            } else {
                break;
            }
        }
        Ok(self.s.get(start..self.pos).unwrap_or("").to_string())
    }
    fn until(&mut self, end: &str) -> Result<&'a str, String> {
        let rest = self.rest();
        match rest.find(end) {
            Some(i) => {
                let out = rest.get(..i).unwrap_or("");
                self.bump(i + end.len());
                Ok(out)
            }
            None => Err(format!("unterminated construct, expected '{end}'")),
        }
    }
}

/// Decode text/attribute content, validating references and characters.
fn decode(text: &str, in_attr: bool) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut it = text.char_indices();
    while let Some((i, c)) = it.next() {
        if !is_xml_char(c) {
            return Err(format!("invalid XML character U+{:04X}", c as u32));
        }
        match c {
            '<' => return Err("'<' not allowed here".into()),
            '&' => {
                let rest = text.get(i + 1..).unwrap_or("");
                let end = rest.find(';').ok_or("unterminated reference")?;
                let r = rest.get(..end).unwrap_or("");
                let ch = match r {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" => '\'',
                    _ if r.starts_with("#x") => u32::from_str_radix(r.get(2..).unwrap_or(""), 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or("bad hex character reference")?,
                    _ if r.starts_with('#') => r
                        .get(1..)
                        .unwrap_or("")
                        .parse::<u32>()
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or("bad character reference")?,
                    _ => return Err(format!("undeclared entity '&{r};'")),
                };
                if !is_xml_char(ch) {
                    return Err("reference to invalid character".into());
                }
                out.push(ch);
                for _ in 0..=end {
                    it.next();
                }
            }
            _ => out.push(c),
        }
    }
    if !in_attr && text.contains("]]>") {
        return Err("']]>' in content".into());
    }
    Ok(out)
}

fn misc(c: &mut Cursor) -> Result<bool, String> {
    if c.skip_ws() {
        return Ok(true);
    }
    if c.starts("<!--") {
        c.bump(4);
        let body = c.until("-->")?;
        if body.contains("--") || body.ends_with('-') {
            return Err("'--' inside comment".into());
        }
        decode_chars(body)?;
        return Ok(true);
    }
    if c.starts("<?") {
        c.bump(2);
        let target = c.name()?;
        if target.eq_ignore_ascii_case("xml") && c.pos != 2 + target.len() {
            return Err("XML declaration not at start".into());
        }
        let body = c.until("?>")?;
        decode_chars(body)?;
        return Ok(true);
    }
    if c.starts("<!DOCTYPE") {
        return Err("DOCTYPE not supported in Veskra SVG".into());
    }
    Ok(false)
}

fn decode_chars(s: &str) -> Result<(), String> {
    match s.chars().find(|c| !is_xml_char(*c)) {
        Some(c) => Err(format!("invalid XML character U+{:04X}", c as u32)),
        None => Ok(()),
    }
}

fn prefix(name: &str) -> Option<&str> {
    name.split_once(':').map(|(p, _)| p)
}

/// Parse and check well-formedness.
pub(crate) fn parse(s: &str) -> Result<XmlDoc, String> {
    let mut c = Cursor { s, pos: 0 };
    if c.starts("\u{FEFF}") {
        c.bump(3);
    }
    while misc(&mut c)? {}
    let mut elements: Vec<Element> = Vec::new();
    // Stack of (element index, declared prefixes at this level).
    let mut stack: Vec<(usize, Vec<String>)> = Vec::new();
    let mut root_done = false;
    loop {
        if stack.is_empty() {
            if root_done {
                while misc(&mut c)? {}
                if c.pos < s.len() {
                    return Err(format!("content after root element at byte {}", c.pos));
                }
                break;
            }
            if !c.starts("<") {
                return Err("expected root element".into());
            }
        }
        if c.pos >= s.len() {
            return Err("unexpected end of document (unclosed element)".into());
        }
        if c.starts("</") {
            c.bump(2);
            let name = c.name()?;
            c.skip_ws();
            if !c.starts(">") {
                return Err("malformed end tag".into());
            }
            c.bump(1);
            let (top, _) = stack.pop().ok_or("end tag without start")?;
            if elements.get(top).map(|e| e.name.as_str()) != Some(name.as_str()) {
                return Err(format!("mismatched end tag </{name}>"));
            }
            if stack.is_empty() {
                root_done = true;
            }
            continue;
        }
        if c.starts("<!--") || c.starts("<?") {
            misc(&mut c)?;
            continue;
        }
        if c.starts("<![CDATA[") {
            if stack.is_empty() {
                return Err("CDATA outside root".into());
            }
            c.bump(9);
            decode_chars(c.until("]]>")?)?;
            continue;
        }
        if c.starts("<") {
            c.bump(1);
            let name = c.name()?;
            let mut attrs: Vec<(String, String)> = Vec::new();
            let mut self_close = false;
            loop {
                let ws = c.skip_ws();
                if c.starts("/>") {
                    c.bump(2);
                    self_close = true;
                    break;
                }
                if c.starts(">") {
                    c.bump(1);
                    break;
                }
                if !ws {
                    return Err(format!("expected whitespace before attribute in <{name}>"));
                }
                let an = c.name()?;
                c.skip_ws();
                if !c.starts("=") {
                    return Err(format!("attribute '{an}' without value"));
                }
                c.bump(1);
                c.skip_ws();
                let q = c.peek().ok_or("unexpected end in attribute")?;
                if q != '"' && q != '\'' {
                    return Err(format!("unquoted attribute '{an}'"));
                }
                c.bump(1);
                let raw = c.until(if q == '"' { "\"" } else { "'" })?;
                let val = decode(raw, true)?;
                if attrs.iter().any(|(k, _)| *k == an) {
                    return Err(format!("duplicate attribute '{an}'"));
                }
                attrs.push((an, val));
            }
            let declared: Vec<String> =
                attrs.iter().filter_map(|(k, _)| k.strip_prefix("xmlns:").map(str::to_string)).collect();
            let in_scope = |p: &str| {
                p == "xml"
                    || p == "xmlns"
                    || declared.iter().any(|d| d == p)
                    || stack.iter().any(|(_, ds)| ds.iter().any(|d| d == p))
            };
            for n in std::iter::once(&name).chain(attrs.iter().map(|(k, _)| k)) {
                if let Some(p) = prefix(n)
                    && !in_scope(p)
                {
                    return Err(format!("undeclared namespace prefix '{p}'"));
                }
            }
            let idx = elements.len();
            elements.push(Element { name, attrs, children: Vec::new() });
            if let Some((parent, _)) = stack.last()
                && let Some(p) = elements.get_mut(*parent)
            {
                p.children.push(idx);
            } else if idx != 0 {
                return Err("multiple root elements".into());
            }
            if self_close {
                if stack.is_empty() {
                    root_done = true;
                }
            } else {
                stack.push((idx, declared));
            }
            continue;
        }
        // Character data up to the next '<'.
        let rest = c.rest();
        let n = rest.find('<').unwrap_or(rest.len());
        decode(rest.get(..n).unwrap_or(""), false)?;
        c.bump(n);
    }
    if elements.is_empty() {
        return Err("no root element".into());
    }
    Ok(XmlDoc { elements })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed() {
        let d = parse(
            "<?xml version=\"1.0\"?>\n<!-- c --><a xmlns:x=\"u\" x:y='1'><b>t &amp; &#x41;</b><![CDATA[<>]]><c/></a>\n",
        )
        .unwrap();
        assert_eq!(d.elements[0].name, "a");
        assert_eq!(d.elements[0].children.len(), 2);
        assert_eq!(d.elements[0].attr("x:y"), Some("1"));
        assert_eq!(d.descendants(0).len(), 2);
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "<a><b></a></b>",
            "<a>",
            "<a></a><b/>",
            "<a x=1/>",
            "<a x='1' x='2'/>",
            "<a>&foo;</a>",
            "<a>x < y</a>",
            "<a x='<'/>",
            "<a><!-- a -- b --></a>",
            "<a y:z='1'/>",
            "<p:a/>",
            "<a>\u{1}</a>",
            "text",
            "<!DOCTYPE a><a/>",
            "<a>]]></a>",
            "<a b='1'c='2'/>",
        ] {
            assert!(parse(bad).is_err(), "accepted: {bad}");
        }
    }
}
