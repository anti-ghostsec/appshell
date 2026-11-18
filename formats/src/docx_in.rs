//! DOCX in.

use crate::images::sniff_image;
use crate::model::*;
use crate::text::{is_digits, is_web_link, py_int, py_int16};
use crate::xml::{self, Element};
use crate::zipio::Zip;
use crate::{FormatError, EMU_PER_PX, MAX_IMAGE};
use std::collections::{BTreeSet, HashMap};

pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
pub const PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
pub const V: &str = "urn:schemas-microsoft-com:vml";
pub const PR: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const MATH: &str = "{http://schemas.openxmlformats.org/officeDocument/2006/math}";

fn w(tag: &str) -> String {
    format!("{{{W}}}{tag}")
}
fn mc(tag: &str) -> String {
    format!("{{{MC}}}{tag}")
}

fn wattr<'a>(el: Option<&'a Element>, name: &str) -> Option<&'a str> {
    el?.get(&w(name))
}

/// <w:b/> is on, <w:b w:val="0"/> is off, missing is None (inherit).
fn on_off(el: Option<&Element>) -> Option<bool> {
    let el = el?;
    let v = el.get(&w("val")).unwrap_or("true");
    Some(!matches!(v, "0" | "false" | "off" | "none"))
}

#[derive(Clone)]
pub struct Rel {
    pub target: String,
    pub external: bool,
    pub typ: String,
}

/// A relationship table in file order; a repeated Id keeps its first place
/// and takes the last value, as a Python dict does.
#[derive(Default)]
pub struct Rels(Vec<(Option<String>, Rel)>);

impl Rels {
    fn insert(&mut self, id: Option<String>, rel: Rel) {
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| *k == id) {
            slot.1 = rel;
        } else {
            self.0.push((id, rel));
        }
    }
    fn get(&self, id: Option<&str>) -> Option<&Rel> {
        self.0.iter().find(|(k, _)| k.as_deref() == id).map(|(_, r)| r)
    }
}

