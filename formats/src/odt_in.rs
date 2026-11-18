//! ODT in.

use crate::docx_in::css_length_px;
use crate::images::sniff_image;
use crate::model::*;
use crate::text::{is_digits, is_web_link, py_float, py_int};
use crate::xml::{self, Element};
use crate::zipio::Zip;
use crate::{FormatError, MAX_IMAGE};
use std::collections::{BTreeSet, HashMap};

pub const NS: [(&str, &str); 11] = [
    ("office", "urn:oasis:names:tc:opendocument:xmlns:office:1.0"),
    ("style", "urn:oasis:names:tc:opendocument:xmlns:style:1.0"),
    ("text", "urn:oasis:names:tc:opendocument:xmlns:text:1.0"),
    ("table", "urn:oasis:names:tc:opendocument:xmlns:table:1.0"),
    ("draw", "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"),
    ("fo", "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"),
    ("xlink", "http://www.w3.org/1999/xlink"),
    ("svg", "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"),
    ("meta", "urn:oasis:names:tc:opendocument:xmlns:meta:1.0"),
    ("manifest", "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"),
    ("loext", "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0"),
];

pub fn ns(prefix: &str) -> &'static str {
    NS.iter().find(|(p, _)| *p == prefix).map(|(_, u)| *u).unwrap()
}

/// "text:p" -> "{urn:...:text:1.0}p"
pub fn q(name: &str) -> String {
    let (prefix, local) = name.split_once(':').unwrap();
    format!("{{{}}}{local}", ns(prefix))
}

#[derive(Default, Clone, Copy)]
struct Props {
    b: Option<bool>,
    i: Option<bool>,
    u: Option<bool>,
    s: Option<bool>,
    sup: Option<bool>,
    sub: Option<bool>,
}

struct Styles {
    styles: HashMap<(Option<String>, Option<String>), Element>,
    lists: HashMap<Option<String>, Element>,
}

impl Styles {
    fn new(roots: &[Option<&Element>]) -> Styles {
        let mut s = Styles { styles: HashMap::new(), lists: HashMap::new() };
        for root in roots.iter().flatten() {
            for container in ["office:styles", "office:automatic-styles"] {
                for c in root.iter_tag(&q(container)) {
                    for st in c.findall(&q("style:style")) {
                        let key = (st.get(&q("style:family")).map(str::to_string), st.get(&q("style:name")).map(str::to_string));
                        s.styles.insert(key, st.clone());
                    }
                    for ls in c.findall(&q("text:list-style")) {
                        s.lists.insert(ls.get(&q("style:name")).map(str::to_string), ls.clone());
                    }
                }
            }
        }
        s
    }

    fn chain(&self, family: &str, name: Option<&str>) -> Vec<&Element> {
        let mut out = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut name = name.map(str::to_string);
        while let Some(n) = name.clone() {
            if n.is_empty() || seen.contains(&n) {
                break;
            }
            let Some(st) = self.styles.get(&(Some(family.to_string()), Some(n.clone()))) else { break };
            seen.push(n);
            out.push(st);
            name = st.get(&q("style:parent-style-name")).map(str::to_string);
        }
        out
    }

    fn text_props(&self, family: &str, name: Option<&str>) -> Props {
        let mut p = Props::default();
        for st in self.chain(family, name) {
            let Some(tp) = st.find(&q("style:text-properties")) else { continue };
            if let Some(fw) = tp.get(&q("fo:font-weight")) {
                p.b = p.b.or(Some(matches!(fw, "bold" | "600" | "700" | "800" | "900")));
            }
            if let Some(fs) = tp.get(&q("fo:font-style")) {
                p.i = p.i.or(Some(matches!(fs, "italic" | "oblique")));
            }
            if let Some(us) = tp.get(&q("style:text-underline-style")) {
                p.u = p.u.or(Some(us != "none"));
            }
            if let Some(ls) = tp.get(&q("style:text-line-through-style")) {
                p.s = p.s.or(Some(ls != "none"));
            }
            if let Some(pos) = tp.get(&q("style:text-position")) {
                let first = pos.split_whitespace().next().unwrap_or("0");
                let shift = match first {
                    "super" => 33.0,
                    "sub" => -33.0,
                    f => py_float(f.trim_end_matches('%')).unwrap_or(0.0),
                };
                p.sup = p.sup.or(Some(shift > 0.0));
                p.sub = p.sub.or(Some(shift < 0.0));
            }
        }
        p
    }

