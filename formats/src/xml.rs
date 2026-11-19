//! XML into a tree shaped like ElementTree's: namespaced names written
//! "{uri}local", and each element's text and tail. A DOCTYPE or ENTITY
//! declaration is refused, however the file is encoded.

use crate::text::{cp1252, latin1, utf16};
use crate::FormatError;

pub const DOCTYPE_REFUSED: &str = "the document contains a DOCTYPE, which Leaflet refuses to read";
pub const TOO_DEEP: &str = "the document is nested too deeply to read";

#[derive(Debug, Clone, Default)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub text: Option<String>,
    pub tail: Option<String>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// ElementTree's find() for the paths the readers use: "a", "a/b", ".//a".
    pub fn find(&self, path: &str) -> Option<&Element> {
        if let Some(rest) = path.strip_prefix(".//") {
            let mut parts = split_path(rest);
            let first = parts.remove(0);
            for d in self.descendants() {
                if d.tag == first {
                    if let Some(found) = d.find_steps(&parts) {
                        return Some(found);
                    }
                }
            }
            return None;
        }
        let parts = split_path(path);
        self.find_steps(&parts)
    }

    fn find_steps(&self, parts: &[String]) -> Option<&Element> {
        if parts.is_empty() {
            return Some(self);
        }
        for c in &self.children {
            if c.tag == parts[0] {
                if let Some(f) = c.find_steps(&parts[1..]) {
                    return Some(f);
                }
            }
        }
        None
    }

    pub fn findall<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.tag == tag)
    }

    /// iter(tag): this element and everything inside it, in document order.
    pub fn iter_tag<'a>(&'a self, tag: &str) -> Vec<&'a Element> {
        let mut out = Vec::new();
        self.collect(tag, &mut out);
        out
    }

    fn collect<'a>(&'a self, tag: &str, out: &mut Vec<&'a Element>) {
        if self.tag == tag {
            out.push(self);
        }
        for c in &self.children {
            c.collect(tag, out);
        }
    }

    /// Everything inside, depth first, not including this element.
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = Vec::new();
        for c in &self.children {
            out.push(c);
            out.extend(c.descendants());
        }
        out
    }
}

fn split_path(path: &str) -> Vec<String> {
    // names are "{uri}local" and uris contain '/', so split only outside braces
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for c in path.chars() {
        match c {
            '{' => {
                depth += 1;
                cur.push(c)
            }
            '}' => {
                depth -= 1;
                cur.push(c)
            }
            '/' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

fn has_declaration(data: &[u8]) -> bool {
    // re.search(rb'<!(DOCTYPE|ENTITY)', data, re.I)
    data.windows(9).any(|w| w[0] == b'<' && w[1] == b'!' && w[2..9].eq_ignore_ascii_case(b"DOCTYPE"))
        || data.windows(8).any(|w| w[0] == b'<' && w[1] == b'!' && w[2..8].eq_ignore_ascii_case(b"ENTITY"))
}

fn declared_encoding(head: &str) -> Option<String> {
    let head = head.trim_start_matches('\u{feff}');
    if !head.starts_with("<?xml") {
        return None;
    }
    let end = head.find("?>")?;
    let decl = &head[..end];
    let at = decl.find("encoding")?;
    let rest = decl[at + 8..].trim_start().strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let close = rest.find(quote)?;
    Some(rest[..close].trim().to_ascii_lowercase())
}

fn broken(msg: impl std::fmt::Display) -> FormatError {
    FormatError::new(format!("broken XML: {msg}"))
}

fn decode(data: &[u8]) -> Result<String, FormatError> {
    if let Some(rest) = data.strip_prefix(b"\xef\xbb\xbf") {
        return String::from_utf8(rest.to_vec()).map_err(broken);
    }
    if data.starts_with(b"\xff\xfe") {
        return Ok(utf16(&data[2..], true));
    }
    if data.starts_with(b"\xfe\xff") {
        return Ok(utf16(&data[2..], false));
    }
    if data.starts_with(b"<\0?\0") {
        return Ok(utf16(data, true));
    }
    if data.starts_with(b"\0<\0?") {
        return Ok(utf16(data, false));
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(200)]).into_owned();
    match declared_encoding(&head).as_deref() {
        None | Some("utf-8") | Some("utf8") | Some("us-ascii") | Some("ascii") => {
            String::from_utf8(data.to_vec()).map_err(broken)
        }
        Some("iso-8859-1") | Some("latin-1") | Some("latin1") | Some("iso8859-1") => Ok(latin1(data)),
        Some("windows-1252") | Some("cp1252") => Ok(cp1252(data)),
        Some(label) => {
            let enc = encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| broken("unknown encoding"))?;
            Ok(enc.decode_without_bom_handling(data).0.into_owned())
        }
    }
}

/// Drop the XML declaration: the text is already decoded, and roxmltree
/// only reads UTF-8 declarations.
fn without_declaration(text: &str) -> &str {
    let t = text.trim_start_matches('\u{feff}');
    if t.starts_with("<?xml") {
        if let Some(end) = t.find("?>") {
            return &t[end + 2..];
        }
    }
    t
}

pub fn parse(data: &[u8]) -> Result<Element, FormatError> {
    if has_declaration(data) {
        return Err(FormatError::new(DOCTYPE_REFUSED));
    }
    let text = decode(data)?;
    let text = without_declaration(&text);
    check_depth(text)?;
    let opts = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opts).map_err(|e| match e {
        roxmltree::Error::DtdDetected => FormatError::new(DOCTYPE_REFUSED),
        e => broken(e),
    })?;
    Ok(convert(doc.root_element()))
}