fn rels_of(z: &mut Zip, part: &str) -> Result<Rels, FormatError> {
    let (folder, name) = match part.rfind('/') {
        Some(i) => (&part[..i], &part[i + 1..]),
        None => ("", part),
    };
    let mut out = Rels::default();
    if let Some(data) = z.read(&format!("{folder}/_rels/{name}.rels"), false)? {
        if !data.is_empty() {
            for rel in &xml::parse(&data)?.children {
                let mut target = rel.get("Target").unwrap_or("").to_string();
                let external = rel.get("TargetMode") == Some("External");
                if !external && !target.starts_with('/') {
                    target = format!("{folder}/{target}");
                }
                out.insert(
                    rel.get("Id").map(str::to_string),
                    Rel { target: target.trim_start_matches('/').to_string(), external, typ: rel.get("Type").unwrap_or("").to_string() },
                );
            }
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Default, Debug)]
struct Props {
    b: Option<bool>,
    i: Option<bool>,
    u: Option<bool>,
    s: Option<bool>,
    sup: Option<bool>,
    sub: Option<bool>,
}

impl Props {
    /// props.setdefault for each key: what's already there stays.
    fn set_default(&mut self, o: &Props) {
        self.b = self.b.or(o.b);
        self.i = self.i.or(o.i);
        self.u = self.u.or(o.u);
        self.s = self.s.or(o.s);
        self.sup = self.sup.or(o.sup);
        self.sub = self.sub.or(o.sub);
    }
    /// props.update: what's in `o` wins.
    fn update(&mut self, o: &Props) {
        self.b = o.b.or(self.b);
        self.i = o.i.or(self.i);
        self.u = o.u.or(self.u);
        self.s = o.s.or(self.s);
        self.sup = o.sup.or(self.sup);
        self.sub = o.sub.or(self.sub);
    }
}

fn rpr_props(rpr: &Element) -> Props {
    let mut p = Props {
        b: on_off(rpr.find(&w("b"))),
        i: on_off(rpr.find(&w("i"))),
        s: on_off(rpr.find(&w("strike"))),
        ..Default::default()
    };
    if let Some(u) = rpr.find(&w("u")) {
        p.u = Some(u.get(&w("val")).unwrap_or("single") != "none");
    }
    if let Some(va) = rpr.find(&w("vertAlign")) {
        let v = va.get(&w("val"));
        p.sup = Some(v == Some("superscript"));
        p.sub = Some(v == Some("subscript"));
    }
    p
}

fn outline_level(ol: Option<&Element>) -> Option<i64> {
    let v = ol?.get(&w("val")).unwrap_or("9");
    if is_digits(v) {
        let n = py_int(v)?;
        if n < 6 {
            return Some(n + 1);
        }
    }
    None
}

struct Styles {
    styles: HashMap<Option<String>, Element>,
    heading: HashMap<Option<String>, i64>,
}

impl Styles {
    fn new(root: Option<Element>) -> Styles {
        let mut s = Styles { styles: HashMap::new(), heading: HashMap::new() };
        let Some(root) = root else { return s };
        for st in root.iter_tag(&w("style")) {
            let sid = st.get(&w("styleId")).map(str::to_string);
            let name = wattr(st.find(&w("name")), "val").unwrap_or("").to_lowercase();
            s.styles.insert(sid.clone(), st.clone());
            let level = if let Some(n) = name.strip_prefix("heading ") {
                if n.len() == 1 && ("1"..="6").contains(&n) { Some(py_int(n).unwrap()) } else { None }
            } else {
                None
            };
            if let Some(l) = level {
                s.heading.insert(sid, l);
            } else if name == "title" {
                s.heading.insert(sid, 1);
            } else if name == "subtitle" {
                s.heading.insert(sid, 2);
            } else if let Some(l) = outline_level(st.find(&format!("{}/{}", w("pPr"), w("outlineLvl")))) {
                s.heading.insert(sid, l);
            }
        }
        s
    }

    fn chain(&self, sid: Option<&str>) -> Vec<&Element> {
        let mut out = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut sid = sid.map(str::to_string);
        while let Some(id) = sid.clone() {
            if id.is_empty() || seen.contains(&id) {
                break;
            }
            let Some(st) = self.styles.get(&Some(id.clone())) else { break };
            seen.push(id);
            out.push(st);
            sid = wattr(st.find(&w("basedOn")), "val").map(str::to_string);
        }
        out
    }

    fn heading_level(&self, sid: Option<&str>) -> i64 {
        for st in self.chain(sid) {
            let id = st.get(&w("styleId")).map(str::to_string);
            if let Some(&l) = self.heading.get(&id) {
                return l;
            }
        }
        0
    }

    fn run_props(&self, sid: Option<&str>) -> Props {
        let mut props = Props::default();
        for st in self.chain(sid) {
            if let Some(rpr) = st.find(&w("rPr")) {
                props.set_default(&rpr_props(rpr));
            }
        }
        props
    }

    fn numpr(&self, sid: Option<&str>) -> Option<Element> {
        for st in self.chain(sid) {
            if let Some(n) = st.find(&format!("{}/{}", w("pPr"), w("numPr"))) {
                return Some(n.clone());
            }
        }
        None
    }
}

struct Numbering {
    abstract_: HashMap<Option<String>, HashMap<i64, String>>,
    nums: HashMap<Option<String>, Option<String>>,
}

impl Numbering {
    fn new(root: Option<Element>) -> Result<Numbering, FormatError> {
        let mut n = Numbering { abstract_: HashMap::new(), nums: HashMap::new() };
        let Some(root) = root else { return Ok(n) };
        for an in root.findall(&w("abstractNum")) {
            let mut levels = HashMap::new();
            for lvl in an.findall(&w("lvl")) {
                let il = lvl.get(&w("ilvl")).unwrap_or("0");
                let il = py_int(il).ok_or_else(|| FormatError::new("broken list numbering"))?;
                levels.insert(il, wattr(lvl.find(&w("numFmt")), "val").unwrap_or("decimal").to_string());
            }
            n.abstract_.insert(an.get(&w("abstractNumId")).map(str::to_string), levels);
        }
        for num in root.findall(&w("num")) {
            n.nums.insert(num.get(&w("numId")).map(str::to_string), wattr(num.find(&w("abstractNumId")), "val").map(str::to_string));
        }
        Ok(n)
    }

    fn ordered(&self, num_id: &str, ilvl: i64) -> bool {
        let abs = self.nums.get(&Some(num_id.to_string())).cloned().flatten();
        let fmt = self.abstract_.get(&abs).and_then(|l| l.get(&ilvl)).map(String::as_str).unwrap_or("decimal");
        !matches!(fmt, "bullet" | "none")
    }
}

enum Item {
    Block(Block),
    Li(String, i64, bool, Para),
}

struct Reader<'a> {
    z: Zip<'a>,
    main: String,
    rels: Rels,
    styles: Styles,
    numbering: Numbering,
    warnings: BTreeSet<String>,
}