    fn para_props(&self, name: Option<&str>) -> (String, bool, i64) {
        let mut align = String::new();
        let mut brk = false;
        let mut heading = 0;
        for st in self.chain("paragraph", name) {
            if let Some(pp) = st.find(&q("style:paragraph-properties")) {
                if let Some(a) = pp.get(&q("fo:text-align")).filter(|a| !a.is_empty()) {
                    if align.is_empty() {
                        align = match a {
                            "center" => "center",
                            "end" | "right" => "right",
                            "justify" => "justify",
                            _ => "-",
                        }
                        .into();
                    }
                }
                if pp.get(&q("fo:break-before")) == Some("page") {
                    brk = true;
                }
            }
            let sname = st.get(&q("style:name")).unwrap_or("");
            let level = sname.strip_prefix("Heading_20_").filter(|n| n.len() == 1 && ("1"..="6").contains(n));
            let outline = st.get(&q("style:default-outline-level")).unwrap_or("");
            if heading == 0 && level.is_some() {
                heading = py_int(level.unwrap()).unwrap();
            } else if heading == 0 && sname == "Title" {
                heading = 1;
            } else if heading == 0 && is_digits(outline) && sname.starts_with("Heading") {
                heading = py_int(outline).unwrap_or(0).min(6);
            }
        }
        (if align == "-" { String::new() } else { align }, brk, heading)
    }

    fn list_ordered(&self, name: Option<&str>, level: i64) -> bool {
        let Some(ls) = self.lists.get(&name.map(str::to_string)) else { return false };
        let want = (level + 1).to_string();
        for child in &ls.children {
            if child.get(&q("text:level")) == Some(want.as_str()) {
                return child.tag == q("text:list-level-style-number") && !child.get(&q("style:num-format")).unwrap_or("1").is_empty();
            }
        }
        false
    }
}

struct Reader<'a> {
    z: Zip<'a>,
    content: Element,
    styles: Styles,
    warnings: BTreeSet<String>,
    styles_raw: Vec<u8>,
}