/// Refuse anything nested deeper than MAX_DEPTH. roxmltree, the conversion
/// below and the readers after it all recurse once per level, so this looks
/// at the text before any of them: a tiny file nested thousands of levels
/// deep would otherwise overflow the stack. It errs on the side of counting
/// deeper; text that isn't well-formed fails in roxmltree anyway.
fn check_depth(text: &str) -> Result<(), FormatError> {
    let b = text.as_bytes();
    let skip_past = |from: usize, end: &[u8]| b[from..].windows(end.len()).position(|w| w == end).map_or(b.len(), |p| from + p + end.len());
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(p) = b[i..].iter().position(|&c| c == b'<') {
        i += p;
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = skip_past(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_past(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_past(i + 2, b"?>");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_past(i + 2, b">");
        } else if rest.starts_with(b"<!") {
            i = skip_past(i + 2, b">");
        } else {
            // a start tag: find its end, past quoted attribute values
            let mut j = i + 1;
            let mut quote = 0u8;
            while j < b.len() {
                let c = b[j];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                j += 1;
            }
            if !(j < b.len() && b[j - 1] == b'/') {
                depth += 1;
                if depth > crate::MAX_DEPTH {
                    return Err(FormatError::new(TOO_DEEP));
                }
            }
            i = j + 1;
        }
        if i >= b.len() {
            break;
        }
    }
    Ok(())
}

fn name_of(ns: Option<&str>, local: &str) -> String {
    match ns {
        Some(uri) if !uri.is_empty() => format!("{{{uri}}}{local}"),
        _ => local.to_string(),
    }
}

fn convert(node: roxmltree::Node) -> Element {
    let tag = name_of(node.tag_name().namespace(), node.tag_name().name());
    let attrs = node.attributes().map(|a| (name_of(a.namespace(), a.name()), a.value().to_string())).collect();
    let mut el = Element { tag, attrs, ..Default::default() };
    let mut text: Option<String> = None;
    for child in node.children() {
        if child.is_element() {
            if let Some(last) = el.children.last_mut() {
                last.tail = text.take();
            } else {
                el.text = text.take();
            }
            el.children.push(convert(child));
        } else if child.is_text() {
            let t = child.text().unwrap_or("");
            text.get_or_insert_with(String::new).push_str(t);
        }
    }
    if let Some(last) = el.children.last_mut() {
        last.tail = text;
    } else {
        el.text = text;
    }
    el
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_tail() {
        let e = parse(b"<a xmlns:x=\"u\">one<x:b k=\"v\" x:k=\"w\">two</x:b>three<!-- c -->four<c/></a>").unwrap();
        assert_eq!(e.text.as_deref(), Some("one"));
        assert_eq!(e.children[0].tag, "{u}b");
        assert_eq!(e.children[0].get("k"), Some("v"));
        assert_eq!(e.children[0].get("{u}k"), Some("w"));
        assert_eq!(e.children[0].tail.as_deref(), Some("threefour"));
        assert!(e.children[1].tail.is_none());
        assert!(e.find(".//{u}b").is_some());
    }

    #[test]
    fn doctype_refused() {
        let r = parse(b"<?xml version=\"1.0\"?><!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>");
        assert_eq!(r.unwrap_err().to_string(), DOCTYPE_REFUSED);
        let utf16: Vec<u8> = "\u{feff}<!DOCTYPE a><a/>".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(parse(&utf16).unwrap_err().to_string(), DOCTYPE_REFUSED);
    }
}