fn find_main_part(ct: &[u8]) -> Option<String> {
    const NEEDLE: &[u8] = b"PartName=\"/";
    const CT: &[u8] = b"ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"";
    let mut from = 0;
    while let Some(p) = ct[from..].windows(NEEDLE.len()).position(|x| x == NEEDLE) {
        let start = from + p + NEEDLE.len();
        from = from + p + 1;
        let Some(q) = ct[start..].iter().position(|&b| b == b'"') else { continue };
        if q == 0 {
            continue;
        }
        let name = &ct[start..start + q];
        let rest = &ct[start + q + 1..];
        let stop = rest.iter().position(|&b| b == b'>').unwrap_or(rest.len());
        if rest[..stop].windows(CT.len()).any(|x| x == CT) {
            return String::from_utf8(name.to_vec()).ok();
        }
    }
    None
}

pub fn read_docx(data: &[u8]) -> Result<Document, FormatError> {
    let mut z = Zip::open(data)?;
    let ct = z.read("[Content_Types].xml", false)?.unwrap_or_default();
    let mut main = "word/document.xml".to_string();
    if let Some(m) = find_main_part(&ct) {
        if z.has(&m) {
            main = m;
        }
    }
    let rels = rels_of(&mut z, &main)?;
    let styles_root = part(&mut z, &rels, "styles")?;
    let numbering_root = part(&mut z, &rels, "numbering")?;
    let mut r = Reader {
        z,
        main,
        rels,
        styles: Styles::new(styles_root),
        numbering: Numbering::new(numbering_root)?,
        warnings: BTreeSet::new(),
    };
    r.read()
}

fn part(z: &mut Zip, rels: &Rels, kind: &str) -> Result<Option<Element>, FormatError> {
    let suffix = format!("/{kind}");
    for (_, rel) in &rels.0 {
        if !rel.external && rel.typ.ends_with(&suffix) {
            let data = z.read(&rel.target, false)?;
            return match data {
                Some(d) if !d.is_empty() => Ok(Some(xml::parse(&d)?)),
                _ => Ok(None),
            };
        }
    }
    Ok(None)
}