pub fn read_odt(data: &[u8]) -> Result<Document, FormatError> {
    let mut z = Zip::open(data)?;
    let content = xml::parse(&z.read("content.xml", true)?.unwrap_or_default())?;
    let styles_data = z.read("styles.xml", false)?;
    let styles_root = match &styles_data {
        Some(d) if !d.is_empty() => Some(xml::parse(d)?),
        _ => None,
    };
    let styles = Styles::new(&[styles_root.as_ref(), Some(&content)]);
    let mut r = Reader { z, content, styles, warnings: BTreeSet::new(), styles_raw: Vec::new() };
    r.read()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

impl Reader<'_> {
    fn warn(&mut self, t: &str) {
        self.warnings.insert(t.to_string());
    }

    fn read(&mut self) -> Result<Document, FormatError> {
        let content = std::mem::take(&mut self.content);
        let text = content
            .find(&format!("{}/{}", q("office:body"), q("office:text")))
            .ok_or_else(|| FormatError::new("this OpenDocument file isn't a text document"))?;
        if content.find(&format!(".//{}", q("text:tracked-changes"))).is_some() {
            self.warn("tracked changes (shown as accepted)");
        }
        let blocks = self.blocks(text, None, 0)?;
        // styles_have_header(): reads styles.xml again, so it counts again
        self.styles_raw = self.z.read("styles.xml", false)?.unwrap_or_default();
        if contains(&self.styles_raw, b"<style:header") || contains(&self.styles_raw, b"<style:footer") {
            self.warn("headers and footers");
        }
        Ok(Document { blocks, warnings: self.warnings.iter().cloned().collect() })
    }

    fn blocks(&mut self, parent: &Element, list_style: Option<&str>, level: i64) -> Result<Vec<Block>, FormatError> {
        let mut out = Vec::new();
        for el in &parent.children {
            let tag = el.tag.as_str();
            if tag == q("text:p") || tag == q("text:h") {
                let style = el.get(&q("text:style-name"));
                let (align, brk, mut heading) = self.styles.para_props(style);
                if tag == q("text:h") {
                    let v = el.get(&q("text:outline-level")).unwrap_or("1");
                    heading = if v.is_empty() { 1 } else { py_int(v).ok_or_else(|| FormatError::new("broken heading level"))? };
                }
                if brk && !out.is_empty() {
                    out.push(Block::PageBreak);
                }
                let base = if heading != 0 { Props::default() } else { self.styles.text_props("paragraph", style) };
                let runs = self.inline(el, &base, None)?;
                let st = if heading != 0 { format!("h{}", heading.clamp(1, 6)) } else { "p".into() };
                out.push(Block::Para(Para::new(merge_runs(runs), &st, &align)));
            } else if tag == q("text:list") {
                let name = el.get(&q("text:style-name")).filter(|s| !s.is_empty()).or(list_style);
                let mut lst = ListBlock { ordered: self.styles.list_ordered(name, level), items: Vec::new() };
                for item in &el.children {
                    if item.tag == q("text:list-item") || item.tag == q("text:list-header") {
                        let b = self.blocks(item, name, level + 1)?;
                        lst.items.push(if b.is_empty() { vec![Block::Para(Para::default())] } else { b });
                    }
                }
                out.push(Block::List(lst));
            } else if tag == q("table:table") {
                out.push(Block::Table(self.table(el)?));
            } else if [
                "text:section",
                "text:index-body",
                "text:table-of-content",
                "text:illustration-index",
                "text:alphabetical-index",
                "text:bibliography",
                "text:user-index",
                "text:object-index",
                "text:table-index",
            ]
            .iter()
            .any(|t| q(t) == tag)
            {
                out.extend(self.blocks(el, list_style, level)?);
            }
        }
        Ok(out)
    }

    fn inline(&mut self, parent: &Element, fmt: &Props, href: Option<&str>) -> Result<Vec<Run>, FormatError> {
        let mut runs = Vec::new();
        if let Some(t) = parent.text.as_deref().filter(|t| !t.is_empty()) {
            runs.push(text_run(t, fmt, href, false));
        }
        for el in &parent.children {
            let tag = el.tag.as_str();
            if tag == q("text:span") {
                let mut props = *fmt;
                let sp = self.styles.text_props("text", el.get(&q("text:style-name")));
                // props.update(): keys the span's style sets win
                props.b = sp.b.or(props.b);
                props.i = sp.i.or(props.i);
                props.u = sp.u.or(props.u);
                props.s = sp.s.or(props.s);
                props.sup = sp.sup.or(props.sup);
                props.sub = sp.sub.or(props.sub);
                runs.extend(self.inline(el, &props, href)?);
            } else if tag == q("text:a") {
                let link = el.get(&q("xlink:href")).filter(|l| !l.is_empty() && is_web_link(l));
                runs.extend(self.inline(el, fmt, link)?);
            } else if tag == q("text:s") {
                let c = el.get(&q("text:c")).unwrap_or("1");
                let c = if c.is_empty() { 1 } else { py_int(c).ok_or_else(|| FormatError::new("broken spaces"))? };
                runs.push(text_run(&" ".repeat(c.clamp(0, 1000) as usize), fmt, href, true));
            } else if tag == q("text:tab") {
                runs.push(text_run("\t", fmt, href, true));
            } else if tag == q("text:line-break") {
                runs.push(Run::Break);
            } else if tag == q("draw:frame") {
                runs.extend(self.frame(el)?);
            } else if tag == q("draw:a") {
                for fr in el.findall(&q("draw:frame")) {
                    runs.extend(self.frame(fr)?);
                }
            } else if tag == q("text:note") {
                self.warn("footnotes and endnotes");
            } else if tag == q("office:annotation") {
                self.warn("comments");
            } else if [
                "office:annotation-end",
                "text:bookmark",
                "text:bookmark-start",
                "text:bookmark-end",
                "text:soft-page-break",
                "text:reference-mark-start",
                "text:reference-mark-end",
                "text:change-start",
                "text:change-end",
                "text:change",
            ]
            .iter()
            .any(|t| q(t) == tag)
            {
            } else if tag.starts_with(&format!("{{{}}}", ns("draw"))) {
                self.warn("text boxes and shapes");
            } else {
                // fields (page numbers, dates...), meta, ruby...: keep their text
                runs.extend(self.inline(el, fmt, href)?);
            }
            if let Some(t) = el.tail.as_deref().filter(|t| !t.is_empty()) {
                runs.push(text_run(t, fmt, href, false));
            }
        }
        Ok(runs)
    }

    fn frame(&mut self, fr: &Element) -> Result<Vec<Run>, FormatError> {
        let Some(img) = fr.find(&q("draw:image")) else {
            if fr.find(&q("draw:text-box")).is_some() {
                self.warn("text boxes and shapes");
            } else if fr.find(&q("draw:object")).is_some() || fr.find(&q("draw:object-ole")).is_some() {
                self.warn("embedded objects (charts, formulas)");
            }
            return Ok(vec![]);
        };
        let target = img.get(&q("xlink:href")).unwrap_or("");
        if has_scheme(target) {
            self.warn("pictures linked from the internet (not loaded)");
            return Ok(vec![]);
        }
        let data = match self.z.read(target.trim_start_matches(['.', '/']), false)? {
            Some(d) if !d.is_empty() && d.len() <= MAX_IMAGE => d,
            _ => return Ok(vec![]),
        };
        let Some(mime) = sniff_image(&data) else {
            self.warn("pictures in formats Leaflet can't show (EMF/WMF/SVG)");
            return Ok(vec![]);
        };
        let desc = fr.find(&q("svg:desc")).and_then(|d| d.text.clone()).filter(|t| !t.is_empty());
        let title = fr.find(&q("svg:title")).and_then(|d| d.text.clone()).filter(|t| !t.is_empty());
        let alt = desc.or(title).unwrap_or_default();
        Ok(vec![Run::Image(Image {
            data,
            mime: mime.into(),
            width: css_length_px(fr.get(&q("svg:width")).unwrap_or(""))?,
            height: css_length_px(fr.get(&q("svg:height")).unwrap_or(""))?,
            alt,
        })])
    }

    fn table(&mut self, el: &Element) -> Result<Table, FormatError> {
        let mut t = Table::default();
        for row in rows(el) {
            let mut cells = Vec::new();
            for c in &row.children {
                if c.tag == q("table:table-cell") {
                    let num = |k: &str| -> Result<i64, FormatError> {
                        let v = c.get(&q(k)).unwrap_or("1");
                        if v.is_empty() { Ok(1) } else { py_int(v).ok_or_else(|| FormatError::new("broken table")) }
                    };
                    let span = num("table:number-columns-spanned")?;
                    if num("table:number-rows-spanned")? > 1 {
                        self.warn("table cells merged across rows (split again)");
                    }
                    let b = self.blocks(c, None, 0)?;
                    cells.push(Cell { blocks: if b.is_empty() { vec![Block::Para(Para::default())] } else { b }, colspan: span.clamp(1, 64) });
                }
            }
            if !cells.is_empty() {
                t.rows.push(cells);
            }
        }
        Ok(t)
    }
}