impl Reader<'_> {
    fn warn(&mut self, text: &str) {
        self.warnings.insert(text.to_string());
    }

    fn read(&mut self) -> Result<Document, FormatError> {
        let main = self.main.clone();
        let data = self.z.read(&main, true)?.unwrap_or_default();
        let root = xml::parse(&data)?;
        let body = root.find(&w("body")).ok_or_else(|| FormatError::new("the document has no body"))?;
        if body.find(&format!(".//{}", w("headerReference"))).is_some() || body.find(&format!(".//{}", w("footerReference"))).is_some() {
            self.warn("headers and footers");
        }
        let blocks = self.blocks(body)?;
        Ok(Document { blocks, warnings: self.warnings.iter().cloned().collect() })
    }

    fn blocks(&mut self, parent: &Element) -> Result<Vec<Block>, FormatError> {
        let mut items = Vec::new();
        for el in children(parent) {
            if el.tag == w("p") {
                items.extend(self.paragraph(el)?);
            } else if el.tag == w("tbl") {
                items.push(Item::Block(Block::Table(self.table(el)?)));
            }
        }
        Ok(group_lists(items))
    }

    fn paragraph(&mut self, p: &Element) -> Result<Vec<Item>, FormatError> {
        let ppr = p.find(&w("pPr"));
        let sid: Option<String> = ppr.and_then(|pp| wattr(pp.find(&w("pStyle")), "val")).map(str::to_string);
        let sid_ref = sid.as_deref().filter(|s| !s.is_empty());
        let mut level = if sid_ref.is_some() { self.styles.heading_level(sid_ref) } else { 0 };
        if let Some(pp) = ppr {
            if level == 0 {
                if let Some(l) = outline_level(pp.find(&w("outlineLvl"))) {
                    level = l;
                }
            }
        }
        let style = if level != 0 { format!("h{level}") } else { "p".to_string() };
        let jc = ppr.and_then(|pp| wattr(pp.find(&w("jc")), "val"));
        let align = match jc {
            Some("center") => "center",
            Some("right") | Some("end") => "right",
            Some("both") | Some("distribute") => "justify",
            _ => "",
        };
        let base = if sid_ref.is_some() && level == 0 { self.styles.run_props(sid_ref) } else { Props::default() };
        let mut out = Vec::new();
        if ppr.and_then(|pp| on_off(pp.find(&w("pageBreakBefore")))) == Some(true) {
            out.push(Item::Block(Block::PageBreak));
        }
        let runs = self.inline(p, &base, None)?;
        let mut pieces: Vec<Vec<Run>> = vec![Vec::new()];
        for r in runs {
            if matches!(r, Run::PageBreak) {
                pieces.push(Vec::new());
            } else {
                pieces.last_mut().unwrap().push(r);
            }
        }
        let mut numpr = ppr.and_then(|pp| pp.find(&w("numPr"))).cloned();
        if numpr.is_none() && sid_ref.is_some() {
            numpr = self.styles.numpr(sid_ref);
        }
        let num_id = numpr.as_ref().and_then(|n| wattr(n.find(&w("numId")), "val")).map(str::to_string);
        let ilvl = match &numpr {
            Some(n) => {
                let v = match n.find(&w("ilvl")) {
                    Some(e) => e.get(&w("val")).unwrap_or("0"),
                    None => "0",
                };
                if v.is_empty() { 0 } else { py_int(v).ok_or_else(|| FormatError::new("broken list level"))? }
            }
            None => 0,
        };
        let count = pieces.len();
        for (n, piece) in pieces.into_iter().enumerate() {
            if n > 0 {
                out.push(Item::Block(Block::PageBreak));
            }
            let q = Para::new(merge_runs(piece), &style, align);
            match &num_id {
                Some(id) if !id.is_empty() && id != "0" && n == 0 => {
                    let ordered = self.numbering.ordered(id, ilvl);
                    out.push(Item::Li(id.clone(), ilvl.min(8), ordered, q));
                }
                _ => {
                    if !q.runs.is_empty() || count == 1 {
                        out.push(Item::Block(Block::Para(q)));
                    }
                }
            }
        }
        Ok(out)
    }

    fn inline(&mut self, parent: &Element, base: &Props, href: Option<&str>) -> Result<Vec<Run>, FormatError> {
        let mut runs = Vec::new();
        for el in &parent.children {
            let tag = el.tag.as_str();
            if tag == w("r") {
                runs.extend(self.run(el, base, href)?);
            } else if tag == w("hyperlink") {
                let rid = el.get(&format!("{{{R}}}id"));
                let mut link: Option<String> = match self.rels.get(rid) {
                    Some(t) if t.external => Some(t.target.clone()),
                    _ => href.map(str::to_string),
                };
                if let Some(l) = &link {
                    if !l.is_empty() && !is_web_link(l) {
                        link = None;
                    }
                }
                let link = link.filter(|l| !l.is_empty());
                runs.extend(self.inline(el, base, link.as_deref())?);
            } else if [w("ins"), w("moveTo"), w("smartTag"), w("customXml"), w("fldSimple"), w("sdtContent")].iter().any(|t| t == tag) {
                if tag == w("ins") || tag == w("moveTo") {
                    self.warn("tracked changes (shown as accepted)");
                }
                runs.extend(self.inline(el, base, href)?);
            } else if tag == w("sdt") {
                if let Some(content) = el.find(&w("sdtContent")) {
                    runs.extend(self.inline(content, base, href)?);
                }
            } else if tag == w("del") || tag == w("moveFrom") {
                self.warn("tracked changes (shown as accepted)");
            } else if tag == w("commentRangeStart") {
                self.warn("comments");
            } else if tag == mc("AlternateContent") {
                if let Some(fb) = el.find(&mc("Fallback")) {
                    runs.extend(self.inline(fb, base, href)?);
                }
            } else if tag.starts_with(MATH) {
                self.warn("equations");
            }
        }
        Ok(runs)
    }

    fn run(&mut self, r: &Element, base: &Props, href: Option<&str>) -> Result<Vec<Run>, FormatError> {
        let mut props = *base;
        if let Some(rpr) = r.find(&w("rPr")) {
            if let Some(rs) = wattr(rpr.find(&w("rStyle")), "val").filter(|s| !s.is_empty()) {
                props.update(&self.styles.run_props(Some(rs)));
            }
            props.update(&rpr_props(rpr));
        }
        let fmt = Fmt {
            b: props.b.unwrap_or(false),
            i: props.i.unwrap_or(false),
            u: if href.is_some() { false } else { props.u.unwrap_or(false) },
            s: props.s.unwrap_or(false),
            sup: props.sup.unwrap_or(false),
            sub: props.sub.unwrap_or(false),
            href: href.map(str::to_string),
        };
        let mut out = Vec::new();
        for el in &r.children {
            let tag = el.tag.as_str();
            if tag == w("t") {
                out.push(Run::Text(Text::new(el.text.clone().unwrap_or_default(), fmt.clone())));
            } else if tag == w("tab") || tag == w("ptab") {
                out.push(Run::Text(Text::new("\t", fmt.clone())));
            } else if tag == w("br") || tag == w("cr") {
                out.push(if el.get(&w("type")) == Some("page") { Run::PageBreak } else { Run::Break });
            } else if tag == w("noBreakHyphen") {
                out.push(Run::Text(Text::new("\u{2011}", fmt.clone())));
            } else if tag == w("sym") {
                if let Some(ch) = el.get(&w("char")).filter(|c| !c.is_empty()) {
                    if let Some(c) = py_int16(ch).and_then(|n| u32::try_from(n).ok()).and_then(char::from_u32) {
                        out.push(Run::Text(Text::new(c.to_string(), fmt.clone())));
                    }
                }
            } else if tag == w("drawing") {
                out.extend(self.drawing(el)?);
            } else if tag == w("pict") || tag == w("object") {
                out.extend(self.vml(el)?);
            } else if tag == mc("AlternateContent") {
                let fb = el.find(&mc("Fallback"));
                let choice = el.find(&mc("Choice"));
                let mut found = Vec::new();
                for alt in [choice, fb].into_iter().flatten() {
                    if !found.is_empty() {
                        break;
                    }
                    for d in alt.iter_tag(&w("drawing")) {
                        found.extend(self.drawing(d)?);
                    }
                    for d in alt.iter_tag(&w("pict")) {
                        found.extend(self.vml(d)?);
                    }
                }
                out.extend(found);
            } else if tag == w("footnoteReference") || tag == w("endnoteReference") {
                self.warn("footnotes and endnotes");
            } else if tag == w("commentReference") {
                self.warn("comments");
            }
        }
        Ok(out)
    }

    fn image_from_rel(&mut self, rid: Option<&str>, width: i64, height: i64, alt: &str) -> Result<Vec<Run>, FormatError> {
        let Some(target) = self.rels.get(rid).cloned() else { return Ok(vec![]) };
        if target.external {
            self.warn("pictures linked from the internet (not loaded)");
            return Ok(vec![]);
        }
        let data = match self.z.read(&target.target, false)? {
            Some(d) if !d.is_empty() && d.len() <= MAX_IMAGE => d,
            _ => return Ok(vec![]),
        };
        let Some(mime) = sniff_image(&data) else {
            self.warn("pictures in formats Leaflet can't show (EMF/WMF/SVG)");
            return Ok(vec![]);
        };
        Ok(vec![Run::Image(Image { data, mime: mime.into(), width, height, alt: alt.into() })])
    }

    fn drawing(&mut self, d: &Element) -> Result<Vec<Run>, FormatError> {
        if d.find(&format!(".//{{{WP}}}txbxContent")).is_some()
            || d.find(".//{http://schemas.microsoft.com/office/word/2010/wordprocessingShape}txbx").is_some()
        {
            self.warn("text boxes and shapes");
        }
        if d.find(".//{http://schemas.openxmlformats.org/drawingml/2006/chart}chart").is_some() {
            self.warn("charts");
        }
        let ext = d.find(&format!(".//{{{WP}}}extent"));
        let emu = |k: &str| -> Result<i64, FormatError> {
            match ext {
                Some(e) => {
                    let n = py_int(e.get(k).unwrap_or("0")).ok_or_else(|| FormatError::new("broken picture size"))?;
                    Ok((n as f64 / EMU_PER_PX as f64) as i64)
                }
                None => Ok(0),
            }
        };
        let (width, height) = (emu("cx")?, emu("cy")?);
        let alt = d.find(&format!(".//{{{WP}}}docPr")).and_then(|p| p.get("descr")).unwrap_or("").to_string();
        if d.find(&format!(".//{{{WP}}}anchor")).is_some() {
            self.warn("floating pictures (placed in the text instead)");
        }
        let mut out = Vec::new();
        for blip in d.iter_tag(&format!("{{{A}}}blip")) {
            let rid = blip.get(&format!("{{{R}}}embed")).filter(|s| !s.is_empty());
            if blip.get(&format!("{{{R}}}link")).is_some_and(|l| !l.is_empty()) && rid.is_none() {
                self.warn("pictures linked from the internet (not loaded)");
            }
            if rid.is_some() {
                out.extend(self.image_from_rel(rid, width, height, &alt)?);
            }
        }
        Ok(out)
    }

    fn vml(&mut self, el: &Element) -> Result<Vec<Run>, FormatError> {
        let mut out = Vec::new();
        if el.find(&format!(".//{{{V}}}textbox")).is_some() {
            self.warn("text boxes and shapes");
        }
        for img in el.iter_tag(&format!("{{{V}}}imagedata")) {
            let Some(rid) = img.get(&format!("{{{R}}}id")).filter(|s| !s.is_empty()) else { continue };
            let (mut width, mut height) = (0, 0);
            if let Some(shape) = el.iter_tag(&format!("{{{V}}}shape")).first() {
                let mut st: HashMap<&str, &str> = HashMap::new();
                for kv in shape.get("style").unwrap_or("").split(';') {
                    if let Some((k, v)) = kv.split_once(':') {
                        st.insert(k, v);
                    }
                }
                width = css_length_px(st.get("width").copied().unwrap_or(""))?;
                height = css_length_px(st.get("height").copied().unwrap_or(""))?;
            }
            out.extend(self.image_from_rel(Some(rid), width, height, "")?);
        }
        Ok(out)
    }

    fn table(&mut self, tbl: &Element) -> Result<Table, FormatError> {
        let mut t = Table::default();
        for tr in children(tbl) {
            if tr.tag != w("tr") {
                continue;
            }
            let mut row = Vec::new();
            for tc in children(tr) {
                if tc.tag != w("tc") {
                    continue;
                }
                let tcpr = tc.find(&w("tcPr"));
                let span = match tcpr {
                    Some(p) => {
                        let v = match p.find(&w("gridSpan")) {
                            Some(g) => g.get(&w("val")).unwrap_or("1"),
                            None => "1",
                        };
                        if v.is_empty() { 1 } else { py_int(v).ok_or_else(|| FormatError::new("broken table"))? }
                    }
                    None => 1,
                };
                let vm = tcpr.and_then(|p| p.find(&w("vMerge")));
                let blocks = if vm.is_some_and(|v| v.get(&w("val")).unwrap_or("continue") == "continue") {
                    self.warn("table cells merged across rows (split again)");
                    vec![Block::Para(Para::default())]
                } else {
                    let b = self.blocks(tc)?;
                    if b.is_empty() { vec![Block::Para(Para::default())] } else { b }
                };
                row.push(Cell { blocks, colspan: span.clamp(1, 64) });
            }
            if !row.is_empty() {
                t.rows.push(row);
            }
        }
        Ok(t)
    }
}

/// Block children, looking through content controls and tracked insertions.
fn children(parent: &Element) -> Vec<&Element> {
    let mut out = Vec::new();
    for el in &parent.children {
        if el.tag == w("sdt") {
            if let Some(c) = el.find(&w("sdtContent")) {
                out.extend(children(c));
            }
        } else if el.tag == w("customXml") || el.tag == w("ins") || el.tag == w("moveTo") {
            out.extend(children(el));
        } else if el.tag == mc("AlternateContent") {
            if let Some(fb) = el.find(&mc("Fallback")) {
                out.extend(children(fb));
            }
        } else {
            out.push(el);
        }
    }
    out
}

/// Turn runs of numbered paragraphs into (nested) lists.
fn group_lists(items: Vec<Item>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut entries: Vec<(String, i64, bool, Para)> = Vec::new();
    for item in items {
        match item {
            Item::Block(b) => {
                if !entries.is_empty() {
                    out.extend(nest(std::mem::take(&mut entries)));
                }
                out.push(b);
            }
            Item::Li(a, b, c, d) => entries.push((a, b, c, d)),
        }
    }
    if !entries.is_empty() {
        out.extend(nest(entries));
    }
    out
}

/// entries: (list id, level, ordered, para) -> list blocks, nested by level.
/// The lists are built as a path of indices into `blocks`, so each new list
/// goes where the Python version's object references would put it.
fn nest(entries: Vec<(String, i64, bool, Para)>) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    // stack: (level, path to the ListBlock, list id, ordered)
    let mut stack: Vec<(i64, Vec<usize>, String, bool)> = Vec::new();
    for (num_id, level, ordered, para) in entries {
        while stack.last().is_some_and(|s| s.0 > level) {
            stack.pop();
        }
        if let Some(top) = stack.last() {
            if top.0 == level && (top.3 != ordered || (level == 0 && num_id != top.2)) {
                stack.pop();
            }
        }
        if stack.last().map_or(true, |s| s.0 < level) {
            let lst = Block::List(ListBlock { ordered, items: Vec::new() });
            let path = if let Some(parent) = stack.last() {
                let parent_list = list_at(&mut blocks, &parent.1);
                if parent_list.items.is_empty() {
                    parent_list.items.push(Vec::new());
                }
                let k = parent_list.items.len() - 1;
                parent_list.items[k].push(lst);
                let mut p = parent.1.clone();
                p.push(k);
                p.push(parent_list.items[k].len() - 1);
                p
            } else {
                blocks.push(lst);
                vec![blocks.len() - 1]
            };
            stack.push((level, path, num_id, ordered));
        }
        let path = stack.last().unwrap().1.clone();
        list_at(&mut blocks, &path).items.push(vec![Block::Para(para)]);
    }
    blocks
}