/// re.match(r'(?i)[a-z][a-z0-9+.-]*:', target)
fn has_scheme(t: &str) -> bool {
    let b = t.as_bytes();
    if b.first().is_none_or(|c| !c.is_ascii_alphabetic()) {
        return false;
    }
    for &c in &b[1..] {
        if c == b':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || c == b'+' || c == b'.' || c == b'-') {
            return false;
        }
    }
    false
}

fn rows(el: &Element) -> Vec<&Element> {
    let mut out = Vec::new();
    for child in &el.children {
        if child.tag == q("table:table-row") {
            out.push(child);
        } else if child.tag == q("table:table-header-rows") || child.tag == q("table:table-rows") || child.tag == q("table:table-row-group") {
            out.extend(rows(child));
        }
    }
    out
}

fn text_run(t: &str, fmt: &Props, href: Option<&str>, literal: bool) -> Run {
    let t = if literal { t.to_string() } else { collapse_ws(t) };
    let u = fmt.u.unwrap_or(false) && href.is_none();
    Run::Text(Text::new(
        t,
        Fmt {
            b: fmt.b.unwrap_or(false),
            i: fmt.i.unwrap_or(false),
            u,
            s: fmt.s.unwrap_or(false),
            sup: fmt.sup.unwrap_or(false),
            sub: fmt.sub.unwrap_or(false),
            href: href.map(str::to_string),
        },
    ))
}

/// re.sub(r'[ \t\r\n]+', ' ', t)
fn collapse_ws(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut in_ws = false;
    for c in t.chars() {
        if matches!(c, ' ' | '\t' | '\r' | '\n') {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}