/// The list at `path`: [index in blocks, item, index in item, item, ...].
fn list_at<'a>(blocks: &'a mut [Block], path: &[usize]) -> &'a mut ListBlock {
    let mut l: &mut ListBlock = match &mut blocks[path[0]] {
        Block::List(l) => l,
        _ => unreachable!(),
    };
    let mut rest = &path[1..];
    while rest.len() >= 2 {
        l = match &mut l.items[rest[0]][rest[1]] {
            Block::List(next) => next,
            _ => unreachable!(),
        };
        rest = &rest[2..];
    }
    l
}

pub fn css_length_px(value: &str) -> Result<i64, FormatError> {
    // re.match(r'\s*([0-9.]+)\s*(pt|px|in|cm|mm|pc|em)?', value)
    let v = value.trim_start_matches(|c: char| c.is_whitespace());
    let num_len = v.bytes().take_while(|b| b.is_ascii_digit() || *b == b'.').count();
    if num_len == 0 {
        return Ok(0);
    }
    let n: f64 = v[..num_len].parse().map_err(|_| FormatError::new(format!("could not convert string to float: '{}'", &v[..num_len])))?;
    let rest = v[num_len..].trim_start_matches(|c: char| c.is_whitespace());
    let factor = ["pt", "px", "in", "cm", "mm", "pc", "em"]
        .iter()
        .find(|u| rest.starts_with(*u))
        .map(|u| match *u {
            "pt" => 96.0 / 72.0,
            "in" => 96.0,
            "cm" => 96.0 / 2.54,
            "mm" => 96.0 / 25.4,
            "pc" | "em" => 16.0,
            _ => 1.0,
        })
        .unwrap_or(1.0);
    Ok((n * factor) as i64)
}
